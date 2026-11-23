//! One `WriteBatch` is one transaction: facts, cursors, registries and derived
//! rows commit together or not at all (spec 3), so after a crash the cursor never
//! runs ahead of the stored events.

use std::collections::HashMap;

use chrono::{NaiveDate, Timelike};
use mtgo_match::{GameView, MatchView, PROJECTION_VERSION};
use mtgo_protocol::{ClockState, EventKind, Subjects};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::arena::{
    ArenaFileProgress, NewArenaEvent, PlatformAccount, PlatformOpponent, RankObservation,
    insert_arena_event, insert_rank_observation, update_arena_file, upsert_platform_account,
    upsert_platform_opponent,
};
use crate::arena_game_sync::{archive_guarded, sync_guarded};
use crate::arena_games::{NewGameMessage, insert_game_message};
use crate::decks::{DeckWrite, apply_deck_write};
use crate::names::{end_reason_str, game_result_str, outcome_str, state_str};
use crate::{Platform, StoreError};

/// `log_events.raw` larger than this is stored zstd-compressed (spec 4.2).
pub const COMPRESS_OVER: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewLogFile {
    pub path: String,
    pub identity_hash: u64,
    pub head_hash: u64,
    pub head_len: u64,
    pub created_at_fs: Option<i64>,
    pub day_anchor: NaiveDate,
    pub timezone: String,
    pub inherited_username: Option<String>,
    pub seen_at_ms: i64,
}

/// Where a file's reading has got to. Committed with the events it covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileProgress {
    pub log_file_id: i64,
    pub cursor_offset: u64,
    pub clock: ClockState,
    pub last_observed_size: u64,
    pub head_hash: u64,
    pub head_len: u64,
    pub anchor: Option<(u64, u64)>,
    pub local_username: Option<String>,
    pub local_login_id: Option<u64>,
    pub seen_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seal {
    pub log_file_id: i64,
    pub reason: String,
    pub at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEvent {
    pub log_file_id: i64,
    pub byte_offset: u64,
    /// `None` only for a quarantined event (it may have failed before classifying).
    pub kind: Option<EventKind>,
    pub subjects: Subjects,
    pub logged_at_ms: i64,
    /// The full event text (header and continuation lines).
    pub raw: String,
    pub quarantined: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAccount {
    pub username: String,
    pub login_id: Option<u64>,
    pub seen_at_ms: i64,
    /// The log time of the Login line that named it, if one did: kept as
    /// `accounts.last_login_at` when newer (the current account rule).
    pub logged_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchWrite {
    /// Insert or replace the derived row and its games.
    Upsert { view: Box<MatchView>, revision: u64 },
    /// The match is no longer visible (discarded): delete its derived row.
    Remove { token: String },
    /// Projection failed: record the failure by token (issue #47) and flag the
    /// stored row, if there is one (spec 5.5, fault handling). The row keeps its
    /// last good view and `derived_version`, so a newer projection retries it.
    Attention { token: String, reason: String },
    /// The match is an untracked account's (release spec 5): record its
    /// token in `untracked_matches` instead of building a row. The first
    /// skip's time is kept.
    Skip { token: String, username: String },
}

impl MatchWrite {
    pub fn token(&self) -> &str {
        match self {
            MatchWrite::Upsert { view, .. } => &view.token,
            MatchWrite::Remove { token }
            | MatchWrite::Attention { token, .. }
            | MatchWrite::Skip { token, .. } => token,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WriteBatch {
    pub seals: Vec<Seal>,
    /// Arena files' reading progress and seals (Arena spec section 5).
    pub arena_files: Vec<ArenaFileProgress>,
    /// Decoded Arena events; a replayed `(file, offset, seq)` is skipped.
    pub arena_events: Vec<NewArenaEvent>,
    /// Raw Arena game messages; a replayed `(file, offset, seq)` is skipped.
    pub arena_game_messages: Vec<NewGameMessage>,
    /// Match tokens whose live game messages move into their archive,
    /// after this batch's messages are inserted.
    pub arena_archive_tokens: Vec<String>,
    /// Match tokens whose Arena game rows are derived from their stored
    /// game messages, after the match upserts and the archiving.
    pub arena_sync_tokens: Vec<String>,
    /// Arena and Endstep accounts and opponents, before the matches.
    pub platform_accounts: Vec<PlatformAccount>,
    pub platform_opponents: Vec<PlatformOpponent>,
    pub rank_observations: Vec<RankObservation>,
    /// The platform of each Arena and Endstep match write in `matches`, by
    /// token (`push_match`): an account such a match names that is not
    /// stored is made on that platform. A token not listed is MTGO's.
    pub platform_matches: Vec<(String, Platform)>,
    /// Arena course snapshots, one row per course (an empty snapshot is one
    /// marker row); a replayed `(file, offset, course)` is skipped.
    pub arena_course_observations: Vec<crate::arena_courses::CourseObservation>,
    /// Each Arena match's `(token, event name)`; the first stored stays.
    pub arena_match_events: Vec<(String, String)>,
    /// Arena accounts (usernames) whose limited runs are synced after the
    /// matches and Arena syncs (`arena_run_sync`).
    pub arena_run_accounts: Vec<String>,
    /// Sync every Arena account's limited runs (once at startup).
    pub arena_runs_resync: bool,
    pub files: Vec<FileProgress>,
    pub events: Vec<NewEvent>,
    pub accounts: Vec<NewAccount>,
    pub matches: Vec<MatchWrite>,
    /// Match tokens whose events move into their archive (spec 4.2).
    pub compact: Vec<String>,
    /// Record a completed rebuild at this projection version.
    pub derivation: Option<u32>,
    /// Settings rows `(key, JSON value)` to insert or replace (spec 4.5).
    pub settings: Vec<(String, String)>,
    /// Deck file syncs and match links (decks spec 3 and 4).
    pub decks: Vec<DeckWrite>,
    /// MTGO's verdicts on log matches from its game history (#454).
    pub history_outcomes: Vec<crate::history_check::HistoryOutcome>,
    /// Every Endstep unit is unsynced, every pulled Endstep match derived
    /// again and the current Endstep sync version stored
    /// (`endstep_sync::resync_all`), before this batch's marks.
    pub endstep_resync: bool,
    /// Endstep ranked results, after the matches they name.
    pub endstep_ranked: Vec<crate::endstep_sync::EndstepRanked>,
    /// Each folded Endstep match's game rows `(match token, games)`, after
    /// the matches they name.
    pub endstep_game_cards: Vec<(String, Vec<endstep_match::EndstepGameCards>)>,
    /// Each synced Endstep unit's refs `(unit id, refs)`: they replace the
    /// unit's stored refs.
    pub endstep_refs: Vec<(String, Vec<String>)>,
    /// Endstep units synced at `ENDSTEP_SYNC_VERSION`, written last, as
    /// `(unit id, file hash)`: the hash of the text that was folded, so a
    /// unit replaced since stays unsynced.
    pub endstep_synced: Vec<(String, String)>,
    /// Wall-clock time of the batch (registry first-seen times, compaction time).
    pub now_ms: i64,
}

impl WriteBatch {
    /// Add a match write of `platform`, noting the platform when it is not
    /// MTGO's (`platform_matches`).
    pub fn push_match(&mut self, write: MatchWrite, platform: Platform) {
        if platform != Platform::Mtgo {
            self.platform_matches
                .push((write.token().to_string(), platform));
        }
        self.matches.push(write);
    }

    pub fn is_empty(&self) -> bool {
        self.seals.is_empty()
            && self.arena_files.is_empty()
            && self.arena_events.is_empty()
            && self.arena_game_messages.is_empty()
            && self.arena_archive_tokens.is_empty()
            && self.arena_sync_tokens.is_empty()
            && self.platform_accounts.is_empty()
            && self.platform_opponents.is_empty()
            && self.rank_observations.is_empty()
            && self.arena_course_observations.is_empty()
            && self.arena_match_events.is_empty()
            && self.arena_run_accounts.is_empty()
            && !self.arena_runs_resync
            && self.files.is_empty()
            && self.events.is_empty()
            && self.accounts.is_empty()
            && self.matches.is_empty()
            && self.compact.is_empty()
            && self.derivation.is_none()
            && self.settings.is_empty()
            && self.decks.is_empty()
            && self.history_outcomes.is_empty()
            && !self.endstep_resync
            && self.endstep_ranked.is_empty()
            && self.endstep_game_cards.is_empty()
            && self.endstep_refs.is_empty()
            && self.endstep_synced.is_empty()
    }
}

/// Hashes are stored as 16 hex digits (an `INTEGER` cannot hold every `u64`).
pub fn hex(hash: u64) -> String {
    format!("{hash:016x}")
}

pub fn unhex(text: &str) -> Option<u64> {
    u64::from_str_radix(text, 16).ok()
}

pub fn encode_raw(text: &str) -> Result<(Vec<u8>, bool), StoreError> {
    if text.len() > COMPRESS_OVER {
        Ok((zstd::encode_all(text.as_bytes(), 3)?, true))
    } else {
        Ok((text.as_bytes().to_vec(), false))
    }
}

pub fn decode_raw(bytes: Vec<u8>, compressed: bool) -> Result<String, StoreError> {
    let bytes = if compressed {
        zstd::decode_all(bytes.as_slice())?
    } else {
        bytes
    };
    String::from_utf8(bytes).map_err(|error| StoreError::Corrupt(error.to_string()))
}

/// A new active `log_files` row. Its clock starts at the day anchor.
pub fn insert_log_file(conn: &Connection, file: &NewLogFile) -> Result<i64, StoreError> {
    conn.execute(
        "INSERT INTO log_files (path, identity_hash, head_hash, head_len, created_at_fs, day_anchor, timezone,
             inherited_username, local_username, clock_date, first_seen_at, last_seen_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?6, ?9, ?9)",
        params![
            file.path,
            hex(file.identity_hash),
            hex(file.head_hash),
            file.head_len as i64,
            file.created_at_fs,
            file.day_anchor.to_string(),
            file.timezone,
            file.inherited_username,
            file.seen_at_ms,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Write the whole batch in one transaction.
pub fn commit(conn: &mut Connection, batch: &WriteBatch) -> Result<(), StoreError> {
    // Immediate: the write lock is taken at the start, so a second writer
    // (a test beside a running tracker) waits on the busy timeout instead of
    // failing when its read snapshot went stale (`SQLITE_BUSY_SNAPSHOT`).
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    for seal in &batch.seals {
        seal_file(&tx, seal)?;
    }
    for event in &batch.events {
        insert_event(&tx, event)?;
    }
    for account in &batch.accounts {
        upsert_account(&tx, account)?;
    }
    // Arena rows before any match: a match's account and opponent exist
    // before the match names them.
    for file in &batch.arena_files {
        update_arena_file(&tx, file)?;
    }
    for event in &batch.arena_events {
        insert_arena_event(&tx, event)?;
    }
    for message in &batch.arena_game_messages {
        insert_game_message(&tx, message)?;
    }
    // Each Arena match's archive and sync run in a savepoint: one that
    // cannot be read is logged and skipped, never failing the batch.
    let mut unarchived: Vec<&String> = Vec::new();
    for token in &batch.arena_archive_tokens {
        if archive_guarded(&tx, token, batch.now_ms)?.is_none() {
            unarchived.push(token);
        }
    }
    for account in &batch.platform_accounts {
        upsert_platform_account(&tx, account)?;
    }
    for opponent in &batch.platform_opponents {
        upsert_platform_opponent(&tx, opponent, batch.now_ms)?;
    }
    for rank in &batch.rank_observations {
        insert_rank_observation(&tx, rank)?;
    }
    for observation in &batch.arena_course_observations {
        crate::arena_courses::insert_observation(&tx, observation)?;
    }
    for (token, event_name) in &batch.arena_match_events {
        crate::arena_courses::insert_match_event(&tx, token, event_name)?;
    }
    // Each match's platform (`platform_matches`), MTGO when not listed.
    let platforms: HashMap<&str, Platform> = batch
        .platform_matches
        .iter()
        .map(|(token, platform)| (token.as_str(), *platform))
        .collect();
    for write in &batch.matches {
        let platform = platforms
            .get(write.token())
            .copied()
            .unwrap_or(Platform::Mtgo);
        write_match(&tx, write, platform, batch.now_ms)?;
    }
    for token in &batch.arena_sync_tokens {
        if !unarchived.contains(&token) {
            sync_guarded(&tx, token, batch.now_ms)?;
        }
    }
    // Each account's run sync runs in a savepoint: one that fails is logged
    // and skipped, never failing the batch.
    if batch.arena_runs_resync {
        crate::arena_run_sync::sync_all_arena_runs_guarded(&tx, batch.now_ms)?;
    } else {
        for username in &batch.arena_run_accounts {
            if let Some(account_id) = arena_account_id(&tx, username)? {
                crate::arena_run_sync::sync_arena_runs_guarded(&tx, account_id, batch.now_ms)?;
            }
        }
    }
    for write in &batch.decks {
        apply_deck_write(&tx, write)?;
    }
    for write in &batch.history_outcomes {
        crate::history_check::apply_history_outcome(&tx, write)?;
    }
    // Endstep: ranked results and game rows after their matches; a unit's
    // refs and its synced mark last, so a unit is marked only with
    // everything it gave.
    if batch.endstep_resync {
        crate::endstep_sync::resync_all(&tx, batch.now_ms)?;
    }
    for ranked in &batch.endstep_ranked {
        crate::endstep_sync::upsert_ranked(&tx, ranked)?;
    }
    for (token, games) in &batch.endstep_game_cards {
        crate::endstep_sync::write_endstep_games(&tx, token, games, batch.now_ms)?;
    }
    for (unit_id, refs) in &batch.endstep_refs {
        crate::endstep_sync::replace_refs(&tx, unit_id, refs)?;
    }
    for (unit_id, file_hash) in &batch.endstep_synced {
        crate::endstep_sync::mark_synced(&tx, unit_id, file_hash)?;
    }
    for file in &batch.files {
        update_file(&tx, file)?;
    }
    for token in &batch.compact {
        crate::compact::compact_match(&tx, token, batch.now_ms)?;
    }
    if let Some(version) = batch.derivation {
        tx.execute(
            "INSERT INTO derivations (name, version, rebuilt_at) VALUES ('match_projection', ?1, ?2)
             ON CONFLICT (name) DO UPDATE SET version = excluded.version, rebuilt_at = excluded.rebuilt_at",
            params![version, batch.now_ms],
        )?;
    }
    for (key, value) in &batch.settings {
        tx.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn arena_account_id(tx: &Transaction, username: &str) -> Result<Option<i64>, StoreError> {
    Ok(tx
        .prepare_cached("SELECT id FROM accounts WHERE username = ?1 AND platform = 'arena'")?
        .query_row(
            [crate::arena::platform_account_username(tx, username)?],
            |row| row.get(0),
        )
        .optional()?)
}

fn seal_file(tx: &Transaction, seal: &Seal) -> Result<(), StoreError> {
    tx.execute(
        "UPDATE log_files SET sealed_at = ?2, seal_reason = ?3 WHERE id = ?1 AND sealed_at IS NULL",
        params![seal.log_file_id, seal.at_ms, seal.reason],
    )?;
    Ok(())
}

fn insert_event(tx: &Transaction, event: &NewEvent) -> Result<(), StoreError> {
    // A quarantined line is kept in `log_events` like any other and never
    // read; `tournament_player_state` is not a stored kind, so it is kept
    // without one (as a line quarantined before classifying).
    let kind = match event.kind {
        Some(EventKind::TournamentPlayerState) if !event.quarantined => {
            return insert_player_state(tx, event);
        }
        Some(EventKind::TournamentPlayerState) => None,
        other => other,
    };
    let (raw, compressed) = encode_raw(&event.raw)?;
    let s = &event.subjects;
    // ON CONFLICT on the key only: a replayed event is skipped, while a CHECK or
    // foreign key violation still fails the batch (INSERT OR IGNORE would hide it).
    tx.prepare_cached(
        "INSERT INTO log_events (log_file_id, byte_offset, kind, match_token, match_id, game_id,
             tournament_token, league_token, league_event_id, draft_token, logged_at, raw, compressed, quarantined)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
         ON CONFLICT (log_file_id, byte_offset) DO NOTHING",
    )?
    .execute(params![
        event.log_file_id,
        event.byte_offset as i64,
        kind.map(EventKind::as_str),
        s.match_token,
        s.match_id.map(|id| id as i64),
        s.game_id.map(|id| id as i64),
        s.tournament_token,
        s.league_token,
        s.league_event_id.map(|id| id as i64),
        s.draft_token,
        event.logged_at_ms,
        raw,
        compressed,
        event.quarantined,
    ])?;
    Ok(())
}

/// A `FlsPlayerEventStateChangedMessage` (`InPlayoffs`) goes to its own fact
/// table (event overlay spec 7.1), for every event: the tournament sync reads it
/// only for joined ones. A line without a token or that does not decode is
/// skipped. Keyed by log position, so a re-read writes nothing new.
fn insert_player_state(tx: &Transaction, event: &NewEvent) -> Result<(), StoreError> {
    let Some(token) = &event.subjects.tournament_token else {
        return Ok(());
    };
    let Some(mtgo_protocol::tournament::TournamentFact::PlayerState {
        new_state,
        in_playoffs,
    }) = mtgo_protocol::tournament::decode_player_state(&event.raw)
    else {
        return Ok(());
    };
    tx.prepare_cached(
        "INSERT INTO tournament_player_states
             (log_file_id, byte_offset, tournament_token, new_state, in_playoffs, logged_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (log_file_id, byte_offset) DO NOTHING",
    )?
    .execute(params![
        event.log_file_id,
        event.byte_offset as i64,
        token,
        new_state,
        in_playoffs,
        event.logged_at_ms,
    ])?;
    Ok(())
}

/// `accounts.login_id` is unique: a login id another account already holds is
/// left out (the log is hostile input), so the batch never fails on it.
fn upsert_account(tx: &Transaction, account: &NewAccount) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO accounts (username, login_id, first_seen_at, last_login_at)
         VALUES (?1, (SELECT ?2 WHERE NOT EXISTS
                     (SELECT 1 FROM accounts WHERE login_id = ?2 AND username <> ?1)), ?3, ?4)
         ON CONFLICT (username) DO UPDATE SET
             login_id = COALESCE(excluded.login_id, accounts.login_id),
             last_login_at = CASE
                 WHEN excluded.last_login_at IS NULL THEN accounts.last_login_at
                 WHEN accounts.last_login_at IS NULL
                     OR excluded.last_login_at > accounts.last_login_at
                     THEN excluded.last_login_at
                 ELSE accounts.last_login_at
             END",
        params![
            account.username,
            account.login_id.map(|id| id as i64),
            account.seen_at_ms,
            account.logged_at_ms
        ],
    )?;
    Ok(())
}

const RECORD_FAILURE: &str =
    "INSERT INTO match_failures (token, reason, failed_at, derived_version)
     VALUES (?1, ?2, ?3, ?4)
     ON CONFLICT (token) DO UPDATE SET reason = excluded.reason,
         failed_at = excluded.failed_at, derived_version = excluded.derived_version";

const CLEAR_SKIP: &str = "DELETE FROM untracked_matches WHERE match_token = ?1";

fn write_match(
    tx: &Transaction,
    write: &MatchWrite,
    platform: Platform,
    now_ms: i64,
) -> Result<(), StoreError> {
    match write {
        MatchWrite::Upsert { view, revision } => {
            // Built after all: a token is never both skipped and a row.
            tx.prepare_cached(CLEAR_SKIP)?.execute([&view.token])?;
            upsert_match(tx, view, *revision, platform, now_ms)
        }
        MatchWrite::Skip { token, username } => {
            tx.prepare_cached(
                "INSERT INTO untracked_matches (match_token, account_id, skipped_at)
                 SELECT ?1, id, ?3 FROM accounts WHERE username = ?2
                 ON CONFLICT (match_token) DO NOTHING",
            )?
            .execute(params![token, username, now_ms])?;
            Ok(())
        }
        MatchWrite::Remove { token } => {
            tx.prepare_cached(CLEAR_SKIP)?.execute([token])?;
            remove_match(tx, token, now_ms)?;
            tx.execute("DELETE FROM match_failures WHERE token = ?1", [token])?;
            Ok(())
        }
        MatchWrite::Attention { token, reason } => {
            // A manual match is never derived, so it never fails.
            if is_manual_token(tx, token)? {
                return Ok(());
            }
            // A match the tracker failed to build is a failure, never also a
            // skip: a tracked account's skip row is queued again at every start.
            tx.prepare_cached(CLEAR_SKIP)?.execute([token])?;
            tx.execute(
                "UPDATE matches SET needs_attention = 1, attention_reason = ?2 WHERE token = ?1",
                params![token, reason],
            )?;
            tx.execute(
                RECORD_FAILURE,
                params![token, reason, now_ms, PROJECTION_VERSION],
            )?;
            Ok(())
        }
    }
}

/// A log match that is no longer visible. A row that carries input (a note,
/// a hidden mark, a hand league link, the player's league choice or edit
/// time (`no_league`, `edited_at`), a shared replay link, a 0.x limited deck,
/// a hand-set archetype or the player's fix, `corrected_at`) is never deleted
/// (target schema section 2): it is
/// detached (`detached_at`, the first removal's time; lists leave it out) and
/// its games go, with the game sync's stamp (`games_synced_at`), until the
/// projection derives it again and the game sync rewrites its cards. A
/// corrected row keeps its games: they are the fix, and the projection never
/// writes them again. Any other row is
/// deleted. The deck link alone is not input: the linker writes it and links
/// the match again when it comes back (v1 keeps no record of who set a link).
fn remove_match(tx: &Transaction, token: &str, now_ms: i64) -> Result<(), StoreError> {
    let detached = tx.execute(
        "UPDATE matches SET detached_at = COALESCE(detached_at, ?2), games_synced_at = NULL
         WHERE token = ?1 AND source = 'log'
           AND (notes IS NOT NULL OR hidden_at IS NOT NULL OR league_linked = 1
                OR no_league = 1 OR edited_at IS NOT NULL
                OR replay_share_uuid IS NOT NULL OR corrected_at IS NOT NULL
                OR EXISTS (SELECT 1 FROM limited_deck_snapshots
                           WHERE match_id = matches.id AND source = 'imported')
                OR EXISTS (SELECT 1 FROM match_archetypes
                           WHERE match_id = matches.id AND manual = 1))",
        params![token, now_ms],
    )?;
    if detached > 0 {
        tx.execute(
            "DELETE FROM games
             WHERE match_id = (SELECT id FROM matches WHERE token = ?1 AND corrected_at IS NULL)",
            [token],
        )?;
    } else {
        tx.execute(
            "DELETE FROM matches WHERE token = ?1 AND source = 'log'",
            [token],
        )?;
    }
    Ok(())
}

/// The id of the row under `token` when the player has corrected it
/// (`corrected_at` is set): the projection and the import leave its columns
/// and games alone (edit a match spec section 2).
pub(crate) fn corrected_match_id(tx: &Transaction, token: &str) -> Result<Option<i64>, StoreError> {
    Ok(tx
        .prepare_cached("SELECT id FROM matches WHERE token = ?1 AND corrected_at IS NOT NULL")?
        .query_row([token], |row| row.get(0))
        .optional()?)
}

pub(crate) fn match_id_of(tx: &Transaction, token: &str) -> Result<Option<i64>, StoreError> {
    Ok(tx
        .prepare_cached("SELECT id FROM matches WHERE token = ?1")?
        .query_row([token], |row| row.get(0))
        .optional()?)
}

/// Whether `token` belongs to a manual match (Plan L). The projection never
/// writes over one: its token is a fresh UUID no log carries, so this only
/// guards against the impossible.
pub(crate) fn is_manual_token(tx: &Transaction, token: &str) -> Result<bool, StoreError> {
    Ok(tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM matches WHERE token = ?1 AND source = 'manual')",
        [token],
        |row| row.get(0),
    )?)
}

/// The id of `username` in a registry (`accounts` or `opponents`), creating it.
/// Registries are never deleted (spec 4.1 rule 2).
pub(crate) fn registry_id(
    tx: &Transaction,
    table: &str,
    username: &str,
    now_ms: i64,
) -> Result<i64, StoreError> {
    tx.execute(
        &format!(
            "INSERT INTO {table} (username, first_seen_at) VALUES (?1, ?2) ON CONFLICT (username) DO NOTHING"
        ),
        params![username, now_ms],
    )?;
    Ok(tx.query_row(
        &format!("SELECT id FROM {table} WHERE username = ?1"),
        [username],
        |row| row.get(0),
    )?)
}

/// The projection's columns only: never `deck_version_id` (the deck
/// linker's), `notes`, `hidden_at`, `league_linked`, `no_league`, `edited_at`,
/// `corrected_at` or `replay_share_*` (input), so a rebuild
/// keeps them. MTGO's verdict (`history_outcome`, #454) is kept too, and is
/// the outcome while it is set. A history or 0.x copy under the same token becomes the live
/// row (`source = 'log'`), and a detached row is attached again
/// (`detached_at` is the projection's). A manual row and a corrected one
/// (`corrected_at` set) are never written: `upsert_match` checks first, and
/// the WHERE refuses should that check ever be skipped.
const UPSERT_MATCH: &str =
    "INSERT INTO matches (token, mtgo_match_id, account_id, opponent_id, source, format,
         match_type, tournament_event_id, tournament_round, state, outcome, started_at, ended_at,
         last_activity_at, needs_attention, attention_reason, revision, derived_version,
         league_token, game_structure)
     VALUES (?1, ?2, ?3, ?4, 'log', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 0, NULL, ?14, ?15,
         ?16, ?17)
     ON CONFLICT (token) DO UPDATE SET
         mtgo_match_id = excluded.mtgo_match_id, account_id = excluded.account_id,
         opponent_id = excluded.opponent_id, source = 'log', format = excluded.format,
         match_type = excluded.match_type, tournament_event_id = excluded.tournament_event_id,
         tournament_round = excluded.tournament_round, league_token = excluded.league_token,
         game_structure = excluded.game_structure, state = excluded.state,
         outcome = COALESCE(matches.history_outcome, excluded.outcome),
         started_at = excluded.started_at, ended_at = excluded.ended_at,
         last_activity_at = excluded.last_activity_at, needs_attention = 0, attention_reason = NULL,
         revision = excluded.revision, derived_version = excluded.derived_version,
         games_won_imported = NULL, games_lost_imported = NULL, legacy_history = 0,
         detached_at = NULL
     WHERE matches.source <> 'manual' AND matches.corrected_at IS NULL
     RETURNING id";

fn upsert_match(
    tx: &Transaction,
    view: &MatchView,
    revision: u64,
    platform: Platform,
    now_ms: i64,
) -> Result<(), StoreError> {
    // A manual match keeps its row and games: no live result replaces it
    // (and the upsert's WHERE refuses to, should this check ever be skipped).
    if is_manual_token(tx, &view.token)? {
        return Ok(());
    }
    // A corrected match keeps the player's fix: its columns, its games and
    // any copy's detail stay as they are (edit a match spec section 2). Only
    // the projection's own bookkeeping moves on: a failure clears, a
    // detached row is attached again, and the derivation version is
    // recorded, so the row is not left failed, hidden or stale.
    if let Some(match_id) = corrected_match_id(tx, &view.token)? {
        tx.prepare_cached(
            "UPDATE matches SET needs_attention = 0, attention_reason = NULL, detached_at = NULL,
                 derived_version = ?2
             WHERE id = ?1",
        )?
        .execute(params![match_id, PROJECTION_VERSION])?;
        tx.execute("DELETE FROM match_failures WHERE token = ?1", [&view.token])?;
        crate::arena_rank::set_match_rank(tx, match_id)?;
        return Ok(());
    }
    let account_id = crate::arena::local_account_id(tx, &view.local_username, platform, now_ms)?;
    let opponent_id = match &view.opponent {
        Some(name) => Some(registry_id(tx, "opponents", name, now_ms)?),
        None => None,
    };
    // Live data wins: a 0.x or history copy under this token loses its
    // per-game rows and game log; the game sync writes the live ones.
    let copy: Option<i64> = tx
        .prepare_cached(
            "SELECT id FROM matches WHERE token = ?1 AND source IN ('imported', 'history')",
        )?
        .query_row([&view.token], |row| row.get(0))
        .optional()?;
    if let Some(copy_id) = copy {
        crate::game_sync::clear_match_detail(tx, copy_id)?;
        // A pulled Arena or Endstep copy's source and its derive stamp go
        // too: the readers and the Arena game sync take the local facts.
        crate::synced_sources::delete(tx, copy_id)?;
        tx.prepare_cached("DELETE FROM arena_game_syncs WHERE match_id = ?1")?
            .execute([copy_id])?;
    }
    // The Brawl repair rewrites `ARENA_BRAWL` to another format: the stored
    // detection is stale, so detect it again. Nothing else resets it.
    tx.prepare_cached(
        "UPDATE matches SET archetypes_detected_at = NULL
         WHERE token = ?1 AND archetypes_detected_at IS NOT NULL
           AND format = 'ARENA_BRAWL' AND format IS NOT ?2",
    )?
    .execute(params![view.token, view.format])?;
    let match_id: i64 = tx.query_row(
        UPSERT_MATCH,
        params![
            view.token,
            view.mtgo_match_id.map(|id| id as i64),
            account_id,
            opponent_id,
            view.format,
            view.match_type,
            view.tournament_event_id.map(|id| id as i64),
            view.tournament_round,
            state_str(view.state),
            outcome_str(view.outcome),
            view.started_at_ms,
            view.ended_at_ms,
            view.last_activity_ms,
            revision as i64,
            PROJECTION_VERSION,
            view.league_token,
            view.game_structure,
        ],
        |row| row.get(0),
    )?;
    tx.execute("DELETE FROM match_failures WHERE token = ?1", [&view.token])?;
    for game in &view.games {
        free_game_id(tx, match_id, game)?;
    }
    for game in &view.games {
        upsert_game(tx, match_id, game)?;
    }
    // Only a game the match no longer has goes. This relies on the
    // projection numbering a match's n games 1 to n, with no gap or repeat
    // (`mtgo_match::games`); a gap would delete a game it still has.
    debug_assert!(
        {
            let mut numbers: Vec<u32> = view.games.iter().map(|game| game.game_number).collect();
            numbers.sort_unstable();
            numbers
                .iter()
                .enumerate()
                .all(|(index, number)| *number as usize == index + 1)
        },
        "the projection numbers games 1 to n: {:?}",
        view.games
            .iter()
            .map(|game| game.game_number)
            .collect::<Vec<_>>()
    );
    tx.prepare_cached("DELETE FROM games WHERE match_id = ?1 AND game_number > ?2")?
        .execute(params![match_id, view.games.len() as i64])?;
    // After the live games, so a corrected copy's fix lands on them.
    if let Some(mtgo_match_id) = view.mtgo_match_id {
        remove_imported_copies(tx, match_id, &view.token, mtgo_match_id as i64)?;
    }
    crate::arena_rank::set_match_rank(tx, match_id)?;
    Ok(())
}

/// A live match replaces 0.x's imported copy of it (importer spec 5) and a
/// history import of it (Plan M): each gave the copy another token but the
/// real Match ID. Each copy with this Match ID and another token hands its
/// records to the live row (`hand_over_records`) and is deleted (its games
/// cascade).
fn remove_imported_copies(
    tx: &Transaction,
    live_id: i64,
    live_token: &str,
    mtgo_match_id: i64,
) -> Result<(), StoreError> {
    let mut copies: Vec<i64> = Vec::new();
    {
        let mut stmt = tx.prepare_cached(
            "SELECT id FROM matches
             WHERE mtgo_match_id = ?1 AND source IN ('imported', 'history') AND token <> ?2",
        )?;
        let rows = stmt.query_map(params![mtgo_match_id, live_token], |row| row.get(0))?;
        for row in rows {
            copies.push(row?);
        }
    }
    for copy_id in copies {
        hand_over_records(tx, copy_id, live_id)?;
        tx.execute(
            "DELETE FROM matches WHERE id = ?1 AND source IN ('imported', 'history')",
            [copy_id],
        )?;
    }
    Ok(())
}

/// Move a 0.x or history copy's own records to the match that replaces it,
/// each only where the new match has none: its note, hidden mark (with the
/// league it was deleted from), deck link, shared replay link (all three
/// columns) and league link (a manual league's
/// or an imported run's: `league_id` with `league_linked`; columns on the
/// row), the player's league choice as one unit (a link, or out of every
/// league with `no_league`) when the new match has neither, the newer
/// `edited_at` (league self-management spec 4.6), its limited registered deck
/// snapshots (Plan N), its archetype rows per seat (archetypes spec section 4: a hand-set row also wins over a
/// detected one, which is not input) and the player's fix of it
/// (`hand_over_correction`). Its imported 0.x detail (Plan O) goes.
/// The copy's row itself is left to the caller, and so is writing the new
/// match's games first: the fix is written over them.
pub(crate) fn hand_over_records(
    tx: &Transaction,
    copy_id: i64,
    new_id: i64,
) -> Result<(), StoreError> {
    // A note or deck link the new match has already wins. The newer player
    // edit time wins (NULL is oldest), so a later pull or push compares the
    // right one.
    tx.execute(
        "UPDATE matches SET
             notes = COALESCE(matches.notes, copy.notes),
             deck_version_id = COALESCE(matches.deck_version_id, copy.deck_version_id),
             edited_at = CASE
                 WHEN copy.edited_at IS NULL THEN matches.edited_at
                 WHEN matches.edited_at IS NULL THEN copy.edited_at
                 ELSE max(matches.edited_at, copy.edited_at)
             END
         FROM (SELECT notes, deck_version_id, edited_at FROM matches WHERE id = ?1) AS copy
         WHERE matches.id = ?2",
        params![copy_id, new_id],
    )?;
    // Its shared replay link, whole, unless the new match has its own.
    tx.execute(
        "UPDATE matches SET replay_share_uuid = copy.replay_share_uuid,
             replay_share_url = copy.replay_share_url, replay_shared_at = copy.replay_shared_at
         FROM (SELECT replay_share_uuid, replay_share_url, replay_shared_at FROM matches
               WHERE id = ?1 AND replay_share_uuid IS NOT NULL) AS copy
         WHERE matches.id = ?2 AND matches.replay_share_uuid IS NULL",
        params![copy_id, new_id],
    )?;
    // Its league link too (a manual league's, or an imported run's, so the
    // 0.x run keeps the match), unless the new match is linked already (a
    // match is in one league). A link wins over the run the sync gave the new
    // match.
    // The new match's own choice (a link, or out of every league) wins; a
    // league the sync gave it is not a choice.
    tx.execute(
        "UPDATE matches SET league_id = copy.league_id, league_linked = 1
         FROM (SELECT league_id FROM matches WHERE id = ?1 AND league_linked = 1) AS copy
         WHERE matches.id = ?2 AND matches.league_linked = 0 AND matches.no_league = 0",
        params![copy_id, new_id],
    )?;
    // The copy taken out of every league: so is the new match.
    tx.execute(
        "UPDATE matches SET no_league = 1, league_linked = 0, league_id = NULL
         WHERE id = ?2 AND league_linked = 0 AND no_league = 0
           AND EXISTS (SELECT 1 FROM matches c WHERE c.id = ?1 AND c.no_league = 1)",
        params![copy_id, new_id],
    )?;
    // A copy the player removed from stats stays removed as the new match,
    // with the league it was deleted from (spec section 13; the trigger
    // recorded the new match's league, which the copy's replaces). Written
    // after the league statements above (spec section 13, write order). A
    // new match hidden already keeps its own time; the league it was
    // deleted from is the one it holds after those statements (a link taken
    // from the copy is recorded by the trigger).
    let took_hide = tx.execute(
        "UPDATE matches SET hidden_at = copy.hidden_at
         FROM (SELECT hidden_at FROM matches WHERE id = ?1 AND hidden_at IS NOT NULL) AS copy
         WHERE matches.id = ?2 AND matches.hidden_at IS NULL",
        params![copy_id, new_id],
    )?;
    if took_hide > 0 {
        tx.execute(
            "UPDATE matches SET deleted_from_league_id =
                 (SELECT deleted_from_league_id FROM matches WHERE id = ?1)
             WHERE id = ?2",
            params![copy_id, new_id],
        )?;
    }
    // Its limited registered decks (0.x's, Plan N), so a run never loses the
    // deck of a match a live row took over and an import rerun finds it under
    // the live match. A deck of the same source the new match has already
    // wins; the rest go with the copy (CASCADE).
    tx.execute(
        "UPDATE OR IGNORE limited_deck_snapshots SET match_id = ?2 WHERE match_id = ?1",
        params![copy_id, new_id],
    )?;
    // The copy's recorded "none" (`opponent_archetype_none`, input, final
    // review I2) moves to the new match unless that one has a hand-set
    // opponent; the new match's detected opponent goes with it, and
    // detection, imports and pulls leave the seat empty from then on. Only
    // the flag is the choice (#300 F3): a copy marked detected without an
    // opponent row (its hand-set archetype deleted, say) hands over nothing.
    tx.execute(
        "UPDATE matches SET opponent_archetype_none = 1
         WHERE id = ?2
           AND EXISTS (SELECT 1 FROM matches c WHERE c.id = ?1 AND c.opponent_archetype_none = 1)
           AND NOT EXISTS (SELECT 1 FROM match_archetypes x
                           WHERE x.match_id = ?2 AND x.is_opponent = 1 AND x.manual = 1)",
        params![copy_id, new_id],
    )?;
    tx.execute(
        "DELETE FROM match_archetypes
         WHERE match_id = ?2 AND is_opponent = 1 AND manual = 0
           AND EXISTS (SELECT 1 FROM matches WHERE id = ?2 AND opponent_archetype_none = 1)",
        params![copy_id, new_id],
    )?;
    // Its archetype rows, seat by seat. Otherwise detection's marker stays
    // the new match's own: detection fills the seats left and replaces only
    // auto rows. An opponent the new match has set to "none" stays empty
    // (#300 R3).
    tx.execute(
        "DELETE FROM match_archetypes
         WHERE match_id = ?2 AND manual = 0 AND is_opponent IN (
             SELECT is_opponent FROM match_archetypes WHERE match_id = ?1 AND manual = 1)",
        params![copy_id, new_id],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO match_archetypes
             (match_id, is_opponent, archetype_id, confidence, manual)
         SELECT ?2, is_opponent, archetype_id, confidence, manual
         FROM match_archetypes
         WHERE match_id = ?1
           AND NOT (is_opponent = 1
                    AND EXISTS (SELECT 1 FROM matches
                                WHERE id = ?2 AND opponent_archetype_none = 1))",
        params![copy_id, new_id],
    )?;
    hand_over_correction(tx, copy_id, new_id)?;
    // The copy's per-game rows and game log never move: the new match has
    // its own data, so they go with the copy (CASCADE).
    Ok(())
}

/// A corrected copy's fix (edit a match spec section 2) moves to the match
/// that replaces it, so the player's fix survives the hand-over: its
/// outcome, times, opponent and `corrected_at`, and each game's result,
/// play or draw and turns by game number (a game the new match lacks is
/// added, one above the copy's count goes). The new match's own fix wins
/// when it is the newer. The new match is corrected from then on, so the
/// projection and the import leave it alone.
fn hand_over_correction(tx: &Transaction, copy_id: i64, new_id: i64) -> Result<(), StoreError> {
    let taken = tx.execute(
        "UPDATE matches SET outcome = copy.outcome, started_at = copy.started_at,
             ended_at = copy.ended_at, opponent_id = copy.opponent_id,
             corrected_at = copy.corrected_at,
             games_won_imported = NULL, games_lost_imported = NULL
         FROM (SELECT outcome, started_at, ended_at, opponent_id, corrected_at FROM matches
               WHERE id = ?1 AND corrected_at IS NOT NULL) AS copy
         WHERE matches.id = ?2
           AND (matches.corrected_at IS NULL OR matches.corrected_at < copy.corrected_at)",
        params![copy_id, new_id],
    )?;
    if taken == 0 {
        return Ok(());
    }
    tx.execute(
        "INSERT INTO games (match_id, game_number, result, end_reason, on_play, local_mulligans,
             opp_mulligans, turn_count, started_at, ended_at)
         SELECT ?2, game_number, result, 'unknown', on_play, 0, 0, turn_count, started_at,
             ended_at
         FROM games WHERE match_id = ?1 ORDER BY game_number
         ON CONFLICT (match_id, game_number) DO UPDATE SET result = excluded.result,
             on_play = excluded.on_play, turn_count = excluded.turn_count",
        params![copy_id, new_id],
    )?;
    tx.execute(
        "DELETE FROM games WHERE match_id = ?2
           AND game_number > (SELECT COUNT(*) FROM games WHERE match_id = ?1)",
        params![copy_id, new_id],
    )?;
    Ok(())
}

/// A stored game of this match that holds `game`'s Game ID under another
/// game number (the games were renumbered between derivations) lets it go,
/// so the upserts never break `UNIQUE (match_id, mtgo_game_id)` part way;
/// each stored game gets its own ID back from its upsert. Run for every game
/// before any upsert, so a view that repeats a Game ID still fails.
fn free_game_id(tx: &Transaction, match_id: i64, game: &GameView) -> Result<(), StoreError> {
    let Some(id) = game.mtgo_game_id else {
        return Ok(());
    };
    tx.prepare_cached(
        "UPDATE games SET mtgo_game_id = NULL
         WHERE match_id = ?1 AND mtgo_game_id = ?2 AND game_number <> ?3",
    )?
    .execute(params![match_id, id as i64, game.game_number])?;
    Ok(())
}

/// Upsert one game in place by `(match_id, game_number)`, writing only the
/// projection's columns, so its id and every other column survive. An Arena
/// game the Arena game sync derived (it has local rows with zone data)
/// keeps the sync's `on_play`, mulligans, turn count and times: the game
/// messages tell them better than the projector.
fn upsert_game(tx: &Transaction, match_id: i64, game: &GameView) -> Result<(), StoreError> {
    let mtgo_game_id = game.mtgo_game_id.map(|id| id as i64);
    let keep = arena_game_derived(tx, match_id, game.game_number)?;
    tx.prepare_cached(
        "INSERT INTO games (match_id, mtgo_game_id, game_number, result, end_reason, on_play,
             local_mulligans, opp_mulligans, local_die_roll, opp_die_roll, turn_count, started_at, ended_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT (match_id, game_number) DO UPDATE SET
             mtgo_game_id = excluded.mtgo_game_id, result = excluded.result,
             end_reason = excluded.end_reason,
             on_play = CASE WHEN ?14 THEN games.on_play ELSE excluded.on_play END,
             local_mulligans = CASE WHEN ?14 THEN games.local_mulligans
                 ELSE excluded.local_mulligans END,
             opp_mulligans = CASE WHEN ?14 THEN games.opp_mulligans
                 ELSE excluded.opp_mulligans END,
             local_die_roll = excluded.local_die_roll, opp_die_roll = excluded.opp_die_roll,
             turn_count = CASE WHEN ?14 THEN games.turn_count ELSE excluded.turn_count END,
             started_at = CASE WHEN ?14 THEN games.started_at ELSE excluded.started_at END,
             ended_at = CASE WHEN ?14 THEN games.ended_at ELSE excluded.ended_at END",
    )?
    .execute(params![
        match_id,
        mtgo_game_id,
        game.game_number,
        game_result_str(game.result),
        end_reason_str(game.end_reason),
        game.on_play,
        game.local_mulligans,
        game.opp_mulligans,
        game.local_die_roll,
        game.opp_die_roll,
        game.turn_count,
        game.started_at_ms,
        game.ended_at_ms,
        keep,
    ])?;
    Ok(())
}

/// Whether game `game_number` of the match is an Arena account's game the
/// Arena game sync wrote local rows with zone data for.
fn arena_game_derived(
    tx: &Transaction,
    match_id: i64,
    game_number: u32,
) -> Result<bool, StoreError> {
    // MTGO games never read the Arena rows.
    let arena: bool = tx
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM matches m
                 JOIN accounts a ON a.id = m.account_id AND a.platform = 'arena'
                 WHERE m.id = ?1)",
        )?
        .query_row([match_id], |row| row.get(0))?;
    if !arena {
        return Ok(false);
    }
    Ok(tx
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM games g
                 JOIN game_cards c ON c.game_id = g.id AND c.has_zone_data = 1
                 WHERE g.match_id = ?1 AND g.game_number = ?2)",
        )?
        .query_row(params![match_id, game_number], |row| row.get(0))?)
}

fn update_file(tx: &Transaction, file: &FileProgress) -> Result<(), StoreError> {
    tx.prepare_cached(
        "UPDATE log_files SET cursor_offset = ?2, clock_date = ?3, clock_last_secs = ?4,
             last_observed_size = ?5, head_hash = ?6, head_len = ?7, anchor_offset = ?8,
             anchor_hash = ?9, local_username = ?10, local_login_id = ?11, last_seen_at = ?12
         WHERE id = ?1",
    )?
    .execute(params![
        file.log_file_id,
        file.cursor_offset as i64,
        file.clock.date.to_string(),
        file.clock.last.map(|time| time.num_seconds_from_midnight()),
        file.last_observed_size as i64,
        hex(file.head_hash),
        file.head_len as i64,
        file.anchor.map(|(offset, _)| offset as i64),
        file.anchor.map(|(_, hash)| hex(hash)),
        file.local_username,
        file.local_login_id.map(|id| id as i64),
        file.seen_at_ms,
    ])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use mtgo_match::{GameResult, Outcome};

    fn deck_sync(deck_id: &str, account: &str, platform: Platform) -> DeckWrite {
        DeckWrite::SyncFile {
            file: decks::DeckFile {
                name: "Qdeck".to_string(),
                net_deck_id: deck_id.to_string(),
                format: "CMODERN".to_string(),
                timestamp_ms: 50_000,
                cards: Vec::new(),
            },
            signature: format!("Q-{deck_id}"),
            account: Some(account.to_string()),
            platform,
            synced_at_ms: 60_000,
            identity: crate::decks::IdentityUpdate::Keep,
        }
    }

    /// #862: an Arena or Endstep match or deck naming an account that is not
    /// stored makes it on its own platform, never as an MTGO account; an
    /// MTGO one is made as before.
    #[test]
    fn a_missing_account_is_made_on_the_writes_platform() {
        let (_dir, mut conn) = db("write-missing-account-platform");
        let mut batch = WriteBatch {
            now_ms: 60_000,
            ..WriteBatch::default()
        };
        let named = |token: &str, account: &str| {
            let mut played = view(token, "Qopponent", 1_000);
            played.local_username = account.to_string();
            MatchWrite::Upsert {
                view: Box::new(played),
                revision: 1,
            }
        };
        batch.push_match(named("q-arena", "Qarena#12345"), Platform::Arena);
        batch.push_match(named("q-endstep", "endstep:QESID1"), Platform::Endstep);
        batch.push_match(named("q-mtgo", "Qmtgo"), Platform::Mtgo);
        batch.decks = vec![
            deck_sync("q-arena-deck", "Qdeckonly#00003", Platform::Arena),
            deck_sync("q-endstep-deck", "endstep:QESID2", Platform::Endstep),
            deck_sync("q-mtgo-deck", "Qmtgodeck", Platform::Mtgo),
        ];
        commit(&mut conn, &batch).expect("commit");
        let rows: Vec<String> = conn
            .prepare(
                "SELECT platform || ' ' || username || ' ' || COALESCE(arena_user_id, '-')
                 FROM accounts ORDER BY username",
            )
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        assert_eq!(
            rows,
            vec![
                "arena Qarena#12345 -".to_string(),
                "arena Qdeckonly#00003 -".to_string(),
                "mtgo Qmtgo -".to_string(),
                "mtgo Qmtgodeck -".to_string(),
                "endstep endstep:QESID1 QESID1".to_string(),
                "endstep endstep:QESID2 QESID2".to_string(),
            ]
        );
    }

    #[test]
    fn player_state_lines_go_to_their_own_table_once() {
        let (_dir, mut conn) = db("write-player-states");
        let file = insert_log_file(&conn, &new_log_file("C:/mtgo.log")).expect("file");
        let token = "63c90ead-8c2c-407d-9f10-67378e0b96d0";
        let line = format!(
            "18:02:56 [INF] (Game Management|Processing Registered Handler for FlsPlayerEventStateChangedMessage in TournamentNotJoinedBetweenRoundsState) Processor: TournamentNotJoinedBetweenRoundsState Message: {{\"Token\":\"{token}\",\"NewState\":24,\"InPlayoffs\":true}} Receiver: WotC.MtGO.Client.Model.Play.TournamentEvent.Tournament"
        );
        let event = |offset: u64, raw: &str, token: Option<&str>| NewEvent {
            log_file_id: file,
            byte_offset: offset,
            kind: Some(EventKind::TournamentPlayerState),
            subjects: mtgo_protocol::Subjects {
                tournament_token: token.map(str::to_string),
                ..mtgo_protocol::Subjects::default()
            },
            logged_at_ms: 1_000,
            raw: raw.to_string(),
            quarantined: false,
        };
        let batch = WriteBatch {
            events: vec![
                event(10, &line, Some(token)),
                event(
                    20,
                    "18:02:57 [INF] (Game Management|damaged) Message: {",
                    Some(token),
                ),
                event(30, &line, None),
            ],
            ..WriteBatch::default()
        };
        commit(&mut conn, &batch).expect("first");
        commit(&mut conn, &batch).expect("a re-read");
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM tournament_player_states"),
            1
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM log_events"), 0);
        let row: (String, i64, i64, i64) = conn
            .query_row(
                "SELECT tournament_token, new_state, in_playoffs, logged_at FROM tournament_player_states",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("row");
        assert_eq!(row, (token.to_string(), 24, 1, 1_000));
    }

    #[test]
    fn a_quarantined_player_state_line_is_kept_quarantined_not_read() {
        let (_dir, mut conn) = db("write-player-state-quarantined");
        let file = insert_log_file(&conn, &new_log_file("C:/mtgo.log")).expect("file");
        let token = "63c90ead-8c2c-407d-9f10-67378e0b96d0";
        let line = format!(
            "18:02:56 [INF] (Game Management|Processing Registered Handler for FlsPlayerEventStateChangedMessage in TournamentNotJoinedBetweenRoundsState) Processor: TournamentNotJoinedBetweenRoundsState Message: {{\"Token\":\"{token}\",\"NewState\":24,\"InPlayoffs\":true}} Receiver: WotC.MtGO.Client.Model.Play.TournamentEvent.Tournament"
        );
        let batch = WriteBatch {
            events: vec![NewEvent {
                log_file_id: file,
                byte_offset: 10,
                kind: Some(EventKind::TournamentPlayerState),
                subjects: mtgo_protocol::Subjects {
                    tournament_token: Some(token.to_string()),
                    ..mtgo_protocol::Subjects::default()
                },
                logged_at_ms: 1_000,
                raw: line,
                quarantined: true,
            }],
            ..WriteBatch::default()
        };
        commit(&mut conn, &batch).expect("commit");
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM tournament_player_states"),
            0
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM log_events WHERE quarantined = 1 AND kind IS NULL"
            ),
            1
        );
    }

    #[test]
    fn settings_rows_are_inserted_then_replaced() {
        let (_dir, mut conn) = db("write-settings");
        let first = WriteBatch {
            settings: vec![
                ("close_to_tray".to_string(), "true".to_string()),
                ("log_folder".to_string(), "null".to_string()),
            ],
            ..WriteBatch::default()
        };
        assert!(
            !first.is_empty(),
            "a batch with only settings is work to do"
        );
        commit(&mut conn, &first).expect("first");
        let second = WriteBatch {
            settings: vec![("close_to_tray".to_string(), "false".to_string())],
            ..WriteBatch::default()
        };
        commit(&mut conn, &second).expect("second");
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM settings"), 2);
        let value: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'close_to_tray'",
                [],
                |row| row.get(0),
            )
            .expect("value");
        assert_eq!(value, "false");
    }

    #[test]
    fn a_batch_commits_events_progress_accounts_and_matches_together() {
        let (_dir, mut conn) = db("write-batch");
        let file = insert_log_file(&conn, &new_log_file("C:/mtgo.log")).expect("file");
        let batch = WriteBatch {
            events: vec![token_event(file, 0, TOKEN), token_event(file, 10, TOKEN)],
            files: vec![progress(file, 20)],
            accounts: vec![NewAccount {
                username: "Qlocal".to_string(),
                login_id: Some(42),
                seen_at_ms: 1,
                logged_at_ms: None,
            }],
            matches: vec![MatchWrite::Upsert {
                view: Box::new(view(TOKEN, "Qopponent", 1_000)),
                revision: 3,
            }],
            now_ms: 7,
            ..WriteBatch::default()
        };
        commit(&mut conn, &batch).expect("commit");
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM log_events"), 2);
        assert_eq!(number(&conn, "SELECT cursor_offset FROM log_files"), 20);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM games"), 3);
        assert_eq!(number(&conn, "SELECT revision FROM matches"), 3);
        assert_eq!(
            number(
                &conn,
                "SELECT login_id FROM accounts WHERE username = 'Qlocal'"
            ),
            42
        );
    }

    #[test]
    fn replayed_events_are_ignored() {
        let (_dir, mut conn) = db("write-replay");
        let file = insert_log_file(&conn, &new_log_file("C:/mtgo.log")).expect("file");
        let batch = WriteBatch {
            events: vec![token_event(file, 0, TOKEN)],
            ..WriteBatch::default()
        };
        commit(&mut conn, &batch).expect("first");
        commit(&mut conn, &batch).expect("second");
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM log_events"), 1);
    }

    #[test]
    fn a_failing_batch_commits_nothing() {
        let (_dir, mut conn) = db("write-atomic");
        let file = insert_log_file(&conn, &new_log_file("C:/mtgo.log")).expect("file");
        let first = WriteBatch {
            files: vec![progress(file, 5)],
            ..WriteBatch::default()
        };
        commit(&mut conn, &first).expect("first");

        let broken = WriteBatch {
            events: vec![token_event(file, 0, TOKEN), token_event(999, 0, TOKEN)],
            files: vec![progress(file, 50)],
            ..WriteBatch::default()
        };
        assert!(commit(&mut conn, &broken).is_err());
        assert_eq!(number(&conn, "SELECT cursor_offset FROM log_files"), 5);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM log_events"), 0);
    }

    #[test]
    fn a_failure_after_many_rows_were_written_rolls_back_all_of_them() {
        let (_dir, mut conn) = db("write-atomic-late");
        let file = insert_log_file(&conn, &new_log_file("C:/mtgo.log")).expect("file");
        commit(
            &mut conn,
            &WriteBatch {
                files: vec![progress(file, 5)],
                ..WriteBatch::default()
            },
        )
        .expect("first");

        // Two games with the same Game ID break UNIQUE (match_id, mtgo_game_id)
        // after the seal, event, account, registry rows, match row and first game
        // were already written inside the transaction.
        let mut clashing = view(TOKEN, "Qopponent", 1_000);
        clashing.games[1].mtgo_game_id = clashing.games[0].mtgo_game_id;
        let broken = WriteBatch {
            seals: vec![Seal {
                log_file_id: file,
                reason: "truncated".to_string(),
                at_ms: 9,
            }],
            events: vec![token_event(file, 0, TOKEN)],
            accounts: vec![NewAccount {
                username: "Qother".to_string(),
                login_id: Some(1),
                seen_at_ms: 1,
                logged_at_ms: None,
            }],
            matches: vec![MatchWrite::Upsert {
                view: Box::new(clashing),
                revision: 1,
            }],
            files: vec![progress(file, 50)],
            ..WriteBatch::default()
        };
        assert!(commit(&mut conn, &broken).is_err());
        assert_eq!(number(&conn, "SELECT cursor_offset FROM log_files"), 5);
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM log_files WHERE sealed_at IS NOT NULL"
            ),
            0
        );
        for table in ["log_events", "accounts", "opponents", "matches", "games"] {
            assert_eq!(
                number(&conn, &format!("SELECT COUNT(*) FROM {table}")),
                0,
                "{table}"
            );
        }
    }

    /// Upsert `view` with `format`, after setting the row's detection stamp
    /// (when it exists) to 5; returns the stamp afterwards.
    fn detected_after_format_change(stored: Option<&str>, next: &str) -> Option<i64> {
        let (_dir, mut conn) = db("write-format-reset");
        let mut first = view(TOKEN, "Qopponent", 1_000);
        first.format = stored.map(str::to_string);
        let upsert = |view: MatchView, revision: u64| WriteBatch {
            matches: vec![MatchWrite::Upsert {
                view: Box::new(view),
                revision,
            }],
            ..WriteBatch::default()
        };
        commit(&mut conn, &upsert(first.clone(), 1)).expect("first");
        conn.execute("UPDATE matches SET archetypes_detected_at = 5", [])
            .expect("stamp");
        first.format = Some(next.to_string());
        commit(&mut conn, &upsert(first, 2)).expect("second");
        conn.query_row("SELECT archetypes_detected_at FROM matches", [], |row| {
            row.get(0)
        })
        .expect("stamp")
    }

    #[test]
    fn a_format_change_keeps_detection_except_for_the_brawl_repair() {
        assert_eq!(detected_after_format_change(None, "CMODERN"), Some(5));
        assert_eq!(
            detected_after_format_change(Some("CMODERN"), "CPAUPER"),
            Some(5)
        );
        assert_eq!(
            detected_after_format_change(Some("ARENA_HISTORIC"), "ARENA_BRAWL"),
            Some(5)
        );
        assert_eq!(
            detected_after_format_change(Some("ARENA_BRAWL"), "ARENA_HISTORIC_BRAWL"),
            None
        );
    }

    #[test]
    fn upserting_a_match_replaces_its_games_and_keeps_registries() {
        let (_dir, mut conn) = db("write-upsert");
        let mut first = view(TOKEN, "Qopponent", 1_000);
        commit(
            &mut conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(first.clone()),
                    revision: 1,
                }],
                ..WriteBatch::default()
            },
        )
        .expect("first");
        first.games.truncate(1);
        first.outcome = Outcome::Loss;
        commit(
            &mut conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(first),
                    revision: 2,
                }],
                ..WriteBatch::default()
            },
        )
        .expect("second");
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), 1);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM games"), 1);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM accounts"), 1);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM opponents"), 1);
        assert_eq!(number(&conn, "SELECT revision FROM matches"), 2);
        let outcome: String = conn
            .query_row("SELECT outcome FROM matches", [], |row| row.get(0))
            .expect("outcome");
        assert_eq!(outcome, "loss");
    }

    #[test]
    fn removing_a_match_deletes_its_games_but_not_registries() {
        let (_dir, mut conn) = db("write-remove");
        for write in [
            MatchWrite::Upsert {
                view: Box::new(view(TOKEN, "Qopponent", 1_000)),
                revision: 1,
            },
            MatchWrite::Remove {
                token: TOKEN.to_string(),
            },
        ] {
            commit(
                &mut conn,
                &WriteBatch {
                    matches: vec![write],
                    ..WriteBatch::default()
                },
            )
            .expect("commit");
        }
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), 0);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM games"), 0);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM opponents"), 1);
    }

    #[test]
    fn attention_marks_a_match_until_it_is_derived_again() {
        let (_dir, mut conn) = db("write-attention");
        let upsert = MatchWrite::Upsert {
            view: Box::new(view(TOKEN, "Qopponent", 1_000)),
            revision: 1,
        };
        let attention = MatchWrite::Attention {
            token: TOKEN.to_string(),
            reason: "projection panicked: boom".to_string(),
        };
        for (write, flagged) in [(upsert.clone(), 0), (attention, 1), (upsert, 0)] {
            commit(
                &mut conn,
                &WriteBatch {
                    matches: vec![write],
                    ..WriteBatch::default()
                },
            )
            .expect("commit");
            assert_eq!(
                number(&conn, "SELECT needs_attention FROM matches"),
                flagged
            );
        }
    }

    #[test]
    fn large_raw_text_is_compressed_and_round_trips() {
        let (_dir, mut conn) = db("write-compress");
        let file = insert_log_file(&conn, &new_log_file("C:/mtgo.log")).expect("file");
        let big = "x".repeat(10_000);
        let mut event = token_event(file, 0, TOKEN);
        event.raw = big.clone();
        commit(
            &mut conn,
            &WriteBatch {
                events: vec![event],
                ..WriteBatch::default()
            },
        )
        .expect("commit");
        let (raw, compressed): (Vec<u8>, bool) = conn
            .query_row("SELECT raw, compressed FROM log_events", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .expect("row");
        assert!(compressed);
        assert!(raw.len() < big.len());
        assert_eq!(decode_raw(raw, compressed).expect("decode"), big);
    }

    #[test]
    fn a_file_is_sealed_once_with_its_first_reason() {
        let (_dir, mut conn) = db("write-seal");
        let file = insert_log_file(&conn, &new_log_file("C:/mtgo.log")).expect("file");
        for (reason, at_ms) in [("truncated", 9), ("head_changed", 10)] {
            commit(
                &mut conn,
                &WriteBatch {
                    seals: vec![Seal {
                        log_file_id: file,
                        reason: reason.to_string(),
                        at_ms,
                    }],
                    ..WriteBatch::default()
                },
            )
            .expect("seal");
        }
        let (reason, at): (String, i64) = conn
            .query_row("SELECT seal_reason, sealed_at FROM log_files", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .expect("row");
        assert_eq!((reason.as_str(), at), ("truncated", 9));
    }

    #[test]
    fn the_derivation_version_is_recorded() {
        let (_dir, mut conn) = db("write-derivation");
        commit(
            &mut conn,
            &WriteBatch {
                derivation: Some(5),
                now_ms: 9,
                ..WriteBatch::default()
            },
        )
        .expect("commit");
        assert_eq!(
            number(
                &conn,
                "SELECT version FROM derivations WHERE name = 'match_projection'"
            ),
            5
        );
    }

    #[test]
    fn a_duplicate_event_key_is_ignored_and_keeps_the_first_row() {
        let (_dir, mut conn) = db("write-duplicate-key");
        let file = insert_log_file(&conn, &new_log_file("C:/mtgo.log")).expect("file");
        let mut second = token_event(file, 0, OTHER);
        second.raw = "a different event at the same key".to_string();
        // Twice in one batch, then again in a later one.
        let batch = WriteBatch {
            events: vec![token_event(file, 0, TOKEN), second.clone()],
            ..WriteBatch::default()
        };
        commit(&mut conn, &batch).expect("duplicate key in one batch");
        commit(
            &mut conn,
            &WriteBatch {
                events: vec![second],
                ..WriteBatch::default()
            },
        )
        .expect("duplicate key in a later batch");
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM log_events"), 1);
        let (token, raw): (String, String) = conn
            .query_row(
                "SELECT match_token, CAST(raw AS TEXT) FROM log_events",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("row");
        assert_eq!((token.as_str(), raw.as_str()), (TOKEN, "state change"));
    }

    #[test]
    fn hashes_round_trip_through_hex_at_the_edges_of_u64() {
        let values = [
            0,
            1,
            i64::MAX as u64,
            i64::MAX as u64 + 1,
            u64::MAX - 1,
            u64::MAX,
        ];
        for value in values {
            let text = hex(value);
            assert_eq!(text.len(), 16, "{value}");
            assert_eq!(unhex(&text), Some(value), "{value}");
        }
        assert_eq!(hex(u64::MAX), "ffffffffffffffff");
        assert_eq!(unhex("not hex"), None);
        assert_eq!(unhex("1ffffffffffffffff"), None, "17 digits overflow u64");
    }

    #[test]
    fn hashes_above_i64_max_are_stored_and_read_back() {
        use crate::read::active_log_files;
        let (_dir, mut conn) = db("write-big-hashes");
        let mut new_file = new_log_file("C:/mtgo.log");
        new_file.identity_hash = u64::MAX;
        new_file.head_hash = i64::MAX as u64 + 1;
        let file = insert_log_file(&conn, &new_file).expect("file");
        let mut moved = progress(file, 20);
        moved.head_hash = u64::MAX;
        moved.anchor = Some((16_384, i64::MAX as u64 + 7));
        commit(
            &mut conn,
            &WriteBatch {
                files: vec![moved.clone()],
                ..WriteBatch::default()
            },
        )
        .expect("commit");
        let active = active_log_files(&conn).expect("files");
        assert_eq!(active[0].head_hash, u64::MAX);
        assert_eq!(active[0].anchor, moved.anchor);

        new_file.path = "C:/other.log".to_string();
        let fresh = insert_log_file(&conn, &new_file).expect("file");
        let stored = active_log_files(&conn).expect("files");
        let row = stored.iter().find(|f| f.id == fresh).expect("fresh row");
        assert_eq!(row.head_hash, i64::MAX as u64 + 1);
    }

    fn live(view: MatchView) -> WriteBatch {
        WriteBatch {
            matches: vec![MatchWrite::Upsert {
                view: Box::new(view),
                revision: 1,
            }],
            now_ms: 9_000,
            ..WriteBatch::default()
        }
    }

    fn remove(token: &str) -> WriteBatch {
        WriteBatch {
            matches: vec![MatchWrite::Remove {
                token: token.to_string(),
            }],
            now_ms: 9_500,
            ..WriteBatch::default()
        }
    }

    #[test]
    fn a_removed_match_with_a_note_is_detached_and_comes_back_with_it() {
        use crate::read::list_matches;
        for input in [
            "notes = 'Q kept note'",
            "hidden_at = 77",
            "notes = 'Q kept note', hidden_at = 77",
            "league_id = 5, league_linked = 1",
            // League self-management spec 4.6: the player's league choice and
            // edit time are input too.
            "no_league = 1",
            "edited_at = 55",
            "imported snapshot",
            // A shared replay link (replays spec section 8).
            "replay_share_uuid = 'Q-uuid', replay_share_url = 'https://mymtgo.test/r/Q', replay_shared_at = 5",
        ] {
            let (_dir, mut conn) = db("write-remove-detaches");
            commit(&mut conn, &live(view(TOKEN, "Qopponent", 1_000))).expect("live");
            // A hand link needs its league; a 0.x deck is a snapshot row.
            conn.execute_batch(
                "INSERT INTO leagues (id, run_key, source, kind, state, name, started_at, created_at,
                     token)
                     VALUES (5, 'manual:Q', 'manual', 'constructed', 'complete', 'Qleague', 1, 1,
                         'manual_Q');",
            )
            .expect("league");
            if input == "imported snapshot" {
                conn.execute(
                    "INSERT INTO limited_deck_snapshots (match_id, source, cards, signature, captured_at)
                     SELECT id, 'imported', '[]', 'Q-sig', 1 FROM matches",
                    [],
                )
                .expect("snapshot");
            } else {
                conn.execute(&format!("UPDATE matches SET {input}"), [])
                    .expect("input");
            }
            let id = number(&conn, "SELECT id FROM matches");

            commit(&mut conn, &remove(TOKEN)).expect("remove");
            assert_eq!(
                number(&conn, "SELECT detached_at FROM matches"),
                9_500,
                "kept, detached ({input})"
            );
            assert_eq!(number(&conn, "SELECT COUNT(*) FROM listed_matches"), 0);
            assert!(list_matches(&conn, 10, 0).expect("list").is_empty());
            assert_eq!(number(&conn, "SELECT COUNT(*) FROM games"), 0);
            // A second removal keeps the first time.
            let mut again = remove(TOKEN);
            again.now_ms = 9_900;
            commit(&mut conn, &again).expect("again");
            assert_eq!(number(&conn, "SELECT detached_at FROM matches"), 9_500);

            commit(&mut conn, &live(view(TOKEN, "Qopponent", 1_000))).expect("back");
            assert_eq!(
                number(
                    &conn,
                    &format!(
                        "SELECT COUNT(*) FROM matches WHERE id = {id} AND detached_at IS NULL
                         AND {}",
                        if input == "imported snapshot" {
                            "EXISTS (SELECT 1 FROM limited_deck_snapshots
                                     WHERE match_id = matches.id AND source = 'imported')"
                                .to_string()
                        } else {
                            input.replace(", ", " AND ")
                        }
                    )
                ),
                1,
                "the same row is back with its input ({input})"
            );
            assert_eq!(number(&conn, "SELECT COUNT(*) FROM games"), 3);
        }
    }

    /// The league unit and edit time of the match `token`:
    /// `no_league|league_linked|league_id|edited_at`.
    fn player_unit(conn: &Connection, token: &str) -> String {
        conn.query_row(
            "SELECT no_league || '|' || league_linked || '|' || ifnull(league_id, '-') || '|'
                 || ifnull(edited_at, '-')
             FROM matches WHERE token = ?1",
            [token],
            |row| row.get(0),
        )
        .expect("the match")
    }

    // League self-management spec 4.6: a live match replacing a copy takes
    // the copy's league unit when it has no choice of its own, and the newer
    // edit time.
    #[test]
    fn a_live_match_replacing_a_copy_takes_its_league_choice_and_newer_edit() {
        use crate::import::import_matches;
        // (the copy's input, the live row's input, the live row after)
        let cases = [
            ("no_league = 1, edited_at = 55", "", "1|0|-|55"),
            ("no_league = 1, edited_at = 55", "league_id = 2", "1|0|-|55"),
            (
                "no_league = 1, edited_at = 55",
                "league_id = 2, league_linked = 1, edited_at = 90",
                "0|1|2|90",
            ),
            (
                "league_id = 1, league_linked = 1, edited_at = 55",
                "edited_at = 40",
                "0|1|1|55",
            ),
            (
                "edited_at = 30",
                "no_league = 1, edited_at = 60",
                "1|0|-|60",
            ),
        ];
        for (copy_input, live_input, expected) in cases {
            let (_dir, mut conn) = db("write-live-takes-league-unit");
            import_matches(&mut conn, &[imported(OTHER, Some(42))], 5_000).expect("0.x");
            insert_manual_league(&conn, 1, None, None, "Qone", 1, 1);
            insert_manual_league(&conn, 2, None, None, "Qtwo", 1, 1);
            conn.execute(
                &format!("UPDATE matches SET {copy_input} WHERE token = ?1"),
                [OTHER],
            )
            .expect("the copy's input");
            // The live match is stored first (Match ID 99), with its own input.
            let mut captured = view(TOKEN, "Qopponent", 1_000);
            commit(&mut conn, &live(captured.clone())).expect("the live match");
            if !live_input.is_empty() {
                conn.execute(
                    &format!("UPDATE matches SET {live_input} WHERE token = ?1"),
                    [TOKEN],
                )
                .expect("the live row's input");
            }

            captured.mtgo_match_id = Some(42);
            commit(&mut conn, &live(captured)).expect("live upsert");

            assert_eq!(
                number(&conn, "SELECT COUNT(*) FROM matches"),
                1,
                "the copy went"
            );
            assert_eq!(
                player_unit(&conn, TOKEN),
                expected,
                "copy: {copy_input}; live: {live_input}"
            );
        }
    }

    #[test]
    fn a_copy_under_the_live_token_keeps_its_player_columns() {
        use crate::history_import::{HistoryMatch, import_history};
        let (_dir, mut conn) = db("write-live-same-token-keeps-unit");
        let copy = HistoryMatch {
            token: Some(TOKEN.to_string()),
            mtgo_match_id: 99,
            local_username: "Qlocal".to_string(),
            opponent: Some("Qopponent".to_string()),
            format: Some("CMODERN".to_string()),
            match_type: "Constructed".to_string(),
            outcome: "win".to_string(),
            started_at_ms: 500,
            ended_at_ms: None,
            games_won: 2,
            games_lost: 1,
            games: Vec::new(),
            deck_version_id: 10,
            log: Vec::new(),
        };
        import_history(&mut conn, &[copy], 5_000).expect("history");
        conn.execute(
            "UPDATE matches SET no_league = 1, hidden_at = 55, edited_at = 55",
            [],
        )
        .expect("player edits");

        commit(&mut conn, &live(view(TOKEN, "Qopponent", 1_000))).expect("live upsert");

        assert_eq!(player_unit(&conn, TOKEN), "1|0|-|55");
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE source = 'log' AND hidden_at = 55"
            ),
            1
        );
    }

    fn archetype_id(conn: &Connection, uuid: &str) -> i64 {
        conn.execute(
            "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
             VALUES (?1, 'Q' || ?1, 'modern', 'manual', 1, 1)",
            [uuid],
        )
        .expect("archetype");
        conn.last_insert_rowid()
    }

    fn archetype_rows(conn: &Connection) -> Vec<(String, i64, i64, i64)> {
        let mut stmt = conn
            .prepare(
                "SELECT m.token, a.is_opponent, a.archetype_id, a.manual
                 FROM match_archetypes a JOIN matches m ON m.id = a.match_id
                 ORDER BY m.token, a.is_opponent",
            )
            .expect("prepare");
        stmt.query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .expect("query")
        .map(|row| row.expect("row"))
        .collect()
    }

    /// A hand-set archetype is input (archetypes spec section 4): the match
    /// carrying it is detached, not deleted. A detected one is not input.
    #[test]
    fn a_detached_match_keeps_its_manual_archetype() {
        for manual in [1, 0] {
            let (_dir, mut conn) = db("write-remove-manual-archetype");
            commit(&mut conn, &live(view(TOKEN, "Qopponent", 1_000))).expect("live");
            let id = archetype_id(&conn, "q-a");
            conn.execute(
                "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence,
                     manual)
                 SELECT id, 1, ?1, 1.0, ?2 FROM matches",
                params![id, manual],
            )
            .expect("seat");
            // The game sync had stamped it (its game_cards were written).
            conn.execute("UPDATE matches SET games_synced_at = 5", [])
                .expect("stamp");
            commit(&mut conn, &remove(TOKEN)).expect("remove");
            if manual == 1 {
                assert_eq!(number(&conn, "SELECT detached_at FROM matches"), 9_500);
                assert_eq!(archetype_rows(&conn).len(), 1);
                // Its games went, so the game sync stamp goes too (Focus 5).
                assert_eq!(
                    number(
                        &conn,
                        "SELECT COUNT(*) FROM matches WHERE games_synced_at IS NULL"
                    ),
                    1,
                    "a detached match is not synced"
                );
                commit(&mut conn, &live(view(TOKEN, "Qopponent", 1_000))).expect("back");
                assert_eq!(
                    archetype_rows(&conn),
                    [(TOKEN.to_string(), 1, id, 1)],
                    "back with its archetype"
                );
                // Re-derived: detection waits for the game sync again.
                let match_id = number(&conn, "SELECT id FROM matches");
                assert!(
                    crate::archetypes::detection_input(&conn, match_id)
                        .expect("input")
                        .is_none(),
                    "not ready before its game sync"
                );
            } else {
                assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), 0);
                assert_eq!(number(&conn, "SELECT COUNT(*) FROM match_archetypes"), 0);
            }
        }
    }

    /// `hand_over_records` moves a copy's archetype rows per seat: where the
    /// live match has none, and a hand-set one over a detected one.
    #[test]
    fn a_live_match_takes_the_imported_copys_archetype_rows() {
        use crate::import::import_matches;
        let (_dir, mut conn) = db("write-live-takes-archetypes");
        import_matches(&mut conn, &[imported(OTHER, Some(42))], 5_000).expect("0.x");
        let mut captured = view(TOKEN, "Qopponent", 1_000);
        captured.mtgo_match_id = Some(43);
        commit(&mut conn, &live(captured.clone())).expect("live");
        let a = archetype_id(&conn, "q-a");
        let b = archetype_id(&conn, "q-b");
        let c = archetype_id(&conn, "q-c");
        conn.execute_batch(&format!(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
             SELECT id, 0, {a}, 0.9, 0 FROM matches WHERE token = '{OTHER}';
             INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
             SELECT id, 1, {b}, 1.0, 1 FROM matches WHERE token = '{OTHER}';
             INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
             SELECT id, 0, {c}, 0.8, 0 FROM matches WHERE token = '{TOKEN}';
             INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
             SELECT id, 1, {c}, 0.8, 0 FROM matches WHERE token = '{TOKEN}';"
        ))
        .expect("seats");

        captured.mtgo_match_id = Some(42);
        commit(&mut conn, &live(captured)).expect("live replaces the copy");
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), 1);
        assert_eq!(
            archetype_rows(&conn),
            [(TOKEN.to_string(), 0, c, 0), (TOKEN.to_string(), 1, b, 1)],
            "the live player seat stays; the copy's hand-set opponent wins"
        );
    }

    /// #300 F3: only the recorded "none" (`opponent_archetype_none`) is the
    /// player's choice. A copy marked detected without an opponent row
    /// although its opponent showed cards (its hand-set archetype was
    /// deleted, say) hands over no choice: the live match keeps its
    /// detected opponent and its own marker.
    #[test]
    fn a_copy_without_the_none_flag_hands_over_no_choice() {
        use crate::import::import_matches;
        let (_dir, mut conn) = db("write-live-no-none-flag");
        import_matches(&mut conn, &[imported(OTHER, Some(42))], 5_000).expect("0.x");
        let mut captured = view(TOKEN, "Qopponent", 1_000);
        captured.mtgo_match_id = Some(43);
        commit(&mut conn, &live(captured.clone())).expect("live");
        let a = archetype_id(&conn, "q-a");
        conn.execute_batch(&format!(
            "INSERT INTO games (match_id, game_number, result, end_reason)
                 SELECT id, 9, 'win', 'win' FROM matches WHERE token = '{OTHER}';
             INSERT INTO game_cards (game_id, side, catalog_id, seen)
                 SELECT g.id, 'opponent', 501, 2 FROM games g
                 JOIN matches m ON m.id = g.match_id WHERE m.token = '{OTHER}';
             UPDATE matches SET archetypes_detected_at = 77 WHERE token = '{OTHER}';
             INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
                 SELECT id, 1, {a}, 0.8, 0 FROM matches WHERE token = '{TOKEN}';
             UPDATE matches SET archetypes_detected_at = 55 WHERE token = '{TOKEN}';"
        ))
        .expect("copy without a choice");

        captured.mtgo_match_id = Some(42);
        commit(&mut conn, &live(captured)).expect("live replaces the copy");
        assert_eq!(
            archetype_rows(&conn),
            [(TOKEN.to_string(), 1, a, 0)],
            "the live match keeps its detected opponent"
        );
        assert_eq!(
            number(&conn, "SELECT archetypes_detected_at FROM matches"),
            55
        );
        assert_eq!(
            number(&conn, "SELECT opponent_archetype_none FROM matches"),
            0
        );
    }

    /// #300 R3: the live match's own "none" stays: a copy's hand-set
    /// opponent does not move onto it, and the flag stays set.
    #[test]
    fn a_live_match_with_none_keeps_it_over_the_copys_hand_set_opponent() {
        use crate::import::import_matches;
        let (_dir, mut conn) = db("write-live-none-beats-copy");
        import_matches(&mut conn, &[imported(OTHER, Some(42))], 5_000).expect("0.x");
        let mut captured = view(TOKEN, "Qopponent", 1_000);
        captured.mtgo_match_id = Some(43);
        commit(&mut conn, &live(captured.clone())).expect("live");
        let a = archetype_id(&conn, "q-a");
        let b = archetype_id(&conn, "q-b");
        conn.execute_batch(&format!(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
                 SELECT id, 1, {b}, 1.0, 1 FROM matches WHERE token = '{OTHER}';
             INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
                 SELECT id, 0, {a}, 0.9, 0 FROM matches WHERE token = '{OTHER}';
             UPDATE matches SET opponent_archetype_none = 1, archetypes_detected_at = 55
                 WHERE token = '{TOKEN}';"
        ))
        .expect("live chose none");

        captured.mtgo_match_id = Some(42);
        commit(&mut conn, &live(captured)).expect("live replaces the copy");
        assert_eq!(
            archetype_rows(&conn),
            [(TOKEN.to_string(), 0, a, 0)],
            "the player seat moves; the opponent stays none"
        );
        assert_eq!(
            number(&conn, "SELECT opponent_archetype_none FROM matches"),
            1
        );
    }

    /// Final review I2: the copy's recorded "none"
    /// (`opponent_archetype_none`) moves to the live match whatever the
    /// copy's marker or cards say (here the marker was cleared by a
    /// re-detection and the opponent showed nothing).
    #[test]
    fn a_live_match_takes_the_copys_none_flag() {
        use crate::import::import_matches;
        let (_dir, mut conn) = db("write-live-takes-none-flag");
        import_matches(&mut conn, &[imported(OTHER, Some(42))], 5_000).expect("0.x");
        let mut captured = view(TOKEN, "Qopponent", 1_000);
        captured.mtgo_match_id = Some(43);
        commit(&mut conn, &live(captured.clone())).expect("live");
        let a = archetype_id(&conn, "q-a");
        conn.execute_batch(&format!(
            "UPDATE matches SET opponent_archetype_none = 1, archetypes_detected_at = NULL
                 WHERE token = '{OTHER}';
             INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
                 SELECT id, 1, {a}, 0.8, 0 FROM matches WHERE token = '{TOKEN}';"
        ))
        .expect("copy chose none");

        captured.mtgo_match_id = Some(42);
        commit(&mut conn, &live(captured)).expect("live replaces the copy");
        assert!(
            archetype_rows(&conn).is_empty(),
            "no detected opponent on the live match"
        );
        assert_eq!(
            number(&conn, "SELECT opponent_archetype_none FROM matches"),
            1
        );
    }

    #[test]
    fn a_copy_without_opponent_cards_hands_over_no_marker() {
        use crate::import::import_matches;
        let (_dir, mut conn) = db("write-live-no-cards");
        import_matches(&mut conn, &[imported(OTHER, Some(42))], 5_000).expect("0.x");
        conn.execute(
            &format!("UPDATE matches SET archetypes_detected_at = 77 WHERE token = '{OTHER}'"),
            [],
        )
        .expect("detected with nothing to read");
        let mut captured = view(TOKEN, "Qopponent", 1_000);
        captured.mtgo_match_id = Some(42);
        commit(&mut conn, &live(captured)).expect("live replaces the copy");
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE archetypes_detected_at IS NULL"
            ),
            1,
            "the live match is detected"
        );
    }

    /// Detection is not a derivation (archetypes spec section 6.3): a
    /// projection rebuild, a game re-sync and the league sync leave the
    /// archetype rows, the marker and the deck archetype alone.
    #[test]
    fn rebuilds_never_touch_archetype_rows() {
        let (_dir, mut conn) = db("write-rebuild-archetypes");
        conn.execute_batch(
            "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source)
                 VALUES (1, 'Q-deck', 'Qdeck', 'CMODERN', 1, 1, 'file');
             INSERT INTO deck_versions (id, deck_id, signature, modified_at)
                 VALUES (10, 1, 'Q-one', 1);",
        )
        .expect("deck");
        commit(&mut conn, &live(view(TOKEN, "Qopponent", 1_000))).expect("live");
        crate::game_sync::sync_games(&mut conn, &[TOKEN.to_string()], &[], chrono_tz::UTC)
            .expect("game sync");
        let a = archetype_id(&conn, "q-a");
        conn.execute_batch(&format!(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
             SELECT id, 0, {a}, 0.9, 0 FROM matches;
             INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
             SELECT id, 1, {a}, 1.0, 1 FROM matches;
             UPDATE matches SET archetypes_detected_at = 4242, deck_version_id = 10;
             UPDATE decks SET archetype_id = {a};"
        ))
        .expect("detected");
        let before = archetype_rows(&conn);

        let mut rebuilt = live(view(TOKEN, "Qopponent", 1_000));
        if let MatchWrite::Upsert { revision, .. } = &mut rebuilt.matches[0] {
            *revision = 2;
        }
        commit(&mut conn, &rebuilt).expect("projection rebuild");
        crate::game_sync::sync_games(&mut conn, &[TOKEN.to_string()], &[], chrono_tz::UTC)
            .expect("game re-sync");
        crate::game_sync::sync_all_games(&mut conn, chrono_tz::UTC).expect("game backfill");
        crate::league_sync::sync_all(&mut conn, chrono_tz::UTC, 9_999).expect("league sync");

        assert_eq!(archetype_rows(&conn), before);
        assert_eq!(
            number(&conn, "SELECT archetypes_detected_at FROM matches"),
            4242
        );
        assert_eq!(number(&conn, "SELECT archetype_id FROM decks"), a);
    }

    fn guide_rows(conn: &Connection) -> Vec<(i64, i64, i64, i64, String)> {
        let mut stmt = conn
            .prepare(
                "SELECT g.id, g.deck_id, g.archetype_id, g.updated_at,
                     COALESCE((SELECT group_concat(c.direction || c.oracle_id || c.quantity, ',')
                               FROM sideboard_guide_cards c WHERE c.guide_id = g.id), '')
                     || '|' ||
                     COALESCE((SELECT group_concat(n.id || n.body || n.created_at, ',')
                               FROM deck_archetype_notes n
                               WHERE n.deck_id = g.deck_id AND n.archetype_id = g.archetype_id), '')
                 FROM sideboard_guides g ORDER BY g.id",
            )
            .expect("prepare");
        stmt.query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .expect("query")
        .map(|row| row.expect("row"))
        .collect()
    }

    /// Input is never overwritten (spec section 21): a projection rebuild,
    /// a game re-sync and backfill, the league sync and a deck file sync
    /// leave a deck's guides, their cards and its notes alone.
    #[test]
    fn guides_survive_every_rebuild() {
        let (_dir, mut conn) = db("write-rebuild-guides");
        conn.execute_batch(
            "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source)
                 VALUES (1, 'Q-deck', 'Qdeck', 'CMODERN', 1, 1, 'file');
             INSERT INTO deck_versions (id, deck_id, signature, modified_at)
                 VALUES (10, 1, 'Q-one', 1);",
        )
        .expect("deck");
        commit(&mut conn, &live(view(TOKEN, "Qopponent", 1_000))).expect("live");
        crate::game_sync::sync_games(&mut conn, &[TOKEN.to_string()], &[], chrono_tz::UTC)
            .expect("game sync");
        let a = archetype_id(&conn, "q-a");
        conn.execute_batch(&format!(
            "UPDATE matches SET deck_version_id = 10;
             INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
             SELECT id, 1, {a}, 1.0, 0 FROM matches;
             INSERT INTO sideboard_guides (id, deck_id, archetype_id, created_at, updated_at)
                 VALUES (5, 1, {a}, 2, 3);
             INSERT INTO sideboard_guide_cards (guide_id, oracle_id, direction, quantity)
                 VALUES (5, 'o-in', 'in', 2), (5, 'o-out', 'out', 1);
             INSERT INTO deck_archetype_notes (deck_id, archetype_id, body, created_at)
                 VALUES (1, {a}, 'Qnote', 4);"
        ))
        .expect("guide");
        let before = guide_rows(&conn);
        assert_eq!(before.len(), 1);

        let mut rebuilt = live(view(TOKEN, "Qopponent", 1_000));
        if let MatchWrite::Upsert { revision, .. } = &mut rebuilt.matches[0] {
            *revision = 2;
        }
        commit(&mut conn, &rebuilt).expect("projection rebuild");
        crate::game_sync::sync_games(&mut conn, &[TOKEN.to_string()], &[], chrono_tz::UTC)
            .expect("game re-sync");
        crate::game_sync::sync_all_games(&mut conn, chrono_tz::UTC).expect("game backfill");
        crate::league_sync::sync_all(&mut conn, chrono_tz::UTC, 9_999).expect("league sync");
        let deck_file = crate::decks::DeckWrite::SyncFile {
            file: decks::DeckFile {
                name: "Qdeck renamed".to_string(),
                net_deck_id: "Q-deck".to_string(),
                format: "CMODERN".to_string(),
                timestamp_ms: 50_000,
                cards: vec![decks::DeckCard {
                    catalog_id: 1001,
                    quantity: 4,
                    sideboard: false,
                }],
            },
            signature: "Q-two".to_string(),
            account: None,
            platform: Platform::Mtgo,
            synced_at_ms: 60_000,
            identity: crate::decks::IdentityUpdate::Keep,
        };
        commit(
            &mut conn,
            &WriteBatch {
                decks: vec![deck_file],
                now_ms: 60_000,
                ..WriteBatch::default()
            },
        )
        .expect("deck sync");
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM deck_versions"),
            2,
            "the deck file was synced"
        );

        assert_eq!(guide_rows(&conn), before);
    }

    #[test]
    fn a_removed_match_without_input_is_deleted() {
        let (_dir, mut conn) = db("write-remove-deletes");
        conn.execute_batch(
            "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source)
                 VALUES (1, 'Q-deck', 'Qdeck', 'CMODERN', 1, 1, 'file');
             INSERT INTO deck_versions (id, deck_id, signature, modified_at)
                 VALUES (10, 1, 'Q-one', 1);",
        )
        .expect("deck");
        commit(&mut conn, &live(view(TOKEN, "Qopponent", 1_000))).expect("live");
        // The linker's link alone is derived, not input.
        conn.execute("UPDATE matches SET deck_version_id = 10", [])
            .expect("link");
        commit(&mut conn, &remove(TOKEN)).expect("remove");
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), 0);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM games"), 0);
    }

    #[test]
    fn a_login_id_another_account_holds_is_left_out() {
        let (_dir, mut conn) = db("write-login-id-unique");
        let account = |username: &str, login_id: u64| NewAccount {
            username: username.to_string(),
            login_id: Some(login_id),
            seen_at_ms: 1,
            logged_at_ms: None,
        };
        let batch = |accounts: Vec<NewAccount>| WriteBatch {
            accounts,
            ..WriteBatch::default()
        };
        commit(&mut conn, &batch(vec![account("Qfirst", 7)])).expect("first");
        commit(
            &mut conn,
            &batch(vec![account("Qsecond", 7), account("Qfirst", 7)]),
        )
        .expect("a clash never fails the batch");
        commit(&mut conn, &batch(vec![account("Qsecond", 8)])).expect("its own");
        let stored: Vec<(String, Option<i64>)> = {
            let mut stmt = conn
                .prepare("SELECT username, login_id FROM accounts ORDER BY id")
                .expect("prepare");
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .expect("query");
            rows.map(|row| row.expect("row")).collect()
        };
        assert_eq!(
            stored,
            vec![
                ("Qfirst".to_string(), Some(7)),
                ("Qsecond".to_string(), Some(8))
            ]
        );
    }

    /// Each game's `(game_number, id, result)`, by game number.
    fn games_of(conn: &Connection, token: &str) -> Vec<(i64, i64, String)> {
        let mut stmt = conn
            .prepare(
                "SELECT g.game_number, g.id, g.result FROM games g
                 JOIN matches m ON m.id = g.match_id
                 WHERE m.token = ?1 ORDER BY g.game_number",
            )
            .expect("prepare");
        let rows = stmt
            .query_map([token], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("query");
        rows.map(|row| row.expect("row")).collect()
    }

    #[test]
    fn games_upsert_in_place_and_drop_only_missing_game_numbers() {
        let (_dir, mut conn) = db("write-games-in-place");
        commit(&mut conn, &live(view(TOKEN, "Qopponent", 1_000))).expect("first");
        // Another match's games come after, so a rewritten game would get a new id.
        commit(&mut conn, &live(view(OTHER, "Qopponent", 2_000))).expect("other");
        let first = games_of(&conn, TOKEN);
        assert_eq!(first.len(), 3);
        let match_id = number(
            &conn,
            &format!("SELECT id FROM matches WHERE token = '{TOKEN}'"),
        );

        let mut changed = view(TOKEN, "Qopponent", 1_000);
        changed.games[1].result = GameResult::Win;
        commit(&mut conn, &live(changed)).expect("changed");
        let second = games_of(&conn, TOKEN);
        let ids = |games: &[(i64, i64, String)]| -> Vec<i64> {
            games.iter().map(|game| game.1).collect()
        };
        assert_eq!(ids(&second), ids(&first), "every game kept its id");
        assert_eq!(second[1].2, "win", "the projection's columns are updated");

        let mut shorter = view(TOKEN, "Qopponent", 1_000);
        shorter.games.truncate(2);
        commit(&mut conn, &live(shorter)).expect("shorter");
        let third = games_of(&conn, TOKEN);
        assert_eq!(
            ids(&third),
            ids(&first[..2]),
            "games 1 and 2 kept their ids"
        );
        assert_eq!(
            number(
                &conn,
                &format!("SELECT id FROM matches WHERE token = '{TOKEN}'")
            ),
            match_id
        );
        assert_eq!(
            games_of(&conn, OTHER).len(),
            3,
            "the other match is untouched"
        );

        // Game IDs that move between game numbers never break the upsert.
        let mut swapped = view(TOKEN, "Qopponent", 1_000);
        swapped.games.truncate(2);
        swapped.games[0].mtgo_game_id = Some(8);
        swapped.games[1].mtgo_game_id = Some(7);
        commit(&mut conn, &live(swapped)).expect("swapped");
        assert_eq!(ids(&games_of(&conn, TOKEN)), ids(&first[..2]));
        assert_eq!(
            number(
                &conn,
                &format!(
                    "SELECT CAST(group_concat(g.mtgo_game_id, '' ORDER BY g.game_number)
                         AS INTEGER)
                     FROM games g JOIN matches m ON m.id = g.match_id WHERE m.token = '{TOKEN}'"
                )
            ),
            87
        );
    }

    #[test]
    fn a_live_match_replacing_a_copy_takes_its_note_hidden_mark_and_deck_link() {
        use crate::history_import::{HistoryMatch, import_history};
        use crate::import::import_matches;
        for source in ["imported", "history"] {
            let (_dir, mut conn) = db("write-live-takes-inputs");
            conn.execute_batch(
                "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source)
                     VALUES (1, 'Q-deck', 'Qdeck', 'CMODERN', 1, 1, 'imported');
                 INSERT INTO deck_versions (id, deck_id, signature, modified_at)
                     VALUES (10, 1, 'Q-one', 1);",
            )
            .expect("deck");
            if source == "imported" {
                import_matches(&mut conn, &[imported(OTHER, Some(42))], 5_000).expect("0.x");
            } else {
                let copy = HistoryMatch {
                    token: Some(OTHER.to_string()),
                    mtgo_match_id: 42,
                    local_username: "Qlocal".to_string(),
                    opponent: Some("Qopponent".to_string()),
                    format: Some("CMODERN".to_string()),
                    match_type: "Constructed".to_string(),
                    outcome: "win".to_string(),
                    started_at_ms: 500,
                    ended_at_ms: None,
                    games_won: 2,
                    games_lost: 1,
                    games: Vec::new(),
                    deck_version_id: 10,
                    log: Vec::new(),
                };
                import_history(&mut conn, &[copy], 5_000).expect("history");
            }
            // The copy is deleted from league 3 (spec section 13).
            insert_manual_league(&conn, 3, None, None, "Q league", 0, 0);
            conn.execute(
                "UPDATE matches SET notes = 'Q copy note', hidden_at = 77, deck_version_id = 10,
                     replay_share_uuid = 'Q-uuid', replay_share_url = 'https://mymtgo.test/r/Q',
                     replay_shared_at = 5, league_id = 3
                 WHERE token = ?1",
                [OTHER],
            )
            .expect("the copy's input");
            assert_eq!(
                number(
                    &conn,
                    &format!("SELECT COUNT(*) FROM matches WHERE source = '{source}'")
                ),
                1
            );

            let mut captured = view(TOKEN, "Qopponent", 1_000);
            captured.mtgo_match_id = Some(42);
            commit(&mut conn, &live(captured)).expect("live upsert");

            let row: (String, String, Option<String>, Option<i64>, Option<i64>) = conn
                .query_row(
                    "SELECT token, source, notes, hidden_at, deck_version_id FROM matches",
                    [],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
                .expect("one match: the live one");
            assert_eq!(
                row,
                (
                    TOKEN.to_string(),
                    "log".to_string(),
                    Some("Q copy note".to_string()),
                    Some(77),
                    Some(10)
                ),
                "copy source: {source}"
            );
            assert_eq!(
                number(&conn, "SELECT deleted_from_league_id FROM matches"),
                3,
                "the hide comes with the league it was deleted from; copy source: {source}"
            );
            // The copy's shared replay link moves with it (replays spec
            // section 8).
            assert_eq!(
                number(
                    &conn,
                    "SELECT COUNT(*) FROM matches WHERE replay_share_uuid = 'Q-uuid'
                         AND replay_share_url = 'https://mymtgo.test/r/Q' AND replay_shared_at = 5"
                ),
                1,
                "copy source: {source}"
            );
        }
    }

    #[test]
    fn a_live_match_with_its_own_link_keeps_it_over_the_copys() {
        use crate::import::import_matches;
        let (_dir, mut conn) = db("write-live-keeps-link");
        import_matches(&mut conn, &[imported(OTHER, Some(42))], 5_000).expect("0.x");
        conn.execute(
            "UPDATE matches SET replay_share_uuid = 'Q-copy', replay_share_url = 'https://mymtgo.test/r/copy',
                 replay_shared_at = 5",
            [],
        )
        .expect("the copy's link");
        let mut captured = view(TOKEN, "Qopponent", 1_000);
        commit(&mut conn, &live(captured.clone())).expect("live first");
        conn.execute(
            "UPDATE matches SET replay_share_uuid = 'Q-live', replay_share_url = 'https://mymtgo.test/r/live',
                 replay_shared_at = 9 WHERE token = ?1",
            [TOKEN],
        )
        .expect("the live link");
        captured.mtgo_match_id = Some(42);
        commit(&mut conn, &live(captured)).expect("live upsert");
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE replay_share_uuid = 'Q-live'
                     AND replay_share_url = 'https://mymtgo.test/r/live' AND replay_shared_at = 9"
            ),
            1
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), 1);
    }

    #[test]
    fn a_live_upsert_removes_the_imported_copy_of_the_same_match() {
        use crate::import::import_matches;
        let (_dir, mut conn) = db("write-live-replaces-imported");
        // 0.x's history import: another token (OTHER), the real Match ID 42.
        let mut copy = imported(OTHER, Some(42));
        copy.note = Some("Q kept the note".to_string());
        // An imported row with another Match ID stays.
        let unrelated = imported("Q-unrelated-token", Some(43));
        import_matches(&mut conn, &[copy, unrelated], 5_000).expect("import");

        let mut captured = view(TOKEN, "Qopponent", 1_000);
        captured.mtgo_match_id = Some(42);
        commit(&mut conn, &live(captured)).expect("live upsert");

        assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), 2);
        let (token, source): (String, String) = conn
            .query_row(
                "SELECT token, source FROM matches WHERE mtgo_match_id = 42",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("the match");
        assert_eq!((token.as_str(), source.as_str()), (TOKEN, "log"));
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM games g JOIN matches m ON m.id = g.match_id WHERE m.mtgo_match_id = 42"
            ),
            3,
            "the live games only; the copy's games went with it"
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM games"), 6);
        let (note_token, body): (String, String) = conn
            .query_row(
                "SELECT token, notes FROM matches WHERE notes IS NOT NULL",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("the note moved");
        assert_eq!(
            (note_token.as_str(), body.as_str()),
            (TOKEN, "Q kept the note")
        );
    }

    #[test]
    fn a_live_upsert_keeps_the_imported_copy_hidden_under_the_live_token() {
        use crate::import::import_matches;
        let (_dir, mut conn) = db("write-live-keeps-hidden");
        import_matches(&mut conn, &[imported(OTHER, Some(42))], 5_000).expect("import");
        conn.execute(
            "UPDATE matches SET hidden_at = 77 WHERE token = ?1",
            [OTHER],
        )
        .expect("hide the copy");

        let mut captured = view(TOKEN, "Qopponent", 1_000);
        captured.mtgo_match_id = Some(42);
        commit(&mut conn, &live(captured)).expect("live upsert");

        let (token, at): (String, i64) = conn
            .query_row(
                "SELECT token, hidden_at FROM matches WHERE hidden_at IS NOT NULL",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("one hidden row");
        assert_eq!((token.as_str(), at), (TOKEN, 77));
    }

    #[test]
    fn a_note_already_on_the_live_token_wins_over_the_imported_copys_note() {
        use crate::import::import_matches;
        let (_dir, mut conn) = db("write-live-note-wins");
        let mut copy = imported(OTHER, Some(42));
        copy.note = Some("Q imported note".to_string());
        import_matches(&mut conn, &[copy], 5_000).expect("import");
        // The live match is stored (Match ID 99) and noted before its Match ID
        // turns out to be the copy's.
        commit(&mut conn, &live(view(TOKEN, "Qopponent", 1_000))).expect("live row");
        conn.execute(
            "UPDATE matches SET notes = 'Q live note' WHERE token = ?1",
            [TOKEN],
        )
        .expect("live note");

        let mut captured = view(TOKEN, "Qopponent", 1_000);
        captured.mtgo_match_id = Some(42);
        commit(&mut conn, &live(captured)).expect("live upsert");

        assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), 1);
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE notes IS NOT NULL"
            ),
            1
        );
        let body: String = conn
            .query_row(
                "SELECT notes FROM matches WHERE token = ?1",
                [TOKEN],
                |row| row.get(0),
            )
            .expect("live note");
        assert_eq!(body, "Q live note");
    }

    #[test]
    fn a_live_upsert_moves_the_imported_copys_deck_link_unless_it_has_one() {
        use crate::import::import_matches;
        for live_linked in [false, true] {
            let (_dir, mut conn) = db("write-live-moves-link");
            import_matches(&mut conn, &[imported(OTHER, Some(42))], 5_000).expect("import");
            conn.execute_batch(
                "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source)
                     VALUES (1, 'Q-deck', 'Qdeck', 'CMODERN', 1, 1, 'imported');
                 INSERT INTO deck_versions (id, deck_id, signature, modified_at)
                     VALUES (10, 1, 'Q-one', 1), (11, 1, 'Q-two', 2);",
            )
            .expect("deck");
            conn.execute(
                "UPDATE matches SET deck_version_id = 10 WHERE token = ?1",
                [OTHER],
            )
            .expect("the copy's link");
            if live_linked {
                commit(&mut conn, &live(view(TOKEN, "Qopponent", 1_000))).expect("live row");
                conn.execute(
                    "UPDATE matches SET deck_version_id = 11 WHERE token = ?1",
                    [TOKEN],
                )
                .expect("the live link");
            }

            let mut captured = view(TOKEN, "Qopponent", 1_000);
            captured.mtgo_match_id = Some(42);
            commit(&mut conn, &live(captured)).expect("live upsert");

            assert_eq!(
                number(
                    &conn,
                    "SELECT COUNT(*) FROM matches WHERE deck_version_id IS NOT NULL"
                ),
                1
            );
            let version = number(
                &conn,
                &format!("SELECT deck_version_id FROM matches WHERE token = '{TOKEN}'"),
            );
            let expected = if live_linked { 11 } else { 10 };
            assert_eq!(version, expected, "live linked: {live_linked}");
        }
    }

    /// A 0.x run's seed: `Q-0x-token`, started at 1.
    fn imported_seed(state: &str, kind: &str) -> crate::league_sync::ImportedSeed {
        crate::league_sync::ImportedSeed {
            token: "Q-0x-token".to_string(),
            event_id: None,
            name: "Qrun".to_string(),
            format: Some("CMODERN".to_string()),
            state: state.to_string(),
            started_at: 1,
            joined_at: None,
            completed_at: None,
            dropped_at: None,
            deck_version_id: None,
            kind: kind.to_string(),
            set_code: None,
            mtgo_course_id: None,
            legacy_id: None,
        }
    }

    #[test]
    fn a_live_upsert_moves_the_imported_copys_manual_league_link() {
        use crate::import::import_matches;
        for live_linked in [false, true] {
            let (_dir, mut conn) = db("write-live-moves-league-link");
            import_matches(&mut conn, &[imported(OTHER, Some(42))], 5_000).expect("import");
            insert_manual_league(&conn, 1, None, None, "Qone", 1, 1);
            insert_manual_league(&conn, 2, None, None, "Qtwo", 1, 1);
            link_league_match(&conn, 1, OTHER);
            if live_linked {
                // The live match is stored first, linked by hand.
                let first = view(TOKEN, "Qopponent", 1_000);
                commit(&mut conn, &live(first)).expect("the live match");
                link_league_match(&conn, 2, TOKEN);
            }

            let mut captured = view(TOKEN, "Qopponent", 1_000);
            captured.mtgo_match_id = Some(42);
            commit(&mut conn, &live(captured)).expect("live upsert");

            assert_eq!(
                number(
                    &conn,
                    "SELECT COUNT(*) FROM matches WHERE league_linked = 1"
                ),
                1
            );
            let league = number(
                &conn,
                &format!("SELECT league_id FROM matches WHERE token = '{TOKEN}'"),
            );
            let expected = if live_linked { 2 } else { 1 };
            assert_eq!(league, expected, "live linked: {live_linked}");
        }
    }

    #[test]
    fn a_live_upsert_moves_the_imported_copys_run_membership_to_the_live_token() {
        use crate::import::import_matches;
        // With or without a league token on the live row: the 0.x run still
        // holds the match (final review Important 1 and Minor 2).
        let (_dir, mut conn) = db("write-live-moves-run-membership");
        import_matches(&mut conn, &[imported(OTHER, Some(42))], 5_000).expect("import");
        let run = insert_imported_run(&conn, 9, None, &imported_seed("active", "constructed"));
        link_imported_match(&conn, 9, OTHER);

        let mut captured = view(TOKEN, "Qopponent", 1_000);
        captured.mtgo_match_id = Some(42);
        commit(&mut conn, &live(captured)).expect("live upsert");

        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE league_linked = 1"
            ),
            1
        );
        assert_eq!(
            number(
                &conn,
                &format!("SELECT league_id FROM matches WHERE token = '{TOKEN}' AND league_linked")
            ),
            run
        );
    }

    #[test]
    fn a_live_upsert_moves_the_imported_copys_limited_deck_to_the_live_token() {
        use crate::import::import_matches;
        use crate::import_limited::{ImportedLimited, LegacyLimitedDeck, import_limited};
        let copy_cards = r#"[{"catalog_id":100,"quantity":1,"sideboard":false}]"#;
        let live_cards = r#"[{"catalog_id":200,"quantity":1,"sideboard":false}]"#;
        let snapshots = |conn: &Connection| -> Vec<String> {
            let mut stmt = conn
                .prepare(
                    "SELECT m.token || '|' || s.source || '|' || s.cards || '|' || s.captured_at
                     FROM limited_deck_snapshots s JOIN matches m ON m.id = s.match_id
                     ORDER BY s.rowid",
                )
                .expect("prepare");
            stmt.query_map([], |row| row.get(0))
                .expect("query")
                .map(|row| row.expect("row"))
                .collect()
        };
        for live_has_one in [false, true] {
            let (_dir, mut conn) = db("write-live-moves-limited-deck");
            import_matches(&mut conn, &[imported(OTHER, Some(42))], 5_000).expect("import");
            insert_imported_run(&conn, 9, None, &imported_seed("complete", "draft"));
            link_imported_match(&conn, 9, OTHER);
            conn.execute(
                "INSERT INTO limited_deck_snapshots (match_id, source, cards, signature,
                     captured_at)
                 SELECT id, 'imported', ?2, '', 100 FROM matches WHERE token = ?1",
                params![OTHER, copy_cards],
            )
            .expect("the copy's deck");
            if live_has_one {
                // The live match is stored before it has its Match ID.
                commit(&mut conn, &live(view(TOKEN, "Qopponent", 1_000))).expect("live row");
                conn.execute(
                    "INSERT INTO limited_deck_snapshots (match_id, source, cards, signature,
                         captured_at)
                     SELECT id, 'imported', ?2, '', 200 FROM matches WHERE token = ?1",
                    params![TOKEN, live_cards],
                )
                .expect("the live match's deck");
            }

            let mut captured = view(TOKEN, "Qopponent", 1_000);
            captured.mtgo_match_id = Some(42);
            commit(&mut conn, &live(captured)).expect("live upsert");

            assert_eq!(
                snapshots(&conn),
                vec![format!(
                    "{TOKEN}|imported|{}|{}",
                    if live_has_one { live_cards } else { copy_cards },
                    if live_has_one { 200 } else { 100 }
                )],
                "one row, under the live match (live has one: {live_has_one})"
            );

            // An import rerun finds the row under the live match: no second row.
            let rerun = ImportedLimited {
                drafts: Vec::new(),
                decks: vec![LegacyLimitedDeck {
                    league_id: 9,
                    match_token: OTHER.to_string(),
                    mtgo_match_id: Some(42),
                    cards: vec![limited::RegisteredCard {
                        catalog_id: 100,
                        quantity: 1,
                        sideboard: false,
                    }],
                    captured_at_ms: 100,
                }],
                league_keys: None,
            };
            import_limited(&mut conn, &rerun, 6_000).expect("rerun");
            assert_eq!(
                number(&conn, "SELECT COUNT(*) FROM limited_deck_snapshots"),
                1
            );
        }
    }

    /// The player's fix of `token` (edit a match spec section 4): a loss in
    /// two games against Qfixed, with new times, stamped `corrected_at`.
    fn correct(conn: &Connection, token: &str) {
        conn.execute_batch(&format!(
            "INSERT INTO opponents (username, first_seen_at) VALUES ('Qfixed', 1);
             UPDATE matches SET outcome = 'loss', started_at = 500, ended_at = 700,
                 opponent_id = (SELECT id FROM opponents WHERE username = 'Qfixed'),
                 corrected_at = 800
             WHERE token = '{token}';
             UPDATE games SET result = 'loss'
             WHERE match_id = (SELECT id FROM matches WHERE token = '{token}');
             DELETE FROM games WHERE game_number = 3
                 AND match_id = (SELECT id FROM matches WHERE token = '{token}');"
        ))
        .expect("the fix");
    }

    /// The columns a fix writes, and its games.
    fn fixed_columns(conn: &Connection, token: &str) -> (String, Vec<Vec<String>>) {
        let row: String = conn
            .query_row(
                "SELECT m.outcome || '|' || m.started_at || '|' || COALESCE(m.ended_at, '-')
                     || '|' || COALESCE(o.username, '-') || '|' || COALESCE(m.corrected_at, '-')
                 FROM matches m LEFT JOIN opponents o ON o.id = m.opponent_id
                 WHERE m.token = ?1",
                [token],
                |row| row.get(0),
            )
            .expect("the row");
        let games = games_of(conn, token)
            .into_iter()
            .map(|(number, id, result)| vec![number.to_string(), id.to_string(), result])
            .collect();
        (row, games)
    }

    #[test]
    fn a_corrected_log_match_keeps_its_fix_through_a_re_projection() {
        let (_dir, mut conn) = db("write-corrected-kept");
        commit(&mut conn, &live(view(TOKEN, "Qopponent", 1_000))).expect("live");
        correct(&conn, TOKEN);
        let fixed = fixed_columns(&conn, TOKEN);
        assert_eq!(fixed.0, "loss|500|700|Qfixed|800");
        assert_eq!(fixed.1.len(), 2);

        // A failure, then the rebuild's projection with the log's values.
        commit(
            &mut conn,
            &WriteBatch {
                matches: vec![MatchWrite::Attention {
                    token: TOKEN.to_string(),
                    reason: "Q boom".to_string(),
                }],
                now_ms: 8_500,
                ..WriteBatch::default()
            },
        )
        .expect("attention");
        commit(&mut conn, &live(view(TOKEN, "Qopponent", 1_000))).expect("re-projection");
        assert_eq!(fixed_columns(&conn, TOKEN), fixed, "the fix is kept");
        assert_eq!(
            number(
                &conn,
                "SELECT needs_attention + (attention_reason IS NOT NULL) FROM matches"
            ),
            0,
            "a later projection still clears the failure"
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM match_failures"), 0);
        assert_eq!(
            number(&conn, "SELECT derived_version FROM matches"),
            i64::from(PROJECTION_VERSION)
        );
    }

    #[test]
    fn a_removed_corrected_match_is_detached_with_its_games_and_comes_back_fixed() {
        let (_dir, mut conn) = db("write-corrected-removed");
        commit(&mut conn, &live(view(TOKEN, "Qopponent", 1_000))).expect("live");
        correct(&conn, TOKEN);
        let fixed = fixed_columns(&conn, TOKEN);

        commit(&mut conn, &remove(TOKEN)).expect("remove");
        assert_eq!(
            number(&conn, "SELECT detached_at FROM matches"),
            9_500,
            "the fix is input: detached, not deleted"
        );
        assert_eq!(fixed_columns(&conn, TOKEN), fixed, "its games stay");

        commit(&mut conn, &live(view(TOKEN, "Qopponent", 1_000))).expect("back");
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE detached_at IS NULL"
            ),
            1,
            "attached again"
        );
        assert_eq!(fixed_columns(&conn, TOKEN), fixed);
    }

    #[test]
    fn a_live_match_takes_the_fix_of_a_corrected_copy_it_replaces() {
        use crate::import::import_matches;
        let (_dir, mut conn) = db("write-live-takes-corrected-copy");
        // A history import of Match ID 42 under another token, fixed by the
        // player: a loss in two games, game 1 on the draw in 4 turns.
        import_matches(&mut conn, &[imported(OTHER, Some(42))], 5_000).expect("import");
        conn.execute("UPDATE matches SET source = 'history'", [])
            .expect("history");
        correct(&conn, OTHER);
        conn.execute(
            "UPDATE games SET on_play = 0, turn_count = 4 WHERE game_number = 1",
            [],
        )
        .expect("game 1");
        let fix = fixed_columns(&conn, OTHER).0;

        let mut captured = view(TOKEN, "Qopponent", 1_000);
        captured.mtgo_match_id = Some(42);
        commit(&mut conn, &live(captured.clone())).expect("live upsert");
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM matches"),
            1,
            "the copy goes"
        );
        let (row, games) = fixed_columns(&conn, TOKEN);
        assert_eq!(row, fix, "the live row carries the fix");
        let results: Vec<&str> = games.iter().map(|game| game[2].as_str()).collect();
        assert_eq!(results, vec!["loss", "loss"], "the live game 3 goes");
        assert_eq!(
            number(
                &conn,
                "SELECT on_play * 100 + turn_count FROM games WHERE game_number = 1"
            ),
            4
        );
        assert_eq!(
            number(
                &conn,
                "SELECT mtgo_game_id FROM games WHERE game_number = 1"
            ),
            7,
            "the live game keeps its own Game ID"
        );

        // The live row is corrected now: the next projection keeps the fix.
        commit(&mut conn, &live(captured)).expect("re-projection");
        assert_eq!(fixed_columns(&conn, TOKEN), (row, games));
    }

    #[test]
    fn an_uncorrected_log_match_still_takes_the_projection() {
        let (_dir, mut conn) = db("write-uncorrected-updates");
        commit(&mut conn, &live(view(TOKEN, "Qopponent", 1_000))).expect("live");
        let mut changed = view(TOKEN, "Qrematch", 2_000);
        changed.outcome = Outcome::Loss;
        changed.games.truncate(2);
        commit(&mut conn, &live(changed)).expect("re-projection");
        let (row, games) = fixed_columns(&conn, TOKEN);
        assert_eq!(row, "loss|2000|6000|Qrematch|-");
        assert_eq!(games.len(), 2);
    }

    #[test]
    fn live_writes_never_touch_a_manual_row() {
        let (_dir, mut conn) = db("write-manual-untouched");
        insert_manual(&conn, TOKEN);
        let matches_before = dump(&conn, "matches");
        let games_before = dump(&conn, "games");

        // A live result under the manual row's token, a failure and a removal.
        let mut same_token = view(TOKEN, "Qother", 1_000);
        same_token.mtgo_match_id = Some(42);
        for write in [
            MatchWrite::Upsert {
                view: Box::new(same_token),
                revision: 9,
            },
            MatchWrite::Attention {
                token: TOKEN.to_string(),
                reason: "projection panicked: Q boom".to_string(),
            },
            MatchWrite::Remove {
                token: TOKEN.to_string(),
            },
        ] {
            commit(
                &mut conn,
                &WriteBatch {
                    matches: vec![write],
                    now_ms: 9_000,
                    ..WriteBatch::default()
                },
            )
            .expect("commit");
            assert_eq!(dump(&conn, "matches"), matches_before);
            assert_eq!(dump(&conn, "games"), games_before);
        }
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM match_failures"),
            0,
            "a manual match never fails"
        );

        // A live match with a Match ID removes imported copies only.
        let mut other = view(OTHER, "Qopponent", 1_000);
        other.mtgo_match_id = Some(42);
        commit(&mut conn, &live(other)).expect("live upsert");
        assert_eq!(
            number(
                &conn,
                &format!(
                    "SELECT COUNT(*) FROM matches WHERE token = '{TOKEN}' AND source = 'manual'"
                )
            ),
            1
        );
        assert_eq!(
            number(
                &conn,
                &format!(
                    "SELECT COUNT(*) FROM games g JOIN matches m ON m.id = g.match_id
                     WHERE m.token = '{TOKEN}'"
                )
            ),
            2
        );
    }

    #[test]
    fn a_failure_is_recorded_with_or_without_a_row_until_the_match_derives_again() {
        let (_dir, mut conn) = db("write-failure");
        let failed = |token: &str| MatchWrite::Attention {
            token: token.to_string(),
            reason: "projection panicked: Q boom".to_string(),
        };
        let batch = |matches: Vec<MatchWrite>| WriteBatch {
            matches,
            now_ms: 5_000,
            ..WriteBatch::default()
        };
        let upsert = |revision: u64| MatchWrite::Upsert {
            view: Box::new(view(TOKEN, "Qopponent", 1_000)),
            revision,
        };
        // TOKEN has a row; OTHER failed before it ever had one.
        commit(&mut conn, &batch(vec![upsert(1)])).expect("row");
        commit(&mut conn, &batch(vec![failed(TOKEN), failed(OTHER)])).expect("failures");
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM match_failures"), 2);
        assert_eq!(
            number(
                &conn,
                &format!("SELECT failed_at FROM match_failures WHERE token = '{OTHER}'")
            ),
            5_000
        );
        assert_eq!(
            number(&conn, "SELECT MIN(derived_version) FROM match_failures"),
            i64::from(PROJECTION_VERSION)
        );
        assert_eq!(number(&conn, "SELECT needs_attention FROM matches"), 1);

        commit(
            &mut conn,
            &batch(vec![
                upsert(2),
                MatchWrite::Remove {
                    token: OTHER.to_string(),
                },
            ]),
        )
        .expect("new results");
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM match_failures"),
            0,
            "a new result or a removal clears the failure"
        );
    }
}
