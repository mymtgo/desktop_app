//! One tick of tracking work (spec 3): rescan for logs, read each file, prepare
//! and project new events, answer the projector's lookups from storage, and hand
//! one batch to the writer. A tick waits on the database only to answer a lookup
//! or to create a file row, both rare.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use chrono::{DateTime, NaiveDate};
use chrono_tz::Tz;
use mtgo_log::identity::{self, ANCHOR_LEN, ANCHOR_OFFSET, Anchor, Recorded, SealReason};
use mtgo_log::{Env, Tailer, detect};
use mtgo_match::{
    ABANDON_AFTER_MS, Derived, Lookup, MatchState, MatchView, PROJECTION_VERSION, Projector,
};
use mtgo_protocol::{EventKind, FileContext, PreparedEvent, RawEvent, Subjects};
use store::accounts::{AccountEdit, AccountEditOutcome};
use store::compact::COMPACT_AFTER_MS;
use store::decks::{DeckEdit, DeckWrite, EditOutcome, link_of};
use store::history_import::{HistoryImportCounts, HistoryMatch};
use store::import::{ImportCounts, ImportedMatch};
use store::manual::{ManualEdit, ManualEditOutcome};
use store::match_edits::{MatchEdit, MatchEditOutcome};
use store::read;
use store::write::{FileProgress, MatchWrite, NewAccount, NewEvent, NewLogFile, Seal, WriteBatch};
use store::writer::{WriterHandle, spawn_writer};
use store::{Connection, Platform, StoreError, open_reader};

use crate::history::History;
use crate::{FileStatus, Status, TrackerConfig, TrackerError, TrackerEvent};

mod arena;
mod arena_courses;
mod deck_sync;
mod deck_use;
mod endstep;
mod game_sync;
mod history_check;
mod league_sync;
mod link;
#[cfg(test)]
pub(crate) mod test_support;
mod tournament_sync;

pub use deck_sync::{DECK_REQUEST_EVERY_MS, DECK_SYNC_EVERY_MS};
pub use game_sync::GAME_SYNC_PENDING;
pub use game_sync::{GAME_LOG_RETRY_MS, GAME_LOG_TRIES, GAME_SYNCS_PER_TICK};
pub use history_check::HISTORY_CHECK_EVERY_MS;
pub use league_sync::{
    LEAGUE_RETRY_FIRST_MS, LEAGUE_RETRY_MAX_MS, LEAGUE_SYNC_AFTER_MS, LEAGUE_SYNC_PENDING,
};
pub use link::{LINK_RETRY_MS, RELINK_LIMIT, RELINK_WITHIN_MS, REREAD_ACTIVE_WITHIN_MS};
pub use tournament_sync::{
    TOURNAMENT_RETRY_FIRST_MS, TOURNAMENT_RETRY_MAX_MS, TOURNAMENT_SYNC_AFTER_MS,
};

/// Poll interval while a log grew live in the last `ACTIVE_FOR_MS` (spec 3).
pub const POLL_ACTIVE_MS: u64 = 75;
pub const POLL_IDLE_MS: u64 = 500;
pub const ACTIVE_FOR_MS: i64 = 60_000;
/// How often the log folders are searched for new files.
pub const RESCAN_EVERY_MS: i64 = 2_000;
/// Two logs growing live within this window means two MTGO clients (spec 5.1).
pub const TWO_CLIENTS_WINDOW_MS: i64 = 10 * 60 * 1000;
/// Rounds of lookups answered per tick before the rest wait for the next tick.
const LOOKUP_ROUNDS: usize = 100;
/// How often finished matches are checked for compaction (when caught up).
pub const COMPACT_EVERY_MS: i64 = 60_000;
/// Stale matches rebuilt per tick after a projection version bump, so the
/// rebuild never stalls live tracking (spec 4.1 rule 3, spec 8).
pub const REBUILDS_PER_TICK: usize = 20;

/// Shared with the supervisor: the event being processed, so a crash can be
/// pinned on it (poison event quarantine, spec 3).
pub type InFlight = Arc<Mutex<Option<InFlightEvent>>>;

/// The event being processed: its `(file_id, byte_offset)` and the length of
/// its text. A file's provisional last event may still grow; each length is
/// another version of it, so crashes on a partial event are counted per
/// version (#50).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InFlightEvent {
    pub key: (i64, u64),
    pub text_len: usize,
}

struct TrackedFile {
    id: i64,
    path: PathBuf,
    recorded: Recorded,
    ctx: FileContext,
    tailer: Tailer,
    size: u64,
    /// (cursor, observed size) last sent to the writer.
    saved: Option<(u64, u64)>,
    /// (offset, len) of the provisional event last previewed.
    previewed: Option<(u64, u64)>,
    /// The matches that preview showed, so parking the file can clear them.
    previewed_tokens: Vec<String>,
    /// The stored timezone did not parse; the engine's current timezone (the
    /// OS zone at the last rescan, or `config.timezone`) is used instead.
    bad_timezone: bool,
}

/// How long each phase of one tick took, in microseconds (spec 8, built-in
/// timing). Phases that did not run are 0.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TickTiming {
    pub rescan_us: u64,
    pub poll_us: u64,
    pub lookups_us: u64,
    pub finalize_us: u64,
    /// Projection rebuild and compaction planning.
    pub rebuild_us: u64,
    /// Deck file sync and deck linking.
    pub decks_us: u64,
    pub write_us: u64,
    /// Sending the game syncs (they run on the writer after the batch).
    pub games_us: u64,
    /// The league sync, when one ran (it waits for the writer).
    pub leagues_us: u64,
    /// The tournament sync, when one ran (it waits for the writer).
    pub tournaments_us: u64,
    pub total_us: u64,
}

fn micros_since(start: Instant) -> u64 {
    start.elapsed().as_micros() as u64
}

/// What one tick produced.
#[derive(Debug, Default)]
pub struct Tick {
    pub events: Vec<TrackerEvent>,
    /// A file still has unread bytes: tick again without sleeping.
    pub more: bool,
    /// A log grew live recently: poll fast.
    pub active: bool,
    /// Game syncs are queued or in flight: tick again as soon as the slice
    /// in flight is answered (`Engine::game_slice_answered`).
    pub syncing: bool,
    pub timing: TickTiming,
    /// The tick read events, sent writes, or produced events. Idle ticks are
    /// not logged.
    pub worked: bool,
}

pub struct Engine {
    config: TrackerConfig,
    /// The zone new files are dated in (spec 5.4), refreshed at every rescan.
    timezone: Tz,
    /// Why the OS zone could not be used, for the status bar.
    timezone_warning: Option<String>,
    env: Env,
    writer: WriterHandle,
    reader: Connection,
    projector: Projector,
    history: History,
    files: Vec<TrackedFile>,
    /// Files detection no longer finds (another folder was chosen, or the file
    /// is gone for now). Not polled; their rows stay open, so finding a file
    /// again resumes its row at its cursor instead of reading it twice.
    parked: Vec<TrackedFile>,
    roots: Vec<PathBuf>,
    /// Tokens the tracker has shown as visible, so a discard can be reported.
    visible: HashSet<String>,
    /// Matches whose last projection failed, as stored in `match_failures`.
    failed: HashSet<String>,
    /// Matches the player removed from stats (`matches.hidden_at`).
    /// They are still derived and stored (live equals rebuild), but never
    /// pushed to the UI again. Read at open; a hide adds to it, and every
    /// edit that can hide or restore a match otherwise (a restore, a league
    /// delete and its Undo, a pull, the website's deletions) reads it again
    /// (`refresh_hidden`, `refresh_hidden_token`).
    hidden: HashSet<String>,
    /// Live matches already checked this run for a hidden 0.x or history
    /// copy they take over (`takes_over_a_hidden_copy`).
    takeover_checked: HashSet<String>,
    last_rescan_ms: Option<i64>,
    newest_event_ms: i64,
    in_flight: InFlight,
    quarantine: Vec<(i64, u64)>,
    quarantined: u32,
    last_status: Option<Status>,
    /// The watcher is on (release spec 4.1). Off, ticks read no log file and
    /// no deck XML and run no time rule; discovery, commands and the stored
    /// derivations go on, and each file keeps its cursor.
    watching: bool,
    batch: WriteBatch,
    pending_events: Vec<TrackerEvent>,
    /// A new local account appeared, or the tracker opened knowing one: joined
    /// matches without a row are loaded at the next lookup round, so the account
    /// can identify them (issue #51).
    recheck_unrowed: bool,
    /// Stale matches still to rebuild after a projection version bump, and
    /// the skipped matches of an account tracked again (release spec 5).
    pending_rebuild: Vec<String>,
    /// The usernames of the untracked accounts: their new log matches are
    /// recorded in `untracked_matches` instead of built (release spec 5).
    untracked: HashSet<String>,
    /// Matches skipped this run for an untracked account: never shown, and
    /// their skip row goes if they stop being matches.
    skipped: HashSet<String>,
    /// A Login was stored or an account edit was done since the last
    /// `AccountChanged`: the next tick commits and reports one.
    account_changed: bool,
    /// Arena rank readings were queued since the last `RankChanged`: the
    /// next caught-up tick commits and reports one.
    rank_changed: bool,
    rebuilding: bool,
    last_compact_ms: Option<i64>,
    /// When the deck files were last read (decks spec 3). `None` until the
    /// first sync, which runs once the tracker has caught up after opening.
    last_deck_sync_ms: Option<i64>,
    /// A deck sync is wanted at the next caught-up tick (a new account, or a
    /// request).
    deck_sync_wanted: bool,
    /// When `request_deck_sync` last asked for a sync.
    last_deck_request_ms: Option<i64>,
    /// MTGO's verdicts from its game history files (#454).
    history_check: history_check::HistoryCheck,
    /// Deck files already logged as skipped, so a lasting problem is logged once.
    skipped_deck_files: HashSet<PathBuf>,
    /// Deck files already warned about as unreadable this session.
    unreadable_deck_files: HashSet<PathBuf>,
    /// Matches waiting to be linked to the deck version played (decks spec 4).
    links: link::LinkState,
    /// A deck write was queued or committed since the last `DecksChanged`:
    /// the next tick commits it and reports one.
    decks_changed: bool,
    /// MTGO deck ids the deck sync wrote since the last
    /// `DeckNeedsArchetype`: the ones still without an archetype are told
    /// once committed.
    synced_deck_ids: Vec<String>,
    /// A league input changed since the last `LeaguesChanged` (a league or
    /// tournament match derived or removed, a league event stored, a league
    /// edit, an import, a tournament round adopting its deck): the next tick
    /// commits it and reports one.
    leagues_changed: bool,
    /// Visible matches with a league token or a tournament id, so their
    /// removal reports `LeaguesChanged`.
    league_matches: HashSet<String>,
    /// Limited data changed since the last `LimitedChanged` (a draft event
    /// stored, a pick note saved, 0.x limited data imported): the next tick
    /// commits it and reports one.
    limited_changed: bool,
    /// Card data changed since the last `CardsChanged` (a population run
    /// filled rows): the next tick reports one.
    cards_changed: bool,
    /// The game sync of log matches' per-game card rows (`game_sync`).
    game_sync: game_sync::GameSyncState,
    /// The league sync of stored runs, drafts and tournaments (`league_sync`).
    leagues: league_sync::LeagueSyncState,
    /// The tournament sync of the event overlay's standings (`tournament_sync`).
    tournaments: tournament_sync::TournamentSyncState,
    /// What this tick's live overlay events belong to (`LiveEventsStored`).
    live: LiveMarks,
    /// Arena's `Player.log` (Arena spec): read beside MTGO's logs into the
    /// same batches.
    arena: arena::ArenaSource,
    /// Arena is on (the `arena_enabled` setting, resolved by the app): off,
    /// the Arena log is not discovered, read, finalized or linked, and no
    /// Arena sync is sent. Turning it on rescans at once.
    arena_enabled: bool,
    /// The stored Endstep captures (endstep support spec), folded into the
    /// same batches.
    endstep: endstep::EndstepSource,
    /// Endstep is on (the `endstep_enabled` setting): off, no capture is
    /// folded, finalized or linked; the captures wait unsynced.
    endstep_enabled: bool,
}

/// The subjects of the events a tick stored that a live match's overlay
/// reads (overlay spec 6 step 1), resolved to match tokens at the end of the
/// tick, once the projection knows every game.
#[derive(Debug, Default)]
struct LiveMarks {
    tokens: HashSet<String>,
    match_ids: HashSet<u64>,
    game_ids: HashSet<u64>,
    /// When the first of them was read.
    detected_at: Option<Instant>,
}

impl LiveMarks {
    /// The event kinds that change what the overlays show.
    const KINDS: [EventKind; 4] = [
        EventKind::GameStateUpdate,
        EventKind::DeckUsed,
        EventKind::GameManagementJson,
        EventKind::MatchStateChanged,
    ];

    fn note(&mut self, kind: EventKind, subjects: &Subjects) {
        if !Self::KINDS.contains(&kind) {
            return;
        }
        if let Some(token) = &subjects.match_token {
            self.tokens.insert(token.clone());
        }
        if let Some(id) = subjects.match_id {
            self.match_ids.insert(id);
        }
        if let Some(id) = subjects.game_id {
            self.game_ids.insert(id);
        }
        if self.detected_at.is_none() {
            self.detected_at = Some(Instant::now());
        }
    }

    /// Add `other`'s marks, keeping the earliest detection time.
    fn merge(&mut self, other: LiveMarks) {
        self.tokens.extend(other.tokens);
        self.match_ids.extend(other.match_ids);
        self.game_ids.extend(other.game_ids);
        self.detected_at = match (self.detected_at, other.detected_at) {
            (Some(mine), Some(theirs)) => Some(mine.min(theirs)),
            (mine, theirs) => mine.or(theirs),
        };
    }

    /// Whether one of the noted events belongs to this match.
    fn names(&self, view: &MatchView) -> bool {
        self.tokens.contains(&view.token)
            || view
                .mtgo_match_id
                .is_some_and(|id| self.match_ids.contains(&id))
            || view.games.iter().any(|game| {
                game.mtgo_game_id
                    .is_some_and(|id| self.game_ids.contains(&id))
            })
    }
}

impl Engine {
    /// Open the store (running migrations) and get ready to tick. `quarantine`
    /// holds event keys that crashed the tracker repeatedly: they are stored as
    /// quarantined and skipped. `now_ms` picks which finished matches are recent
    /// enough to still change with time.
    pub fn open(
        config: TrackerConfig,
        env: Env,
        in_flight: InFlight,
        quarantine: Vec<(i64, u64)>,
        now_ms: i64,
    ) -> Result<Self, TrackerError> {
        let writer = spawn_writer(&config.db_path)?;
        let reader = open_reader(&config.db_path)?;
        let projector = Projector::new(read::known_accounts(&reader)?);
        // A Login stored just before the process stopped may not have reloaded
        // the matches waiting for it, so the first lookup round rechecks them.
        let recheck_unrowed = !projector.known_accounts().is_empty();
        let failed: HashSet<String> = read::failed_match_tokens(&reader)?.into_iter().collect();
        let hidden: HashSet<String> = store::match_edits::hidden_match_tokens(&reader)?
            .into_iter()
            .collect();
        let untracked: HashSet<String> = store::accounts::untracked_usernames(&reader)?
            .into_iter()
            .collect();
        let arena = arena::ArenaSource::open(
            &reader,
            env.clone(),
            config.arena_log_overrides.clone(),
            now_ms,
        )?;
        let endstep = endstep::EndstepSource::open(&reader)?;
        let timezone = config.timezone;
        let watching = config.watching;
        let arena_enabled = config.arena_enabled;
        let endstep_enabled = config.endstep_enabled;
        let mut engine = Self {
            config,
            timezone,
            timezone_warning: None,
            env,
            writer,
            reader,
            projector,
            history: History::default(),
            files: Vec::new(),
            parked: Vec::new(),
            roots: Vec::new(),
            visible: HashSet::new(),
            failed,
            hidden,
            takeover_checked: HashSet::new(),
            last_rescan_ms: None,
            newest_event_ms: 0,
            in_flight,
            quarantine,
            quarantined: 0,
            last_status: None,
            watching,
            batch: WriteBatch::default(),
            pending_events: Vec::new(),
            recheck_unrowed,
            pending_rebuild: Vec::new(),
            untracked,
            skipped: HashSet::new(),
            account_changed: false,
            rank_changed: false,
            rebuilding: false,
            last_compact_ms: None,
            last_deck_sync_ms: None,
            history_check: history_check::HistoryCheck::default(),
            deck_sync_wanted: false,
            last_deck_request_ms: None,
            skipped_deck_files: HashSet::new(),
            unreadable_deck_files: HashSet::new(),
            links: link::LinkState::default(),
            decks_changed: false,
            synced_deck_ids: Vec::new(),
            leagues_changed: false,
            league_matches: HashSet::new(),
            limited_changed: false,
            cards_changed: false,
            game_sync: game_sync::GameSyncState::default(),
            leagues: league_sync::LeagueSyncState::default(),
            tournaments: tournament_sync::TournamentSyncState::default(),
            live: LiveMarks::default(),
            arena,
            arena_enabled,
            endstep,
            endstep_enabled,
        };
        engine.batch.now_ms = now_ms;
        engine.refresh_timezone();
        engine.resume_files()?;
        engine.rehydrate(now_ms)?;
        engine.plan_rebuild()?;
        engine.plan_retrack()?;
        engine.plan_game_backfill()?;
        engine.plan_league_sync(now_ms)?;
        engine.plan_tournament_sync(now_ms)?;
        engine.plan_arena_unstored(now_ms);
        engine.plan_arena_backfill(now_ms)?;
        engine.plan_arena_game_sync()?;
        engine.plan_arena_runs_resync();
        engine.plan_endstep_resync()?;
        Ok(engine)
    }

    pub fn tick(&mut self, now_ms: i64) -> Result<Tick, TrackerError> {
        if let Some(failure) = self.writer.failure() {
            return Err(TrackerError::Store(StoreError::Failed(failure)));
        }
        #[cfg(test)]
        if !self.watching && test_hooks::take_paused_tick_failure(&self.config.db_path) {
            return Err(TrackerError::Store(StoreError::Failed(
                "injected paused tick failure".to_string(),
            )));
        }
        let started = Instant::now();
        let mut timing = TickTiming::default();
        self.batch.now_ms = now_ms;
        self.links.new_tick();
        if self
            .last_rescan_ms
            .is_none_or(|at| now_ms - at >= RESCAN_EVERY_MS)
        {
            let phase = Instant::now();
            self.last_rescan_ms = Some(now_ms);
            self.rescan(now_ms)?;
            if self.arena_enabled {
                self.arena.rescan(&self.reader, &self.writer, now_ms)?;
            }
            timing.rescan_us = micros_since(phase);
        }

        let phase = Instant::now();
        let mut more = false;
        let mut index = 0;
        // Paused: no file is read, so each keeps its cursor.
        while self.watching && index < self.files.len() {
            let count_before = self.files.len();
            if self.poll_file(index, now_ms)? {
                more = true;
            }
            // A rotation whose new file vanished removes this entry, so the
            // next file has moved into `index`: poll it before moving on.
            if self.files.len() == count_before {
                index += 1;
            }
        }
        // MTGO's time rules follow MTGO's backlog only.
        let mtgo_more = more;
        let mut arena_more = false;
        if self.watching && self.arena_enabled {
            let polled = self
                .arena
                .poll(&self.reader, &mut self.batch, now_ms, self.timezone)?;
            self.push_arena(&polled.matches);
            self.queue_limited_runs(&polled.matches);
            for write in polled.matches {
                self.batch.push_match(write, Platform::Arena);
            }
            arena_more = polled.more;
            if polled.account_changed {
                self.account_changed = true;
            }
            if polled.rank_changed {
                self.rank_changed = true;
            }
            more |= arena_more;
        }
        let mut endstep_more = false;
        if self.watching && self.endstep_enabled {
            let polled = self
                .endstep
                .poll(&self.reader, &self.writer, &mut self.batch, now_ms)?;
            self.take_endstep_writes(polled.matches);
            endstep_more = polled.more;
            more |= endstep_more;
        }
        timing.poll_us = micros_since(phase);

        let phase = Instant::now();
        self.answer_lookups()?;
        timing.lookups_us = micros_since(phase);

        // While catching up, time-based rules run on log time, as if live.
        // Paused, the log is not read, so its silence means nothing: no time
        // rule runs on it (a match open mid-pause must not be abandoned).
        let phase = Instant::now();
        if self.watching {
            let sweep_ms = if mtgo_more {
                self.newest_event_ms
            } else {
                now_ms
            };
            let changed = self.projector.finalize(sweep_ms);
            self.handle_changed(changed, None);
            self.projector.retire_quiet(sweep_ms);
            if self.arena_enabled {
                let arena_settled = self.arena.finalize(now_ms, arena_more);
                self.push_arena(&arena_settled);
                self.queue_limited_runs(&arena_settled);
                for write in arena_settled {
                    self.batch.push_match(write, Platform::Arena);
                }
            }
            // Not while units wait: a backlog may still bring a quiet match's
            // end, and must not push a passing Abandoned first.
            if self.endstep_enabled && !endstep_more {
                let quiet =
                    self.endstep
                        .finalize(&self.reader, &self.writer, &mut self.batch, now_ms)?;
                self.take_endstep_writes(quiet.matches);
            }
        }
        timing.finalize_us = micros_since(phase);

        let phase = Instant::now();
        self.rebuild_some(now_ms)?;
        if !more
            && self
                .last_compact_ms
                .is_none_or(|at| now_ms - at >= COMPACT_EVERY_MS)
        {
            self.last_compact_ms = Some(now_ms);
            self.plan_compaction(now_ms)?;
        }
        timing.rebuild_us = micros_since(phase);

        let phase = Instant::now();
        // Deck files are read once caught up, so the local account is known.
        if self.watching && !more && self.deck_sync_due(now_ms) {
            self.sync_deck_files(now_ms)?;
        }
        // MTGO's verdicts check the stored outcomes once caught up.
        if self.watching && !more && self.history_check_due(now_ms) {
            self.check_history(now_ms)?;
        }
        // Linking reads the stored deck lists, so it waits until caught up too.
        if !more {
            self.link_matches(now_ms)?;
        }
        // An Arena match links once its deck write has committed. Paused,
        // the Arena poll sends no deck write, so the links wait for it.
        if self.watching && self.arena_enabled && self.arena.has_pending_links() {
            self.flush()?;
            self.arena.link(&self.reader, &mut self.batch)?;
        }
        // An Endstep match links the same way, once its deck write committed.
        if self.watching && self.endstep_enabled && self.endstep.has_pending_links() {
            self.flush()?;
            self.endstep.link(&self.reader, &mut self.batch)?;
        }
        // Arena and Endstep decks synced this tick get the archetype prefill
        // like MTGO's deck files (#690): told once the batch has committed.
        let mut written = self.arena.take_written_deck_ids();
        written.extend(self.endstep.take_written_deck_ids());
        if !written.is_empty() {
            self.synced_deck_ids.extend(written);
            self.decks_changed = true;
        }
        timing.decks_us = micros_since(phase);

        let phase = Instant::now();
        let wrote = !self.batch.is_empty();
        self.send_batch()?;
        // Told after the batch commits and before this tick's game syncs are
        // queued behind it, so the overlay reads the frames it is told of.
        if let Some(event) = self.live_events_stored() {
            self.writer.flush()?;
            self.pending_events.push(event);
        }
        timing.write_us = micros_since(phase);

        // The syncs are separate writer requests after the batch, so the
        // match push never waits on them. They run once caught up and after
        // a projection rebuild, so a backlog is synced once.
        let caught_up = !more && !self.rebuilding;
        let phase = Instant::now();
        let mut syncing = self.send_game_syncs(now_ms, caught_up)?;
        self.send_arena_syncs()?;
        syncing |= self.take_arena_sync_replies();
        timing.games_us = micros_since(phase);
        let phase = Instant::now();
        // A tick that pushes a match returns it at once: a due sync runs at
        // the next tick.
        let pushes_a_match = self
            .pending_events
            .iter()
            .any(|event| matches!(event, TrackerEvent::MatchUpdated { .. }));
        if caught_up && self.league_sync_due(now_ms) {
            if !pushes_a_match || self.leagues.postponed {
                self.sync_leagues(now_ms)?;
            } else {
                self.leagues.postponed = true;
            }
        }
        timing.leagues_us = micros_since(phase);
        let phase = Instant::now();
        // The event overlay's tournaments: independent of the league sync
        // and its events.
        if caught_up && self.tournament_sync_due(now_ms) {
            self.sync_tournaments(now_ms)?;
        }
        timing.tournaments_us = micros_since(phase);

        let phase = Instant::now();
        // The leagues and Limited pages read the league sync's rows: told
        // once it has run.
        let held = self.league_events_held();
        let leagues_changed = self.leagues_changed && !held;
        let limited_changed = self.limited_changed && !held;
        // Every page refetches on it: held while a backlog is caught up (a
        // first run reads a Login per log file), then told once.
        let account_changed = self.account_changed && !more;
        // The rank views refetch on it: held while a backlog is caught up,
        // like `AccountChanged`.
        let rank_changed = self.rank_changed && !more;
        if self.decks_changed
            || leagues_changed
            || limited_changed
            || account_changed
            || rank_changed
        {
            // The UI rereads the decks, leagues and Limited pages when told:
            // commit before telling it.
            self.writer.flush()?;
        }
        if self.decks_changed {
            self.decks_changed = false;
            self.pending_events.push(TrackerEvent::DecksChanged);
            // Committed above (the flush), so the reader sees the decks.
            let synced = std::mem::take(&mut self.synced_deck_ids);
            if !synced.is_empty() {
                let deck_ids = store::archetypes::decks_without_archetype(&self.reader, &synced)?;
                if !deck_ids.is_empty() {
                    self.pending_events
                        .push(TrackerEvent::DeckNeedsArchetype { deck_ids });
                }
            }
        }
        if leagues_changed {
            self.leagues_changed = false;
            self.pending_events.push(TrackerEvent::LeaguesChanged);
        }
        // Committed above (the flush), so the pages read the new account.
        if account_changed {
            self.account_changed = false;
            self.pending_events.push(TrackerEvent::AccountChanged);
        }
        if rank_changed {
            self.rank_changed = false;
            self.pending_events.push(TrackerEvent::RankChanged);
        }
        if limited_changed {
            self.limited_changed = false;
            self.pending_events.push(TrackerEvent::LimitedChanged);
        }
        // Answered after its commit, so the rows are stored.
        if self.cards_changed {
            self.cards_changed = false;
            self.pending_events.push(TrackerEvent::CardsChanged);
        }
        // Counted by the writer after each commit, so these rows are stored.
        let stubs = self.writer.take_stubs_added();
        if stubs > 0 {
            self.pending_events
                .push(TrackerEvent::CardStubsAdded { count: stubs });
        }
        timing.write_us += micros_since(phase);

        self.push_status(now_ms);
        let active = self.files.iter().any(|file| {
            file.tailer
                .last_live_growth_ms()
                .is_some_and(|at| now_ms - at <= ACTIVE_FOR_MS)
        });
        let events = coalesce_events(std::mem::take(&mut self.pending_events));
        timing.total_us = micros_since(started);
        Ok(Tick {
            worked: wrote || more || !events.is_empty(),
            events,
            more,
            active,
            syncing,
            timing,
        })
    }

    /// The tick's `LiveEventsStored`, if it stored overlay events of an
    /// in-progress, visible log match. A file's backlog being caught up is
    /// not live: `poll_file` dropped its marks already.
    fn live_events_stored(&mut self) -> Option<TrackerEvent> {
        let marks = std::mem::take(&mut self.live);
        let detected_at = marks.detected_at?;
        let mut tokens: Vec<String> = Vec::new();
        for view in self.projector.views() {
            if view.state == MatchState::InProgress
                && !self.hidden.contains(&view.token)
                && !self.skipped.contains(&view.token)
                && marks.names(view)
            {
                tokens.push(view.token.clone());
            }
        }
        if tokens.is_empty() {
            return None;
        }
        tokens.sort();
        Some(TrackerEvent::LiveEventsStored {
            tokens,
            detected_at,
        })
    }

    /// Add an event whose foreign key fails, so the next batch fails.
    #[cfg(test)]
    pub(crate) fn inject_broken_event(&mut self) {
        self.batch.events.push(test_hooks::broken_event());
    }

    /// Send what is pending and wait until it is committed.
    pub fn flush(&mut self) -> Result<(), TrackerError> {
        self.send_batch()?;
        self.writer.flush()?;
        Ok(())
    }

    /// Stop: send what is pending, run the pending game syncs and a pending
    /// league sync, and wait until everything is committed.
    pub fn finish(&mut self) -> Result<(), TrackerError> {
        self.send_batch()?;
        self.send_arena_syncs()?;
        self.send_pending_game_syncs()?;
        self.sync_leagues_before_stopping()?;
        self.writer.flush()?;
        Ok(())
    }

    /// The newest event time read from a log this run, MTGO or Arena (0
    /// before the first): what the replay clock (`Clock::Log`) runs on.
    pub fn newest_log_ms(&self) -> i64 {
        self.newest_event_ms.max(self.arena.newest_ms())
    }

    /// Whether the watcher is on (release spec 4.1).
    pub fn watching(&self) -> bool {
        self.watching
    }

    /// Switch the watcher on or off. Off, the next ticks read nothing; on,
    /// reading resumes from each file's cursor. The status says which.
    pub fn set_watching(&mut self, watching: bool) {
        if self.watching != watching {
            tracing::info!(watching, "the watcher was switched");
        }
        self.watching = watching;
    }

    /// Whether Arena is on (the `arena_enabled` setting).
    pub fn arena_enabled(&self) -> bool {
        self.arena_enabled
    }

    /// Switch Arena on or off. Off, the next ticks discover and read no
    /// Arena log; on, the next tick rescans for it and reading resumes from
    /// each file's cursor.
    pub fn set_arena_enabled(&mut self, on: bool) {
        if self.arena_enabled != on {
            tracing::info!(on, "Arena was switched");
        }
        if on && !self.arena_enabled {
            self.last_rescan_ms = None;
        }
        self.arena_enabled = on;
    }

    /// Whether Endstep is on (the `endstep_enabled` setting).
    pub fn endstep_enabled(&self) -> bool {
        self.endstep_enabled
    }

    /// Switch Endstep on or off. Off, the next ticks fold no capture; on,
    /// the next tick folds the captures stored meanwhile.
    pub fn set_endstep_enabled(&mut self, on: bool) {
        if self.endstep_enabled != on {
            tracing::info!(on, "Endstep was switched");
        }
        if on && !self.endstep_enabled {
            self.endstep.wake();
        }
        self.endstep_enabled = on;
    }

    /// Store settings rows with the next batch and wait until they are
    /// committed (spec 4.5: settings writes go through the writer thread).
    pub fn save_settings(&mut self, rows: Vec<(String, String)>) -> Result<(), TrackerError> {
        self.batch.settings.extend(rows);
        self.flush()
    }

    /// Store 0.x matches (importer spec section 7) and wait for the counts.
    /// Pending live writes commit first, so a live row written this tick wins.
    /// Every imported token with a stored row is then treated as `rehydrate`
    /// treats stored matches: a later live event for it reloads its history.
    pub fn import(
        &mut self,
        matches: Vec<ImportedMatch>,
        now_ms: i64,
    ) -> Result<ImportCounts, TrackerError> {
        self.send_batch()?;
        let mut tokens: Vec<String> = Vec::new();
        for imported in &matches {
            tokens.push(imported.token.clone());
        }
        // A 0.x match replaces a history copy with another token: those
        // tokens are gone after the write and are reported removed.
        let copies_before = self.history_copy_tokens();
        let counts = self.writer.import(matches, now_ms)?;
        self.report_copies_removed(copies_before);
        let revisions = tokens
            .into_iter()
            .map(|token| {
                let revision = read::match_revision(&self.reader, &token);
                (token, revision)
            })
            .collect();
        self.mark_imported(revisions);
        Ok(counts)
    }

    /// Every history copy token stored now: an import replaces only those
    /// (#314 row 2). A failed read is logged and counts as none (nothing is
    /// reported removed).
    fn history_copy_tokens(&self) -> Vec<String> {
        match read::history_match_tokens(&self.reader) {
            Ok(tokens) => tokens,
            Err(error) => {
                tracing::warn!(%error, "the stored history tokens could not be read");
                Vec::new()
            }
        }
    }

    /// Report each token in `before` that no longer has a row removed, unless
    /// the player had removed it from stats (reported when it was hidden).
    /// A history copy imported under its own token becomes the 0.x match:
    /// its row stays, so it is not removed. A failed read reports nothing.
    fn report_copies_removed(&mut self, before: Vec<String>) {
        if before.is_empty() {
            return;
        }
        let after: HashSet<String> = match read::copy_match_tokens(&self.reader) {
            Ok(tokens) => tokens.into_iter().collect(),
            Err(error) => {
                tracing::warn!(%error, "the stored copy tokens could not be read");
                return;
            }
        };
        for token in before {
            if after.contains(&token) {
                continue;
            }
            self.visible.remove(&token);
            if !self.hidden.remove(&token) {
                self.pending_events
                    .push(TrackerEvent::MatchRemoved { token });
            }
        }
    }

    /// Mark each written token stored. A token skipped for another
    /// token's live row has no row of its own. The import is written already:
    /// a failed read is logged and the later tokens are still marked (a rerun
    /// is idempotent).
    fn mark_imported(&mut self, revisions: Vec<(String, Result<Option<u64>, store::StoreError>)>) {
        for (token, revision) in revisions {
            match revision {
                Ok(Some(_)) => {
                    // Not marked visible: a copy is not a live match, so a live
                    // event that derives it hidden must not report it removed
                    // (its row stays stored).
                    self.projector.mark_stored(&token);
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(%token, %error, "an imported match's revision could not be read");
                }
            }
        }
    }

    /// Register the 0.x accounts and their login ids (Plan O) and wait for
    /// the counts. Pending live writes commit first, so an account the log
    /// just registered keeps its login id. An account the import untracked
    /// skips its new log matches from now on, and a seeded switch reports
    /// `AccountChanged` (issue #310).
    pub fn import_accounts(
        &mut self,
        accounts: Vec<store::import::ImportedAccount>,
        now_ms: i64,
    ) -> Result<store::import::AccountImportCounts, TrackerError> {
        self.send_batch()?;
        let counts = self.writer.import_accounts(accounts, now_ms)?;
        if counts.untracked > 0 {
            self.untracked = store::accounts::untracked_usernames(&self.reader)?
                .into_iter()
                .collect();
        }
        if counts.untracked > 0 || counts.active_seeded {
            self.account_changed = true;
        }
        Ok(counts)
    }

    /// Store 0.x cards, decks, versions and match links (decks spec section 9)
    /// and wait for the counts. Sent after the matches, so links find their
    /// rows; pending live writes commit first, so live and file data win. The
    /// imported cards can link matches through oracle ids, so recent unlinked
    /// matches are tried again at the next caught-up tick.
    pub fn import_decks(
        &mut self,
        data: store::import_decks::ImportedDecks,
        now_ms: i64,
    ) -> Result<store::import_decks::DeckImportCounts, TrackerError> {
        self.send_batch()?;
        let counts = self.writer.import_decks(data, now_ms)?;
        self.decks_changed = true;
        self.links.candidates_changed();
        Ok(counts)
    }

    /// Store 0.x leagues (leagues spec section 9) and wait for the counts.
    /// Sent after the matches and the decks, so members and deck versions are
    /// found; pending live writes commit first, so live matches stay in their
    /// derived runs.
    pub fn import_leagues(
        &mut self,
        leagues: Vec<store::import_leagues::ImportedLeague>,
        now_ms: i64,
    ) -> Result<store::import_leagues::LeagueImportCounts, TrackerError> {
        self.send_batch()?;
        let counts = self.writer.import_leagues(leagues, now_ms)?;
        self.leagues_changed = true;
        // Draft and sealed leagues are imported too (Plan N).
        self.limited_changed = true;
        self.mark_every_owner();
        Ok(counts)
    }

    /// Store 0.x drafts, picks, notes and limited registered decks (limited
    /// spec section 7) and wait for the counts. Sent after the leagues, so
    /// drafts and decks find their runs; pending live writes commit first,
    /// so a live match row wins the deck's token.
    pub fn import_limited(
        &mut self,
        data: store::import_limited::ImportedLimited,
        now_ms: i64,
    ) -> Result<store::import_limited::LimitedImportCounts, TrackerError> {
        self.send_batch()?;
        let counts = self.writer.import_limited(data, now_ms)?;
        self.limited_changed = true;
        self.mark_every_owner();
        Ok(counts)
    }

    /// Store 0.x's API state (07 spec section 11) and wait for the counts.
    /// Sent after the matches, decks and leagues; pending live writes commit
    /// first. The deck sync flags it copies show on the deck pages.
    pub fn import_api(
        &mut self,
        state: store::import_api::ImportedApiState,
        now_ms: i64,
    ) -> Result<store::import_api::ApiImportCounts, TrackerError> {
        self.send_batch()?;
        let counts = self.writer.import_api(state, now_ms)?;
        if counts.deck_flags > 0 {
            self.decks_changed = true;
        }
        Ok(counts)
    }

    /// Store 0.x's archetype data (archetypes spec sections 15 and 21.5) and
    /// wait for the counts. Sent after the matches and decks; pending live
    /// writes commit first, so a match derived this tick is found. New
    /// archetypes tell the archetype pages; deck links, guides and notes the
    /// deck pages.
    pub fn import_archetypes(
        &mut self,
        data: store::import_archetypes::ImportedArchetypes,
        now_ms: i64,
    ) -> Result<store::import_archetypes::ArchetypeImportCounts, TrackerError> {
        self.send_batch()?;
        let counts = self.writer.import_archetypes(data, now_ms)?;
        if counts.archetypes_added + counts.archetypes_updated > 0 {
            self.pending_events.push(TrackerEvent::ArchetypesChanged);
        }
        if counts.decks_linked + counts.guides_added + counts.notes_added > 0 {
            self.decks_changed = true;
        }
        Ok(counts)
    }

    /// Store 0.x's overlay deck labels (overlay spec section 12) and wait for
    /// the counts. Sent after the decks; pending live writes commit first. A
    /// new label tells the deck pages and the league card (`DecksChanged`).
    pub fn import_overlay_labels(
        &mut self,
        labels: Vec<store::import_overlay::LegacyDeckLabel>,
    ) -> Result<store::import_overlay::OverlayImportCounts, TrackerError> {
        self.send_batch()?;
        let counts = self.writer.import_overlay_labels(labels)?;
        if counts.labels_added > 0 {
            self.decks_changed = true;
        }
        Ok(counts)
    }

    /// Apply a user's pick note edit (limited spec section 7) in its own
    /// transaction and return whether it was done or refused. A done edit
    /// tells the Limited pages at once: a pick note is input the league sync
    /// never reads or writes, so there is nothing to sync after it. Only the
    /// marked owners are synced before it, never a due full re-sync.
    pub fn edit_draft_note(
        &mut self,
        edit: store::limited::DraftNoteEdit,
    ) -> Result<store::limited::DraftNoteOutcome, TrackerError> {
        // Pending writes and a pending league sync commit first, so a pick
        // read this tick is stored and found.
        self.send_batch()?;
        self.sync_marked_leagues()?;
        let outcome = self.writer.edit_draft_note(edit)?;
        if outcome == store::limited::DraftNoteOutcome::Done {
            self.limited_changed = true;
        }
        Ok(outcome)
    }

    /// Apply an archetype edit (archetypes spec section 3) in its own
    /// transaction and return whether it was done or refused. A done edit
    /// tells the pages at the next tick: `ArchetypesChanged` when archetype
    /// rows or lists changed, `MatchArchetypesChanged` for the matches it
    /// changed, `DecksChanged` when a deck's archetype changed.
    /// Archetype rows are input or detection's, never a derivation's, so
    /// nothing is synced after it.
    pub fn edit_archetypes(
        &mut self,
        edit: store::archetype_edit::ArchetypeEdit,
    ) -> Result<store::archetype_edit::ArchetypeEditOutcome, TrackerError> {
        // Pending writes commit first, so a match derived this tick is found.
        self.send_batch()?;
        // Every edit of archetype rows or their lists: a definition, a stub,
        // and each manual edit (create, update, variants, merges, delete).
        // The game overlay reads its candidates again on it (overlay spec
        // 14); removing a variant moves no row count or `updated_at`.
        use store::archetype_edit::ArchetypeEdit as Edit;
        let archetypes_edit = matches!(
            edit,
            Edit::EnsureDownloaded { .. }
                | Edit::ApplyDefinitions { .. }
                | Edit::EnsureStub { .. }
                | Edit::Reinstate { .. }
                | Edit::CreateManual(_)
                | Edit::UpdateManual { .. }
                | Edit::RemoveVariant { .. }
                | Edit::ReassignVariant { .. }
                | Edit::Merge { .. }
                | Edit::Unmerge { .. }
                | Edit::DeleteManual { .. }
        );
        let outcome = self.writer.edit_archetypes(edit)?;
        if let store::archetype_edit::ArchetypeEditOutcome::Done { matches, decks } = &outcome {
            if archetypes_edit {
                self.pending_events.push(TrackerEvent::ArchetypesChanged);
            }
            if !matches.is_empty() {
                self.pending_events
                    .push(TrackerEvent::MatchArchetypesChanged {
                        match_ids: matches.clone(),
                    });
            }
            if !decks.is_empty() {
                self.decks_changed = true;
            }
        }
        Ok(outcome)
    }

    /// Write a stub for every stored catalog id without a `cards` row (the
    /// full scan, cards spec section 5) and return how many were added.
    /// Pending writes commit first, so their ids are seen. The next tick
    /// reports `CardStubsAdded` when any was added.
    pub fn scan_card_stubs(&mut self) -> Result<u32, TrackerError> {
        self.send_batch()?;
        Ok(self.writer.scan_card_stubs()?)
    }

    /// Write a stub for each given catalog id without a `cards` row and
    /// return how many were added. The next tick reports `CardStubsAdded`
    /// when any was added.
    pub fn ensure_card_stubs(&mut self, catalog_ids: Vec<u32>) -> Result<u32, TrackerError> {
        Ok(self.writer.ensure_card_stubs(catalog_ids)?)
    }

    /// Write a named token row for each token without one (an Arena
    /// replay's tokens with no art) and return how many were added. The next
    /// tick reports `CardStubsAdded` when any was added.
    pub fn ensure_token_stubs(
        &mut self,
        tokens: Vec<cards::tokens::TokenCard>,
    ) -> Result<u32, TrackerError> {
        Ok(self.writer.ensure_token_stubs(tokens)?)
    }

    /// Apply a population run (token names, then the API's answers; cards
    /// spec section 6) in its own transaction and return what it did. The
    /// log matches whose game rows read a changed name or oracle id are
    /// synced again in the background, one transaction per match. The next
    /// tick reports `CardsChanged` when any row was written, and the tick
    /// that sees the last resync answered reports it again.
    pub fn resolve_cards(
        &mut self,
        update: store::cards::CardsUpdate,
    ) -> Result<store::cards::ResolveOutcome, TrackerError> {
        // Pending writes commit first, so their stubs and game rows are seen.
        self.send_batch()?;
        let (outcome, resync) = self.writer.resolve_cards(update)?;
        if outcome.updated > 0 {
            self.cards_changed = true;
        }
        self.queue_card_resync(resync);
        Ok(outcome)
    }

    /// "Regenerate" a deck's card stats (cards spec section 8): its complete
    /// log matches are synced again and the imported rule is applied again to
    /// its imported and history matches. Returns how many matches that is.
    pub fn regenerate_card_stats(&mut self, deck_id: i64) -> Result<u32, TrackerError> {
        // Pending writes commit first, so a match that just completed counts.
        self.send_batch()?;
        let done = self.writer.regenerate_card_stats(deck_id)?;
        if done.rebuilt > 0 {
            self.cards_changed = true;
        }
        let count = done.count();
        self.queue_card_resync(done.log_tokens);
        Ok(count)
    }

    /// Record a card's local image files in their own transaction. Pages
    /// keep the remote URL until they next read, so nothing is reported.
    pub fn store_card_images(
        &mut self,
        stored: store::writer::StoredImages,
    ) -> Result<u32, TrackerError> {
        Ok(self.writer.store_card_images(stored)?)
    }

    /// Store recorder files in their own transaction.
    pub fn store_recordings(
        &mut self,
        recordings: Vec<store::recorded_replays::NewRecording>,
        now_ms: i64,
    ) -> Result<store::recorded_replays::ImportReport, TrackerError> {
        Ok(self.writer.store_recordings(recordings, now_ms)?)
    }

    /// Store Endstep capture files in their own transaction. New or changed
    /// units wake the Endstep source: the next tick folds their matches.
    pub fn store_endstep_captures(
        &mut self,
        captures: Vec<store::endstep_captures::NewCapture>,
        now_ms: i64,
    ) -> Result<store::endstep_captures::CaptureImportReport, TrackerError> {
        let report = self.writer.store_endstep_captures(captures, now_ms)?;
        // New or changed units: the next tick folds their matches.
        if report.added > 0 || report.replaced > 0 {
            self.endstep.wake();
        }
        Ok(report)
    }

    /// Forget the local files not in `files` in its own transaction. The
    /// next tick reports `CardsChanged` when any was forgotten.
    pub fn keep_present_card_images(
        &mut self,
        files: std::collections::BTreeSet<String>,
    ) -> Result<u32, TrackerError> {
        let nulled = self.writer.keep_present_images(files)?;
        if nulled > 0 {
            self.cards_changed = true;
        }
        Ok(nulled)
    }

    /// Apply an API worker's outbox edit (07 spec section 7) and wait for
    /// what it did. The outbox is the worker's own table: nothing pending
    /// needs to commit first.
    pub fn edit_outbox(
        &mut self,
        edit: store::outbox::OutboxEdit,
    ) -> Result<store::outbox::OutboxOutcome, TrackerError> {
        Ok(self.writer.edit_outbox(edit)?)
    }

    /// Apply an API worker's sync edit (07 spec section 10) and wait for how
    /// many rows it changed. Pending live writes commit first, so a pulled
    /// deck merges into the deck the files just wrote (live and file data
    /// win). A pulled deck or a slot ledger changes the deck pages.
    pub fn edit_sync(&mut self, edit: store::sync::SyncEdit) -> Result<u32, TrackerError> {
        use store::sync::SyncEdit;
        // A pulled Arena run may make or link its course deck.
        let decks = matches!(
            edit,
            SyncEdit::PulledDeck { .. }
                | SyncEdit::ApplySlots { .. }
                | SyncEdit::SignedOut
                | SyncEdit::PulledArenaRun { .. }
                | SyncEdit::ArenaRunsAfterPull { .. }
        );
        let pulled = matches!(edit, SyncEdit::PulledDeck { .. });
        // A pulled match counts in the deck, league and dashboard pages; a
        // pulled league, draft or registered deck in the league pages.
        let pulled_match = match &edit {
            SyncEdit::PulledMatch { pulled, .. } => Some(pulled.imported.token.clone()),
            _ => None,
        };
        let leagues = matches!(
            edit,
            SyncEdit::PulledMatch { .. }
                | SyncEdit::PulledLeague { .. }
                | SyncEdit::PulledArenaRun { .. }
                | SyncEdit::ArenaRunsAfterPull { .. }
                | SyncEdit::PulledSnapshots { .. }
                | SyncEdit::Tombstones { .. }
        );
        let tombstones = matches!(edit, SyncEdit::Tombstones { .. });
        self.send_batch()?;
        let changed = self.writer.edit_sync(edit)?;
        if tombstones && changed > 0 {
            // A match deleted on the website leaves the deck pages too.
            self.decks_changed = true;
        }
        if decks && changed > 0 {
            self.decks_changed = true;
        }
        if pulled {
            self.links.candidates_changed();
        }
        if let Some(token) = pulled_match {
            // As `import` treats a stored 0.x match.
            if read::match_revision(&self.reader, &token)?.is_some() {
                self.projector.mark_stored(&token);
            }
            self.decks_changed = true;
            // Its player-field step may have hidden or restored it (spec
            // 6.3), possibly on the live row with its MTGO match id under
            // another token: a change reads every hidden mark again.
            if changed > 0 {
                self.refresh_hidden()?;
            } else {
                self.refresh_hidden_token(&token)?;
            }
        }
        if tombstones && changed > 0 {
            // Matches deleted on the website (spec 6.4).
            self.refresh_hidden()?;
        }
        if leagues {
            self.leagues_changed = true;
            self.limited_changed = true;
            self.mark_every_owner();
        }
        Ok(changed)
    }

    /// Store one imported 0.x match's detail (Plan O) and wait for what it
    /// did. Sent after the matches, so it finds its row; pending live writes
    /// commit first, so a live row written this tick wins and its token never
    /// gets the detail.
    pub fn import_detail(
        &mut self,
        token: String,
        detail: store::imported_detail::ImportedDetail,
    ) -> Result<store::imported_detail::DetailImport, TrackerError> {
        self.send_batch()?;
        Ok(self.writer.import_detail(token, detail)?)
    }

    /// Store a batch of MTGO history matches (Plan M) and wait for the
    /// counts. Pending live writes commit first, so a live row written this
    /// tick wins. Each stored token is then treated as `import` treats one,
    /// and the deck, league and dashboard pages are told: the matches count
    /// in all of them.
    pub fn import_history(
        &mut self,
        matches: Vec<HistoryMatch>,
        now_ms: i64,
    ) -> Result<HistoryImportCounts, TrackerError> {
        self.send_batch()?;
        let counts = self.writer.import_history(matches, now_ms)?;
        for token in &counts.imported {
            self.projector.mark_stored(token);
        }
        if !counts.imported.is_empty() {
            self.decks_changed = true;
            self.leagues_changed = true;
            self.mark_every_owner();
        }
        Ok(counts)
    }

    /// Remove every history import ("Delete all imported", Plan M) and
    /// return the removed tokens. Each one still shown is reported removed,
    /// and the deck, league and dashboard pages are told.
    pub fn delete_history_imports(&mut self) -> Result<Vec<String>, TrackerError> {
        // Pending writes commit first, like the edits.
        self.send_batch()?;
        let removed = self.writer.delete_history_imports()?;
        for token in &removed {
            self.visible.remove(token);
            // A hidden match was reported removed when it was hidden.
            if !self.hidden.remove(token) {
                self.pending_events.push(TrackerEvent::MatchRemoved {
                    token: token.clone(),
                });
            }
        }
        if !removed.is_empty() {
            self.decks_changed = true;
            self.leagues_changed = true;
            self.mark_every_owner();
        }
        Ok(removed)
    }

    /// Apply a user's deck edit in its own transaction and return whether it
    /// was done or refused (decks spec 6).
    pub fn edit_deck(&mut self, edit: DeckEdit) -> Result<EditOutcome, TrackerError> {
        // Pending writes commit first, so the edit applies over them (a deck
        // file sync sent this tick, say).
        self.send_batch()?;
        let outcome = self.writer.edit_deck(edit)?;
        if outcome == EditOutcome::Done {
            self.decks_changed = true;
        }
        Ok(outcome)
    }

    /// Apply a user's league edit in its own transaction and return whether
    /// it was done or refused (leagues spec 5). A move, a removal, a status,
    /// a delete, an Undo, a Restore or a drop changes the fold's input or
    /// what is shown: its owner is synced again. A new manual league and
    /// notes are input the sync never folds.
    pub fn edit_league(
        &mut self,
        edit: store::leagues::LeagueEdit,
    ) -> Result<store::leagues::LeagueEditOutcome, TrackerError> {
        use store::leagues::LeagueEdit;
        // Pending writes and a pending league sync commit first, so the edit
        // sees the matches, links and runs as the pages show them.
        self.send_batch()?;
        self.sync_pending_leagues()?;
        let folded_run = match &edit {
            LeagueEdit::LinkMatches { run_key, .. }
            | LeagueEdit::UnlinkMatch { run_key, .. }
            | LeagueEdit::SetStatus { run_key, .. }
            | LeagueEdit::Delete { run_key, .. }
            | LeagueEdit::Restore { run_key, .. } => Some(run_key.clone()),
            // Undo names the row by id: its owner is the run key's now. No
            // such row is NotFound, which marks nothing.
            LeagueEdit::UndoDelete { league_id, .. } => {
                read::league_run_key(&self.reader, *league_id)?
            }
            LeagueEdit::MoveMatch { to_run_key, .. } => to_run_key.clone(),
            LeagueEdit::CreateManual { .. } | LeagueEdit::SetNotes { .. } => None,
        };
        // A delete can hide matches and its Undo shows them again: the
        // decks count them.
        let hides_matches = matches!(
            &edit,
            LeagueEdit::Delete { .. } | LeagueEdit::UndoDelete { .. }
        );
        let outcome = self.writer.edit_league(edit, self.timezone)?;
        if outcome == store::leagues::LeagueEditOutcome::Done {
            self.leagues_changed = true;
            if hides_matches {
                self.decks_changed = true;
                self.refresh_hidden()?;
            }
            if let Some(run_key) = folded_run {
                self.mark_run_owner(&run_key);
            }
        }
        Ok(outcome)
    }

    /// Apply a user's match edit (notes, "Remove from stats", the deck) in
    /// its own transaction and return whether it was done or refused
    /// (dashboard spec 4). A hide reports the match removed and refreshes
    /// decks and leagues (and so the dashboard): it counted in all of them.
    /// A new deck refreshes decks and leagues too, and syncs the league runs
    /// of its matches again (a run's deck version follows its matches'
    /// links). A note changes no pushed data, so it reports nothing; the
    /// caller has its answer.
    pub fn edit_match(&mut self, edit: MatchEdit) -> Result<MatchEditOutcome, TrackerError> {
        // Pending writes commit first, so a match derived this tick is found.
        self.send_batch()?;
        let hidden_token = match &edit {
            MatchEdit::Hide { token, .. } => Some(token.clone()),
            MatchEdit::SetNotes { .. }
            | MatchEdit::SetReplayShare { .. }
            | MatchEdit::ClearReplayShare { .. }
            | MatchEdit::Restore { .. }
            | MatchEdit::SetDeckVersion { .. } => None,
        };
        // A new deck: the links before the edit, so only the matches it
        // changed have their league runs synced again.
        let deck_links = match &edit {
            MatchEdit::SetDeckVersion {
                tokens,
                deck_version_id,
            } => {
                self.writer.flush()?;
                let mut before = Vec::new();
                for token in tokens {
                    before.push((token.clone(), link_of(&self.reader, token)?));
                }
                Some((*deck_version_id, before))
            }
            _ => None,
        };
        // A restore shows the match again (league self-management spec 4.5).
        let restored_token = match &edit {
            MatchEdit::Restore { token, .. } => Some(token.clone()),
            _ => None,
        };
        let outcome = self.writer.edit_match(edit)?;
        if outcome == MatchEditOutcome::Done
            && let Some((version, before)) = deck_links
        {
            self.decks_changed = true;
            self.leagues_changed = true;
            for (token, link) in before {
                if link != Some(version) && link_of(&self.reader, &token)? == Some(version) {
                    self.mark_match_owner(&token);
                }
            }
        }
        if outcome == MatchEditOutcome::Done
            && let Some(token) = hidden_token
        {
            self.hidden.insert(token.clone());
            self.decks_changed = true;
            self.leagues_changed = true;
            // A hidden match leaves the league runs it counted in.
            self.mark_match_owner(&token);
            self.pending_events
                .push(TrackerEvent::MatchRemoved { token });
        }
        if outcome == MatchEditOutcome::Done
            && let Some(token) = restored_token
        {
            self.refresh_hidden_token(&token)?;
        }
        Ok(outcome)
    }

    /// Read one match's hidden mark again after an edit that may have
    /// changed it (a restore, a pull): `hidden` follows the row, and a change
    /// is reported (`hidden_changed`). A match with no row counts as not
    /// hidden.
    fn refresh_hidden_token(&mut self, token: &str) -> Result<(), TrackerError> {
        let now_hidden = store::match_edits::is_hidden(&self.reader, token)?;
        if now_hidden != self.hidden.contains(token) {
            self.hidden_changed(token.to_string(), now_hidden)?;
        }
        Ok(())
    }

    /// Read every hidden mark again after an edit that may hide or restore
    /// many matches (a league deleted with its matches, its Undo, the
    /// website's deletions). Changes are reported in token order.
    fn refresh_hidden(&mut self) -> Result<(), TrackerError> {
        let stored: HashSet<String> = store::match_edits::hidden_match_tokens(&self.reader)?
            .into_iter()
            .collect();
        let mut changes: Vec<(String, bool)> = Vec::new();
        for token in &stored {
            if !self.hidden.contains(token) {
                changes.push((token.clone(), true));
            }
        }
        for token in &self.hidden {
            if !stored.contains(token) {
                changes.push((token.clone(), false));
            }
        }
        changes.sort();
        for (token, hidden) in changes {
            self.hidden_changed(token, hidden)?;
        }
        Ok(())
    }

    /// One match hidden or shown again by an edit other than "Delete match"
    /// (league self-management spec 4.6). Hidden: reported removed, as a
    /// hide is. Shown again: a log match the projector holds is pushed as it
    /// has it, a manual match as stored, an Arena or Endstep log match as
    /// stored (its source keeps no live view of it); an MTGO log match the
    /// projector no longer holds comes back when its page reloads. Either way decks and leagues are told and the match's league
    /// runs are synced again (it counts in them again, or no longer).
    fn hidden_changed(&mut self, token: String, hidden: bool) -> Result<(), TrackerError> {
        self.decks_changed = true;
        self.leagues_changed = true;
        self.mark_match_owner(&token);
        if hidden {
            self.hidden.insert(token.clone());
            self.pending_events
                .push(TrackerEvent::MatchRemoved { token });
            return Ok(());
        }
        self.hidden.remove(&token);
        if let Some(Derived::Visible(view)) = self.projector.derived(&token) {
            let view = view.as_ref().clone();
            let revision = self.projector.revision(&token);
            self.push_projected(view, revision);
        } else if !self.push_manual(&token)?
            && let Some(platform @ (Platform::Arena | Platform::Endstep)) =
                store::platform::match_platform(&self.reader, &token)?
        {
            self.push_stored(&token, platform)?;
        }
        Ok(())
    }

    /// Push the stored Arena or Endstep log match `token` (if there is one)
    /// on `platform`, as the match list shows it, with its opponent's shown
    /// name.
    fn push_stored(&mut self, token: &str, platform: Platform) -> Result<(), TrackerError> {
        if let Some((view, revision)) = store::manual::stored_view(&self.reader, token)? {
            let (_, opponent_name) = store::manual::manual_push_parts(&self.reader, token)?;
            self.push_platform_view(view, revision, platform, opponent_name);
        }
        Ok(())
    }

    /// Apply a user's account edit (release spec 5: a switch, a tracking
    /// change) in its own transaction and return whether it was done or
    /// refused. A done edit reports `AccountChanged`. Tracking turned back on
    /// brings the account's skipped matches back: a match still in the live
    /// working set is derived again at once, the others join the rebuild
    /// queue (`rebuild_some`), so they come back as tracking all along would
    /// have made them.
    pub fn edit_account(&mut self, edit: AccountEdit) -> Result<AccountEditOutcome, TrackerError> {
        // Pending writes commit first, so a skip recorded this tick is
        // handed back.
        self.send_batch()?;
        let outcome = self.writer.edit_account(edit.clone())?;
        let AccountEditOutcome::Done { retrack } = &outcome else {
            return Ok(outcome);
        };
        self.account_changed = true;
        if let AccountEdit::SetTracked { username, tracked } = &edit {
            if *tracked {
                self.untracked.remove(username);
            } else {
                self.untracked.insert(username.clone());
            }
        }
        for token in retrack.clone() {
            self.retrack(token);
        }
        Ok(outcome)
    }

    /// Build a skipped match of an account tracked again: at once when it
    /// is in the live working set, else through the rebuild queue. It stays
    /// in `skipped` until then, so a rebuild that derives no match removes
    /// its skip row (`record_derived`); a built one clears it on upsert.
    fn retrack(&mut self, token: String) {
        self.skipped.insert(token.clone());
        if self.projector.derived(&token).is_some() && !self.projector.is_retired(&token) {
            self.handle_changed(vec![token], None);
        } else if !self.pending_rebuild.contains(&token) {
            self.pending_rebuild.push(token);
            self.rebuilding = true;
        }
    }

    /// Queue the skipped matches of tracked accounts: tracking came back
    /// and the tracker stopped before their rebuild wrote them.
    fn plan_retrack(&mut self) -> Result<(), TrackerError> {
        for token in store::accounts::retrack_queue(&self.reader)? {
            self.retrack(token);
        }
        Ok(())
    }

    /// Whether a derived match is an untracked account's new one: it has no
    /// row (one that exists is never removed by untracking, release spec 5).
    /// A failed read counts as a row: the match is built, nothing is lost.
    fn skips(&self, view: &MatchView) -> bool {
        self.skips_new(&view.token, &view.local_username)
    }

    /// `skips` for `token`, played by `username`.
    fn skips_new(&self, token: &str, username: &str) -> bool {
        if !self.is_skippable(token, username) {
            return false;
        }
        match read::match_revision(&self.reader, token) {
            Ok(revision) => revision.is_none(),
            Err(error) => {
                tracing::warn!(%error, "could not read whether a match has a row");
                false
            }
        }
    }

    /// Whether `token`, played by `username`, is an untracked account's and
    /// not shown this run.
    fn is_skippable(&self, token: &str, username: &str) -> bool {
        self.untracked.contains(username) && !self.visible.contains(token)
    }

    /// Whether a failed match is skipped as an untracked account's new one
    /// (`skips`): not one already counted as failed, which stays a failure.
    fn skips_failed(&self, token: &str, username: &str) -> bool {
        !self.failed.contains(token) && self.skips_new(token, username)
    }

    /// Apply a user's manual match edit (Plan L: create, hand, sideboard,
    /// reveals; and "Edit match", a correction of any match) in its own
    /// transaction and return whether it was done or refused. A done edit
    /// pushes a manual match (a new one, or a new revision) unless it was
    /// removed from stats, and refreshes decks and leagues (and so the
    /// dashboard): every match counts in all of them.
    pub fn edit_manual(&mut self, edit: ManualEdit) -> Result<ManualEditOutcome, TrackerError> {
        // Pending writes and a pending league sync commit first, like the
        // league edits.
        self.send_batch()?;
        self.sync_pending_leagues()?;
        let mut edited = edit.token().map(str::to_string);
        // A correction names its match by row id; its league owners are
        // synced in the edit's own transaction.
        if let ManualEdit::Correct { match_id, .. } = &edit {
            edited = store::manual::match_token(&self.reader, *match_id)?;
        }
        // A match placed in a league run is fold input.
        let placed = matches!(
            &edit,
            ManualEdit::Create(manual)
                if !matches!(manual.placement, store::manual::ManualPlacement::None)
        );
        // A claimed rank gap moves the rank views. Only an Arena deck's match
        // claims one: the store ignores a gap sent with an MTGO deck.
        let claims_gap = match &edit {
            ManualEdit::Create(manual) if manual.rank_gap.is_some() => {
                store::platform::deck_platform(&self.reader, manual.deck_id)? == Platform::Arena
            }
            _ => false,
        };
        let outcome = self.writer.edit_manual(edit, self.timezone)?;
        if claims_gap && matches!(outcome, ManualEditOutcome::Created { .. }) {
            self.rank_changed = true;
        }
        let token = match &outcome {
            ManualEditOutcome::Created { token } => Some(token.clone()),
            ManualEditOutcome::Done | ManualEditOutcome::HandCleared => edited,
            _ => None,
        };
        let Some(token) = token else {
            return Ok(outcome);
        };
        self.decks_changed = true;
        self.leagues_changed = true;
        if placed {
            self.mark_match_owner(&token);
        }
        if !self.hidden.contains(&token) {
            self.push_manual(&token)?;
        }
        Ok(outcome)
    }

    /// Push the stored manual match `token` (if there is one) on its
    /// account's platform, with an Arena opponent's shown name. Returns
    /// whether it was a manual match.
    fn push_manual(&mut self, token: &str) -> Result<bool, TrackerError> {
        if let Some((view, revision)) = store::manual::manual_view(&self.reader, token)? {
            let (platform, opponent_name) = store::manual::manual_push_parts(&self.reader, token)?;
            self.pending_events.push(TrackerEvent::MatchUpdated {
                view,
                revision,
                cause: None,
                manual: true,
                platform,
                opponent_name,
            });
            return Ok(true);
        }
        Ok(false)
    }

    /// Push each Arena match written, with its platform and its opponent's
    /// display name; one the player removed from stats is stored, never
    /// shown (as MTGO's).
    fn push_arena(&mut self, writes: &[MatchWrite]) {
        for write in writes {
            let MatchWrite::Upsert { view, revision } = write else {
                continue;
            };
            if self.hidden.contains(&view.token) {
                continue;
            }
            let opponent_name = self.arena.opponent_name(view.opponent.as_deref());
            self.push_platform_view(
                view.as_ref().clone(),
                *revision,
                Platform::Arena,
                opponent_name,
            );
        }
    }

    /// Push a match of another platform than MTGO, with its opponent's
    /// display name (its view names the opponent by registry key).
    fn push_platform_view(
        &mut self,
        view: MatchView,
        revision: u64,
        platform: Platform,
        opponent_name: Option<String>,
    ) {
        self.pending_events.push(TrackerEvent::MatchUpdated {
            view,
            revision,
            cause: None,
            manual: false,
            platform,
            opponent_name,
        });
    }

    /// Continue every unsealed file at its committed cursor and clock.
    fn resume_files(&mut self) -> Result<(), TrackerError> {
        for file in read::active_log_files(&self.reader)? {
            let parsed: Option<Tz> = file.timezone.parse().ok();
            let bad_timezone = parsed.is_none();
            if bad_timezone {
                tracing::warn!(
                    file = file.id,
                    timezone = %file.timezone,
                    "a stored log timezone did not parse; dating the file in the current zone"
                );
            }
            let tz = parsed.unwrap_or(self.timezone);
            let ctx = FileContext::resume(
                file.id as u64,
                tz,
                file.clock,
                file.local_username.clone(),
                file.local_login_id,
            );
            let recorded = Recorded {
                last_observed_size: file.last_observed_size,
                created_ms: file.created_at_fs,
                head_hash: file.head_hash,
                head_len: file.head_len,
                anchor: file.anchor.map(|(offset, hash)| Anchor { offset, hash }),
            };
            let path = PathBuf::from(&file.path);
            let tracked = TrackedFile {
                id: file.id,
                tailer: Tailer::new(path.clone(), file.cursor_offset),
                path,
                recorded,
                ctx,
                size: file.last_observed_size,
                saved: Some((file.cursor_offset, file.last_observed_size)),
                previewed: None,
                previewed_tokens: Vec::new(),
                bad_timezone,
            };
            // A file that is gone (an old ClickOnce folder, #86) waits in
            // `parked`: never polled, and resumed at its cursor if a rescan
            // finds it again. Its row stays active.
            if tracked.path.exists() {
                self.files.push(tracked);
            } else {
                self.parked.push(tracked);
            }
        }
        Ok(())
    }

    /// Bring matches that can still change on their own back into memory (spec 3,
    /// startup rehydration), and mark every other stored match so its next event
    /// reloads its history. A finished match active within two abandon windows
    /// can still change with time (a lost connection settles after
    /// `ABANDON_AFTER_MS`), so it comes back too; `retire_quiet` drops it again
    /// once its result is stable.
    fn rehydrate(&mut self, now_ms: i64) -> Result<(), TrackerError> {
        let active_since_ms = now_ms - 2 * ABANDON_AFTER_MS;
        for (token, _) in read::open_matches(&self.reader, active_since_ms)? {
            self.visible.insert(token.clone());
            self.reload(&token)?;
        }
        for token in read::stored_match_tokens(&self.reader)? {
            self.visible.insert(token.clone());
            self.projector.mark_stored(&token);
        }
        // A 0.x or history copy is stored, not a live match to report removed.
        for token in read::copy_match_tokens(&self.reader)? {
            self.visible.remove(&token);
        }
        // A detached match is stored but not shown: its removal was reported.
        for token in read::detached_match_tokens(&self.reader)? {
            self.visible.remove(&token);
        }
        // A match joined without a game state yet has stored events but no row.
        // Tokens never joined are lobby noise: they cannot become visible, so
        // they are not marked (thousands per hour of play).
        for token in read::joined_match_tokens(&self.reader)? {
            self.projector.mark_stored(&token);
        }
        Ok(())
    }

    fn plan_rebuild(&mut self) -> Result<(), TrackerError> {
        match read::derivation_version(&self.reader)? {
            None => self.batch.derivation = Some(PROJECTION_VERSION),
            Some(version) if version < PROJECTION_VERSION => {
                let mut tokens = read::stale_matches(&self.reader, PROJECTION_VERSION)?;
                // A match that failed before it had a row is only in match_failures.
                for token in read::stale_failures(&self.reader, PROJECTION_VERSION)? {
                    if !tokens.contains(&token) {
                        tokens.push(token);
                    }
                }
                self.pending_rebuild = tokens;
                self.rebuilding = true;
            }
            Some(_) => {}
        }
        Ok(())
    }

    /// Re-derive a few stale matches from storage with a fresh projector.
    fn rebuild_some(&mut self, now_ms: i64) -> Result<(), TrackerError> {
        if !self.rebuilding {
            return Ok(());
        }
        // Storage must hold every event read so far: a Login ingested earlier in
        // this tick changes the usernames the history reads (and caches).
        self.flush()?;
        let count = self.pending_rebuild.len().min(REBUILDS_PER_TICK);
        let tokens: Vec<String> = self.pending_rebuild.drain(..count).collect();
        for token in tokens {
            // The live working set re-derives its own matches already.
            if self.projector.derived(&token).is_some() && !self.projector.is_retired(&token) {
                continue;
            }
            let stored_revision = read::match_revision(&self.reader, &token)?.unwrap_or(0);
            let events = self.history.match_events(&self.reader, &token)?;
            let mut projector = Projector::new(self.projector.known_accounts().to_vec());
            projector.seed_revision(&token, stored_revision);
            for event in &events {
                projector.apply(event);
            }
            projector.finalize(now_ms);
            let revision = projector.revision(&token);
            let derived = projector.derived(&token).cloned();
            let player = projector.local_player(&token);
            self.record_derived_for(token, derived, revision, None, player);
        }
        if self.pending_rebuild.is_empty() {
            self.rebuilding = false;
            self.batch.derivation = Some(PROJECTION_VERSION);
        }
        Ok(())
    }

    /// Archive finished, quiet matches that are not in the live working set.
    fn plan_compaction(&mut self, now_ms: i64) -> Result<(), TrackerError> {
        self.flush()?;
        for token in read::compactable_matches(&self.reader, now_ms - COMPACT_AFTER_MS)? {
            let in_memory =
                self.projector.derived(&token).is_some() && !self.projector.is_retired(&token);
            if !in_memory {
                self.batch.compact.push(token);
            }
        }
        Ok(())
    }

    /// MTGO replaced the file: seal its row and start a new one at offset 0,
    /// keeping the local username (rotation does not log in again).
    fn rotate(
        &mut self,
        index: usize,
        reason: SealReason,
        now_ms: i64,
    ) -> Result<(), TrackerError> {
        let inherited = self.files[index].ctx.local_username().map(str::to_string);
        let path = self.files[index].path.clone();
        self.batch.seals.push(Seal {
            log_file_id: self.files[index].id,
            reason: reason.as_str().to_string(),
            at_ms: now_ms,
        });
        // The seal must commit before the new row: one active row per path.
        self.send_batch()?;
        match self.start_file(path, inherited, now_ms)? {
            Some(file) => self.files[index] = file,
            None => {
                self.files.remove(index);
            }
        }
        Ok(())
    }

    /// Read the OS timezone again, when the app gave the engine a way to.
    fn refresh_timezone(&mut self) {
        if let Some(read) = self.config.os_timezone {
            let (timezone, warning) = read();
            self.timezone = timezone;
            self.timezone_warning = warning;
        }
    }

    fn rescan(&mut self, now_ms: i64) -> Result<(), TrackerError> {
        let before = self.timezone;
        self.refresh_timezone();
        if self.timezone != before {
            // Run names, pick times and game rows depend on the zone.
            tracing::info!(from = %before.name(), to = %self.timezone.name(), "the OS timezone changed; folding leagues and games again");
            self.refold_for_new_zone();
            self.restart_game_backfill()?;
        }
        let detection = detect(&self.env, &self.config.log_overrides);
        self.roots = detection.roots;
        self.park_undetected(&detection.logs);
        for path in detection.logs {
            if self.is_tracked(&path) {
                continue;
            }
            if let Some(file) = self.take_parked(&path) {
                self.files.push(file);
                continue;
            }
            let inherited = read::latest_username_for_path(&self.reader, &path.to_string_lossy())?;
            if let Some(file) = self.start_file(path, inherited, now_ms)? {
                self.files.push(file);
            }
        }
        Ok(())
    }

    /// Move every tracked file that `detected` does not contain (by path or real
    /// path) to `parked`.
    fn park_undetected(&mut self, detected: &[PathBuf]) {
        let detected_real: Vec<PathBuf> = detected.iter().map(|path| real_path(path)).collect();
        let mut index = 0;
        while index < self.files.len() {
            let path = &self.files[index].path;
            let found = detected.contains(path) || detected_real.contains(&real_path(path));
            if found {
                index += 1;
            } else {
                let mut file = self.files.remove(index);
                tracing::info!(path = %file.path.display(), "log no longer detected; not polling it");
                self.clear_preview_of(&mut file);
                self.parked.push(file);
            }
        }
    }

    /// A parked file is not read, so its provisional event is never
    /// committed or superseded: show each match it previewed as the
    /// projector has it (or remove the preview of a match it never held).
    /// The file previews its last event again when it resumes.
    fn clear_preview_of(&mut self, file: &mut TrackedFile) {
        file.previewed = None;
        for token in std::mem::take(&mut file.previewed_tokens) {
            match self.projector.derived(&token) {
                Some(Derived::Visible(view)) if !self.hidden.contains(&token) => {
                    let view = view.as_ref().clone();
                    let revision = self.projector.revision(&token);
                    self.push_projected(view, revision);
                }
                _ => self
                    .pending_events
                    .push(TrackerEvent::MatchRemoved { token }),
            }
        }
    }

    /// The parked file for `path` (or an alias of it), removed from `parked`.
    fn take_parked(&mut self, path: &Path) -> Option<TrackedFile> {
        let real = real_path(path);
        let index = self
            .parked
            .iter()
            .position(|file| file.path == path || real_path(&file.path) == real)?;
        Some(self.parked.remove(index))
    }

    /// Is this log already tracked, under this path or an alias of it? Discovery
    /// keeps one path per physical file, but which alias it keeps can change
    /// between scans (a symlinked root appears), so compare real paths.
    fn is_tracked(&self, path: &Path) -> bool {
        let real = real_path(path);
        self.files
            .iter()
            .any(|file| file.path == path || real_path(&file.path) == real)
    }

    /// A new `log_files` row for `path`, read from offset 0. `None` if the file
    /// vanished or is locked; the next rescan tries again.
    fn start_file(
        &mut self,
        path: PathBuf,
        inherited: Option<String>,
        now_ms: i64,
    ) -> Result<Option<TrackedFile>, TrackerError> {
        #[cfg(test)]
        if test_hooks::take_vanish(&path) {
            return Ok(None);
        }
        let Ok(probe) = identity::probe(&path, None) else {
            return Ok(None);
        };
        let day_anchor = match self.config.replay_day_anchor {
            Some(day) => day,
            None => new_file_day_anchor(&path, &probe, now_ms, self.timezone),
        };
        let id = self.writer.create_log_file(NewLogFile {
            path: path.to_string_lossy().to_string(),
            identity_hash: identity::identity_hash(&probe),
            head_hash: probe.head_hash,
            head_len: probe.head_len,
            created_at_fs: probe.created_ms,
            day_anchor,
            timezone: self.timezone.name().to_string(),
            inherited_username: inherited.clone(),
            seen_at_ms: now_ms,
        })?;
        Ok(Some(TrackedFile {
            id,
            path: path.clone(),
            recorded: Recorded::first(&probe),
            ctx: FileContext::new(id as u64, day_anchor, self.timezone, inherited),
            tailer: Tailer::new(path, 0),
            size: 0,
            saved: None,
            previewed: None,
            previewed_tokens: Vec::new(),
            bad_timezone: false,
        }))
    }

    /// Read one file. Returns `true` when it still has unread bytes.
    fn poll_file(&mut self, index: usize, now_ms: i64) -> Result<bool, TrackerError> {
        let anchor_offset = self.files[index]
            .recorded
            .anchor
            .map(|anchor| anchor.offset);
        // A file that vanished or is locked (Windows sharing) is retried next tick.
        // The probe runs just before `poll`, which reopens the file by path.
        let Ok(probe) = identity::probe(&self.files[index].path, anchor_offset) else {
            return Ok(false);
        };
        // Seal a replaced file before reading a byte of its replacement.
        if let Some(reason) = identity::detect_rotation(&self.files[index].recorded, &probe) {
            self.rotate(index, reason, now_ms)?;
            return Ok(true);
        }
        identity::observe(&mut self.files[index].recorded, &probe);
        self.files[index].size = probe.size;
        let Ok(read) = self.files[index].tailer.poll(probe.size, now_ms) else {
            return Ok(false);
        };
        let live = if self.config.replay_day_anchor.is_some() {
            None
        } else {
            read.live_read_at_ms
        };
        // The overlay marks of a file still catching up a backlog are not
        // live: dropped, while the other files' marks are kept (#303 F7).
        let kept = std::mem::take(&mut self.live);
        for raw in &read.events {
            self.ingest(index, raw, live);
        }
        let read_marks = std::mem::replace(&mut self.live, kept);
        if read.at_eof {
            self.live.merge(read_marks);
        }
        self.preview(index, read.provisional.as_ref(), live);
        self.record_progress(index, now_ms);
        Ok(!read.at_eof)
    }

    fn record_progress(&mut self, index: usize, now_ms: i64) {
        let file = &mut self.files[index];
        let cursor = file.tailer.cursor();
        if file.recorded.anchor.is_none() && cursor >= ANCHOR_OFFSET + ANCHOR_LEN {
            file.recorded.anchor = identity::read_anchor(&file.path).ok().flatten();
        }
        let mark = (cursor, file.recorded.last_observed_size);
        if file.saved == Some(mark) {
            return;
        }
        file.saved = Some(mark);
        self.batch.files.push(FileProgress {
            log_file_id: file.id,
            cursor_offset: cursor,
            clock: file.ctx.clock_state(),
            last_observed_size: file.recorded.last_observed_size,
            head_hash: file.recorded.head_hash,
            head_len: file.recorded.head_len,
            anchor: file
                .recorded
                .anchor
                .map(|anchor| (anchor.offset, anchor.hash)),
            local_username: file.ctx.local_username().map(str::to_string),
            local_login_id: file.ctx.local_login_id(),
            seen_at_ms: now_ms,
        });
    }

    fn set_in_flight(&self, event: Option<InFlightEvent>) {
        // A poisoned lock still holds a usable slot: keep pinning events.
        match self.in_flight.lock() {
            Ok(mut slot) => *slot = event,
            Err(poisoned) => *poisoned.into_inner() = event,
        }
    }

    /// Prepare, store and project one committed event.
    fn ingest(&mut self, index: usize, raw: &RawEvent, live: Option<i64>) {
        let file_id = self.files[index].id;
        let key = (file_id, raw.offset);
        self.set_in_flight(Some(InFlightEvent {
            key,
            text_len: raw.text.len(),
        }));
        if self.quarantine.contains(&key) {
            self.batch.events.push(NewEvent {
                log_file_id: file_id,
                byte_offset: raw.offset,
                kind: None,
                subjects: Subjects::default(),
                logged_at_ms: self.newest_event_ms,
                raw: raw.text.clone(),
                quarantined: true,
            });
            self.quarantined += 1;
            self.set_in_flight(None);
            return;
        }
        #[cfg(test)]
        test_hooks::panic_if_poisoned(&self.config.db_path, key);
        if let Some(event) = self.files[index].ctx.prepare_with(raw, live) {
            self.newest_event_ms = self.newest_event_ms.max(event.at_ms);
            self.batch.events.push(NewEvent {
                log_file_id: file_id,
                byte_offset: event.offset,
                kind: Some(event.kind),
                subjects: event.subjects.clone(),
                logged_at_ms: event.at_ms,
                raw: event.text.clone(),
                quarantined: false,
            });
            self.live.note(event.kind, &event.subjects);
            self.note_tournament_event(&event);
            if event.kind == EventKind::Login {
                self.learn_login(index, &event, key);
            }
            if matches!(
                event.kind,
                EventKind::LeagueJoined | EventKind::LeagueDropped
            ) {
                self.leagues_changed = true;
            }
            if store::limited::DRAFT_KINDS.contains(&event.kind) {
                self.limited_changed = true;
            }
            // The league sync's inputs: the file player's league, draft,
            // registered deck and tournament round lines (a match's
            // tournament token), and which account a file's lines belong to
            // (its logins).
            if store::limited::DRAFT_KINDS.contains(&event.kind)
                || matches!(
                    event.kind,
                    EventKind::LeagueJoined
                        | EventKind::LeagueDropped
                        | EventKind::MatchDeckRegistered
                        | EventKind::TournamentRoundInfo
                        | EventKind::Login
                )
            {
                let player = self.files[index].ctx.local_username().map(str::to_string);
                self.mark_account(player.as_deref());
            }
            let changed = self.projector.apply(&event);
            self.handle_changed(changed, Some(key));
        }
        self.set_in_flight(None);
    }

    fn learn_login(&mut self, index: usize, event: &PreparedEvent, key: (i64, u64)) {
        self.history.forget_file(self.files[index].id);
        let Some(name) = event.local_username.clone() else {
            return;
        };
        // A Login activates (0.x): the current account may be another now.
        self.account_changed = true;
        self.batch.accounts.push(NewAccount {
            username: name.clone(),
            login_id: self.files[index].ctx.local_login_id(),
            seen_at_ms: self.batch.now_ms,
            logged_at_ms: Some(event.at_ms),
        });
        let is_new = !self.projector.known_accounts().contains(&name);
        let changed = self.projector.add_account(&name);
        self.handle_changed(changed, Some(key));
        // A match that waited for this username before a restart is only in
        // storage, where `add_account` cannot see it.
        if is_new {
            self.recheck_unrowed = true;
            // A new account's deck files, and 0.x's first-account adoption of
            // decks stored without one (the store checks it is the first).
            self.deck_sync_wanted = true;
            self.batch.decks.push(DeckWrite::AdoptOrphans {
                account: name,
                at_ms: self.batch.now_ms,
            });
            self.decks_changed = true;
            self.links.candidates_changed();
        }
    }

    /// Show what the provisional last event would change (spec 5.2), once per
    /// version of it.
    fn preview(&mut self, index: usize, provisional: Option<&RawEvent>, live: Option<i64>) {
        let Some(raw) = provisional else {
            self.files[index].previewed = None;
            return;
        };
        let mark = Some((raw.offset, raw.len));
        if self.files[index].previewed == mark {
            return;
        }
        self.files[index].previewed = mark;
        let key = (self.files[index].id, raw.offset);
        // A quarantined event is never previewed: it crashed the tracker before.
        if self.quarantine.contains(&key) {
            return;
        }
        self.set_in_flight(Some(InFlightEvent {
            key,
            text_len: raw.text.len(),
        }));
        #[cfg(test)]
        test_hooks::panic_if_poisoned(&self.config.db_path, key);
        let mut ctx = self.files[index].ctx.clone();
        if let Some(event) = ctx.prepare_with(raw, live) {
            self.files[index].previewed_tokens.clear();
            for view in self.projector.preview(&event) {
                if self.hidden.contains(&view.token) || self.skips(&view) {
                    continue;
                }
                self.files[index].previewed_tokens.push(view.token.clone());
                self.pending_events.push(TrackerEvent::MatchPreview {
                    view,
                    platform: Platform::Mtgo,
                    opponent_name: None,
                });
            }
        }
        self.set_in_flight(None);
    }

    fn handle_changed(&mut self, tokens: Vec<String>, cause: Option<(i64, u64)>) {
        for token in tokens {
            let revision = self.projector.revision(&token);
            let derived = self.projector.derived(&token).cloned();
            self.record_derived(token, derived, revision, cause);
        }
    }

    /// Queue the stored write and the push for one match's new derived result.
    fn record_derived(
        &mut self,
        token: String,
        derived: Option<Derived>,
        revision: u64,
        cause: Option<(i64, u64)>,
    ) {
        let player = match derived {
            Some(Derived::Failed(_)) => self.projector.local_player(&token),
            _ => None,
        };
        self.record_derived_for(token, derived, revision, cause, player);
    }

    /// `record_derived`, with the local player of a failed match read from
    /// the projector that derived it (`None` when it names none).
    fn record_derived_for(
        &mut self,
        token: String,
        derived: Option<Derived>,
        revision: u64,
        cause: Option<(i64, u64)>,
        failed_player: Option<String>,
    ) {
        match derived {
            Some(Derived::Visible(view)) if self.skips(&view) => {
                // An untracked account's new match (0.x `ProcessMatchEvents`):
                // its events stay stored, and it is built again when its
                // account is tracked again (release spec 5, ruling 7).
                self.skipped.insert(token.clone());
                self.batch.matches.push(MatchWrite::Skip {
                    token,
                    username: view.local_username.clone(),
                });
            }
            Some(Derived::Visible(view)) => {
                self.skipped.remove(&token);
                self.visible.insert(token.clone());
                self.failed.remove(&token);
                if in_league_or_tournament(&view) {
                    self.league_matches.insert(token.clone());
                    self.leagues_changed = true;
                    self.mark_account(Some(&view.local_username));
                }
                self.queue_link(&view);
                self.note_game_sync(&view, revision);
                if self.takes_over_a_hidden_copy(&view) {
                    self.hidden.insert(token.clone());
                }
                self.report_replaced_copies(&view);
                self.batch.matches.push(MatchWrite::Upsert {
                    view: view.clone(),
                    revision,
                });
                // Removed from stats: stored as derived, never shown again.
                if !self.hidden.contains(&token) {
                    // Stored with MTGO's verdict over the derived outcome
                    // (`UPSERT_MATCH`): shown the same way.
                    let mut view = *view;
                    view.outcome = self.history_verdict(&token, view.outcome);
                    self.pending_events.push(TrackerEvent::MatchUpdated {
                        view,
                        revision,
                        cause,
                        manual: false,
                        platform: Platform::Mtgo,
                        opponent_name: None,
                    });
                }
            }
            Some(Derived::Failed(_))
                if failed_player
                    .as_deref()
                    .is_some_and(|player| self.skips_failed(&token, player)) =>
            {
                // An untracked account's new match is skipped whether it
                // builds or fails: its failure is neither stored nor told
                // (#316). Tracked again, it is rebuilt and fails as a
                // tracked match would.
                self.skipped.insert(token.clone());
                self.batch.matches.push(MatchWrite::Skip {
                    token,
                    username: failed_player.unwrap_or_default(),
                });
            }
            Some(Derived::Failed(reason)) => {
                // A warning, so Sentry keeps it as a breadcrumb: the panic hook
                // already reported the caught panic once.
                tracing::warn!(%token, %reason, "a match could not be projected");
                self.skipped.remove(&token);
                self.failed.insert(token.clone());
                self.links.forget(&token);
                self.batch.matches.push(MatchWrite::Attention {
                    token: token.clone(),
                    reason: reason.clone(),
                });
                self.pending_events
                    .push(TrackerEvent::MatchFailed { token, reason });
            }
            _ => {
                let was_linked = self.links.is_linked(&token);
                self.links.forget(&token);
                self.forget_game_sync(&token);
                let was_visible = self.visible.remove(&token);
                let was_failed = self.failed.remove(&token);
                let was_skipped = self.skipped.remove(&token);
                // Remove deletes the row, the failure and the skip, whichever
                // exists.
                if was_visible || was_failed || was_skipped {
                    self.batch.matches.push(MatchWrite::Remove {
                        token: token.clone(),
                    });
                }
                if was_visible && self.was_league_match(&token) {
                    self.leagues_changed = true;
                    self.mark_match_owner(&token);
                }
                if was_visible {
                    // A linked match counted in its deck's numbers. Only links
                    // this run knows are checked; the UI also refreshes decks
                    // on every `MatchRemoved`.
                    if was_linked {
                        self.decks_changed = true;
                    }
                    self.pending_events
                        .push(TrackerEvent::MatchRemoved { token });
                }
            }
        }
    }

    /// The upsert below deletes each 0.x or history copy of this match (same
    /// Match ID, another token): tell the UI, so an open list does not show
    /// both. One indexed read per upsert; a copy already reported this tick
    /// (the delete is not written yet) is not reported twice. A failed read
    /// is logged: the list is right after its next refetch.
    fn report_replaced_copies(&mut self, view: &MatchView) {
        let Some(mtgo_match_id) = view.mtgo_match_id else {
            return;
        };
        let copies = match store::match_edits::copy_tokens_for_match_id(
            &self.reader,
            &view.token,
            mtgo_match_id as i64,
        ) {
            Ok(copies) => copies,
            Err(error) => {
                tracing::warn!(%error, "a copy check failed");
                return;
            }
        };
        for copy in copies {
            self.visible.remove(&copy);
            // A copy removed from stats was reported removed when it was hidden.
            if self.hidden.contains(&copy) {
                continue;
            }
            let event = TrackerEvent::MatchRemoved { token: copy };
            if !self.pending_events.contains(&event) {
                self.pending_events.push(event);
            }
        }
    }

    /// Whether this live match takes over a 0.x or history copy (same Match
    /// ID, another token) the player removed from stats: the upsert hands the
    /// copy's hidden mark to the live row (`hand_over_records`), so the match
    /// is hidden from now on. Read once per match and run, when its Match ID
    /// is first known; a copy is gone once taken over, and the next run loads
    /// the live row's own mark. A failed read counts as no: the match shows
    /// until the next run.
    fn takes_over_a_hidden_copy(&mut self, view: &MatchView) -> bool {
        let Some(mtgo_match_id) = view.mtgo_match_id else {
            return false;
        };
        if self.hidden.contains(&view.token) || !self.takeover_checked.insert(view.token.clone()) {
            return false;
        }
        match store::match_edits::hidden_copy_exists(
            &self.reader,
            &view.token,
            mtgo_match_id as i64,
        ) {
            Ok(found) => found,
            Err(error) => {
                tracing::warn!(%error, "a hidden copy check failed");
                false
            }
        }
    }

    /// Whether a match being removed was a league or tournament match: one
    /// seen visible as such in this run, or stored as such (a stored match
    /// reloaded and removed before this run derived it). A failed read counts
    /// as yes: an extra refresh is harmless.
    fn was_league_match(&mut self, token: &str) -> bool {
        if self.league_matches.remove(token) {
            return true;
        }
        match read::is_league_or_tournament_match(&self.reader, token) {
            Ok(stored) => stored,
            Err(error) => {
                tracing::warn!(%token, %error, "could not read whether a removed match was a league match");
                true
            }
        }
    }

    /// Answer the projector's questions from storage (see `mtgo_match::Lookup`).
    /// Every lookup taken is answered here: a found match is reloaded, and a
    /// Match ID or Game ID that storage does not know is dropped (its held events
    /// wait in the projector for the mapping, or `retire_quiet` drops them).
    fn answer_lookups(&mut self) -> Result<(), TrackerError> {
        let mut flushed = false;
        if self.recheck_unrowed {
            self.recheck_unrowed = false;
            self.flush()?;
            flushed = true;
            for token in read::joined_tokens_without_match(&self.reader)? {
                // A failed match is retried by the next projection version;
                // reloading it here would report its failure again on every start.
                if self.projector.is_retired(&token) && !self.failed.contains(&token) {
                    self.reload(&token)?;
                }
            }
        }
        for _ in 0..LOOKUP_ROUNDS {
            let lookups = self.projector.take_lookups();
            if lookups.is_empty() {
                return Ok(());
            }
            if !flushed {
                // Storage must hold every event the projector has seen, including
                // the ones that raised these lookups.
                self.flush()?;
                flushed = true;
            }
            for lookup in lookups {
                match self.lookup_token(lookup)? {
                    Some(token) => self.reload(&token)?,
                    None => {
                        // Storage has not seen the mapping either: nothing to load.
                    }
                }
            }
        }
        Ok(())
    }

    fn lookup_token(&self, lookup: Lookup) -> Result<Option<String>, TrackerError> {
        Ok(match lookup {
            Lookup::Reload(token) => Some(token),
            Lookup::MatchId(id) => read::token_for_match_id(&self.reader, id)?,
            Lookup::GameId(id) => match read::match_id_for_game_id(&self.reader, id)? {
                Some(match_id) => read::token_for_match_id(&self.reader, match_id)?,
                None => None,
            },
        })
    }

    /// Replay a match's stored history into the projector, continuing its
    /// stored revision.
    fn reload(&mut self, token: &str) -> Result<(), TrackerError> {
        if let Some(revision) = read::match_revision(&self.reader, token)? {
            self.projector.seed_revision(token, revision);
        }
        let history = self.history.match_events(&self.reader, token)?;
        let changed = self.projector.restore(token, &history);
        self.handle_changed(changed, None);
        Ok(())
    }

    fn send_batch(&mut self) -> Result<(), TrackerError> {
        let now_ms = self.batch.now_ms;
        let mut batch = std::mem::take(&mut self.batch);
        self.batch.now_ms = now_ms;
        batch.matches = coalesce_matches(batch.matches);
        // Only a batch that carries events: the failure must lose real work.
        #[cfg(test)]
        if !batch.events.is_empty() && test_hooks::take_batch_failure(&self.config.db_path) {
            batch.events.push(test_hooks::broken_event());
        }
        self.writer.write(batch)?;
        Ok(())
    }

    fn push_status(&mut self, now_ms: i64) {
        let files: Vec<FileStatus> = self
            .files
            .iter()
            .map(|file| FileStatus {
                path: file.path.clone(),
                size: file.size,
                cursor: file.tailer.cursor(),
                last_live_growth_ms: file.tailer.last_live_growth_ms(),
                dropped_orphan_lines: file.tailer.dropped_orphan_lines(),
                bad_timezone: file.bad_timezone,
            })
            .collect();
        let growing = self
            .files
            .iter()
            .filter(|file| {
                file.tailer
                    .last_live_growth_ms()
                    .is_some_and(|at| now_ms - at <= TWO_CLIENTS_WINDOW_MS)
            })
            .count();
        let (arena_searched, arena_files) = self.arena.status();
        let status = Status {
            roots: self.roots.clone(),
            files,
            arena_searched,
            arena_files,
            two_clients: growing >= 2,
            awaiting_identity: self.projector.awaiting_identity(),
            quarantined: self.quarantined,
            failed_matches: self.failed.len() as u32,
            timezone_warning: self.timezone_warning.clone(),
            paused: !self.watching,
        };
        if self.last_status.as_ref() != Some(&status) {
            self.pending_events
                .push(TrackerEvent::Status(status.clone()));
            self.last_status = Some(status);
        }
    }
}

/// A match that league runs or tournaments are built from: a non-blank
/// league token or a tournament id.
fn in_league_or_tournament(view: &MatchView) -> bool {
    let league = view
        .league_token
        .as_deref()
        .is_some_and(|token| !token.trim().is_empty());
    league || view.tournament_event_id.is_some()
}

/// Keep only the last write per match: a catch-up changes a match many times in
/// one tick.
fn coalesce_matches(writes: Vec<MatchWrite>) -> Vec<MatchWrite> {
    let mut seen = HashSet::new();
    let mut kept: Vec<MatchWrite> = writes
        .into_iter()
        .rev()
        .filter(|write| seen.insert(write.token().to_string()))
        .collect();
    kept.reverse();
    kept
}

/// Keep only the last update per match in one tick (spec 3, coalesced pushes).
fn coalesce_events(events: Vec<TrackerEvent>) -> Vec<TrackerEvent> {
    let mut seen = HashSet::new();
    let mut kept: Vec<TrackerEvent> = events
        .into_iter()
        .rev()
        .filter(|event| match event {
            TrackerEvent::MatchUpdated { view, .. } => seen.insert(view.token.clone()),
            _ => true,
        })
        .collect();
    kept.reverse();
    kept
}

/// The path with symlinks resolved, or the path as given if that fails (the file
/// vanished, or is locked).
fn real_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The local calendar date of a UTC instant.
fn local_date(ms: i64, tz: Tz) -> NaiveDate {
    DateTime::from_timestamp_millis(ms)
        .map(|at| at.with_timezone(&tz).date_naive())
        .unwrap_or_default()
}

/// The local date a new file's first event belongs to (spec 5.4 rule 1, issue
/// #48): worked back from its last write, never its creation time. Only the
/// bytes the probe saw are read, the ones that last write covers. A file with
/// no events yet (or unreadable now) starts today.
fn new_file_day_anchor(path: &Path, probe: &identity::Probe, now_ms: i64, tz: Tz) -> NaiveDate {
    let times = identity::header_times(path, probe.size).unwrap_or_default();
    probe
        .modified_ms
        .and_then(|written| mtgo_protocol::day_anchor_from_last_write(&times, written, tz))
        .unwrap_or_else(|| local_date(now_ms, tz))
}

/// What tells a file's contents apart without reading it: its size and
/// modified time.
type FileStamp = (u64, Option<std::time::SystemTime>);

/// A file's stamp, `None` when it is gone or is not a file.
fn file_stamp(path: &Path) -> Option<FileStamp> {
    let meta = std::fs::metadata(path).ok()?;
    meta.is_file().then(|| (meta.len(), meta.modified().ok()))
}

/// Test-only failure injection, keyed by database path so parallel tests never
/// interfere.
#[cfg(test)]
pub(crate) mod test_hooks {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use mtgo_protocol::{EventKind, Subjects};
    use store::write::NewEvent;

    /// Panic when this database's engine ingests (or previews) this event key.
    pub static PANIC_AT: Mutex<Vec<(PathBuf, (i64, u64))>> = Mutex::new(Vec::new());
    /// Make this database's next batch that carries events fail, once.
    pub static FAIL_NEXT_BATCH: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    /// Hold the supervisor before it reopens this database's engine after a
    /// failure, until `release_reopen`.
    pub static HOLD_REOPEN: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    /// Make this database's final flush (while stopping) fail, once.
    pub static FAIL_FINAL_FLUSH: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    /// Make this database's next tick fail while the watcher is off, once.
    pub static FAIL_PAUSED_TICK: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

    pub fn take_paused_tick_failure(db: &Path) -> bool {
        let Ok(mut list) = FAIL_PAUSED_TICK.lock() else {
            return false;
        };
        match list.iter().position(|path| path == db) {
            Some(index) => {
                list.remove(index);
                true
            }
            None => false,
        }
    }

    pub fn take_final_flush_failure(db: &Path) -> bool {
        let Ok(mut list) = FAIL_FINAL_FLUSH.lock() else {
            return false;
        };
        match list.iter().position(|path| path == db) {
            Some(index) => {
                list.remove(index);
                true
            }
            None => false,
        }
    }

    /// Treat this log as vanished the next time a row is started for it.
    pub static VANISH_ON_START: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

    pub fn take_vanish(log: &Path) -> bool {
        let Ok(mut list) = VANISH_ON_START.lock() else {
            return false;
        };
        match list.iter().position(|path| path == log) {
            Some(index) => {
                list.remove(index);
                true
            }
            None => false,
        }
    }

    pub fn panic_if_poisoned(db: &Path, key: (i64, u64)) {
        let hit = PANIC_AT
            .lock()
            .map(|list| list.iter().any(|(path, at)| path == db && *at == key))
            .unwrap_or(false);
        if hit {
            panic!("injected ingest failure at {key:?}");
        }
    }

    pub fn take_batch_failure(db: &Path) -> bool {
        let Ok(mut list) = FAIL_NEXT_BATCH.lock() else {
            return false;
        };
        match list.iter().position(|path| path == db) {
            Some(index) => {
                list.remove(index);
                true
            }
            None => false,
        }
    }

    pub fn release_reopen(db: &Path) {
        if let Ok(mut list) = HOLD_REOPEN.lock() {
            list.retain(|path| path != db);
        }
    }

    /// Wait while this database's reopen is held (or until the tracker stops).
    pub fn wait_while_reopen_held(db: &Path, stop: &AtomicBool) {
        loop {
            let held = HOLD_REOPEN
                .lock()
                .map(|list| list.iter().any(|path| path == db))
                .unwrap_or(false);
            if !held || stop.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// An event for a file that does not exist: its foreign key fails the batch.
    pub fn broken_event() -> NewEvent {
        NewEvent {
            log_file_id: -1,
            byte_offset: 0,
            kind: Some(EventKind::Login),
            subjects: Subjects::default(),
            logged_at_ms: 0,
            raw: String::new(),
            quarantined: false,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::test_support::{config, open, run_until_settled};
    use super::*;
    use fixture_tools::synth::Synth;
    use fixture_tools::{Fixture, TestDir, load_fixture};
    use store::manual::{ManualGame, ManualMatch, ManualPlacement};

    #[test]
    fn a_rebuild_in_the_same_tick_as_a_login_caches_the_new_username() {
        let fixture = load_fixture("draft_hobbit");
        let dir = TestDir::new("engine-rebuild-login");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        // Hours later, so no finished match is rehydrated: the rebuild reads
        // their history itself.
        let now = fixture.meta.now_ms() + 3 * ABANDON_AFTER_MS;
        let mut first = open(&config, now);
        run_until_settled(&mut first, now);
        drop(first);
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute_batch(
                "UPDATE matches SET derived_version = 0;
                 UPDATE derivations SET version = 0;",
            )
            .expect("age the stored rows");

        let login_offset = fixture.bytes.len() as u64;
        dir.append(
            "logs/mtgo.log",
            b"23:59:58 [INF] (Login|MtGO Login Success) Username: Qnewname (8)\n23:59:59 [INF] (UI|after)\n",
        );

        let mut second = open(&config, now + 1_000);
        second.tick(now + 1_000).expect("tick");
        assert_eq!(
            second.history.cached_username(1, login_offset),
            Some(Some("Qnewname".to_string())),
            "the rebuild must see the Login ingested earlier in its tick"
        );
    }

    /// Every game card row, mulligan row and hand size, as text.
    fn game_rows(config: &TrackerConfig) -> Vec<String> {
        let conn = store::open_reader(&config.db_path).expect("reader");
        let mut rows = Vec::new();
        for sql in [
            "SELECT m.token || ':' || g.game_number || ':' || c.side || ':' || c.catalog_id || ':'
                 || c.main_qty || ',' || c.side_qty || ',' || c.opening_hand || ',' || c.bottomed
                 || ',' || c.first_draw || ',' || c.seen || ',' || c.`cast`
             FROM game_cards c JOIN games g ON g.id = c.game_id JOIN matches m ON m.id = g.match_id
             ORDER BY 1",
            "SELECT m.token || ':' || g.game_number || ':' || c.hand || ':' || c.catalog_id || ':'
                 || c.quantity
             FROM game_mulligan_cards c JOIN games g ON g.id = c.game_id
             JOIN matches m ON m.id = g.match_id ORDER BY 1",
            "SELECT m.token || ':' || g.game_number || ':' || coalesce(g.local_opening_hand_size, '-')
                 || ',' || coalesce(g.local_starting_hand_size, '-') || ','
                 || coalesce(g.opp_opening_hand_size, '-') || ','
                 || coalesce(g.opp_starting_hand_size, '-')
             FROM games g JOIN matches m ON m.id = g.match_id ORDER BY 1",
        ] {
            let mut stmt = conn.prepare(sql).expect(sql);
            let found = stmt.query_map([], |row| row.get::<_, String>(0)).expect(sql);
            for row in found {
                rows.push(row.expect("row"));
            }
        }
        rows
    }

    #[test]
    fn settled_games_get_their_card_rows_and_the_backfill_rebuilds_them() {
        let fixture = load_fixture("draft_hobbit");
        let dir = TestDir::new("engine-game-sync");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let now = fixture.meta.now_ms() + 3 * ABANDON_AFTER_MS;
        let mut first = open(&config, now);
        run_until_settled(&mut first, now);
        drop(first);
        assert!(count(&config, "SELECT COUNT(*) FROM game_cards") > 0);
        assert!(count(&config, "SELECT COUNT(*) FROM game_mulligan_cards") > 0);
        assert_eq!(
            value(
                &config,
                "SELECT version FROM derivations WHERE name = 'game_cards'"
            ),
            store::game_sync::GAME_CARDS_VERSION
        );
        let synced = game_rows(&config);

        // An older build's rows: the backfill syncs every log match again.
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute_batch(
                "DELETE FROM game_cards;
                 DELETE FROM game_mulligan_cards;
                 UPDATE games SET local_opening_hand_size = NULL, local_starting_hand_size = NULL,
                     opp_opening_hand_size = NULL, opp_starting_hand_size = NULL;
                 UPDATE derivations SET version = 0 WHERE name = 'game_cards';",
            )
            .expect("age the game rows");
        let mut second = open(&config, now + 1_000);
        assert!(second.game_sync.backfilling_now());
        for tick in 0..5 {
            second.tick(now + 1_000 + tick).expect("tick");
        }
        second.flush().expect("flush");
        drop(second);
        assert_eq!(
            game_rows(&config),
            synced,
            "the backfill equals the live syncs"
        );
        assert_eq!(
            value(
                &config,
                "SELECT version FROM derivations WHERE name = 'game_cards'"
            ),
            store::game_sync::GAME_CARDS_VERSION
        );
    }

    /// Each local card row's zone counters, by match, game and card.
    fn zone_rows(config: &TrackerConfig) -> Vec<String> {
        let conn = store::open_reader(&config.db_path).expect("reader");
        let mut stmt = conn
            .prepare(
                "SELECT m.token || ':' || g.game_number || ':' || c.catalog_id || ':'
                     || c.hand_seen || ',' || c.graveyard_seen || ',' || c.exile_seen || ','
                     || c.battlefield_seen || ',' || c.discarded || ',' || c.has_zone_data
                 FROM game_cards c JOIN games g ON g.id = c.game_id
                 JOIN matches m ON m.id = g.match_id WHERE c.side = 'local' ORDER BY 1",
            )
            .expect("zones");
        let found = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("zones");
        found.map(|row| row.expect("row")).collect()
    }

    #[test]
    fn the_version_bump_resyncs_every_log_match_once() {
        let fixture = load_fixture("draft_hobbit");
        let dir = TestDir::new("engine-game-sync-version");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let now = fixture.meta.now_ms() + 3 * ABANDON_AFTER_MS;
        let mut first = open(&config, now);
        run_until_settled(&mut first, now);
        drop(first);
        let zones = zone_rows(&config);
        assert!(
            zones.iter().any(|row| row.ends_with(",1")),
            "frames give zone data"
        );
        let tokens = store::game_sync::log_match_tokens(
            &store::open_reader(&config.db_path).expect("reader"),
        )
        .expect("tokens");

        // Rows of the build before zone counters (version 1).
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute_batch(
                "UPDATE game_cards SET hand_seen = 0, graveyard_seen = 0, exile_seen = 0,
                     battlefield_seen = 0, discarded = 0, has_zone_data = 0;
                 UPDATE derivations SET version = 1 WHERE name = 'game_cards';",
            )
            .expect("age the game rows");
        let mut second = open(&config, now + 1_000);
        assert!(second.game_sync.backfilling_now());
        assert_eq!(
            second.game_sync.backfill_queue(),
            tokens,
            "every log match, once"
        );
        for tick in 0..5 {
            second.tick(now + 1_000 + tick).expect("tick");
        }
        second.flush().expect("flush");
        drop(second);
        assert_eq!(zone_rows(&config), zones, "the resync fills the counters");
        assert_eq!(
            value(
                &config,
                "SELECT version FROM derivations WHERE name = 'game_cards'"
            ),
            store::game_sync::GAME_CARDS_VERSION
        );

        // Recorded: the next start syncs nothing again.
        let third = open(&config, now + 2_000);
        assert!(!third.game_sync.backfilling_now());
    }

    #[test]
    fn a_large_game_sync_queue_goes_in_slices_one_at_a_time() {
        let fixture = load_fixture("draft_hobbit");
        let dir = TestDir::new("engine-game-sync-slices");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let now = fixture.meta.now_ms() + 3 * ABANDON_AFTER_MS;
        let mut engine = open(&config, now);
        run_until_settled(&mut engine, now);
        // A rebuild or a backlog queues many matches at once.
        let tokens: Vec<String> = (0..25).map(|n| format!("q-queued-{n}")).collect();
        engine.game_sync.queue_for_test(tokens);
        let mut at = now + 10_000;
        let mut left = Vec::new();
        loop {
            let tick = engine.tick(at).expect("tick");
            assert!(engine.game_sync.in_flight() <= 1, "one slice in flight");
            left.push(engine.game_sync.queued());
            if !tick.syncing {
                break;
            }
            engine.flush().expect("the slice is answered");
            at += 1;
        }
        assert_eq!(left[..3], [15, 5, 0], "slices of {GAME_SYNCS_PER_TICK}");
    }

    /// A live game sync pushes again only the matches whose rows it changed
    /// (#207): the others in its slice have nothing new to show.
    #[test]
    fn a_game_sync_pushes_again_only_the_matches_it_changed() {
        let fixture = load_fixture("draft_hobbit");
        let dir = TestDir::new("engine-game-sync-push-changed");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let now = fixture.meta.now_ms();
        let mut engine = open(&config, now);
        run_until_settled(&mut engine, now);
        let mut tokens: Vec<String> = engine
            .projector
            .views()
            .iter()
            .map(|view| view.token.clone())
            .filter(|token| engine.visible.contains(token))
            .collect();
        tokens.sort();
        assert!(tokens.len() >= 2, "two live visible matches: {tokens:?}");
        let (stale, kept) = (tokens[0].clone(), tokens[1].clone());
        // Only one match's rows are out of date: only its sync writes rows.
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute(
                "UPDATE game_cards SET seen = 99
                 WHERE game_id IN (SELECT g.id FROM games g JOIN matches m ON m.id = g.match_id
                                   WHERE m.token = ?1)",
                [&stale],
            )
            .expect("stale rows");
        engine
            .game_sync
            .queue_for_test(vec![stale.clone(), kept.clone()]);
        let events = settle_syncs(&mut engine, now + 10_000);
        let pushed: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                TrackerEvent::MatchUpdated { view, .. } => Some(view.token.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(pushed, vec![stale.as_str()], "{kept} changed no row");
    }

    #[test]
    fn a_backfill_looks_for_each_game_log_once_with_one_walk() {
        let fixture = load_fixture("draft_hobbit");
        let dir = TestDir::new("engine-backfill-one-walk");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let now = fixture.meta.now_ms() + 3 * ABANDON_AFTER_MS;
        let mut first = open(&config, now);
        run_until_settled(&mut first, now);
        first.flush().expect("flush");
        drop(first);
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute(
                "UPDATE derivations SET version = 0 WHERE name = 'game_cards'",
                [],
            )
            .expect("an older build's rows");

        let later = now + 20 * GAME_LOG_RETRY_MS;
        let mut second = open(&config, later);
        assert!(second.game_sync.backfilling_now());
        run_until_settled(&mut second, later);
        assert!(!second.game_sync.backfilling_now());
        assert_eq!(
            second.game_sync.walks(),
            1,
            "one walk for the backfill pass"
        );
        assert_eq!(
            second.game_sync.dat_lookups(),
            0,
            "an old match's file does not appear later: one look"
        );
        for minute in 1..=3 {
            second
                .tick(later + minute * GAME_LOG_RETRY_MS)
                .expect("tick");
        }
        assert_eq!(second.game_sync.walks(), 1);
    }

    #[test]
    fn a_backfill_with_an_unreadable_archive_syncs_the_rest_and_records_the_version() {
        let fixture = load_fixture("draft_hobbit");
        let dir = TestDir::new("engine-backfill-corrupt");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let now = fixture.meta.now_ms() + 3 * ABANDON_AFTER_MS;
        let mut first = open(&config, now);
        run_until_settled(&mut first, now);
        drop(first);
        let synced = game_rows(&config);
        let broken: String = store::open_reader(&config.db_path)
            .expect("reader")
            .query_row(
                "SELECT token FROM matches WHERE source = 'log' ORDER BY id LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("a log match");
        let conn = store::open_writer(&config.db_path).expect("writer");
        conn.execute_batch(
            "DELETE FROM game_cards;
             DELETE FROM game_mulligan_cards;
             UPDATE games SET local_opening_hand_size = NULL, local_starting_hand_size = NULL,
                 opp_opening_hand_size = NULL, opp_starting_hand_size = NULL;
             UPDATE derivations SET version = 0 WHERE name = 'game_cards';",
        )
        .expect("age the game rows");
        conn.execute(
            "INSERT OR REPLACE INTO match_event_archives (match_token, raw, event_count, compacted_at)
             VALUES (?1, x'00', 1, 1)",
            [&broken],
        )
        .expect("an unreadable archive");
        drop(conn);

        // Past the relink window: the deck linker reads archives too, and
        // fails its tick on an unreadable one (#206).
        let later = now + RELINK_WITHIN_MS + 24 * 60 * 60 * 1000;
        let mut second = open(&config, later);
        run_until_settled(&mut second, later);
        drop(second);
        let others: Vec<String> = synced
            .iter()
            .filter(|row| !row.starts_with(&broken))
            .cloned()
            .collect();
        let after: Vec<String> = game_rows(&config)
            .into_iter()
            .filter(|row| !row.starts_with(&broken))
            .collect();
        assert_eq!(after, others, "every other match is synced");
        assert_eq!(
            value(
                &config,
                "SELECT version FROM derivations WHERE name = 'game_cards'"
            ),
            store::game_sync::GAME_CARDS_VERSION,
            "recorded: the skipped match is synced again on its next change"
        );
    }

    /// A minute after the fixture's last match activity: its matches were
    /// played just now.
    fn just_after_the_fixture(fixture: &Fixture, label: &str) -> i64 {
        let dir = TestDir::new(label);
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, fixture);
        let now = fixture.meta.now_ms();
        let mut engine = open(&config, now);
        run_until_settled(&mut engine, now);
        drop(engine);
        value(&config, "SELECT MAX(last_activity_at) FROM matches") + 60_000
    }

    #[test]
    fn a_complete_match_looks_for_its_game_log_file_until_found() {
        let fixture = load_fixture("draft_hobbit");
        let now = just_after_the_fixture(&fixture, "engine-game-log-file-time");
        let dir = TestDir::new("engine-game-log-file");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let mut engine = open(&config, now);
        run_until_settled(&mut engine, now);
        let token: String = store::open_reader(&config.db_path)
            .expect("reader")
            .query_row(
                "SELECT token FROM matches WHERE state = 'complete' ORDER BY ended_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("a complete match");
        assert!(engine.game_sync.looks_for_dat(&token), "missed once so far");
        assert_eq!(count(&config, "SELECT COUNT(*) FROM game_logs"), 0);

        let bytes = fixture_tools::load_game_log("clean_2_0_win");
        dir.write(&format!("mtgo-data/Match_GameLog_{token}.dat"), &bytes);
        // Not due yet.
        engine
            .tick(now + 5_000 + GAME_LOG_RETRY_MS / 2)
            .expect("tick");
        engine.flush().expect("flush");
        assert_eq!(count(&config, "SELECT COUNT(*) FROM game_logs"), 0);
        engine.tick(now + 5_000 + GAME_LOG_RETRY_MS).expect("tick");
        engine.flush().expect("flush");
        assert!(
            engine.game_sync.looks_for_dat(&token),
            "watched once more: it may still be being written"
        );
        engine
            .tick(now + 5_000 + 2 * GAME_LOG_RETRY_MS)
            .expect("tick");
        assert!(!engine.game_sync.looks_for_dat(&token), "unchanged: done");
        assert_eq!(
            count(
                &config,
                &format!(
                    "SELECT COUNT(*) FROM game_logs l JOIN matches m ON m.id = l.match_id
                     WHERE m.token = '{token}'"
                )
            ),
            1,
            "decoded once into game_logs"
        );
    }

    /// The two (fake) players of the `clean_2_0_win.dat` fixture.
    const DAT_PLAYERS: [&str; 2] = ["Qaxxxxxx", "Qbxxxxxxxx"];

    /// `token`'s local account and opponent take the `.dat` fixture's
    /// players' names, so its game log matches the stored match.
    fn rename_players_to_the_dat_fixture(config: &TrackerConfig, token: &str) {
        let conn = store::open_writer(&config.db_path).expect("writer");
        conn.execute(
            "UPDATE accounts SET username = ?2
             WHERE id = (SELECT account_id FROM matches WHERE token = ?1)",
            [token, DAT_PLAYERS[0]],
        )
        .expect("local name");
        conn.execute(
            "UPDATE opponents SET username = ?2
             WHERE id = (SELECT opponent_id FROM matches WHERE token = ?1)",
            [token, DAT_PLAYERS[1]],
        )
        .expect("opponent name");
    }

    #[test]
    fn a_backfilled_match_with_a_dat_file_gets_its_game_log_and_log_only_reveals() {
        let fixture = load_fixture("draft_hobbit");
        let dir = TestDir::new("engine-backfill-dat");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        // Days later: catching up looks for every complete match's file, and
        // none is on disk yet.
        let now = fixture.meta.now_ms() + 3 * ABANDON_AFTER_MS;
        let mut first = open(&config, now);
        run_until_settled(&mut first, now);
        for tick in 1..=GAME_LOG_TRIES as i64 {
            first.tick(now + tick * GAME_LOG_RETRY_MS).expect("tick");
        }
        first.flush().expect("flush");
        drop(first);
        let token: String = store::open_reader(&config.db_path)
            .expect("reader")
            .query_row(
                "SELECT token FROM matches WHERE state = 'complete' ORDER BY ended_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("a complete match");
        rename_players_to_the_dat_fixture(&config, &token);
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute_batch(
                "DELETE FROM game_cards;
                 DELETE FROM game_mulligan_cards;
                 DELETE FROM game_logs;
                 UPDATE derivations SET version = 0 WHERE name = 'game_cards';",
            )
            .expect("an older build's rows");
        dir.write(
            &format!("mtgo-data/Q/Match_GameLog_{token}.dat"),
            &fixture_tools::load_game_log("clean_2_0_win"),
        );

        let later = now + 20 * GAME_LOG_RETRY_MS;
        let mut second = open(&config, later);
        run_until_settled(&mut second, later);
        drop(second);
        let logged = count(
            &config,
            &format!(
                "SELECT COUNT(*) FROM game_logs l JOIN matches m ON m.id = l.match_id
                 WHERE m.token = '{token}'"
            ),
        );
        assert_eq!(logged, 1, "the backfill found and decoded the file");
        let revealed = count(
            &config,
            &format!(
                "SELECT COUNT(*) FROM game_cards c JOIN games g ON g.id = c.game_id
                 JOIN matches m ON m.id = g.match_id
                 WHERE m.token = '{token}' AND c.side = 'opponent' AND c.seen > 0"
            ),
        );
        assert!(revealed > 0, "the log-only reveals");
    }

    #[test]
    fn a_game_log_file_still_growing_is_read_again() {
        let fixture = load_fixture("draft_hobbit");
        let now = just_after_the_fixture(&fixture, "engine-game-log-growing-time");
        let dir = TestDir::new("engine-game-log-growing");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let mut engine = open(&config, now);
        run_until_settled(&mut engine, now);
        let token: String = store::open_reader(&config.db_path)
            .expect("reader")
            .query_row(
                "SELECT token FROM matches WHERE state = 'complete' ORDER BY ended_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("a complete match");
        let bytes = fixture_tools::load_game_log("clean_2_0_win");
        let half = &bytes[..bytes.len() / 2];
        let file = format!("mtgo-data/Match_GameLog_{token}.dat");
        dir.write(&file, half);
        let mut at = now + 5_000 + GAME_LOG_RETRY_MS;
        engine.tick(at).expect("tick");
        engine.flush().expect("flush");
        let entries = |config: &TrackerConfig| {
            value(
                config,
                &format!(
                    "SELECT COALESCE(MAX(l.entry_count), 0) FROM game_logs l
                     JOIN matches m ON m.id = l.match_id WHERE m.token = '{token}'"
                ),
            )
        };
        let partial = entries(&config);
        assert!(partial > 0, "the half-written file was read");

        // MTGO finishes writing it: the next look reads it again.
        dir.write(&file, &bytes);
        at += GAME_LOG_RETRY_MS;
        engine.tick(at).expect("tick");
        engine.flush().expect("flush");
        assert!(
            entries(&config) > partial,
            "the whole file replaced the part"
        );
        at += GAME_LOG_RETRY_MS;
        engine.tick(at).expect("tick");
        assert!(
            !engine.game_sync.looks_for_dat(&token),
            "unchanged since: done"
        );
    }

    #[test]
    fn a_match_whose_game_log_never_appears_stops_being_looked_for() {
        let fixture = load_fixture("draft_hobbit");
        let now = just_after_the_fixture(&fixture, "engine-game-log-missing-time");
        let dir = TestDir::new("engine-game-log-missing");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let mut engine = open(&config, now);
        run_until_settled(&mut engine, now);
        let token: String = store::open_reader(&config.db_path)
            .expect("reader")
            .query_row(
                "SELECT token FROM matches WHERE state = 'complete' ORDER BY ended_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("a complete match");
        assert!(engine.game_sync.looks_for_dat(&token));
        let mut at = now + 5_000;
        for _ in 0..GAME_LOG_TRIES {
            at += GAME_LOG_RETRY_MS;
            engine.tick(at).expect("tick");
        }
        assert!(!engine.game_sync.looks_for_dat(&token));
    }

    #[test]
    fn a_file_that_vanishes_during_rotation_does_not_skip_the_next_file() {
        let dir = TestDir::new("engine-rotate-vanish");
        dir.write(
            "logs/a/mtgo.log",
            b"12:00:00 [INF] (UI|a)\n12:00:01 [INF] (UI|a2)\n",
        );
        let b_first = b"12:00:00 [INF] (UI|b)\n";
        dir.write("logs/b/mtgo.log", b_first);
        let config = TrackerConfig {
            db_path: dir.join("data/mymtgo.sqlite"),
            timezone: chrono_tz::UTC,
            os_timezone: None,
            log_overrides: vec![dir.join("logs")],
            arena_log_overrides: vec![dir.join("arena/Player.log")],
            mtgo_data_folder: Some(dir.join("mtgo-data")),
            replay_day_anchor: None,
            watching: true,
            arena_enabled: true,
            endstep_enabled: false,
        };
        let start = 1_767_000_000_000;
        let mut engine = open(&config, start);
        engine.tick(start).expect("catch-up tick");
        assert_eq!(engine.files.len(), 2);

        // `a` shrinks (a rotation) and is gone again by the time its new row is
        // started; `b` grows in the same tick.
        let a = dir.join("logs/a/mtgo.log");
        dir.write("logs/a/mtgo.log", b"x\n");
        test_hooks::VANISH_ON_START
            .lock()
            .expect("hook")
            .push(a.clone());
        dir.append("logs/b/mtgo.log", b"12:00:01 [INF] (UI|b2)\n");

        engine.tick(start + 100).expect("tick");
        assert_eq!(engine.files.len(), 1);
        assert_eq!(engine.files[0].path, dir.join("logs/b/mtgo.log"));
        assert_eq!(
            engine.files[0].tailer.cursor(),
            b_first.len() as u64,
            "b is read in the tick that dropped a"
        );
    }

    #[test]
    fn a_match_that_fails_before_it_has_a_row_is_counted_and_retried_by_the_next_version() {
        let fixture = load_fixture("draft_hobbit");
        let dir = TestDir::new("engine-failed-first-derive");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        // Hours later, so no finished match is rehydrated on open.
        let now = fixture.meta.now_ms() + 3 * ABANDON_AFTER_MS;
        let mut first = open(&config, now);
        run_until_settled(&mut first, now);
        drop(first);
        let token: String = store::open_reader(&config.db_path)
            .expect("reader")
            .query_row("SELECT token FROM matches ORDER BY id LIMIT 1", [], |row| {
                row.get(0)
            })
            .expect("a stored match");
        // The match failed on its first derive, so it never had a row.
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute("DELETE FROM matches WHERE token = ?1", [&token])
            .expect("forget the row");

        let mut second = open(&config, now + 1_000);
        second.record_derived(
            token.clone(),
            Some(Derived::Failed("projection panicked: Q boom".to_string())),
            1,
            None,
        );
        run_until_settled(&mut second, now + 1_000);
        assert_eq!(
            count(&config, "SELECT COUNT(*) FROM match_failures"),
            1,
            "the failure is stored although the match has no row"
        );
        let failed = |engine: &Engine| engine.last_status.as_ref().map(|s| s.failed_matches);
        assert_eq!(failed(&second), Some(1));
        drop(second);

        let mut third = open(&config, now + 2_000);
        third.tick(now + 2_000).expect("tick");
        assert_eq!(failed(&third), Some(1), "the count survives a restart");
        drop(third);

        store::open_writer(&config.db_path)
            .expect("writer")
            .execute_batch(
                "UPDATE match_failures SET derived_version = 0;
                 UPDATE derivations SET version = 0;",
            )
            .expect("a newer projection version");
        let mut fourth = open(&config, now + 3_000);
        run_until_settled(&mut fourth, now + 3_000);
        assert_eq!(
            count(
                &config,
                &format!("SELECT COUNT(*) FROM matches WHERE token = '{token}'")
            ),
            1,
            "the rebuild derived the match again"
        );
        assert_eq!(count(&config, "SELECT COUNT(*) FROM match_failures"), 0);
        assert_eq!(failed(&fourth), Some(0));
    }

    #[test]
    fn opening_does_not_recheck_a_match_that_already_failed() {
        const TOKEN: &str = "f5e33a1f-c2e7-4678-b30d-309b63f17a42";
        let dir = TestDir::new("engine-recheck-skips-failed");
        let mut synth = Synth::new(None);
        let mut log = String::new();
        for event in [
            synth.joined(TOKEN, 99, 7, &[]),
            synth.game_state(99, 7, &["Qlocal", "Qopponent"]),
        ] {
            log.push_str(event.text.trim_end());
            log.push('\n');
        }
        dir.write("logs/mtgo.log", log.as_bytes());
        let day = NaiveDate::from_ymd_opt(2026, 7, 1).expect("date");
        let config = TrackerConfig {
            db_path: dir.join("data/mymtgo.sqlite"),
            timezone: chrono_tz::UTC,
            os_timezone: None,
            log_overrides: vec![dir.join("logs")],
            arena_log_overrides: vec![dir.join("arena/Player.log")],
            mtgo_data_folder: Some(dir.join("mtgo-data")),
            replay_day_anchor: Some(day),
            watching: true,
            arena_enabled: true,
            endstep_enabled: false,
        };
        let now = day
            .and_hms_opt(12, 1, 0)
            .expect("time")
            .and_utc()
            .timestamp_millis();
        let mut first = open(&config, now);
        run_until_settled(&mut first, now);
        // The match failed before it had a row (the account is known now, so a
        // recheck would identify it).
        first.record_derived(
            TOKEN.to_string(),
            Some(Derived::Failed("projection panicked: Q boom".to_string())),
            1,
            None,
        );
        run_until_settled(&mut first, now);
        drop(first);
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute(
                "INSERT INTO accounts (username, login_id, first_seen_at) VALUES ('Qlocal', 8, ?1)",
                [now],
            )
            .expect("store the account");
        assert_eq!(count(&config, "SELECT COUNT(*) FROM match_failures"), 1);

        // A failed match is retried by the next projection version, not by every
        // start: rechecking it would repeat its failure report on each restart.
        let mut second = open(&config, now + 60_000);
        run_until_settled(&mut second, now + 60_000);
        assert_eq!(
            count(&config, "SELECT COUNT(*) FROM match_failures"),
            1,
            "the failed match was not derived again on open"
        );
        assert_eq!(count(&config, "SELECT COUNT(*) FROM matches"), 0);
    }

    /// A `COUNT(*)` query's result.
    fn count(config: &TrackerConfig, sql: &str) -> i64 {
        value(config, sql)
    }

    /// The first column of the query's first row, as an integer.
    fn value(config: &TrackerConfig, sql: &str) -> i64 {
        store::open_reader(&config.db_path)
            .expect("reader")
            .query_row(sql, [], |row| row.get(0))
            .expect(sql)
    }

    fn london() -> (Tz, Option<String>) {
        (chrono_tz::Europe::London, None)
    }

    fn unreadable() -> (Tz, Option<String>) {
        (chrono_tz::UTC, Some("Q timezone warning".to_string()))
    }

    static USE_LONDON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// UTC until `USE_LONDON` is set. Only `a_timezone_change_dates_the_next_new_file` uses it.
    fn switchable() -> (Tz, Option<String>) {
        if USE_LONDON.load(std::sync::atomic::Ordering::SeqCst) {
            london()
        } else {
            (chrono_tz::UTC, None)
        }
    }

    fn zones(config: &TrackerConfig) -> Vec<String> {
        let conn = store::open_reader(&config.db_path).expect("reader");
        let mut stmt = conn
            .prepare("SELECT timezone FROM log_files ORDER BY id")
            .expect("prepare");
        stmt.query_map([], |row| row.get(0))
            .expect("query")
            .map(|zone| zone.expect("zone"))
            .collect()
    }

    #[test]
    fn a_log_caught_up_later_is_dated_from_its_last_write_not_its_creation() {
        let dir = TestDir::new("engine-anchor-last-write");
        let path = dir.write(
            "logs/mtgo.log",
            b"16:17:47 [INF] (Login|MtGO Login Success) Username: Qzone (8)\n16:18:50 [INF] (UI|after)\n",
        );
        // MTGO rewrote the file on a day well after its creation (as on Windows,
        // #48). The date is in the future so the creation time is earlier on every
        // platform: macOS pulls the creation time back to an earlier last write.
        let day = NaiveDate::from_ymd_opt(2099, 6, 15).expect("date");
        let written = day.and_hms_opt(16, 18, 57).expect("time").and_utc();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .expect("open")
            .set_modified(written.into())
            .expect("set the last write");
        let config = TrackerConfig {
            db_path: dir.join("data/mymtgo.sqlite"),
            timezone: chrono_tz::UTC,
            os_timezone: None,
            log_overrides: vec![dir.join("logs")],
            arena_log_overrides: vec![dir.join("arena/Player.log")],
            mtgo_data_folder: Some(dir.join("mtgo-data")),
            replay_day_anchor: None,
            watching: true,
            arena_enabled: true,
            endstep_enabled: false,
        };
        // Caught up five days later, so dating from `now` (where a platform
        // has no creation time) would give the wrong day.
        let now = written.timestamp_millis() + 5 * 24 * 60 * 60 * 1000;
        let mut engine = open(&config, now);
        run_until_settled(&mut engine, now);
        let expected = day
            .and_hms_opt(16, 17, 47)
            .expect("time")
            .and_utc()
            .timestamp_millis();
        assert_eq!(
            value(
                &config,
                "SELECT logged_at FROM log_events WHERE kind = 'login'"
            ),
            expected
        );
    }

    #[test]
    fn a_new_log_file_records_the_os_timezone() {
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("engine-os-timezone");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let mut config = config(&dir, &fixture);
        config.timezone = chrono_tz::UTC;
        config.os_timezone = Some(london);
        let now = fixture.meta.now_ms();
        let mut engine = open(&config, now);
        run_until_settled(&mut engine, now);
        assert_eq!(zones(&config), vec!["Europe/London".to_string()]);
    }

    #[test]
    fn a_timezone_change_dates_the_next_new_file() {
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("engine-os-timezone-change");
        dir.write("logs/first/mtgo.log", &fixture.bytes);
        let mut config = config(&dir, &fixture);
        config.os_timezone = Some(switchable);
        let now = fixture.meta.now_ms();
        let mut engine = open(&config, now);
        run_until_settled(&mut engine, now);

        USE_LONDON.store(true, std::sync::atomic::Ordering::SeqCst);
        dir.write("logs/second/mtgo.log", &fixture.bytes);
        run_until_settled(&mut engine, now + 60_000);
        assert_eq!(
            zones(&config),
            vec!["UTC".to_string(), "Europe/London".to_string()],
            "the first file keeps its zone; the new one gets the zone read when it appeared"
        );
    }

    #[test]
    fn the_os_timezone_warning_reaches_the_status() {
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("engine-os-timezone-warning");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let mut config = config(&dir, &fixture);
        config.os_timezone = Some(unreadable);
        let now = fixture.meta.now_ms();
        let mut engine = open(&config, now);
        run_until_settled(&mut engine, now);
        let status = engine.last_status.clone().expect("status");
        assert_eq!(
            status.timezone_warning.as_deref(),
            Some("Q timezone warning")
        );
    }

    fn tracked_paths(engine: &Engine) -> Vec<PathBuf> {
        engine.files.iter().map(|file| file.path.clone()).collect()
    }

    #[test]
    fn choosing_another_folder_stops_polling_the_old_folders_log() {
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("engine-park-switch");
        dir.write("logs-a/mtgo.log", &fixture.bytes);
        let second_log = dir.write("logs-b/mtgo.log", &fixture.bytes);
        let mut config = config(&dir, &fixture);
        config.log_overrides = vec![dir.join("logs-a")];
        let now = fixture.meta.now_ms();
        let mut first = open(&config, now);
        run_until_settled(&mut first, now);
        drop(first);

        // The app restarts tracking with the new folder (Plan C, apply_settings).
        config.log_overrides = vec![dir.join("logs-b")];
        let mut second = open(&config, now + 10_000);
        run_until_settled(&mut second, now + 10_000);
        assert_eq!(tracked_paths(&second), vec![second_log]);
        let status = second.last_status.clone().expect("a status was pushed");
        assert_eq!(status.files.len(), 1, "the status bar says Watching 1 log");
    }

    #[test]
    fn parking_a_file_clears_its_provisional_preview() {
        const TOKEN: &str = "f5e33a1f-c2e7-4678-b30d-309b63f17a60";
        let dir = TestDir::new("engine-park-preview");
        let mut synth = Synth::new(None);
        let mut log =
            String::from("12:00:00 [INF] (Login|MtGO Login Success) Username: Qlocal (8)\n");
        for event in [
            synth.joined(TOKEN, 99, 7, &[]),
            synth.game_state(99, 7, &["Qlocal", "Qopponent"]),
        ] {
            log.push_str(event.text.trim_end());
            log.push('\n');
        }
        dir.write("logs/mtgo.log", log.as_bytes());
        let day = NaiveDate::from_ymd_opt(2026, 7, 1).expect("date");
        let config = TrackerConfig {
            db_path: dir.join("data/mymtgo.sqlite"),
            timezone: chrono_tz::UTC,
            os_timezone: None,
            log_overrides: vec![dir.join("logs")],
            arena_log_overrides: vec![dir.join("arena/Player.log")],
            mtgo_data_folder: Some(dir.join("mtgo-data")),
            replay_day_anchor: Some(day),
            watching: true,
            arena_enabled: true,
            endstep_enabled: false,
        };
        let now = day
            .and_hms_opt(12, 1, 0)
            .expect("time")
            .and_utc()
            .timestamp_millis();
        let mut engine = open(&config, now);
        let tick = engine.tick(now).expect("tick");
        assert!(
            tick.events
                .iter()
                .any(|event| matches!(event, TrackerEvent::MatchPreview { .. })),
            "the last event is previewed: {:?}",
            tick.events
        );

        std::fs::rename(dir.join("logs"), dir.join("moved")).expect("hide the folder");
        let mut events = Vec::new();
        for step in 0..3 {
            events.extend(engine.tick(now + 60_000 + step).expect("tick").events);
        }
        assert!(tracked_paths(&engine).is_empty(), "parked");
        assert!(
            events.iter().any(|event| matches!(
                event,
                TrackerEvent::MatchUpdated { .. } | TrackerEvent::MatchRemoved { .. }
            )),
            "the preview is superseded: {events:?}"
        );
    }

    /// A row whose file is gone (an old ClickOnce folder, #86) is parked at
    /// open, not polled and parked again at the first rescan; the file
    /// coming back resumes its row.
    #[test]
    fn a_row_whose_file_is_gone_is_parked_at_open() {
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("engine-resume-gone");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let now = fixture.meta.now_ms();
        let mut engine = open(&config, now);
        run_until_settled(&mut engine, now);
        drop(engine);
        let events = count(&config, "SELECT COUNT(*) FROM log_events");

        std::fs::rename(dir.join("logs"), dir.join("moved")).expect("hide the folder");
        let mut engine = open(&config, now + 60_000);
        assert!(tracked_paths(&engine).is_empty(), "not resumed for polling");
        assert_eq!(engine.parked.len(), 1, "parked at open");

        std::fs::rename(dir.join("moved"), dir.join("logs")).expect("restore the folder");
        run_until_settled(&mut engine, now + 120_000);
        assert_eq!(tracked_paths(&engine).len(), 1);
        assert!(engine.parked.is_empty());
        assert_eq!(count(&config, "SELECT COUNT(*) FROM log_files"), 1);
        assert_eq!(count(&config, "SELECT COUNT(*) FROM log_events"), events);
    }

    #[test]
    fn a_log_found_again_resumes_its_row_and_is_never_read_twice() {
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("engine-park-resume");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let now = fixture.meta.now_ms();
        let mut engine = open(&config, now);
        run_until_settled(&mut engine, now);
        let events = count(&config, "SELECT COUNT(*) FROM log_events");

        std::fs::rename(dir.join("logs"), dir.join("moved")).expect("hide the folder");
        engine.tick(now + 60_000).expect("tick");
        assert!(
            tracked_paths(&engine).is_empty(),
            "a log detection no longer finds is not polled"
        );

        std::fs::rename(dir.join("moved"), dir.join("logs")).expect("restore the folder");
        run_until_settled(&mut engine, now + 120_000);
        assert_eq!(tracked_paths(&engine).len(), 1);
        assert_eq!(
            count(&config, "SELECT COUNT(*) FROM log_files"),
            1,
            "the stored row resumed; no second row for the same file"
        );
        assert_eq!(
            count(&config, "SELECT COUNT(*) FROM log_events"),
            events,
            "no event was stored twice"
        );
    }

    #[test]
    fn a_login_after_a_restart_identifies_a_match_that_was_waiting() {
        const TOKEN: &str = "f5e33a1f-c2e7-4678-b30d-309b63f17a40";
        let dir = TestDir::new("engine-login-after-restart");
        // A match played before MTGO logged in: nobody is known to be local.
        let mut synth = Synth::new(None);
        let mut log = String::new();
        for event in [
            synth.joined(TOKEN, 99, 7, &[]),
            synth.game_state(99, 7, &["Qlocal", "Qopponent"]),
        ] {
            log.push_str(event.text.trim_end());
            log.push('\n');
        }
        dir.write("logs/mtgo.log", log.as_bytes());
        let day = NaiveDate::from_ymd_opt(2026, 7, 1).expect("date");
        let config = TrackerConfig {
            db_path: dir.join("data/mymtgo.sqlite"),
            timezone: chrono_tz::UTC,
            os_timezone: None,
            log_overrides: vec![dir.join("logs")],
            arena_log_overrides: vec![dir.join("arena/Player.log")],
            mtgo_data_folder: Some(dir.join("mtgo-data")),
            replay_day_anchor: Some(day),
            watching: true,
            arena_enabled: true,
            endstep_enabled: false,
        };
        let now = day
            .and_hms_opt(12, 1, 0)
            .expect("time")
            .and_utc()
            .timestamp_millis();
        let mut first = open(&config, now);
        run_until_settled(&mut first, now);
        let waiting = |engine: &Engine| {
            engine
                .last_status
                .as_ref()
                .map(|status| status.awaiting_identity.clone())
        };
        assert_eq!(waiting(&first), Some(vec![TOKEN.to_string()]));
        drop(first);

        let mut second = open(&config, now + 60_000);
        run_until_settled(&mut second, now + 60_000);
        dir.append(
            "logs/mtgo.log",
            b"12:02:00 [INF] (Login|MtGO Login Success) Username: Qlocal (8)\n12:02:01 [INF] (UI|after)\n",
        );
        run_until_settled(&mut second, now + 120_000);
        assert_eq!(
            count(&config, "SELECT COUNT(*) FROM matches"),
            1,
            "the Login identified the match that waited before the restart"
        );
        assert_eq!(waiting(&second), Some(Vec::new()));
    }

    #[test]
    fn a_match_still_waiting_when_the_login_was_stored_is_identified_on_open() {
        const TOKEN: &str = "f5e33a1f-c2e7-4678-b30d-309b63f17a41";
        let dir = TestDir::new("engine-recheck-on-open");
        // A match played before MTGO logged in: nobody is known to be local.
        let mut synth = Synth::new(None);
        let mut log = String::new();
        for event in [
            synth.joined(TOKEN, 99, 7, &[]),
            synth.game_state(99, 7, &["Qlocal", "Qopponent"]),
        ] {
            log.push_str(event.text.trim_end());
            log.push('\n');
        }
        dir.write("logs/mtgo.log", log.as_bytes());
        let day = NaiveDate::from_ymd_opt(2026, 7, 1).expect("date");
        let config = TrackerConfig {
            db_path: dir.join("data/mymtgo.sqlite"),
            timezone: chrono_tz::UTC,
            os_timezone: None,
            log_overrides: vec![dir.join("logs")],
            arena_log_overrides: vec![dir.join("arena/Player.log")],
            mtgo_data_folder: Some(dir.join("mtgo-data")),
            replay_day_anchor: Some(day),
            watching: true,
            arena_enabled: true,
            endstep_enabled: false,
        };
        let now = day
            .and_hms_opt(12, 1, 0)
            .expect("time")
            .and_utc()
            .timestamp_millis();
        let mut first = open(&config, now);
        run_until_settled(&mut first, now);
        drop(first);
        assert_eq!(count(&config, "SELECT COUNT(*) FROM matches"), 0);

        // The process stopped after the Login's account was stored but before
        // the waiting match was reloaded: the account is in storage, the match
        // still has no row and no failure.
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute(
                "INSERT INTO accounts (username, login_id, first_seen_at) VALUES ('Qlocal', 8, ?1)",
                [now],
            )
            .expect("store the account");

        let mut second = open(&config, now + 60_000);
        run_until_settled(&mut second, now + 60_000);
        assert_eq!(
            count(&config, "SELECT COUNT(*) FROM matches"),
            1,
            "opening rechecked the match that waited for the stored account"
        );
    }

    #[test]
    fn a_reloaded_phantom_writes_no_row_and_removes_nothing() {
        const TOKEN: &str = "f5e33a1f-c2e7-4678-b30d-309b63f17a42";
        let dir = TestDir::new("engine-reloaded-phantom");
        // Joined before MTGO logged in, with the local player alone in the
        // game state: once the Login names them, it derives as a Phantom (a
        // joined token that never becomes a match).
        let mut synth = Synth::new(None);
        let mut log = String::new();
        for event in [
            synth.joined(TOKEN, 99, 7, &[]),
            synth.game_state(99, 7, &["Qlocal"]),
        ] {
            log.push_str(event.text.trim_end());
            log.push('\n');
        }
        dir.write("logs/mtgo.log", log.as_bytes());
        let day = NaiveDate::from_ymd_opt(2026, 7, 1).expect("date");
        let config = TrackerConfig {
            db_path: dir.join("data/mymtgo.sqlite"),
            timezone: chrono_tz::UTC,
            os_timezone: None,
            log_overrides: vec![dir.join("logs")],
            arena_log_overrides: vec![dir.join("arena/Player.log")],
            mtgo_data_folder: Some(dir.join("mtgo-data")),
            replay_day_anchor: Some(day),
            watching: true,
            arena_enabled: true,
            endstep_enabled: false,
        };
        let now = day
            .and_hms_opt(12, 1, 0)
            .expect("time")
            .and_utc()
            .timestamp_millis();
        let mut first = open(&config, now);
        run_until_settled(&mut first, now);
        drop(first);

        let mut second = open(&config, now + 60_000);
        run_until_settled(&mut second, now + 60_000);
        dir.append(
            "logs/mtgo.log",
            b"12:02:00 [INF] (Login|MtGO Login Success) Username: Qlocal (8)\n12:02:01 [INF] (UI|after)\n",
        );
        let (events, _) = run_until_settled(&mut second, now + 120_000);
        assert_eq!(
            second.projector.derived(TOKEN),
            Some(&Derived::Phantom),
            "the Login reloaded the match from storage"
        );
        assert_eq!(count(&config, "SELECT COUNT(*) FROM matches"), 0);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, TrackerEvent::MatchRemoved { .. })),
            "events: {events:?}"
        );
    }

    #[test]
    fn a_failed_match_that_is_not_shown_loses_its_failure_when_it_derives_again() {
        let dir = TestDir::new("engine-failed-then-hidden");
        dir.write("logs/mtgo.log", b"12:00:00 [INF] (UI|first)\n");
        let day = NaiveDate::from_ymd_opt(2026, 7, 1).expect("date");
        let config = TrackerConfig {
            db_path: dir.join("data/mymtgo.sqlite"),
            timezone: chrono_tz::UTC,
            os_timezone: None,
            log_overrides: vec![dir.join("logs")],
            arena_log_overrides: vec![dir.join("arena/Player.log")],
            mtgo_data_folder: Some(dir.join("mtgo-data")),
            replay_day_anchor: Some(day),
            watching: true,
            arena_enabled: true,
            endstep_enabled: false,
        };
        let now = day
            .and_hms_opt(12, 1, 0)
            .expect("time")
            .and_utc()
            .timestamp_millis();
        let failed = |engine: &Engine| engine.last_status.as_ref().map(|s| s.failed_matches);
        let tokens = [
            ("f5e33a1f-c2e7-4678-b30d-309b63f17a43", Derived::Hidden),
            (
                "f5e33a1f-c2e7-4678-b30d-309b63f17a44",
                Derived::AwaitingIdentity,
            ),
            ("f5e33a1f-c2e7-4678-b30d-309b63f17a45", Derived::Phantom),
        ];
        let mut engine = open(&config, now);
        for (token, _) in &tokens {
            engine.record_derived(
                token.to_string(),
                Some(Derived::Failed("projection panicked: Q boom".to_string())),
                1,
                None,
            );
        }
        run_until_settled(&mut engine, now);
        assert_eq!(count(&config, "SELECT COUNT(*) FROM match_failures"), 3);
        assert_eq!(failed(&engine), Some(3));

        for (round, (token, derived)) in tokens.into_iter().enumerate() {
            engine.record_derived(token.to_string(), Some(derived), 2, None);
            let (events, _) = run_until_settled(&mut engine, now + 1_000 * (round as i64 + 1));
            let left = 2 - round as i64;
            assert_eq!(
                count(&config, "SELECT COUNT(*) FROM match_failures"),
                left,
                "{token}'s failure is removed"
            );
            assert_eq!(failed(&engine), Some(left as u32));
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, TrackerEvent::MatchRemoved { .. })),
                "a match that was never shown is not removed from view"
            );
        }
    }

    fn match_failed(events: &[TrackerEvent]) -> usize {
        events
            .iter()
            .filter(|event| matches!(event, TrackerEvent::MatchFailed { .. }))
            .count()
    }

    const UNTRACKED_TOKEN: &str = "f5e33a1f-c2e7-4678-b30d-309b63f17a46";

    fn boom() -> Option<Derived> {
        Some(Derived::Failed("projection panicked: Q boom".to_string()))
    }

    /// Qlocal untracked after its Login, then a new match of Qlocal's
    /// (`UNTRACKED_TOKEN`), skipped: the engine, its config and the time.
    fn untracked_synth_match(dir: &TestDir) -> (Engine, TrackerConfig, i64) {
        dir.write(
            "logs/mtgo.log",
            b"12:00:00 [INF] (Login|MtGO Login Success) Username: Qlocal (8)\n12:00:01 [INF] (UI|after)\n",
        );
        let day = NaiveDate::from_ymd_opt(2026, 7, 1).expect("date");
        let config = TrackerConfig {
            db_path: dir.join("data/mymtgo.sqlite"),
            timezone: chrono_tz::UTC,
            os_timezone: None,
            log_overrides: vec![dir.join("logs")],
            arena_log_overrides: vec![dir.join("arena/Player.log")],
            mtgo_data_folder: Some(dir.join("mtgo-data")),
            replay_day_anchor: Some(day),
            watching: true,
            arena_enabled: true,
            endstep_enabled: false,
        };
        let now = day
            .and_hms_opt(12, 1, 0)
            .expect("time")
            .and_utc()
            .timestamp_millis();
        let mut engine = open(&config, now);
        run_until_settled(&mut engine, now);
        engine
            .edit_account(AccountEdit::SetTracked {
                username: "Qlocal".to_string(),
                tracked: false,
            })
            .expect("untrack");
        let mut synth = Synth::new(None);
        let mut log = String::new();
        for event in [
            synth.joined(UNTRACKED_TOKEN, 99, 7, &[]),
            synth.game_state(99, 7, &["Qlocal", "Qopponent"]),
        ] {
            log.push_str(event.text.trim_end());
            log.push('\n');
        }
        log.push_str("12:00:30 [INF] (UI|later)\n");
        dir.append("logs/mtgo.log", log.as_bytes());
        run_until_settled(&mut engine, now + 1_000);
        assert_eq!(count(&config, "SELECT COUNT(*) FROM untracked_matches"), 1);
        (engine, config, now)
    }

    /// A platform switch is a done account edit, so pages refetch.
    #[test]
    fn a_platform_switch_reports_account_changed() {
        let dir = TestDir::new("engine-platform-switch");
        let (mut engine, _config, now) = untracked_synth_match(&dir);
        let before = engine.tick(now + 2_000).expect("tick");
        assert!(
            !before
                .events
                .iter()
                .any(|event| matches!(event, TrackerEvent::AccountChanged))
        );
        let outcome = engine
            .edit_account(AccountEdit::SwitchPlatform {
                platform: store::Platform::Arena,
            })
            .expect("switch");
        assert!(matches!(outcome, AccountEditOutcome::Done { .. }));
        let tick = engine.tick(now + 3_000).expect("tick");
        assert!(
            tick.events
                .iter()
                .any(|event| matches!(event, TrackerEvent::AccountChanged))
        );
    }

    /// An untracked account's new match that fails to build is skipped like
    /// one that builds (#316 row 4): no failure row, no `MatchFailed`, so
    /// nothing is reported for a match the player asked not to track.
    #[test]
    fn an_untracked_accounts_failed_match_is_skipped_not_reported() {
        const TOKEN: &str = UNTRACKED_TOKEN;
        let dir = TestDir::new("engine-untracked-failed");
        let (mut engine, config, now) = untracked_synth_match(&dir);
        assert_eq!(
            engine.projector.local_player(TOKEN).as_deref(),
            Some("Qlocal")
        );

        engine.record_derived(
            TOKEN.to_string(),
            Some(Derived::Failed("projection panicked: Q boom".to_string())),
            2,
            None,
        );
        let (events, _) = run_until_settled(&mut engine, now + 2_000);
        assert_eq!(match_failed(&events), 0, "{events:?}");
        assert_eq!(count(&config, "SELECT COUNT(*) FROM match_failures"), 0);
        assert_eq!(
            count(&config, "SELECT COUNT(*) FROM untracked_matches"),
            1,
            "still a skipped match"
        );
        assert_eq!(
            engine.last_status.as_ref().map(|s| s.failed_matches),
            Some(0)
        );
        drop(engine);

        let mut reopened = open(&config, now + 60_000);
        let (events, _) = run_until_settled(&mut reopened, now + 60_000);
        assert_eq!(match_failed(&events), 0, "nothing reported on open");
        assert_eq!(count(&config, "SELECT COUNT(*) FROM match_failures"), 0);
    }

    /// The rebuild path (`rebuild_some`) skips an untracked account's failed
    /// match the same way, naming the player from its own projector.
    #[test]
    fn a_rebuild_skips_an_untracked_accounts_failed_match() {
        let dir = TestDir::new("engine-untracked-failed-rebuild");
        let (engine, config, now) = untracked_synth_match(&dir);
        drop(engine);
        let mut engine = open(&config, now + 60_000);
        run_until_settled(&mut engine, now + 60_000);
        // What `rebuild_some` does for the match, with a failed derive.
        let events = engine
            .history
            .match_events(&engine.reader, UNTRACKED_TOKEN)
            .expect("history");
        let mut projector = Projector::new(engine.projector.known_accounts().to_vec());
        for event in &events {
            projector.apply(event);
        }
        let player = projector.local_player(UNTRACKED_TOKEN);
        assert_eq!(player.as_deref(), Some("Qlocal"));
        engine.record_derived_for(UNTRACKED_TOKEN.to_string(), boom(), 3, None, player);
        let (events, _) = run_until_settled(&mut engine, now + 61_000);
        assert_eq!(match_failed(&events), 0, "{events:?}");
        assert_eq!(count(&config, "SELECT COUNT(*) FROM match_failures"), 0);
        assert_eq!(count(&config, "SELECT COUNT(*) FROM untracked_matches"), 1);
    }

    /// Tracked again, a skipped match that fails is a failure: told once,
    /// its skip row cleared, so the next start neither queues it again nor
    /// tells it again.
    #[test]
    fn a_retracked_match_that_fails_is_told_once() {
        let dir = TestDir::new("engine-untracked-failed-retrack");
        let (mut engine, config, now) = untracked_synth_match(&dir);
        engine
            .edit_account(AccountEdit::SetTracked {
                username: "Qlocal".to_string(),
                tracked: true,
            })
            .expect("track");
        // The re-track's derive fails (it replaces the build it queued).
        engine.record_derived(UNTRACKED_TOKEN.to_string(), boom(), 3, None);
        let (events, _) = run_until_settled(&mut engine, now + 2_000);
        assert_eq!(match_failed(&events), 1, "{events:?}");
        assert_eq!(count(&config, "SELECT COUNT(*) FROM match_failures"), 1);
        assert_eq!(count(&config, "SELECT COUNT(*) FROM untracked_matches"), 0);
        drop(engine);

        let mut reopened = open(&config, now + 60_000);
        assert!(reopened.pending_rebuild.is_empty(), "nothing re-queued");
        let (events, _) = run_until_settled(&mut reopened, now + 60_000);
        assert_eq!(match_failed(&events), 0, "not told again");
        assert_eq!(count(&config, "SELECT COUNT(*) FROM match_failures"), 1);
    }

    /// Untracking keeps a match that already has a row (release spec 5),
    /// so its provisional last event is previewed (#316 row 5), even when
    /// the match is not shown this run (a detached row).
    #[test]
    fn an_untracked_accounts_match_with_a_row_is_still_previewed() {
        const TOKEN: &str = "f5e33a1f-c2e7-4678-b30d-309b63f17a47";
        let dir = TestDir::new("engine-untracked-preview");
        let mut synth = Synth::new(None);
        let mut log =
            String::from("12:00:00 [INF] (Login|MtGO Login Success) Username: Qlocal (8)\n");
        for event in [
            synth.joined(TOKEN, 99, 7, &[]),
            synth.game_state(99, 7, &["Qlocal", "Qopponent"]),
        ] {
            log.push_str(event.text.trim_end());
            log.push('\n');
        }
        log.push_str("12:00:30 [INF] (UI|later)\n");
        dir.write("logs/mtgo.log", log.as_bytes());
        let day = NaiveDate::from_ymd_opt(2026, 7, 1).expect("date");
        let config = TrackerConfig {
            db_path: dir.join("data/mymtgo.sqlite"),
            timezone: chrono_tz::UTC,
            os_timezone: None,
            log_overrides: vec![dir.join("logs")],
            arena_log_overrides: vec![dir.join("arena/Player.log")],
            mtgo_data_folder: Some(dir.join("mtgo-data")),
            replay_day_anchor: Some(day),
            watching: true,
            arena_enabled: true,
            endstep_enabled: false,
        };
        let now = day
            .and_hms_opt(12, 1, 0)
            .expect("time")
            .and_utc()
            .timestamp_millis();
        let mut engine = open(&config, now);
        run_until_settled(&mut engine, now);
        assert_eq!(count(&config, "SELECT COUNT(*) FROM matches"), 1);
        engine
            .edit_account(AccountEdit::SetTracked {
                username: "Qlocal".to_string(),
                tracked: false,
            })
            .expect("untrack");
        drop(engine);
        // Detached (stored, not shown): after a restart the match is in
        // memory but not in `visible`, so only its row says it is kept.
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute(
                "UPDATE matches SET detached_at = 1 WHERE token = ?1",
                [TOKEN],
            )
            .expect("detach");

        let previewed = |events: &[TrackerEvent]| {
            events.iter().any(
                |event| matches!(event, TrackerEvent::MatchPreview { view, .. } if view.token == TOKEN),
            )
        };
        let mut engine = open(&config, now + 1_000);
        assert!(!engine.visible.contains(TOKEN), "not shown");
        assert!(engine.projector.derived(TOKEN).is_some(), "in memory");
        // Game 2 starts: the new game is the file's provisional last event.
        let second = synth.game_state(99, 8, &["Qlocal", "Qopponent"]);
        dir.append("logs/mtgo.log", second.text.trim_end().as_bytes());
        dir.append("logs/mtgo.log", b"\n");
        let tick = engine.tick(now + 5_000).expect("tick");
        assert!(previewed(&tick.events), "{:?}", tick.events);
    }

    #[test]
    fn a_resumed_file_keeps_its_stored_timezone() {
        let dir = TestDir::new("engine-resume-timezone");
        dir.write(
            "logs/mtgo.log",
            b"12:00:00 [INF] (UI|first)\n12:00:01 [INF] (UI|second)\n",
        );
        let day = NaiveDate::from_ymd_opt(2026, 7, 1).expect("date");
        let mut config = TrackerConfig {
            db_path: dir.join("data/mymtgo.sqlite"),
            timezone: chrono_tz::UTC,
            os_timezone: None,
            log_overrides: vec![dir.join("logs")],
            arena_log_overrides: vec![dir.join("arena/Player.log")],
            mtgo_data_folder: Some(dir.join("mtgo-data")),
            replay_day_anchor: Some(day),
            watching: true,
            arena_enabled: true,
            endstep_enabled: false,
        };
        let now = day
            .and_hms_opt(12, 1, 0)
            .expect("time")
            .and_utc()
            .timestamp_millis();
        let mut first = open(&config, now);
        run_until_settled(&mut first, now);
        drop(first);

        // The OS zone changed while the app was closed.
        config.timezone = chrono_tz::Europe::London;
        dir.append(
            "logs/mtgo.log",
            b"13:00:00 [INF] (Login|MtGO Login Success) Username: Qzone (8)\n13:00:01 [INF] (UI|after)\n",
        );
        let mut second = open(&config, now + 60_000);
        run_until_settled(&mut second, now + 60_000);
        let expected = day
            .and_hms_opt(13, 0, 0)
            .expect("time")
            .and_utc()
            .timestamp_millis();
        assert_eq!(
            value(
                &config,
                "SELECT logged_at FROM log_events WHERE kind = 'login'"
            ),
            expected,
            "dated in the file's stored zone (UTC), not the new one (London, UTC+1 in July)"
        );
    }

    /// An imported match shaped like the one the synthetic log below plays.
    pub(crate) fn imported(token: &str, mtgo_match_id: i64) -> store::import::ImportedMatch {
        store::import::ImportedMatch {
            token: token.to_string(),
            mtgo_match_id: Some(mtgo_match_id),
            local_username: "Qlocal".to_string(),
            opponent: Some("Qopponent".to_string()),
            format: Some("CMODERN".to_string()),
            match_type: Some("Constructed".to_string()),
            tournament_event_id: None,
            tournament_round: None,
            state: "complete".to_string(),
            outcome: "win".to_string(),
            started_at_ms: 1_767_000_000_000,
            ended_at_ms: Some(1_767_000_600_000),
            note: None,
            games: Vec::new(),
            manual: None,
            games_won_imported: None,
            games_lost_imported: None,
            deck_linked: false,
            replay_share: None,
            legacy_history: false,
        }
    }

    #[test]
    fn an_import_marks_the_later_tokens_when_one_revision_read_fails() {
        let dir = TestDir::new("engine-import-read-fails");
        let (_config, mut engine, _now) = league_engine(&dir, "");
        engine.mark_imported(vec![
            (
                "Q-bad".to_string(),
                Err(store::StoreError::Corrupt("unreadable".to_string())),
            ),
            ("Q-good".to_string(), Ok(Some(1))),
        ]);
        assert!(!engine.projector.is_retired("Q-bad"));
        assert!(
            engine.projector.is_retired("Q-good"),
            "the later token is marked"
        );
    }

    #[test]
    fn an_import_is_written_and_its_tokens_reload_on_a_live_event() {
        const TOKEN: &str = "f5e33a1f-c2e7-4678-b30d-309b63f17a43";
        let dir = TestDir::new("engine-import");
        dir.write(
            "logs/mtgo.log",
            b"12:00:00 [INF] (Login|MtGO Login Success) Username: Qlocal (8)\n12:00:01 [INF] (UI|first)\n",
        );
        let day = NaiveDate::from_ymd_opt(2026, 7, 1).expect("date");
        let config = TrackerConfig {
            db_path: dir.join("data/mymtgo.sqlite"),
            timezone: chrono_tz::UTC,
            os_timezone: None,
            log_overrides: vec![dir.join("logs")],
            arena_log_overrides: vec![dir.join("arena/Player.log")],
            mtgo_data_folder: Some(dir.join("mtgo-data")),
            replay_day_anchor: Some(day),
            watching: true,
            arena_enabled: true,
            endstep_enabled: false,
        };
        let now = day
            .and_hms_opt(12, 1, 0)
            .expect("time")
            .and_utc()
            .timestamp_millis();
        let mut engine = open(&config, now);
        run_until_settled(&mut engine, now);

        let counts = engine
            .import(vec![imported(TOKEN, 99)], now)
            .expect("import");
        assert_eq!(counts.added, 1);
        assert_eq!(
            count(
                &config,
                "SELECT COUNT(*) FROM matches WHERE source = 'imported'"
            ),
            1
        );
        assert!(
            engine.projector.is_retired(TOKEN),
            "a live event for the token reloads it from storage"
        );
        assert!(
            !engine.visible.contains(TOKEN),
            "an imported row is not a live match: a live event that derives it hidden has nothing to remove"
        );

        // MTGO plays the same match live: the log's row replaces the import.
        let mut synth = Synth::new(None);
        let mut log = String::new();
        for event in [
            synth.joined(TOKEN, 99, 7, &[]),
            synth.game_state(99, 7, &["Qlocal", "Qopponent"]),
        ] {
            log.push_str(event.text.trim_end());
            log.push('\n');
        }
        log.push_str("12:02:01 [INF] (UI|after)\n");
        dir.append("logs/mtgo.log", log.as_bytes());
        run_until_settled(&mut engine, now + 60_000);
        assert_eq!(count(&config, "SELECT COUNT(*) FROM matches"), 1);
        assert_eq!(
            count(&config, "SELECT COUNT(*) FROM matches WHERE source = 'log'"),
            1,
            "the live row won"
        );
    }

    #[test]
    fn a_live_match_replacing_an_imported_copy_reports_the_copy_removed() {
        const TOKEN: &str = "f5e33a1f-c2e7-4678-b30d-309b63f17a43";
        let dir = TestDir::new("engine-takeover-removed");
        let (config, mut engine, now) = league_engine(&dir, "");
        engine
            .import(vec![imported("Q-copy", 99)], now)
            .expect("import");
        let mut synth = Synth::new(None);
        let mut log = String::new();
        for event in [
            synth.joined(TOKEN, 99, 7, &[]),
            synth.game_state(99, 7, &["Qlocal", "Qopponent"]),
        ] {
            log.push_str(event.text.trim_end());
            log.push('\n');
        }
        log.push_str("12:02:01 [INF] (UI|after)\n");
        dir.append("logs/mtgo.log", log.as_bytes());
        let (events, _) = run_until_settled(&mut engine, now + 60_000);
        assert_eq!(count(&config, "SELECT COUNT(*) FROM matches"), 1);
        let removed: Vec<&TrackerEvent> = events
            .iter()
            .filter(|event| matches!(event, TrackerEvent::MatchRemoved { .. }))
            .collect();
        assert_eq!(
            removed,
            vec![&TrackerEvent::MatchRemoved {
                token: "Q-copy".to_string()
            }]
        );
    }

    #[test]
    fn a_0x_import_replacing_a_history_copy_reports_it_removed() {
        let dir = TestDir::new("engine-import-removes-history");
        let (config, mut engine, now) = league_engine(&dir, "");
        engine
            .import_history(vec![history_match("q-h1", 501)], now)
            .expect("history");
        engine.tick(now + 1).expect("tick");
        engine
            .import(vec![imported("Q-0x", 501)], now + 2)
            .expect("import");
        assert_eq!(
            count(&config, "SELECT COUNT(*) FROM matches WHERE token = 'q-h1'"),
            0
        );
        let events = engine.tick(now + 3).expect("tick").events;
        assert!(
            events.contains(&TrackerEvent::MatchRemoved {
                token: "q-h1".to_string()
            }),
            "{events:?}"
        );
    }

    /// A 0.x import under a history copy's own token turns the copy into
    /// the 0.x match: the row stays, so nothing is reported removed and a
    /// hidden copy stays hidden (#314 row 2 review).
    #[test]
    fn a_0x_import_under_a_history_copys_token_removes_nothing() {
        let dir = TestDir::new("engine-import-same-token");
        let (config, mut engine, now) = league_engine(&dir, "");
        engine
            .import_history(vec![history_match("Q-same", 502)], now)
            .expect("history");
        engine.tick(now + 1).expect("tick");
        assert_eq!(
            engine
                .edit_match(hide_edit("Q-same", now + 2))
                .expect("hide"),
            MatchEditOutcome::Done
        );
        engine.tick(now + 3).expect("tick");
        engine
            .import(vec![imported("Q-same", 502)], now + 4)
            .expect("import");
        assert_eq!(
            count(
                &config,
                "SELECT COUNT(*) FROM matches WHERE token = 'Q-same' AND source = 'imported'"
            ),
            1,
            "the copy became the 0.x match"
        );
        let events = engine.tick(now + 5).expect("tick").events;
        assert!(
            !events.contains(&TrackerEvent::MatchRemoved {
                token: "Q-same".to_string()
            }),
            "{events:?}"
        );
        assert!(engine.hidden.contains("Q-same"), "still removed from stats");
    }

    /// The removals a tick reports.
    fn removals(events: &[TrackerEvent]) -> Vec<&TrackerEvent> {
        events
            .iter()
            .filter(|event| matches!(event, TrackerEvent::MatchRemoved { .. }))
            .collect()
    }

    /// A 0.x copy and a history copy are stored, not live matches: a restart
    /// (`rehydrate`) leaves them out of `visible`, so a live event that
    /// derives one hidden reports nothing removed (#314 row 3).
    #[test]
    fn a_restart_leaves_copies_out_of_the_live_matches() {
        let dir = TestDir::new("engine-rehydrate-copies");
        let (config, mut engine, now) = league_engine(&dir, "");
        engine
            .import(vec![imported("Q-0x-copy", 601)], now)
            .expect("import");
        engine
            .import_history(vec![history_match("q-history-copy", 602)], now)
            .expect("history");
        engine.flush().expect("flush");
        drop(engine);

        let mut engine = open(&config, now + 1_000);
        for token in ["Q-0x-copy", "q-history-copy"] {
            assert!(!engine.visible.contains(token), "{token} is not live");
            assert!(engine.projector.is_retired(token), "{token} reloads");
            engine.record_derived(token.to_string(), Some(Derived::Hidden), 1, None);
        }
        let events = engine.tick(now + 2_000).expect("tick").events;
        assert!(removals(&events).is_empty(), "{events:?}");
        assert_eq!(count(&config, "SELECT COUNT(*) FROM matches"), 2);
    }

    /// A match pulled from the cloud is stored as a 0.x copy is: not
    /// visible, so a live event that derives it hidden reports nothing
    /// removed (#314 row 3).
    #[test]
    fn a_pulled_match_is_not_a_live_match() {
        let dir = TestDir::new("engine-pulled-copy");
        let (config, mut engine, now) = league_engine(&dir, "");
        let pulled = store::import::PulledMatch {
            platform: Default::default(),
            sources: None,
            local_player_fields: Default::default(),
            opponent_player_fields: Default::default(),
            endstep_ranked: None,
            endstep_manual_cards: Vec::new(),
            game_structure: None,
            imported: imported("Q-pulled", 603),
            detail: store::imported_detail::ImportedDetail::default(),
            deck: None,
            league_client_id: None,
            history: false,
            archetypes: Vec::new(),
            player: Default::default(),
        };
        engine
            .edit_sync(store::sync::SyncEdit::PulledMatch {
                client_id: "q-client".to_string(),
                pulled: Box::new(pulled),
                hash: "q-hash".to_string(),
                carry: String::new(),
                timezone: chrono_tz::UTC,
                now_ms: now,
            })
            .expect("pulled");
        assert_eq!(
            count(
                &config,
                "SELECT COUNT(*) FROM matches WHERE token = 'Q-pulled'"
            ),
            1
        );
        assert!(!engine.visible.contains("Q-pulled"));
        assert!(engine.projector.is_retired("Q-pulled"));
        engine.record_derived("Q-pulled".to_string(), Some(Derived::Hidden), 1, None);
        let events = engine.tick(now + 1).expect("tick").events;
        assert!(removals(&events).is_empty(), "{events:?}");
    }

    /// A gameless 1-2 history loss by Qhistory, linked to no deck version.
    pub(crate) fn history_match(token: &str, mtgo_match_id: i64) -> HistoryMatch {
        HistoryMatch {
            token: Some(token.to_string()),
            mtgo_match_id,
            local_username: "Qhistory".to_string(),
            opponent: Some("Qopponent".to_string()),
            format: Some("CMODERN".to_string()),
            match_type: "Constructed".to_string(),
            outcome: "loss".to_string(),
            started_at_ms: 1_000,
            ended_at_ms: None,
            games_won: 1,
            games_lost: 2,
            games: Vec::new(),
            deck_version_id: 99,
            log: Vec::new(),
        }
    }

    fn card_stubs_added(events: &[TrackerEvent]) -> Vec<u32> {
        events
            .iter()
            .filter_map(|event| match event {
                TrackerEvent::CardStubsAdded { count } => Some(*count),
                _ => None,
            })
            .collect()
    }

    /// `game_rows` with the sideboarding counters, which read oracle ids.
    fn card_rows_with_sideboarding(config: &TrackerConfig) -> Vec<String> {
        let mut rows = game_rows(config);
        let conn = store::open_reader(&config.db_path).expect("reader");
        let mut stmt = conn
            .prepare(
                "SELECT g.game_number || ':' || c.side || ':' || c.catalog_id || ':'
                     || c.sided_in || ',' || c.sided_out
                 FROM game_cards c JOIN games g ON g.id = c.game_id ORDER BY 1",
            )
            .expect("prepare");
        let found = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query");
        for row in found {
            rows.push(row.expect("row"));
        }
        rows
    }

    /// Every catalog id the log matches' game rows use.
    fn game_card_ids(config: &TrackerConfig) -> Vec<u32> {
        let conn = store::open_reader(&config.db_path).expect("reader");
        let mut stmt = conn
            .prepare("SELECT DISTINCT catalog_id FROM game_cards ORDER BY catalog_id")
            .expect("prepare");
        let rows = stmt.query_map([], |row| row.get(0)).expect("query");
        rows.map(|row| row.expect("row")).collect()
    }

    /// An answer for each id: its own name and oracle id, except that the
    /// fixture's sided in 38315 is another printing of the sided out 126126,
    /// so their sideboarding cancels once the oracle ids are known.
    fn answers_for(ids: &[u32]) -> store::cards::CardsUpdate {
        let oracle = |id: u32| if id == 38_315 { 126_126 } else { id };
        let resolved = ids
            .iter()
            .map(|&id| mymtgo_api::Resolution {
                mtgo_id: id,
                card: mymtgo_api::CardAnswer {
                    scryfall_id: Some(format!("q-scryfall-{id}")),
                    oracle_id: Some(format!("q-oracle-{}", oracle(id))),
                    name: Some(format!("Qcard {}", oracle(id))),
                    ..mymtgo_api::CardAnswer::default()
                },
                is_token: false,
            })
            .collect();
        store::cards::CardsUpdate {
            tokens: Vec::new(),
            resolutions: mymtgo_api::Resolutions {
                resolved,
                unknown: Vec::new(),
            },
            unknown_at_ms: 0,
        }
    }

    /// Tick until the game syncs are done.
    fn settle_syncs(engine: &mut Engine, mut now_ms: i64) -> Vec<TrackerEvent> {
        let mut events = Vec::new();
        for _ in 0..1_000 {
            engine.flush().expect("flush");
            now_ms += 1;
            let tick = engine.tick(now_ms).expect("tick");
            events.extend(tick.events);
            if !tick.syncing {
                break;
            }
        }
        engine.flush().expect("flush");
        events
    }

    #[test]
    fn resolving_cards_resyncs_the_matches_that_use_them() {
        let fixture = load_fixture("draft_hobbit");
        let dir = TestDir::new("engine-cards-resync");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let now = fixture.meta.now_ms() + 3 * ABANDON_AFTER_MS;
        let mut engine = open(&config, now);
        run_until_settled(&mut engine, now);
        let synced = game_rows(&config);
        let ids = game_card_ids(&config);
        let card = ids[0];
        let users: Vec<String> = {
            let conn = store::open_reader(&config.db_path).expect("reader");
            store::cards::log_matches_using(&conn, &[card]).expect("users")
        };
        assert!(!users.is_empty());
        // Stale rows for the card: only a resync puts them back.
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute(
                "UPDATE game_cards SET seen = 99 WHERE catalog_id = ?1",
                [card],
            )
            .expect("stale");
        let mut one = answers_for(&[card]);
        one.resolutions.resolved[0].card.oracle_id = Some("q-oracle-alone".to_string());
        let outcome = engine.resolve_cards(one).expect("resolved");
        assert_eq!(outcome.changed, vec![card]);
        // Ticked by hand: whether each `CardsChanged` came before or after
        // the last resync was written.
        let mut now_ms = now + 10_000;
        let mut told = 0;
        let mut told_after_the_resyncs = false;
        for _ in 0..1_000 {
            engine.flush().expect("flush");
            now_ms += 1;
            let tick = engine.tick(now_ms).expect("tick");
            let changed = tick
                .events
                .iter()
                .filter(|event| matches!(event, TrackerEvent::CardsChanged))
                .count();
            if changed > 0 {
                told += changed;
                told_after_the_resyncs = !engine.game_sync.card_resync_outstanding();
            }
            if !tick.syncing && !engine.game_sync.card_resync_outstanding() {
                break;
            }
        }
        engine.flush().expect("flush");
        assert_eq!(engine.game_sync.resyncs_sent(), users.as_slice());
        assert_eq!(
            game_rows(&config),
            synced,
            "the stale rows were synced again"
        );
        assert_eq!(told, 2, "names at once, then the resynced rows");
        assert!(
            told_after_the_resyncs,
            "open pages refetch after the last resync was written"
        );
        // An answer that changes nothing the game sync reads resyncs nothing.
        let mut same = answers_for(&[card]);
        same.resolutions.resolved[0].card.oracle_id = Some("q-oracle-alone".to_string());
        assert!(
            engine
                .resolve_cards(same)
                .expect("again")
                .changed
                .is_empty()
        );
        settle_syncs(&mut engine, now + 20_000);
        assert_eq!(engine.game_sync.resyncs_sent(), users.as_slice());
    }

    #[test]
    fn regenerating_card_stats_resyncs_the_decks_complete_log_matches() {
        let fixture = load_fixture("draft_hobbit");
        let dir = TestDir::new("engine-regenerate-card-stats");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let now = fixture.meta.now_ms() + 3 * ABANDON_AFTER_MS;
        let mut engine = open(&config, now);
        run_until_settled(&mut engine, now);
        let synced = game_rows(&config);
        // Every complete match with games is linked to one deck's version.
        let (deck_id, tokens) = {
            let conn = store::open_writer(&config.db_path).expect("writer");
            let deck_id: i64 = conn
                .query_row(
                    "INSERT INTO decks (mtgo_id, name, format, created_at, updated_at, source)
                     VALUES ('q-deck', 'Qdeck', 'CMODERN', 0, 0, 'file') RETURNING id",
                    [],
                    |row| row.get(0),
                )
                .expect("deck");
            let version: i64 = conn
                .query_row(
                    "INSERT INTO deck_versions (deck_id, signature, modified_at)
                     VALUES (?1, 'Q', 0) RETURNING id",
                    [deck_id],
                    |row| row.get(0),
                )
                .expect("version");
            conn.execute(
                "UPDATE matches SET deck_version_id = ?1 WHERE state = 'complete'
                 AND EXISTS (SELECT 1 FROM games g WHERE g.match_id = matches.id)",
                [version],
            )
            .expect("link");
            conn.execute("UPDATE game_cards SET seen = 99", [])
                .expect("stale");
            let mut stmt = conn
                .prepare("SELECT token FROM matches WHERE deck_version_id = ?1 ORDER BY id")
                .expect("tokens");
            let tokens: Vec<String> = stmt
                .query_map([version], |row| row.get(0))
                .expect("query")
                .map(|row| row.expect("row"))
                .collect();
            (deck_id, tokens)
        };
        assert!(!tokens.is_empty());

        let count = engine.regenerate_card_stats(deck_id).expect("regenerated");
        assert_eq!(count as usize, tokens.len());
        let events = settle_syncs(&mut engine, now + 10_000);
        assert_eq!(engine.game_sync.resyncs_sent(), tokens.as_slice());
        assert_eq!(
            game_rows(&config),
            synced,
            "the stale rows were synced again"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, TrackerEvent::CardsChanged)),
            "open pages refetch once the resyncs are written"
        );
        // Another deck has nothing to regenerate.
        assert_eq!(engine.regenerate_card_stats(deck_id + 1).expect("none"), 0);
    }

    #[test]
    fn a_resync_after_resolution_equals_a_fresh_sync() {
        let fixture = load_fixture("draft_hobbit");
        let now = fixture.meta.now_ms() + 3 * ABANDON_AFTER_MS;
        // Live: the cards resolve after the games were synced.
        let live_dir = TestDir::new("engine-cards-resync-live");
        live_dir.write("logs/mtgo.log", &fixture.bytes);
        let live = config(&live_dir, &fixture);
        let mut engine = open(&live, now);
        run_until_settled(&mut engine, now);
        let before = card_rows_with_sideboarding(&live);
        let ids = game_card_ids(&live);
        engine.resolve_cards(answers_for(&ids)).expect("resolved");
        settle_syncs(&mut engine, now + 10_000);
        drop(engine);
        let resynced = card_rows_with_sideboarding(&live);
        assert_ne!(resynced, before, "the merged printings change the rows");

        // Rebuild: the same card table before the games are synced.
        let fresh_dir = TestDir::new("engine-cards-resync-fresh");
        fresh_dir.write("logs/mtgo.log", &fixture.bytes);
        let fresh = config(&fresh_dir, &fixture);
        let mut engine = open(&fresh, now);
        {
            let mut conn = store::open_writer(&fresh.db_path).expect("writer");
            let tx = conn.transaction().expect("tx");
            let stubs: Vec<u32> = ids.clone();
            store::cards::ensure_id_stubs(&tx, &stubs).expect("rows");
            store::cards::apply_resolutions(&tx, &answers_for(&ids)).expect("apply");
            tx.commit().expect("commit");
        }
        run_until_settled(&mut engine, now);
        drop(engine);
        assert_eq!(
            card_rows_with_sideboarding(&fresh),
            resynced,
            "live equals rebuild"
        );
    }

    /// 0.x `CreateMissingCardsFromLocalDataTest` "queues the detail fill
    /// when it creates anything": v1 reports `CardStubsAdded`, which wakes
    /// the cards worker.
    #[test]
    fn a_card_stub_scan_that_adds_stubs_reports_card_stubs_added() {
        let dir = TestDir::new("engine-card-stubs");
        let (config, mut engine, now) = league_engine(&dir, "");
        run_until_settled(&mut engine, now);
        let signature = decks::signature(&[decks::DeckCard {
            catalog_id: 101,
            quantity: 4,
            sideboard: false,
        }]);
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute_batch(&format!(
                "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source)
                     VALUES (900, 'q-stub-deck', 'Q deck', 'CMODERN', 0, 0, 'imported');
                 INSERT INTO deck_versions (deck_id, signature, modified_at)
                     VALUES (900, '{signature}', 0);"
            ))
            .expect("a version without card rows");
        assert_eq!(engine.scan_card_stubs().expect("scan"), 1);
        engine.flush().expect("flush");
        let events = engine.tick(now + 1).expect("tick").events;
        assert_eq!(card_stubs_added(&events), vec![1], "{events:?}");
        // Told once; a scan that adds nothing tells nothing.
        assert_eq!(engine.scan_card_stubs().expect("again"), 0);
        engine.flush().expect("flush");
        let events = engine.tick(now + 2).expect("tick").events;
        assert!(card_stubs_added(&events).is_empty(), "{events:?}");
        assert_eq!(
            count(&config, "SELECT COUNT(*) FROM cards WHERE catalog_id = 101"),
            1
        );
    }

    #[test]
    fn a_history_import_is_announced_and_deleting_it_reports_each_match_removed() {
        let dir = TestDir::new("engine-history-import");
        let (config, mut engine, now) = league_engine(&dir, "");
        let counts = engine
            .import_history(
                vec![history_match("q-h1", 501), history_match("q-h2", 502)],
                now,
            )
            .expect("import");
        assert_eq!(
            counts.imported,
            vec!["q-h1".to_string(), "q-h2".to_string()]
        );
        assert!(!engine.visible.contains("q-h1"));
        assert!(engine.projector.is_retired("q-h1"));
        let events = engine.tick(now + 1).expect("tick").events;
        assert!(events.contains(&TrackerEvent::DecksChanged), "{events:?}");
        assert_eq!(leagues_changed(&events), 1);

        // Nothing new: nothing to announce.
        let again = engine
            .import_history(vec![history_match("q-h3", 501)], now)
            .expect("rerun");
        assert!(again.imported.is_empty());
        assert_eq!(again.skipped_existing, 1);
        let events = engine.tick(now + 2).expect("tick").events;
        assert!(!events.contains(&TrackerEvent::DecksChanged), "{events:?}");

        // A hidden one was reported removed when it was hidden.
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute("UPDATE matches SET hidden_at = 1 WHERE token = 'q-h2'", [])
            .expect("hide");
        engine.hidden.insert("q-h2".to_string());
        let removed = engine.delete_history_imports().expect("delete");
        assert_eq!(removed, vec!["q-h1".to_string(), "q-h2".to_string()]);
        assert!(!engine.visible.contains("q-h1"));
        assert!(!engine.hidden.contains("q-h2"));
        // After the league sync's debounce, so its report is in this tick.
        let events = engine
            .tick(now + 2 + LEAGUE_SYNC_AFTER_MS)
            .expect("tick")
            .events;
        let removals: Vec<&TrackerEvent> = events
            .iter()
            .filter(|event| matches!(event, TrackerEvent::MatchRemoved { .. }))
            .collect();
        assert_eq!(
            removals,
            vec![&TrackerEvent::MatchRemoved {
                token: "q-h1".to_string()
            }]
        );
        assert!(events.contains(&TrackerEvent::DecksChanged), "{events:?}");
        assert_eq!(leagues_changed(&events), 1);
        assert_eq!(count(&config, "SELECT COUNT(*) FROM matches"), 0);
    }

    #[test]
    fn a_projection_rebuild_leaves_history_imports_alone() {
        let fixture = load_fixture("draft_hobbit");
        let dir = TestDir::new("engine-rebuild-history");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let now = fixture.meta.now_ms() + 3 * ABANDON_AFTER_MS;
        let mut first = open(&config, now);
        run_until_settled(&mut first, now);
        first
            .import_history(vec![history_match("q-history", 501)], now)
            .expect("history import");
        drop(first);
        const IMPORTED: &str = "SELECT token || source || outcome || revision || derived_version
                 || started_at || needs_attention || games_won_imported || games_lost_imported
             FROM matches WHERE source = 'history'";
        let imported_row = |config: &TrackerConfig| -> String {
            store::open_reader(&config.db_path)
                .expect("reader")
                .query_row(IMPORTED, [], |row| row.get(0))
                .expect("the history import")
        };
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute_batch(
                "UPDATE matches SET derived_version = 0;
                 UPDATE derivations SET version = 0;",
            )
            .expect("stale rows");
        let before = imported_row(&config);

        let mut second = open(&config, now + 1_000);
        let (events, _) = run_until_settled(&mut second, now + 1_000);
        assert!(
            count(
                &config,
                "SELECT MIN(derived_version) FROM matches WHERE source = 'log'"
            ) > 0,
            "the log matches were rebuilt"
        );
        assert_eq!(imported_row(&config), before);
        assert!(
            !events.iter().any(|event| matches!(
                event,
                TrackerEvent::MatchRemoved { token } if token == "q-history"
            )),
            "{events:?}"
        );
    }

    // Limited changes (Plan N: the Limited pages refresh on `LimitedChanged`)

    fn limited_changed(events: &[TrackerEvent]) -> usize {
        events
            .iter()
            .filter(|event| matches!(event, TrackerEvent::LimitedChanged))
            .count()
    }

    #[test]
    fn draft_events_and_saved_pick_notes_report_limited_changed() {
        use store::limited::{DraftNoteEdit, DraftNoteOutcome};
        let fixture = load_fixture("draft_hobbit");
        let dir = TestDir::new("engine-limited-changed");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let now = fixture.meta.now_ms();
        let mut engine = open(&config, now);
        let (events, _) = run_until_settled(&mut engine, now);
        assert!(limited_changed(&events) >= 1, "{events:?}");

        // The stored events read back per account and fold into the draft.
        let reader = store::open_reader(&config.db_path).expect("reader");
        let account: i64 = reader
            .query_row(
                "SELECT id FROM accounts WHERE username = ?1",
                [&fixture.meta.local_username],
                |row| row.get(0),
            )
            .expect("the local account");
        let drafts = limited::fold_drafts(
            &store::limited::draft_events(&reader, account).expect("events"),
            fixture.meta.tz(),
            now,
        );
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].picks.len(), 42);
        assert_eq!(
            limited_changed(&engine.tick(now + 1).expect("tick").events),
            0,
            "a quiet tick reports nothing"
        );

        let note = |ordinal: u32| DraftNoteEdit::Set {
            draft_token: drafts[0].token.clone(),
            ordinal,
            note: "Q took the bomb".to_string(),
            now_ms: now,
        };
        assert_eq!(
            engine.edit_draft_note(note(43)).expect("edit"),
            DraftNoteOutcome::NotFound
        );
        assert_eq!(
            limited_changed(&engine.tick(now + 2).expect("tick").events),
            0,
            "a refused edit changes nothing"
        );
        assert_eq!(
            engine.edit_draft_note(note(1)).expect("edit"),
            DraftNoteOutcome::Done
        );
        assert_eq!(
            limited_changed(&engine.tick(now + 3).expect("tick").events),
            1
        );
        assert_eq!(
            count(
                &config,
                "SELECT COUNT(*) FROM draft_picks WHERE note IS NOT NULL"
            ),
            1,
            "committed before it was reported"
        );
    }

    // League changes (leagues spec 8: the pages refresh on `LeaguesChanged`)

    /// An engine over a log holding only a login (and `extra` lines after it),
    /// read to the end, and the time after it.
    fn league_engine(dir: &TestDir, extra: &str) -> (TrackerConfig, Engine, i64) {
        dir.write(
            "logs/mtgo.log",
            format!(
                "12:00:00 [INF] (Login|MtGO Login Success) Username: Qlocal (7)\n{extra}12:00:30 [INF] (UI|x)\n"
            )
            .as_bytes(),
        );
        let day = NaiveDate::from_ymd_opt(2026, 7, 1).expect("date");
        let config = TrackerConfig {
            db_path: dir.join("data/mymtgo.sqlite"),
            timezone: chrono_tz::UTC,
            os_timezone: None,
            log_overrides: vec![dir.join("logs")],
            arena_log_overrides: vec![dir.join("arena/Player.log")],
            mtgo_data_folder: Some(dir.join("mtgo-data")),
            replay_day_anchor: Some(day),
            watching: true,
            arena_enabled: true,
            endstep_enabled: false,
        };
        let now = day
            .and_hms_opt(12, 1, 0)
            .expect("time")
            .and_utc()
            .timestamp_millis();
        let mut engine = open(&config, now);
        run_until_settled(&mut engine, now);
        (config, engine, now + 10_000)
    }

    fn leagues_changed(events: &[TrackerEvent]) -> usize {
        events
            .iter()
            .filter(|event| matches!(event, TrackerEvent::LeaguesChanged))
            .count()
    }

    /// A complete win by Qlocal, with a league token or a tournament id.
    fn league_view(token: &str, league: Option<&str>, tournament: Option<u64>) -> Box<MatchView> {
        Box::new(MatchView {
            token: token.to_string(),
            mtgo_match_id: Some(99),
            local_username: "Qlocal".to_string(),
            opponent: Some("Qopponent".to_string()),
            format: Some("CMODERN".to_string()),
            match_type: Some("Constructed".to_string()),
            tournament_event_id: tournament,
            tournament_round: tournament.map(|_| 1),
            league_token: league.map(str::to_string),
            game_structure: Some("Modern".to_string()),
            state: mtgo_match::MatchState::Complete,
            resolution: None,
            outcome: mtgo_match::Outcome::Win,
            tentative_outcome: None,
            wins: 2,
            losses: 0,
            started_at_ms: 1_000,
            ended_at_ms: Some(2_000),
            last_activity_ms: 2_000,
            games: Vec::new(),
        })
    }

    #[test]
    fn league_and_tournament_matches_report_leagues_changed_once_per_tick() {
        let dir = TestDir::new("engine-leagues-changed");
        let (config, mut engine, now) = league_engine(&dir, "");
        engine.record_derived(
            "Q-league".to_string(),
            Some(Derived::Visible(league_view(
                "Q-league",
                Some("Q-token"),
                None,
            ))),
            1,
            None,
        );
        engine.record_derived(
            "Q-round".to_string(),
            Some(Derived::Visible(league_view("Q-round", None, Some(500)))),
            1,
            None,
        );
        // The tick that pushes them leaves the due league sync to the next.
        let tick = engine.tick(now).expect("tick");
        assert_eq!(leagues_changed(&tick.events), 0, "{:?}", tick.events);
        let tick = engine.tick(now + 1).expect("tick");
        assert_eq!(leagues_changed(&tick.events), 1, "{:?}", tick.events);
        // Reported after the commit: a fresh reader sees both rows.
        assert_eq!(count(&config, "SELECT COUNT(*) FROM matches"), 2);
        assert_eq!(
            leagues_changed(&engine.tick(now + 2).expect("tick").events),
            0
        );

        // Removing one reports again, once its league sync has run.
        engine.record_derived("Q-league".to_string(), Some(Derived::Hidden), 2, None);
        let tick = engine.tick(now + 2 + LEAGUE_SYNC_AFTER_MS).expect("tick");
        assert_eq!(leagues_changed(&tick.events), 1, "{:?}", tick.events);
    }

    #[test]
    fn a_match_outside_leagues_and_tournaments_reports_no_leagues_changed() {
        let dir = TestDir::new("engine-leagues-unchanged");
        let (_config, mut engine, now) = league_engine(&dir, "");
        engine.record_derived(
            "Q-plain".to_string(),
            Some(Derived::Visible(league_view("Q-plain", Some("  "), None))),
            1,
            None,
        );
        assert_eq!(leagues_changed(&engine.tick(now).expect("tick").events), 0);
        engine.record_derived("Q-plain".to_string(), Some(Derived::Hidden), 2, None);
        assert_eq!(
            leagues_changed(&engine.tick(now + 1).expect("tick").events),
            0
        );
    }

    #[test]
    fn a_stored_league_match_removed_before_this_run_derived_it_reports_leagues_changed() {
        let dir = TestDir::new("engine-leagues-stored-removed");
        let (_config, mut engine, now) = league_engine(&dir, "");
        engine.record_derived(
            "Q-league".to_string(),
            Some(Derived::Visible(league_view(
                "Q-league",
                Some("Q-token"),
                None,
            ))),
            1,
            None,
        );
        engine.tick(now).expect("tick");
        // As after a restart: stored and visible, not seen derived this run.
        engine.league_matches.clear();
        engine.record_derived("Q-league".to_string(), Some(Derived::Hidden), 2, None);
        assert_eq!(
            leagues_changed(
                &engine
                    .tick(now + LEAGUE_SYNC_AFTER_MS)
                    .expect("tick")
                    .events
            ),
            1
        );
    }

    #[test]
    fn a_league_drop_logged_reports_leagues_changed() {
        let dir = TestDir::new("engine-leagues-drop");
        let (_config, mut engine, now) = league_engine(&dir, "");
        dir.append(
            "logs/mtgo.log",
            b"12:02:00 [INF] (DEFAULT|) Send Class: FlsLeagueUserDropReqMessage\n12:02:01 [INF] (UI|y)\n",
        );
        let (events, _) = run_until_settled(&mut engine, now);
        assert_eq!(leagues_changed(&events), 1, "{events:?}");
        // Nothing league-like in the next lines: no more reports.
        dir.append(
            "logs/mtgo.log",
            b"12:03:00 [INF] (UI|z)\n12:03:01 [INF] (UI|w)\n",
        );
        assert_eq!(
            leagues_changed(&run_until_settled(&mut engine, now + 60_000).0),
            0
        );
    }

    #[test]
    fn a_done_league_edit_and_a_league_import_report_leagues_changed() {
        let dir = TestDir::new("engine-leagues-edits");
        let (config, mut engine, now) = league_engine(&dir, "");
        let account = count(&config, "SELECT id FROM accounts WHERE username = 'Qlocal'");
        let refused = engine
            .edit_league(store::leagues::LeagueEdit::SetStatus {
                run_key: "manual:404".to_string(),
                status: Some(store::leagues::UserState::Dropped),
                now_ms: now,
            })
            .expect("edit");
        assert_eq!(refused, store::leagues::LeagueEditOutcome::NotFound);
        assert_eq!(leagues_changed(&engine.tick(now).expect("tick").events), 0);

        store::open_writer(&config.db_path)
            .expect("writer")
            .execute_batch(&format!(
                "INSERT INTO decks (id, mtgo_id, account_id, name, format, created_at, updated_at, source)
                 VALUES (1, 'Q-deck', {account}, 'Qdeck', 'CMODERN', 1, 1, 'file');"
            ))
            .expect("deck");
        let done = engine
            .edit_league(store::leagues::LeagueEdit::CreateManual {
                account_id: account,
                deck_id: 1,
                name: "Qmanual".to_string(),
                token: "manual_QqQqQqQqQqQqQqQqQqQqQqQq".to_string(),
                started_at: now,
                now_ms: now,
            })
            .expect("edit");
        assert_eq!(done, store::leagues::LeagueEditOutcome::Done);
        assert_eq!(
            leagues_changed(&engine.tick(now + 1).expect("tick").events),
            1
        );

        // An import is synced after the debounce, like a tracker change.
        engine.import_leagues(Vec::new(), now + 2).expect("import");
        assert_eq!(
            leagues_changed(
                &engine
                    .tick(now + 1 + LEAGUE_SYNC_AFTER_MS)
                    .expect("tick")
                    .events
            ),
            1
        );
    }

    // The game and league syncs (Plan R, R5)

    /// The offset of the start of the line holding `needle`.
    fn line_start_before(bytes: &[u8], needle: &str) -> usize {
        let at = bytes
            .windows(needle.len())
            .position(|window| window == needle.as_bytes())
            .expect("the needle is in the fixture");
        bytes[..at]
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |newline| newline + 1)
    }

    /// Tick at `now_ms` until every byte is read, returning the events.
    fn catch_up(engine: &mut Engine, now_ms: i64) -> Vec<TrackerEvent> {
        let mut events = Vec::new();
        loop {
            let tick = engine.tick(now_ms).expect("tick");
            events.extend(tick.events);
            if !tick.more {
                return events;
            }
        }
    }

    /// The last draft match of `draft_hobbit`, a league match.
    const HOBBIT_LAST: &str = "6250e242-fcec-4d44-b642-9268b6581719";

    #[test]
    fn the_match_push_does_not_wait_for_either_sync() {
        let fixture = load_fixture("draft_hobbit");
        let cut = line_start_before(&fixture.bytes, HOBBIT_LAST);
        let dir = TestDir::new("engine-push-before-syncs");
        dir.write("logs/mtgo.log", &fixture.bytes[..cut]);
        let config = config(&dir, &fixture);
        let now = fixture.meta.now_ms();
        let mut engine = open(&config, now);
        run_until_settled(&mut engine, now);
        let syncs = engine.leagues.syncs;

        // The last match is played: read at one instant, so the league
        // sync's deadline has not passed when the tracker has caught up.
        dir.append("logs/mtgo.log", &fixture.bytes[cut..]);
        let later = now + 10_000;
        let events = catch_up(&mut engine, later);
        assert!(pushed_update(&events, HOBBIT_LAST), "{events:?}");
        assert_eq!(leagues_changed(&events), 0, "held for the league sync");
        assert_eq!(engine.leagues.syncs, syncs, "the league sync waits");
        engine.flush().expect("flush");
        let in_league = format!(
            "SELECT COUNT(*) FROM matches WHERE token = '{HOBBIT_LAST}' AND league_id IS NOT NULL"
        );
        assert_eq!(count(&config, &in_league), 0, "not synced yet");
        let cards = format!(
            "SELECT COUNT(*) FROM game_cards c JOIN games g ON g.id = c.game_id
             JOIN matches m ON m.id = g.match_id WHERE m.token = '{HOBBIT_LAST}'"
        );
        assert!(
            count(&config, &cards) > 0,
            "the game sync followed the batch"
        );

        // The game sync wrote the hand: the match is pushed again, so an
        // open match page shows it.
        let tick = engine.tick(later + 1).expect("tick");
        assert!(
            pushed_update(&tick.events, HOBBIT_LAST),
            "{:?}",
            tick.events
        );

        let tick = engine.tick(later + LEAGUE_SYNC_AFTER_MS).expect("tick");
        assert_eq!(leagues_changed(&tick.events), 1, "{:?}", tick.events);
        assert_eq!(engine.leagues.syncs, syncs + 1);
        assert_eq!(count(&config, &in_league), 1);
    }

    // The event overlay's tournaments (event overlay spec 7.1).

    /// The joined Challenge of the tournament tests.
    const JOINED: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

    /// A round match token (`index` 0 is the account's).
    fn round_match(index: usize) -> String {
        format!("{index:08x}-0000-4000-8000-{index:012x}")
    }

    /// `token`'s state change to `to`, at `time`.
    fn tournament_state(time: &str, token: &str, to: &str) -> String {
        format!(
            "{time} [INF] (Game Management|Tournament State Changed for {token} from \
             TournamentUninitializedState to {to}) \n"
        )
    }

    /// `token`'s registration snapshot and round 1 of `matches` matches, the
    /// first one the account's (login id 7).
    fn tournament_round_one(token: &str, matches: usize) -> String {
        let mut games = Vec::new();
        for index in 0..matches {
            let login = if index == 0 { 7 } else { 1_000 + index };
            games.push(format!(
                "{{\"EventToken\":\"{}\",\"Players\":[{{\"LoginID\":{login}}},{{\"LoginID\":{}}}]}}",
                round_match(index),
                5_000 + index
            ));
        }
        format!(
            "12:00:41 [INF] (Game Management|Processing Registered Handler for EventSyncData_t in \
             TournamentUninitializedState) Processor: TournamentUninitializedState Message: \
             {{\"EventToken\":\"{token}\",\"EventID\":12858913,\"Description\":\"Q Challenge\",\
             \"GameStructureCd\":\"CLEGACY\"}} Receiver: \
             WotC.MtGO.Client.Model.Play.TournamentEvent.Tournament\n\
             12:00:42 [INF] (Game Management|Processing Registered Handler for \
             FlsTournamentRoundInfoMessage in PremierJoinedAwaitingStartState) Processor: \
             PremierJoinedAwaitingStartState Message: {{\"Token\":\"{token}\",\"ID\":12858913,\
             \"Round\":{{\"Number\":1,\"Matches\":[{}],\"ByeList\":[]}}}} Receiver: \
             WotC.MtGO.Client.Model.Play.TournamentEvent.Tournament\n",
            games.join(",")
        )
    }

    fn tournaments_changed(events: &[TrackerEvent]) -> usize {
        events
            .iter()
            .filter(|event| matches!(event, TrackerEvent::TournamentsChanged))
            .count()
    }

    /// Append `lines` (and a closing line) and read them, then tick past the
    /// tournament sync's debounce. Returns every event.
    fn read_and_settle(
        dir: &TestDir,
        engine: &mut Engine,
        now: i64,
        lines: &str,
    ) -> Vec<TrackerEvent> {
        dir.append(
            "logs/mtgo.log",
            format!("{lines}12:00:59 [INF] (UI|x)\n").as_bytes(),
        );
        let mut events = catch_up(engine, now);
        events.extend(
            engine
                .tick(now + TOURNAMENT_SYNC_AFTER_MS)
                .expect("tick")
                .events,
        );
        events
    }

    #[test]
    fn a_lobby_tournament_causes_no_tournament_sync() {
        let dir = TestDir::new("engine-tournament-lobby");
        let (_config, mut engine, now) = league_engine(&dir, "");
        engine.tick(now).expect("tick");
        let syncs = engine.tournaments.syncs;
        let lobby = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
        let lines = format!(
            "{}{}",
            tournament_state("12:00:40", lobby, "TournamentNotJoinedRoundInProgressState"),
            tournament_round_one(lobby, 3).replace(
                "PremierJoinedAwaitingStartState",
                "TournamentNotJoinedBetweenRoundsState"
            )
        );
        let events = read_and_settle(&dir, &mut engine, now, &lines);
        assert_eq!(
            engine.tournaments.syncs, syncs,
            "a lobby event is not synced"
        );
        assert_eq!(tournaments_changed(&events), 0);
    }

    #[test]
    fn a_joined_tournament_syncs_and_a_burst_of_closes_syncs_once() {
        let dir = TestDir::new("engine-tournament-joined");
        let (config, mut engine, now) = league_engine(&dir, "");
        engine.tick(now).expect("tick");
        let syncs = engine.tournaments.syncs;
        let lines = format!(
            "{}{}",
            tournament_state("12:00:40", JOINED, "PremierJoinedAwaitingMaxPlayersState"),
            tournament_round_one(JOINED, 151)
        );
        let events = read_and_settle(&dir, &mut engine, now, &lines);
        assert_eq!(engine.tournaments.syncs, syncs + 1);
        assert_eq!(tournaments_changed(&events), 1);
        assert_eq!(
            count(&config, "SELECT matches_left FROM tournament_standings"),
            151
        );

        // 150 other players' matches close in one tick.
        let mut closes = String::new();
        for index in 1..151 {
            closes.push_str(&format!(
                "12:00:50 [INF] (Game Management|Match State Changed for {} from \
                 TournamentMatchNotJoinedEventUnderwayState to TournamentMatchClosedState) \n",
                round_match(index)
            ));
        }
        let events = read_and_settle(&dir, &mut engine, now, &closes);
        assert_eq!(
            engine.tournaments.syncs,
            syncs + 2,
            "one sync for the burst"
        );
        assert_eq!(tournaments_changed(&events), 1);
        assert_eq!(leagues_changed(&events), 0, "the league pages are not told");
        assert_eq!(
            count(&config, "SELECT matches_left FROM tournament_standings"),
            1
        );

        // State changes that are not a close never mark the sync.
        let mut changes = String::new();
        for index in 1..151 {
            changes.push_str(&format!(
                "12:00:51 [INF] (Game Management|Match State Changed for {} from \
                 TournamentMatchNotJoinedEventUnderwayState to \
                 TournamentMatchNotJoinedSideboardingState) \n",
                round_match(index)
            ));
        }
        read_and_settle(&dir, &mut engine, now, &changes);
        assert_eq!(
            engine.tournaments.syncs,
            syncs + 2,
            "no sync for non-closes"
        );

        // An InPlayoffs line of the joined tournament marks it.
        let playoffs = format!(
            "12:00:52 [INF] (Game Management|Processing Registered Handler for \
             FlsPlayerEventStateChangedMessage in TournamentJoinedBetweenRoundsState) Processor: \
             TournamentJoinedBetweenRoundsState Message: {{\"Token\":\"{JOINED}\",\
             \"NewState\":24,\"InPlayoffs\":true}} Receiver: \
             WotC.MtGO.Client.Model.Play.TournamentEvent.Tournament\n"
        );
        read_and_settle(&dir, &mut engine, now, &playoffs);
        assert_eq!(
            engine.tournaments.syncs,
            syncs + 3,
            "an InPlayoffs line syncs"
        );

        // Nothing new: no sync, no event.
        let events = read_and_settle(&dir, &mut engine, now, "");
        assert_eq!(engine.tournaments.syncs, syncs + 3);
        assert_eq!(tournaments_changed(&events), 0);
    }

    #[test]
    fn an_unregister_line_deletes_the_stored_tournament_and_tells_the_overlay() {
        let dir = TestDir::new("engine-tournament-unregister");
        let (config, mut engine, now) = league_engine(&dir, "");
        engine.tick(now).expect("tick");
        let lines = format!(
            "{}{}",
            tournament_state("12:00:40", JOINED, "PremierJoinedAwaitingMaxPlayersState"),
            tournament_round_one(JOINED, 3)
        );
        read_and_settle(&dir, &mut engine, now, &lines);
        assert_eq!(
            count(&config, "SELECT COUNT(*) FROM tournament_standings"),
            1
        );

        let unregister = tournament_state(
            "12:00:45",
            JOINED,
            "PremierNotJoinedAwaitingMaxPlayersState",
        );
        let events = read_and_settle(&dir, &mut engine, now, &unregister);
        assert_eq!(
            count(&config, "SELECT COUNT(*) FROM tournament_standings"),
            0
        );
        assert_eq!(tournaments_changed(&events), 1);
    }

    // 07 Q8: a round info line is league sync input (a tournament match's
    // `tournament_token` can be logged after the match), so reading one
    // syncs the file's account after the debounce.
    #[test]
    fn a_round_info_line_syncs_the_leagues() {
        let dir = TestDir::new("engine-round-info-sync");
        let (_config, mut engine, now) = league_engine(&dir, "");
        engine.tick(now).expect("tick");
        let syncs = engine.leagues.syncs;
        assert!(!engine.league_sync_pending());
        dir.append(
            "logs/mtgo.log",
            concat!(
                "12:00:40 [INF] (Game Management|Processing Registered Handler for ",
                "FlsTournamentRoundInfoMessage in TournamentNotJoinedFiredState) Processor: ",
                "TournamentNotJoinedFiredState Message: {\"Token\":\"bbbbbbbb-bbbb-bbbb-bbbb-",
                "bbbbbbbbbbbb\",\"ID\":12840476,\"Round\":{\"Number\":1}} Receiver: ",
                "WotC.MtGO.Client.Model.Play.TournamentEvent.Tournament\n",
                "12:00:50 [INF] (UI|x)\n"
            )
            .as_bytes(),
        );
        catch_up(&mut engine, now);
        assert!(
            engine.league_sync_pending(),
            "the round info marked the owner"
        );
        engine.tick(now + LEAGUE_SYNC_AFTER_MS).expect("tick");
        assert_eq!(engine.leagues.syncs, syncs + 1);
    }

    #[test]
    fn a_league_match_syncs_once_after_the_debounce_when_caught_up() {
        let dir = TestDir::new("engine-league-debounce");
        let (config, mut engine, now) = league_engine(&dir, "");
        engine.tick(now).expect("tick");
        let syncs = engine.leagues.syncs;
        engine.record_derived(
            "Q-league".to_string(),
            Some(Derived::Visible(league_view(
                "Q-league",
                Some("Q-token"),
                None,
            ))),
            1,
            None,
        );
        let tick = engine.tick(now).expect("tick");
        assert!(pushed_update(&tick.events, "Q-league"), "{:?}", tick.events);
        assert_eq!(leagues_changed(&tick.events), 0, "{:?}", tick.events);
        let tick = engine.tick(now + LEAGUE_SYNC_AFTER_MS - 1).expect("tick");
        assert_eq!(leagues_changed(&tick.events), 0, "{:?}", tick.events);
        assert_eq!(engine.leagues.syncs, syncs);

        let tick = engine.tick(now + LEAGUE_SYNC_AFTER_MS).expect("tick");
        assert_eq!(leagues_changed(&tick.events), 1, "{:?}", tick.events);
        assert_eq!(engine.leagues.syncs, syncs + 1);
        assert_eq!(
            count(
                &config,
                "SELECT COUNT(*) FROM matches WHERE token = 'Q-league' AND league_id IS NOT NULL"
            ),
            1,
            "committed before it was reported"
        );
        let tick = engine.tick(now + 5_000).expect("tick");
        assert_eq!(leagues_changed(&tick.events), 0, "{:?}", tick.events);
        assert_eq!(engine.leagues.syncs, syncs + 1, "synced once");
    }

    #[test]
    fn catching_up_a_backlog_syncs_once_at_the_end() {
        let fixture = load_fixture("draft_hobbit");
        // The log up to its login: the rest is read over several ticks.
        let login = line_start_before(&fixture.bytes, "MtGO Login Success");
        let cut = login
            + fixture.bytes[login..]
                .iter()
                .position(|byte| *byte == b'\n')
                .expect("a whole line")
            + 1;
        let dir = TestDir::new("engine-league-backlog");
        dir.write("logs/mtgo.log", &fixture.bytes[..cut]);
        let config = config(&dir, &fixture);
        let now = fixture.meta.now_ms();
        let mut engine = open(&config, now);
        run_until_settled(&mut engine, now);
        let syncs = engine.leagues.syncs;

        dir.append("logs/mtgo.log", &fixture.bytes[cut..]);
        let mut at = now + 10_000;
        let mut ticks = 0;
        loop {
            let tick = engine.tick(at).expect("tick");
            ticks += 1;
            if !tick.more {
                break;
            }
            assert_eq!(engine.leagues.syncs, syncs, "no sync while catching up");
            at += 1;
        }
        assert!(ticks > 1, "a backlog takes several ticks");
        let mut events = Vec::new();
        for _ in 0..3 {
            at += LEAGUE_SYNC_AFTER_MS;
            events.extend(engine.tick(at).expect("tick").events);
        }
        assert_eq!(engine.leagues.syncs, syncs + 1, "one sync at the end");
        assert_eq!(leagues_changed(&events), 1, "{events:?}");
        assert_eq!(limited_changed(&events), 1, "{events:?}");
        assert_eq!(count(&config, "SELECT COUNT(*) FROM drafts"), 1);
    }

    #[test]
    fn a_fresh_database_backfills_games_and_leagues_on_the_first_caught_up_tick() {
        let fixture = load_fixture("draft_hobbit");
        let dir = TestDir::new("engine-fresh-backfill");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let now = fixture.meta.now_ms();
        let mut engine = open(&config, now);
        assert!(engine.leagues.sync_all_wanted, "nothing folded yet");
        let mut last = loop {
            let tick = engine.tick(now).expect("tick");
            if !tick.more {
                break tick;
            }
            assert_eq!(engine.leagues.syncs, 0, "no sync while catching up");
        };
        // A caught-up tick that pushes a match leaves the sync to the next,
        // and the next syncs even when it pushes again (the game sync's
        // answers push the synced matches again).
        if last
            .events
            .iter()
            .any(|event| matches!(event, TrackerEvent::MatchUpdated { .. }))
        {
            assert_eq!(engine.leagues.syncs, 0);
            last = engine.tick(now).expect("tick");
        }
        assert_eq!(engine.leagues.syncs, 1);
        assert_eq!(leagues_changed(&last.events), 1, "{:?}", last.events);
        assert_eq!(limited_changed(&last.events), 1, "{:?}", last.events);
        assert!(last.timing.leagues_us > 0);
        engine.flush().expect("flush");
        assert_eq!(count(&config, "SELECT COUNT(*) FROM drafts"), 1);
        assert!(count(&config, "SELECT COUNT(*) FROM leagues") > 0);
        assert_eq!(
            value(
                &config,
                "SELECT version FROM derivations WHERE name = 'league_fold'"
            ),
            leagues::FOLD_VERSION
        );
        assert!(count(&config, "SELECT COUNT(*) FROM game_cards") > 0);
        let tick = engine.tick(now + 1).expect("tick");
        assert_eq!(engine.leagues.syncs, 1, "nothing left to sync");
        assert_eq!(leagues_changed(&tick.events), 0, "{:?}", tick.events);
    }

    static USE_TOKYO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// UTC until `USE_TOKYO` is set. Only `a_zone_change_refolds_everything` uses it.
    fn tokyo_later() -> (Tz, Option<String>) {
        if USE_TOKYO.load(std::sync::atomic::Ordering::SeqCst) {
            (chrono_tz::Asia::Tokyo, None)
        } else {
            (chrono_tz::UTC, None)
        }
    }

    #[test]
    fn a_zone_change_refolds_everything() {
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("engine-zone-refold");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let mut config = config(&dir, &fixture);
        config.os_timezone = Some(tokyo_later);
        let now = fixture.meta.now_ms();
        let mut engine = open(&config, now);
        run_until_settled(&mut engine, now);
        let zone = |config: &TrackerConfig| -> String {
            store::open_reader(&config.db_path)
                .expect("reader")
                .query_row(
                    "SELECT value FROM settings WHERE key = 'league_fold_timezone'",
                    [],
                    |row| row.get(0),
                )
                .expect("the fold's zone")
        };
        assert_eq!(zone(&config), "\"UTC\"");
        let syncs = engine.leagues.syncs;
        assert!(!engine.game_sync.backfilling_now());

        USE_TOKYO.store(true, std::sync::atomic::Ordering::SeqCst);
        let later = now + 60_000;
        engine.rescan(later).expect("rescan");
        assert!(engine.leagues.sync_all_wanted);
        assert!(engine.game_sync.backfilling_now(), "the game rows too");
        engine.tick(later).expect("tick");
        assert_eq!(engine.leagues.syncs, syncs + 1);
        engine.flush().expect("flush");
        assert_eq!(zone(&config), "\"Asia/Tokyo\"");
        assert!(!engine.game_sync.backfilling_now());
    }

    #[test]
    fn an_open_draft_is_abandoned_by_the_deadline_without_a_new_event() {
        let fixture = load_fixture("draft_hobbit");
        // Half way through the picks.
        let text = String::from_utf8_lossy(&fixture.bytes).to_string();
        let mut picks: Vec<usize> = Vec::new();
        let mut offset = 0;
        for line in text.split_inclusive('\n') {
            if line.contains("FlsBoosterDraftPendingPickMessage") {
                picks.push(offset);
            }
            offset += line.len();
        }
        let cut = picks[picks.len() / 2];
        let dir = TestDir::new("engine-draft-abandoned");
        dir.write("logs/mtgo.log", &fixture.bytes[..cut]);
        let config = config(&dir, &fixture);
        let time = &text[cut..cut + 8];
        let at = chrono::NaiveTime::parse_from_str(time, "%H:%M:%S").expect("a time");
        // The line's time is the log's local clock, so read it in the
        // fixture's timezone.
        use chrono::TimeZone;
        let now = fixture
            .meta
            .tz()
            .from_local_datetime(&fixture.meta.day_anchor().and_time(at))
            .earliest()
            .expect("a local time")
            .timestamp_millis()
            + 60_000;
        let mut engine = open(&config, now);
        run_until_settled(&mut engine, now);
        let state = |config: &TrackerConfig| -> (String, Option<i64>) {
            store::open_reader(&config.db_path)
                .expect("reader")
                .query_row("SELECT state, abandons_at FROM drafts", [], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .expect("the draft")
        };
        let (open_state, deadline) = state(&config);
        assert_eq!(open_state, "picking");
        let deadline = deadline.expect("an open draft's deadline");
        assert_eq!(engine.leagues.next_abandon_ms, Some(deadline));
        let syncs = engine.leagues.syncs;

        let tick = engine.tick(deadline).expect("tick");
        assert_eq!(engine.leagues.syncs, syncs, "still open at its deadline");
        assert_eq!(limited_changed(&tick.events), 0, "{:?}", tick.events);
        let tick = engine.tick(deadline + 1).expect("tick");
        assert_eq!(engine.leagues.syncs, syncs + 1);
        assert_eq!(limited_changed(&tick.events), 1, "{:?}", tick.events);
        assert_eq!(state(&config), ("abandoned".to_string(), None));
        assert_eq!(engine.leagues.next_abandon_ms, None);
    }

    /// A setting's stored value, `None` when there is no such row.
    fn setting(config: &TrackerConfig, key: &str) -> Option<String> {
        store::read::settings(&store::open_reader(&config.db_path).expect("reader"))
            .expect("settings")
            .into_iter()
            .find(|(stored, _)| stored == key)
            .map(|(_, value)| value)
    }

    fn q_league_in_run(config: &TrackerConfig) -> i64 {
        count(
            config,
            "SELECT COUNT(*) FROM matches WHERE token = 'Q-league' AND league_id IS NOT NULL",
        )
    }

    fn derive_league_match(engine: &mut Engine, token: &str) {
        engine.record_derived(
            token.to_string(),
            Some(Derived::Visible(league_view(token, Some("Q-token"), None))),
            1,
            None,
        );
    }

    #[test]
    fn back_to_back_ticks_that_push_matches_postpone_the_league_sync_once() {
        let dir = TestDir::new("engine-league-sync-bursts");
        let (_config, mut engine, now) = league_engine(&dir, "");
        engine.tick(now).expect("tick");
        let syncs = engine.leagues.syncs;
        derive_league_match(&mut engine, "Q-a");
        engine.tick(now).expect("tick");
        derive_league_match(&mut engine, "Q-b");
        engine.tick(now + LEAGUE_SYNC_AFTER_MS).expect("tick");
        assert_eq!(engine.leagues.syncs, syncs, "due, but this tick pushes");
        derive_league_match(&mut engine, "Q-c");
        engine.tick(now + 2 * LEAGUE_SYNC_AFTER_MS).expect("tick");
        assert_eq!(engine.leagues.syncs, syncs + 1, "one tick of delay at most");
    }

    #[test]
    fn a_pick_note_edit_does_not_run_a_due_full_resync() {
        use store::limited::DraftNoteEdit;
        let dir = TestDir::new("engine-pick-note-no-full-sync");
        let (config, mut engine, now) = league_engine(&dir, "");
        engine.tick(now).expect("tick");
        let note = DraftNoteEdit::Set {
            draft_token: "Q-draft".to_string(),
            ordinal: 1,
            note: "Q".to_string(),
            now_ms: now,
        };
        engine.refold_for_new_zone();
        let syncs = engine.leagues.syncs;
        engine.edit_draft_note(note.clone()).expect("edit");
        assert_eq!(engine.leagues.syncs, syncs, "nothing marked: no sync");
        assert!(engine.leagues.sync_all_wanted, "the full sync still waits");

        // A marked owner is still synced first, so its picks are found.
        derive_league_match(&mut engine, "Q-league");
        engine.edit_draft_note(note).expect("edit");
        assert_eq!(engine.leagues.syncs, syncs + 1);
        assert_eq!(q_league_in_run(&config), 1);
        assert!(engine.leagues.sync_all_wanted, "the full sync still waits");
    }

    #[test]
    fn a_pending_league_sync_runs_when_the_tracker_stops() {
        let dir = TestDir::new("engine-league-sync-at-stop");
        let (config, mut engine, now) = league_engine(&dir, "");
        engine.tick(now).expect("tick");
        derive_league_match(&mut engine, "Q-league");
        engine.tick(now).expect("tick");
        assert_eq!(q_league_in_run(&config), 0, "before its deadline");
        engine.finish().expect("stop");
        assert_eq!(q_league_in_run(&config), 1, "synced while stopping");
        assert_eq!(
            setting(&config, LEAGUE_SYNC_PENDING).as_deref(),
            Some("false")
        );
    }

    #[test]
    fn a_league_sync_before_an_edit_leaves_no_stale_pending_marker() {
        let dir = TestDir::new("engine-league-sync-marker-order");
        let (config, mut engine, now) = league_engine(&dir, "");
        engine.tick(now).expect("tick");
        engine.flush().expect("flush");
        // An edit's mark between ticks, then the sync the next edit runs
        // first: the mark's "true" is still in the unsent batch.
        engine.mark_every_owner();
        engine.sync_pending_leagues().expect("sync before the edit");
        engine.flush().expect("flush");
        assert_ne!(
            setting(&config, LEAGUE_SYNC_PENDING).as_deref(),
            Some("true"),
            "nothing is pending after the sync"
        );
    }

    #[test]
    fn a_pending_league_sync_survives_a_crash() {
        let dir = TestDir::new("engine-league-sync-after-crash");
        let (config, mut engine, now) = league_engine(&dir, "");
        engine.tick(now).expect("tick");
        derive_league_match(&mut engine, "Q-league");
        engine.tick(now).expect("tick");
        // Gone before the deadline, without stopping: only the batch commits.
        drop(engine);
        assert_eq!(q_league_in_run(&config), 0);
        assert_eq!(
            setting(&config, LEAGUE_SYNC_PENDING).as_deref(),
            Some("true")
        );

        let mut engine = open(&config, now + 60_000);
        run_until_settled(&mut engine, now + 60_000);
        assert_eq!(q_league_in_run(&config), 1, "synced after the reopen");
        assert_eq!(
            setting(&config, LEAGUE_SYNC_PENDING).as_deref(),
            Some("false")
        );
    }

    #[test]
    fn pending_game_syncs_survive_a_crash() {
        let fixture = load_fixture("draft_hobbit");
        let login = line_start_before(&fixture.bytes, "MtGO Login Success");
        let cut = login
            + fixture.bytes[login..]
                .iter()
                .position(|byte| *byte == b'\n')
                .expect("a whole line")
            + 1;
        let dir = TestDir::new("engine-game-sync-after-crash");
        dir.write("logs/mtgo.log", &fixture.bytes[..cut]);
        let config = config(&dir, &fixture);
        let now = fixture.meta.now_ms();
        let mut engine = open(&config, now);
        run_until_settled(&mut engine, now);
        drop(engine);

        // One tick of a backlog, then gone: its games were never synced.
        dir.append("logs/mtgo.log", &fixture.bytes[cut..]);
        let mut engine = open(&config, now);
        assert!(!engine.game_sync.backfilling_now());
        assert!(engine.tick(now).expect("tick").more, "a backlog");
        drop(engine);
        assert_eq!(setting(&config, GAME_SYNC_PENDING).as_deref(), Some("true"));

        let mut engine = open(&config, now);
        assert!(engine.game_sync.backfilling_now(), "the lost syncs");
        run_until_settled(&mut engine, now);
        assert!(count(&config, "SELECT COUNT(*) FROM game_cards") > 0);
        assert_eq!(
            setting(&config, GAME_SYNC_PENDING).as_deref(),
            Some("false")
        );
    }

    #[test]
    fn a_failed_game_sync_keeps_its_pending_marker() {
        let fixture = load_fixture("draft_hobbit");
        let login = line_start_before(&fixture.bytes, "MtGO Login Success");
        let cut = login
            + fixture.bytes[login..]
                .iter()
                .position(|byte| *byte == b'\n')
                .expect("a whole line")
            + 1;
        let dir = TestDir::new("engine-game-sync-failed-marker");
        dir.write("logs/mtgo.log", &fixture.bytes[..cut]);
        let config = config(&dir, &fixture);
        let now = fixture.meta.now_ms();
        let mut engine = open(&config, now);
        run_until_settled(&mut engine, now);
        drop(engine);

        let rename = |from: &str, to: &str| {
            store::open_writer(&config.db_path)
                .expect("writer")
                .execute_batch(&format!("ALTER TABLE {from} RENAME TO {to};"))
                .expect("rename");
        };
        rename("game_cards", "q_game_cards");
        dir.append("logs/mtgo.log", &fixture.bytes[cut..]);
        let mut engine = open(&config, now);
        run_until_settled(&mut engine, now);
        assert_eq!(
            setting(&config, GAME_SYNC_PENDING).as_deref(),
            Some("true"),
            "the failed sync is not forgotten"
        );
        drop(engine);

        rename("q_game_cards", "game_cards");
        let mut engine = open(&config, now);
        assert!(engine.game_sync.backfilling_now(), "synced again at start");
        run_until_settled(&mut engine, now);
        assert!(count(&config, "SELECT COUNT(*) FROM game_cards") > 0);
        assert_eq!(
            setting(&config, GAME_SYNC_PENDING).as_deref(),
            Some("false")
        );
    }

    #[test]
    fn a_due_league_sync_waits_for_a_tick_without_a_match_push() {
        let dir = TestDir::new("engine-league-sync-after-push");
        let (_config, mut engine, now) = league_engine(&dir, "");
        engine.tick(now).expect("tick");
        derive_league_match(&mut engine, "Q-league");
        engine.tick(now).expect("tick");
        let syncs = engine.leagues.syncs;

        // The deadline tick also derives a match: it is pushed at once.
        derive_league_match(&mut engine, "Q-other");
        let tick = engine.tick(now + LEAGUE_SYNC_AFTER_MS).expect("tick");
        assert!(pushed_update(&tick.events, "Q-other"), "{:?}", tick.events);
        assert_eq!(engine.leagues.syncs, syncs, "the sync waits a tick");
        assert_eq!(leagues_changed(&tick.events), 0, "{:?}", tick.events);
        let tick = engine.tick(now + LEAGUE_SYNC_AFTER_MS + 1).expect("tick");
        assert_eq!(engine.leagues.syncs, syncs + 1);
        assert_eq!(leagues_changed(&tick.events), 1, "{:?}", tick.events);
    }

    #[test]
    fn a_failed_league_sync_is_retried_with_a_backoff() {
        let dir = TestDir::new("engine-league-sync-retry");
        let (config, mut engine, now) = league_engine(&dir, "");
        engine.tick(now).expect("tick");
        let rename = |from: &str, to: &str| {
            store::open_writer(&config.db_path)
                .expect("writer")
                .execute_batch(&format!("ALTER TABLE {from} RENAME TO {to};"))
                .expect("rename");
        };
        rename("drafts", "q_drafts");
        derive_league_match(&mut engine, "Q-league");
        // Pushed first: the sync never runs on that tick.
        engine.tick(now).expect("tick");
        let syncs = engine.leagues.syncs;
        engine.tick(now + LEAGUE_SYNC_AFTER_MS).expect("tick");
        assert_eq!(engine.leagues.syncs, syncs + 1, "tried and failed");
        let retry_at = now + LEAGUE_SYNC_AFTER_MS + LEAGUE_RETRY_FIRST_MS;
        assert_eq!(engine.leagues.league_sync_due_ms, Some(retry_at));

        engine.tick(retry_at - 1).expect("tick");
        assert_eq!(engine.leagues.syncs, syncs + 1, "waits for the backoff");
        engine.tick(retry_at).expect("tick");
        assert_eq!(engine.leagues.syncs, syncs + 2, "failed again");
        assert_eq!(
            engine.leagues.league_sync_due_ms,
            Some(retry_at + 2 * LEAGUE_RETRY_FIRST_MS),
            "the backoff doubles"
        );

        rename("q_drafts", "drafts");
        engine
            .tick(retry_at + 2 * LEAGUE_RETRY_FIRST_MS)
            .expect("tick");
        assert_eq!(engine.leagues.syncs, syncs + 3);
        assert_eq!(q_league_in_run(&config), 1);
        assert_eq!(engine.leagues.league_sync_due_ms, None);
    }

    #[test]
    fn a_zone_change_while_closed_refolds_leagues_and_games() {
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("engine-zone-change-closed");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let mut config = config(&dir, &fixture);
        let now = fixture.meta.now_ms();
        let mut engine = open(&config, now);
        run_until_settled(&mut engine, now);
        drop(engine);

        config.timezone = chrono_tz::Asia::Tokyo;
        let engine = open(&config, now);
        assert!(engine.leagues.sync_all_wanted);
        assert!(engine.game_sync.backfilling_now(), "the game rows too");
    }

    // Match edits (Plan K: notes and "Remove from stats")

    fn hide_edit(token: &str, now_ms: i64) -> MatchEdit {
        MatchEdit::Hide {
            token: token.to_string(),
            now_ms,
        }
    }

    fn pushed_update(events: &[TrackerEvent], token: &str) -> bool {
        events.iter().any(|event| match event {
            TrackerEvent::MatchUpdated { view, .. } | TrackerEvent::MatchPreview { view, .. } => {
                view.token == token
            }
            _ => false,
        })
    }

    fn decks_changed(events: &[TrackerEvent]) -> usize {
        events
            .iter()
            .filter(|event| matches!(event, TrackerEvent::DecksChanged))
            .count()
    }

    #[test]
    fn hiding_a_match_reports_its_removal_and_refreshes_decks_and_leagues() {
        let dir = TestDir::new("engine-hide");
        let (config, mut engine, now) = league_engine(&dir, "");
        let league = || {
            Some(Derived::Visible(league_view(
                "Q-league",
                Some("Q-token"),
                None,
            )))
        };
        engine.record_derived("Q-league".to_string(), league(), 1, None);
        engine.tick(now).expect("tick");

        let done = engine.edit_match(hide_edit("Q-league", now)).expect("edit");
        assert_eq!(done, MatchEditOutcome::Done);
        let tick = engine.tick(now + 1).expect("tick");
        let removed = TrackerEvent::MatchRemoved {
            token: "Q-league".to_string(),
        };
        assert!(tick.events.contains(&removed), "{:?}", tick.events);
        assert_eq!(decks_changed(&tick.events), 1, "{:?}", tick.events);
        // The leagues are told once, after the league sync (the derive's
        // sync was left to this tick, so it covers the hide too).
        let mut reported = leagues_changed(&tick.events);
        let tick = engine.tick(now + LEAGUE_SYNC_AFTER_MS).expect("tick");
        reported += leagues_changed(&tick.events);
        assert_eq!(reported, 1, "{:?}", tick.events);
        assert_eq!(
            count(
                &config,
                "SELECT COUNT(*) FROM matches WHERE hidden_at IS NOT NULL"
            ),
            1
        );

        // A later result of the hidden match is stored, never pushed.
        engine.record_derived("Q-league".to_string(), league(), 2, None);
        let tick = engine.tick(now + 2).expect("tick");
        assert!(
            !pushed_update(&tick.events, "Q-league"),
            "{:?}",
            tick.events
        );
        // Its league report waits for the sync: wait for the writer here.
        engine.flush().expect("flush");
        assert_eq!(
            count(
                &config,
                "SELECT revision FROM matches WHERE token = 'Q-league'"
            ),
            2
        );
        assert_eq!(
            count(
                &config,
                "SELECT COUNT(*) FROM matches WHERE hidden_at IS NOT NULL"
            ),
            1
        );
    }

    #[test]
    fn a_live_match_taking_over_a_hidden_copy_stays_hidden_and_unpushed() {
        let dir = TestDir::new("engine-takeover-hidden");
        let (config, mut engine, now) = league_engine(&dir, "");
        // A 0.x copy of Match ID 99 under another token, removed from stats.
        engine
            .import(vec![imported("Q-copy", 99)], now)
            .expect("import");
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute(
                "UPDATE matches SET hidden_at = 1 WHERE token = 'Q-copy'",
                [],
            )
            .expect("hide the copy");
        engine.tick(now).expect("tick");

        // The live match (Match ID 99) takes the copy over.
        engine.record_derived(
            "Q-live".to_string(),
            Some(Derived::Visible(league_view("Q-live", None, None))),
            1,
            None,
        );
        let tick = engine.tick(now + 1).expect("tick");
        assert!(!pushed_update(&tick.events, "Q-live"), "{:?}", tick.events);
        // A tick only waits for the writer when it tells a page: wait here.
        engine.flush().expect("flush");
        assert_eq!(
            count(
                &config,
                "SELECT COUNT(*) FROM matches WHERE token = 'Q-live' AND hidden_at = 1"
            ),
            1
        );
        // A later result is not pushed either.
        engine.record_derived(
            "Q-live".to_string(),
            Some(Derived::Visible(league_view("Q-live", None, None))),
            2,
            None,
        );
        let tick = engine.tick(now + 2).expect("tick");
        assert!(!pushed_update(&tick.events, "Q-live"), "{:?}", tick.events);
    }

    fn manual_match(placement: ManualPlacement) -> ManualEdit {
        ManualEdit::Create(ManualMatch {
            deck_id: 1,
            opponent: "Qkai".to_string(),
            started_at_ms: 5_000,
            ended_at_ms: 125_000,
            games: vec![
                ManualGame {
                    won: true,
                    on_play: true,
                    turns: None,
                },
                ManualGame {
                    won: true,
                    on_play: false,
                    turns: Some(7),
                },
            ],
            placement,
            opponent_archetype_id: None,
            ranked: false,
            best_of: store::BestOf::Three,
            rank_gap: None,
            now_ms: 6_000,
        })
    }

    fn restore_edit(token: &str, now_ms: i64) -> MatchEdit {
        MatchEdit::Restore {
            token: token.to_string(),
            now_ms,
        }
    }

    // League self-management spec 4.6: a restore takes the match out of the
    // hidden set, so its later results are pushed again.
    #[test]
    fn restoring_a_hidden_match_shows_it_again() {
        let dir = TestDir::new("engine-restore");
        let (config, mut engine, now) = league_engine(&dir, "");
        let plain = || Some(Derived::Visible(league_view("Q-plain", None, None)));
        engine.record_derived("Q-plain".to_string(), plain(), 1, None);
        engine.tick(now).expect("tick");
        let hidden = engine.edit_match(hide_edit("Q-plain", now)).expect("hide");
        assert_eq!(hidden, MatchEditOutcome::Done);
        engine.tick(now + 1).expect("tick");

        let restored = engine
            .edit_match(restore_edit("Q-plain", now + 2))
            .expect("restore");
        assert_eq!(restored, MatchEditOutcome::Done);
        assert!(!engine.hidden.contains("Q-plain"));
        let tick = engine.tick(now + 3).expect("tick");
        assert_eq!(decks_changed(&tick.events), 1, "{:?}", tick.events);
        assert_eq!(
            count(
                &config,
                "SELECT COUNT(*) FROM matches WHERE hidden_at IS NOT NULL"
            ),
            0
        );

        engine.record_derived("Q-plain".to_string(), plain(), 2, None);
        let tick = engine.tick(now + 4).expect("tick");
        assert_eq!(
            pushed_revisions(&tick.events, "Q-plain"),
            vec![2],
            "{:?}",
            tick.events
        );
    }

    // #454: every push of a projected match shows MTGO's verdict over the
    // derived outcome (a restore, a cleared preview, a game sync's push).
    #[test]
    fn a_projected_match_is_pushed_with_mtgo_s_verdict() {
        let dir = TestDir::new("engine-push-verdict");
        let (_config, mut engine, _now) = league_engine(&dir, "");
        engine
            .history_check
            .verdicts
            .insert("Q-left".to_string(), mtgo_match::Outcome::Win);
        let mut draw = *league_view("Q-left", None, None);
        draw.outcome = mtgo_match::Outcome::Draw;
        engine.push_projected(draw.clone(), 4);
        let mut other = draw;
        other.token = "Q-other".to_string();
        engine.push_projected(other, 2);

        let pushed: Vec<(String, mtgo_match::Outcome, u64, bool)> = engine
            .pending_events
            .iter()
            .filter_map(|event| match event {
                TrackerEvent::MatchUpdated {
                    view,
                    revision,
                    cause: None,
                    manual,
                    ..
                } => Some((view.token.clone(), view.outcome, *revision, *manual)),
                _ => None,
            })
            .collect();
        assert_eq!(
            pushed,
            vec![
                ("Q-left".to_string(), mtgo_match::Outcome::Win, 4, false),
                ("Q-other".to_string(), mtgo_match::Outcome::Draw, 2, false),
            ]
        );
    }

    #[test]
    fn restoring_a_manual_match_pushes_it_as_manual() {
        let dir = TestDir::new("engine-restore-manual");
        let (config, mut engine, now) = league_engine(&dir, "");
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute_batch(
                "INSERT INTO decks (id, mtgo_id, account_id, name, format, created_at,
                     updated_at, source)
                 VALUES (1, 'Q-deck', (SELECT id FROM accounts WHERE username = 'Qlocal'),
                     'Qdeck', 'CMODERN', 0, 0, 'file');
                 INSERT INTO deck_versions (id, deck_id, signature, modified_at)
                 VALUES (10, 1, 'Q-sig', 1);",
            )
            .expect("a deck");
        engine.tick(now).expect("tick");
        let created = engine
            .edit_manual(manual_match(ManualPlacement::None))
            .expect("edit");
        let ManualEditOutcome::Created { token } = created else {
            panic!("not created: {created:?}");
        };
        engine.tick(now + 1).expect("tick");
        let hidden = engine.edit_match(hide_edit(&token, now + 2)).expect("hide");
        assert_eq!(hidden, MatchEditOutcome::Done);
        engine.tick(now + 3).expect("tick");

        let restored = engine
            .edit_match(restore_edit(&token, now + 4))
            .expect("restore");
        assert_eq!(restored, MatchEditOutcome::Done);
        let tick = engine.tick(now + 5).expect("tick");
        let pushed_manual = tick.events.iter().any(|event| {
            matches!(
                event,
                TrackerEvent::MatchUpdated { view, manual: true, .. } if view.token == token
            )
        });
        assert!(pushed_manual, "{:?}", tick.events);
    }

    #[test]
    fn a_manual_arena_match_is_pushed_on_arena_with_its_opponent_name() {
        let dir = TestDir::new("engine-manual-arena-push");
        let (config, mut engine, now) = league_engine(&dir, "");
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute_batch(
                "INSERT INTO accounts (id, username, first_seen_at, platform, arena_user_id)
                 VALUES (90, 'Qarena#12345', 0, 'arena', 'Qarena-id');
                 INSERT INTO decks (id, mtgo_id, account_id, name, format, created_at,
                     updated_at, source)
                 VALUES (1, 'Q-deck', 90, 'Qdeck', 'CSTANDARD', 0, 0, 'file');
                 INSERT INTO deck_versions (id, deck_id, signature, modified_at)
                 VALUES (10, 1, 'Q-sig', 1);
                 INSERT INTO arena_log_files (id, path, identity_hash, head_hash, head_len,
                     first_seen_at) VALUES (1, '/arena/Player.log', 1, 2, 3, 4);
                 INSERT INTO arena_rank_observations (account_id, ladder, season, observed_at,
                     class, level, step, matches_won, matches_lost, arena_log_file_id,
                     byte_offset)
                 VALUES (90, 'constructed', 95, 500, 'Gold', 3, 0, 0, 0, 1, 50),
                        (90, 'constructed', 95, 1000, 'Gold', 2, 0, 1, 0, 1, 100);",
            )
            .expect("an Arena deck");
        engine.tick(now).expect("tick");
        let ManualEdit::Create(mut manual) = manual_match(ManualPlacement::None) else {
            panic!("a create");
        };
        manual.opponent = "Qrival".to_string();
        manual.ranked = true;
        manual.rank_gap = Some((1, 100));
        let created = engine
            .edit_manual(ManualEdit::Create(manual))
            .expect("edit");
        let ManualEditOutcome::Created { token } = created else {
            panic!("not created: {created:?}");
        };
        let tick = engine.tick(now + 1).expect("tick");
        let pushed = tick.events.iter().find_map(|event| match event {
            TrackerEvent::MatchUpdated {
                view,
                manual: true,
                platform,
                opponent_name,
                ..
            } if view.token == token => Some((*platform, opponent_name.clone())),
            _ => None,
        });
        assert_eq!(
            pushed,
            Some((Platform::Arena, Some("Qrival".to_string()))),
            "{:?}",
            tick.events
        );
        assert!(
            tick.events
                .iter()
                .any(|event| matches!(event, TrackerEvent::RankChanged)),
            "a claimed gap moves the rank views: {:?}",
            tick.events
        );
    }

    #[test]
    fn a_corrected_arena_match_is_pushed_on_arena_with_its_new_opponent_name() {
        use store::manual::{ManualGame, MatchCorrection};
        let dir = TestDir::new("engine-manual-arena-correct-push");
        let (config, mut engine, now) = league_engine(&dir, "");
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute_batch(
                "INSERT INTO accounts (id, username, first_seen_at, platform, arena_user_id)
                 VALUES (90, 'Qarena#12345', 0, 'arena', 'Qarena-id');
                 INSERT INTO decks (id, mtgo_id, account_id, name, format, created_at,
                     updated_at, source)
                 VALUES (1, 'Q-deck', 90, 'Qdeck', 'CSTANDARD', 0, 0, 'file');
                 INSERT INTO deck_versions (id, deck_id, signature, modified_at)
                 VALUES (10, 1, 'Q-sig', 1);",
            )
            .expect("an Arena deck");
        engine.tick(now).expect("tick");
        let ManualEdit::Create(mut manual) = manual_match(ManualPlacement::None) else {
            panic!("a create");
        };
        manual.opponent = "Qrival".to_string();
        manual.ranked = false;
        let created = engine
            .edit_manual(ManualEdit::Create(manual))
            .expect("edit");
        let ManualEditOutcome::Created { token } = created else {
            panic!("not created: {created:?}");
        };
        engine.tick(now + 1).expect("tick");
        let match_id: i64 = engine
            .reader
            .query_row("SELECT id FROM matches WHERE token = ?1", [&token], |row| {
                row.get(0)
            })
            .expect("the match");
        let correct = ManualEdit::Correct {
            match_id,
            correction: MatchCorrection {
                opponent_name: Some("Qother".to_string()),
                started_at_ms: None,
                ended_at_ms: None,
                games: vec![ManualGame {
                    won: true,
                    on_play: true,
                    turns: None,
                }],
                now_ms: now + 2,
            },
        };
        assert_eq!(
            engine.edit_manual(correct).expect("edit"),
            ManualEditOutcome::Done
        );
        let tick = engine.tick(now + 2).expect("tick");
        let pushed = tick.events.iter().find_map(|event| match event {
            TrackerEvent::MatchUpdated {
                view,
                manual: true,
                platform,
                opponent_name,
                ..
            } if view.token == token => Some((*platform, opponent_name.clone())),
            _ => None,
        });
        assert_eq!(
            pushed,
            Some((Platform::Arena, Some("Qother".to_string()))),
            "{:?}",
            tick.events
        );
    }

    // League self-management spec 4.4 and 4.5: deleting a league with its
    // matches hides them, and its Undo shows them again.
    #[test]
    fn deleting_a_league_with_its_matches_and_undoing_it_follow_the_hidden_marks() {
        use store::leagues::{DeleteMatches, LeagueEdit, LeagueEditOutcome};
        let dir = TestDir::new("engine-delete-league-hides");
        let (config, mut engine, now) = league_engine(&dir, "");
        let plain = || Some(Derived::Visible(league_view("Q-plain", None, None)));
        engine.record_derived("Q-plain".to_string(), plain(), 1, None);
        engine.tick(now).expect("tick");
        engine.flush().expect("flush");
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute_batch(
                "INSERT INTO leagues (id, run_key, account_id, source, kind, state, name,
                     started_at, created_at, token)
                 VALUES (5, 'manual:5', (SELECT id FROM accounts WHERE username = 'Qlocal'),
                     'manual', 'constructed', 'complete', 'Qleague', 1, 1, 'manual_Q5');
                 UPDATE matches SET league_id = 5, league_linked = 1 WHERE token = 'Q-plain';",
            )
            .expect("a manual league holding the match");

        let deleted = engine
            .edit_league(LeagueEdit::Delete {
                run_key: "manual:5".to_string(),
                matches: DeleteMatches::DeleteToo,
                now_ms: now + 1,
            })
            .expect("delete");
        assert_eq!(deleted, LeagueEditOutcome::Done);
        assert!(engine.hidden.contains("Q-plain"));
        let tick = engine.tick(now + 2).expect("tick");
        let removed = TrackerEvent::MatchRemoved {
            token: "Q-plain".to_string(),
        };
        assert!(tick.events.contains(&removed), "{:?}", tick.events);

        let undone = engine
            .edit_league(LeagueEdit::UndoDelete {
                league_id: 5,
                removed_at: now + 1,
                now_ms: now + 3,
            })
            .expect("undo");
        assert_eq!(undone, LeagueEditOutcome::Done);
        assert!(!engine.hidden.contains("Q-plain"));
        engine.record_derived("Q-plain".to_string(), plain(), 2, None);
        let tick = engine.tick(now + 4).expect("tick");
        assert_eq!(
            pushed_revisions(&tick.events, "Q-plain"),
            vec![2],
            "{:?}",
            tick.events
        );
    }

    // League self-management spec 4.6 and 6.3: a pull reads the pulled
    // row's hidden mark again, either way. The mark is written by hand
    // here, as the player-field step leaves it, so the test is about the
    // engine alone.
    #[test]
    fn a_pulled_hidden_mark_and_its_restore_follow_the_row() {
        let dir = TestDir::new("engine-pulled-hidden");
        let (config, mut engine, now) = league_engine(&dir, "");
        let pull = |now_ms: i64| store::sync::SyncEdit::PulledMatch {
            client_id: "q-client".to_string(),
            pulled: Box::new(store::import::PulledMatch {
                platform: Default::default(),
                sources: None,
                local_player_fields: Default::default(),
                opponent_player_fields: Default::default(),
                endstep_ranked: None,
                endstep_manual_cards: Vec::new(),
                game_structure: None,
                imported: imported("Q-pulled", 603),
                detail: store::imported_detail::ImportedDetail::default(),
                deck: None,
                league_client_id: None,
                history: false,
                archetypes: Vec::new(),
                player: Default::default(),
            }),
            hash: "q-hash".to_string(),
            carry: String::new(),
            timezone: chrono_tz::UTC,
            now_ms,
        };
        engine.edit_sync(pull(now)).expect("pulled");
        assert!(!engine.hidden.contains("Q-pulled"));

        let set_hidden = |sql: &str| {
            store::open_writer(&config.db_path)
                .expect("writer")
                .execute(sql, [])
                .expect("the pulled mark");
        };
        set_hidden("UPDATE matches SET hidden_at = 70, edited_at = 70 WHERE token = 'Q-pulled'");
        engine.edit_sync(pull(now + 1)).expect("pulled again");
        assert!(engine.hidden.contains("Q-pulled"));
        let tick = engine.tick(now + 2).expect("tick");
        let removed = TrackerEvent::MatchRemoved {
            token: "Q-pulled".to_string(),
        };
        assert!(tick.events.contains(&removed), "{:?}", tick.events);

        set_hidden("UPDATE matches SET hidden_at = NULL, edited_at = 80 WHERE token = 'Q-pulled'");
        engine.edit_sync(pull(now + 3)).expect("pulled restored");
        assert!(!engine.hidden.contains("Q-pulled"));
    }

    // Final review finding 4: a pulled edit lands on the live row found by
    // its MTGO match id, under another token than the pulled one; the
    // engine reads every hidden mark again, so that live match is hidden.
    #[test]
    fn a_pulled_hide_of_a_live_match_under_another_token_hides_it() {
        let dir = TestDir::new("engine-pulled-hidden-live");
        let (_config, mut engine, now) = league_engine(&dir, "");
        let live = || Some(Derived::Visible(league_view("Q-live", None, None)));
        engine.record_derived("Q-live".to_string(), live(), 1, None);
        engine.tick(now).expect("tick");
        engine.flush().expect("flush");

        let mut pulled = store::import::PulledMatch {
            platform: Default::default(),
            sources: None,
            local_player_fields: Default::default(),
            opponent_player_fields: Default::default(),
            endstep_ranked: None,
            endstep_manual_cards: Vec::new(),
            game_structure: None,
            imported: imported("Q-pulled", 99),
            detail: store::imported_detail::ImportedDetail::default(),
            deck: None,
            league_client_id: None,
            history: false,
            archetypes: Vec::new(),
            player: Default::default(),
        };
        pulled.player.hidden_at_ms = Some(now + 1);
        pulled.player.edited_at_ms = Some(now + 1);
        let changed = engine
            .edit_sync(store::sync::SyncEdit::PulledMatch {
                client_id: "Q-pulled".to_string(),
                pulled: Box::new(pulled),
                hash: "q-hash".to_string(),
                carry: String::new(),
                timezone: chrono_tz::UTC,
                now_ms: now + 1,
            })
            .expect("pulled");
        assert!(changed > 0);
        assert!(engine.hidden.contains("Q-live"));
        let tick = engine.tick(now + 2).expect("tick");
        let removed = TrackerEvent::MatchRemoved {
            token: "Q-live".to_string(),
        };
        assert!(tick.events.contains(&removed), "{:?}", tick.events);
    }

    // League self-management spec 6.4: a match deleted on the website
    // (Task 11's tombstones) leaves the engine's set of shown matches.
    #[test]
    fn a_website_deletion_hides_a_live_match() {
        let dir = TestDir::new("engine-tombstone");
        let (_config, mut engine, now) = league_engine(&dir, "");
        let plain = || Some(Derived::Visible(league_view("Q-plain", None, None)));
        engine.record_derived("Q-plain".to_string(), plain(), 1, None);
        engine.tick(now).expect("tick");
        engine.flush().expect("flush");

        let hidden = engine
            .edit_sync(store::sync::SyncEdit::Tombstones {
                client_ids: vec!["Q-plain".to_string()],
                since_ms: None,
                now_ms: now + 1,
            })
            .expect("tombstones");
        assert_eq!(hidden, 1);
        assert!(engine.hidden.contains("Q-plain"));
        let tick = engine.tick(now + 2).expect("tick");
        let removed = TrackerEvent::MatchRemoved {
            token: "Q-plain".to_string(),
        };
        assert!(tick.events.contains(&removed), "{:?}", tick.events);
    }

    /// The revision of every `MatchUpdated` pushed for `token`.
    fn pushed_revisions(events: &[TrackerEvent], token: &str) -> Vec<u64> {
        events
            .iter()
            .filter_map(|event| match event {
                TrackerEvent::MatchUpdated { view, revision, .. } if view.token == token => {
                    Some(*revision)
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_correction_pushes_a_manual_match_and_refreshes_decks_and_leagues() {
        use store::manual::{ManualGame, MatchCorrection};
        let dir = TestDir::new("engine-manual-correct");
        let (config, mut engine, now) = league_engine(&dir, "");
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute_batch(
                "INSERT INTO decks (id, mtgo_id, account_id, name, format, created_at,
                     updated_at, source)
                 VALUES (1, 'Q-deck', (SELECT id FROM accounts WHERE username = 'Qlocal'),
                     'Qdeck', 'CMODERN', 0, 0, 'file');
                 INSERT INTO deck_versions (id, deck_id, signature, modified_at)
                 VALUES (10, 1, 'Q-sig', 1);",
            )
            .expect("a deck");
        engine.tick(now).expect("tick");
        let created = engine
            .edit_manual(manual_match(ManualPlacement::None))
            .expect("edit");
        let ManualEditOutcome::Created { token } = created else {
            panic!("not created: {created:?}");
        };
        engine.tick(now + 1).expect("tick");
        let match_id: i64 = engine
            .reader
            .query_row("SELECT id FROM matches WHERE token = ?1", [&token], |row| {
                row.get(0)
            })
            .expect("the match");

        let loss = |on_play| ManualGame {
            won: false,
            on_play,
            turns: None,
        };
        let correct = |match_id| ManualEdit::Correct {
            match_id,
            correction: MatchCorrection {
                opponent_name: Some("Qrival".to_string()),
                started_at_ms: Some(1_000),
                ended_at_ms: Some(2_000),
                games: vec![loss(true), loss(false)],
                now_ms: now + 2,
            },
        };
        assert_eq!(
            engine.edit_manual(correct(match_id)).expect("edit"),
            ManualEditOutcome::Done
        );
        let tick = engine.tick(now + 2).expect("tick");
        assert_eq!(pushed_revisions(&tick.events, &token), vec![2]);
        let view = tick
            .events
            .iter()
            .find_map(|event| match event {
                TrackerEvent::MatchUpdated { view, .. } => Some(view.clone()),
                _ => None,
            })
            .expect("pushed");
        assert_eq!(view.opponent.as_deref(), Some("Qrival"));
        assert_eq!((view.wins, view.losses), (0, 2));
        assert_eq!(decks_changed(&tick.events), 1, "{:?}", tick.events);
        assert_eq!(leagues_changed(&tick.events), 1, "{:?}", tick.events);

        assert_eq!(
            engine.edit_manual(correct(match_id + 100)).expect("edit"),
            ManualEditOutcome::NotFound
        );
        let tick = engine.tick(now + 3).expect("tick");
        assert!(
            tick.events
                .iter()
                .all(|event| matches!(event, TrackerEvent::Status(_))),
            "{:?}",
            tick.events
        );
    }

    /// An MTGO deck's match ignores a stray rank gap: no `RankChanged` (#555).
    #[test]
    fn an_mtgo_manual_match_with_a_stray_rank_gap_moves_no_rank() {
        let dir = TestDir::new("engine-manual-mtgo-stray-gap");
        let (config, mut engine, now) = league_engine(&dir, "");
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute_batch(
                "INSERT INTO decks (id, mtgo_id, account_id, name, format, created_at,
                     updated_at, source)
                 VALUES (1, 'Q-deck', (SELECT id FROM accounts WHERE username = 'Qlocal'),
                     'Qdeck', 'CMODERN', 0, 0, 'file');
                 INSERT INTO deck_versions (id, deck_id, signature, modified_at)
                 VALUES (10, 1, 'Q-sig', 1);",
            )
            .expect("a deck");
        engine.tick(now).expect("tick");
        let ManualEdit::Create(mut manual) = manual_match(ManualPlacement::None) else {
            panic!("a create");
        };
        manual.rank_gap = Some((1, 100));
        let created = engine
            .edit_manual(ManualEdit::Create(manual))
            .expect("edit");
        assert!(
            matches!(created, ManualEditOutcome::Created { .. }),
            "{created:?}"
        );
        let tick = engine.tick(now + 1).expect("tick");
        assert!(
            !tick
                .events
                .iter()
                .any(|event| matches!(event, TrackerEvent::RankChanged)),
            "{:?}",
            tick.events
        );
    }

    #[test]
    fn a_manual_edit_pushes_the_match_and_refreshes_decks_and_leagues() {
        let dir = TestDir::new("engine-manual-edit");
        let (config, mut engine, now) = league_engine(&dir, "");
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute_batch(
                "INSERT INTO decks (id, mtgo_id, account_id, name, format, created_at,
                     updated_at, source)
                 VALUES (1, 'Q-deck', (SELECT id FROM accounts WHERE username = 'Qlocal'),
                     'Qdeck', 'CMODERN', 0, 0, 'file');
                 INSERT INTO deck_versions (id, deck_id, signature, modified_at)
                 VALUES (10, 1, 'Q-sig', 1);",
            )
            .expect("a deck");
        engine.tick(now).expect("tick");

        let created = engine
            .edit_manual(manual_match(ManualPlacement::None))
            .expect("edit");
        let ManualEditOutcome::Created { token } = created else {
            panic!("not created: {created:?}");
        };
        let tick = engine.tick(now + 1).expect("tick");
        assert_eq!(pushed_revisions(&tick.events, &token), vec![1]);
        let (view, manual) = tick
            .events
            .iter()
            .find_map(|event| match event {
                TrackerEvent::MatchUpdated { view, manual, .. } => Some((view.clone(), *manual)),
                _ => None,
            })
            .expect("pushed");
        assert!(manual, "a manual match is pushed as manual");
        assert_eq!(view.opponent.as_deref(), Some("Qkai"));
        assert_eq!((view.wins, view.losses), (2, 0));
        assert_eq!(decks_changed(&tick.events), 1, "{:?}", tick.events);
        assert_eq!(leagues_changed(&tick.events), 1, "{:?}", tick.events);

        // An edit is a new revision, pushed and refreshed the same way.
        let reveals = ManualEdit::SetReveals {
            token: token.clone(),
            game: 2,
            cards: Vec::new(),
        };
        assert_eq!(
            engine.edit_manual(reveals).expect("edit"),
            ManualEditOutcome::Done
        );
        let tick = engine.tick(now + 2).expect("tick");
        assert_eq!(pushed_revisions(&tick.events, &token), vec![2]);
        assert_eq!(decks_changed(&tick.events), 1, "{:?}", tick.events);
        assert_eq!(leagues_changed(&tick.events), 1, "{:?}", tick.events);

        // A refused edit reports nothing, and a match removed from stats
        // cannot be edited.
        let missing = ManualEdit::SetReveals {
            token: "Q-missing".to_string(),
            game: 1,
            cards: Vec::new(),
        };
        assert_eq!(
            engine.edit_manual(missing).expect("edit"),
            ManualEditOutcome::NotFound
        );
        let tick = engine.tick(now + 3).expect("tick");
        assert!(
            tick.events
                .iter()
                .all(|event| matches!(event, TrackerEvent::Status(_))),
            "{:?}",
            tick.events
        );
        assert_eq!(
            engine.edit_match(hide_edit(&token, now)).expect("hide"),
            MatchEditOutcome::Done
        );
        engine.tick(now + 4).expect("tick");
        let hand = ManualEdit::SetHand {
            token: token.clone(),
            game: 1,
            mulligan_count: 0,
            starting: Vec::new(),
            bottomed: Vec::new(),
            mulligans: Vec::new(),
        };
        assert_eq!(
            engine.edit_manual(hand).expect("edit"),
            ManualEditOutcome::NotFound
        );
        let tick = engine.tick(now + 5).expect("tick");
        assert!(pushed_revisions(&tick.events, &token).is_empty());
    }

    #[test]
    fn a_projection_rebuild_keeps_every_input_column() {
        let fixture = load_fixture("draft_hobbit");
        let dir = TestDir::new("engine-rebuild-inputs");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let now = fixture.meta.now_ms() + 3 * ABANDON_AFTER_MS;
        let mut first = open(&config, now);
        run_until_settled(&mut first, now);
        drop(first);
        let token: String = store::open_reader(&config.db_path)
            .expect("reader")
            .query_row(
                "SELECT token FROM matches m WHERE source = 'log'
                     AND (SELECT COUNT(*) FROM games g WHERE g.match_id = m.id) > 1
                 ORDER BY id LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("a log match with games");
        // The match's id, input columns and game ids, as one text.
        let snapshot = |config: &TrackerConfig| -> String {
            store::open_reader(&config.db_path)
                .expect("reader")
                .query_row(
                    "SELECT concat_ws('|', id, notes, hidden_at, deck_version_id,
                         (SELECT group_concat(g.game_number || ':' || g.id, ',')
                          FROM (SELECT game_number, id FROM games WHERE match_id = m.id
                                ORDER BY game_number) g))
                     FROM matches m WHERE token = ?1",
                    [&token],
                    |row| row.get(0),
                )
                .expect("the match")
        };
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute_batch(&format!(
                "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source)
                     VALUES (900, 'Q-input-deck', 'Qdeck', 'CMODERN', 1, 1, 'imported');
                 INSERT INTO deck_versions (id, deck_id, signature, modified_at)
                     VALUES (900, 900, 'Q-input', 1);
                 UPDATE matches SET notes = 'Q kept note', hidden_at = 77, deck_version_id = 900
                     WHERE token = '{token}';
                 UPDATE matches SET derived_version = 0;
                 UPDATE derivations SET version = 0;"
            ))
            .expect("input and stale rows");
        let before = snapshot(&config);
        assert!(before.contains("|Q kept note|77|900|"), "{before}");

        let mut second = open(&config, now + 1_000);
        run_until_settled(&mut second, now + 1_000);
        assert_eq!(
            count(
                &config,
                &format!("SELECT derived_version FROM matches WHERE token = '{token}'")
            ),
            i64::from(PROJECTION_VERSION),
            "the rebuild derived the match again"
        );
        assert_eq!(snapshot(&config), before);
    }

    // Replays spec section 8: a shared replay link is input, so a rebuild
    // keeps it (better than 0.x, whose reprocess lost it while the link
    // lived on), and sharing it went through the match edit.
    #[test]
    fn a_rebuild_keeps_the_share() {
        let fixture = load_fixture("draft_hobbit");
        let dir = TestDir::new("engine-rebuild-share");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let now = fixture.meta.now_ms() + 3 * ABANDON_AFTER_MS;
        let mut first = open(&config, now);
        run_until_settled(&mut first, now);
        let token: String = store::open_reader(&config.db_path)
            .expect("reader")
            .query_row(
                "SELECT token FROM matches m WHERE source = 'log'
                     AND (SELECT COUNT(*) FROM games g WHERE g.match_id = m.id) > 1
                 ORDER BY id LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("a log match with games");
        let shared = first
            .edit_match(MatchEdit::SetReplayShare {
                token: token.clone(),
                uuid: "9b2f6f4e-0000-4000-8000-000000000001".to_string(),
                url: "https://mymtgo.test/players/Qplayer/replays/9b2f6f4e".to_string(),
                shared_at_ms: now,
            })
            .expect("share");
        assert_eq!(shared, MatchEditOutcome::Done);
        drop(first);
        let share = |config: &TrackerConfig| -> String {
            store::open_reader(&config.db_path)
                .expect("reader")
                .query_row(
                    "SELECT concat_ws('|', replay_share_uuid, replay_share_url, replay_shared_at)
                     FROM matches WHERE token = ?1",
                    [&token],
                    |row| row.get(0),
                )
                .expect("the match")
        };
        let before = share(&config);
        assert!(
            before.starts_with("9b2f6f4e-0000-4000-8000-000000000001|"),
            "{before}"
        );
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute_batch(
                "UPDATE matches SET derived_version = 0; UPDATE derivations SET version = 0;",
            )
            .expect("stale rows");

        let mut second = open(&config, now + 1_000);
        run_until_settled(&mut second, now + 1_000);
        assert_eq!(
            count(
                &config,
                &format!("SELECT derived_version FROM matches WHERE token = '{token}'")
            ),
            i64::from(PROJECTION_VERSION),
            "the rebuild derived the match again"
        );
        assert_eq!(share(&config), before);
    }

    #[test]
    fn a_projection_rebuild_leaves_manual_matches_alone() {
        let fixture = load_fixture("draft_hobbit");
        let dir = TestDir::new("engine-rebuild-manual");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let now = fixture.meta.now_ms() + 3 * ABANDON_AFTER_MS;
        let mut first = open(&config, now);
        run_until_settled(&mut first, now);
        drop(first);
        const MANUAL: &str = "SELECT token || source || outcome || revision || derived_version
                 || started_at || needs_attention || (SELECT group_concat(result || on_play)
                     FROM games WHERE match_id = matches.id)
             FROM matches WHERE source = 'manual'";
        let manual_row = |config: &TrackerConfig| -> String {
            store::open_reader(&config.db_path)
                .expect("reader")
                .query_row(MANUAL, [], |row| row.get(0))
                .expect("the manual match")
        };
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute_batch(
                "INSERT INTO matches (token, account_id, source, format, match_type, state,
                     outcome, started_at, ended_at, last_activity_at, revision, derived_version)
                 VALUES ('d3c1b7a2-0000-4000-8000-00000000000a', (SELECT MIN(id) FROM accounts),
                     'manual', 'CMODERN', 'Constructed', 'complete', 'win', 1, 2, 2, 1, 0);
                 INSERT INTO games (match_id, game_number, result, end_reason, on_play)
                 SELECT id, 1, 'win', 'unknown', 1 FROM matches WHERE source = 'manual';
                 UPDATE matches SET derived_version = 0;
                 UPDATE derivations SET version = 0;",
            )
            .expect("a manual match and stale rows");
        let before = manual_row(&config);

        let mut second = open(&config, now + 1_000);
        let (events, _) = run_until_settled(&mut second, now + 1_000);
        assert!(
            count(
                &config,
                "SELECT MIN(derived_version) FROM matches WHERE source = 'log'"
            ) > 0,
            "the log matches were rebuilt"
        );
        assert_eq!(manual_row(&config), before);
        assert!(
            !events.iter().any(|event| matches!(
                event,
                TrackerEvent::MatchRemoved { token } if token.starts_with("d3c1b7a2")
            )),
            "{events:?}"
        );
    }

    #[test]
    fn a_notes_edit_or_a_refused_edit_reports_nothing() {
        let dir = TestDir::new("engine-match-notes");
        let (config, mut engine, now) = league_engine(&dir, "");
        engine.record_derived(
            "Q-plain".to_string(),
            Some(Derived::Visible(league_view("Q-plain", None, None))),
            1,
            None,
        );
        engine.tick(now).expect("tick");

        let notes = MatchEdit::SetNotes {
            token: "Q-plain".to_string(),
            notes: "Q note".to_string(),
        };
        assert_eq!(
            engine.edit_match(notes).expect("edit"),
            MatchEditOutcome::Done
        );
        assert_eq!(
            engine
                .edit_match(hide_edit("Q-missing", now))
                .expect("edit"),
            MatchEditOutcome::NotFound
        );
        let tick = engine.tick(now + 1).expect("tick");
        assert!(
            tick.events
                .iter()
                .all(|event| matches!(event, TrackerEvent::Status(_))),
            "{:?}",
            tick.events
        );
        assert_eq!(
            count(
                &config,
                "SELECT COUNT(*) FROM matches WHERE notes IS NOT NULL"
            ),
            1
        );
        assert_eq!(
            count(
                &config,
                "SELECT COUNT(*) FROM matches WHERE hidden_at IS NOT NULL"
            ),
            0
        );
    }

    #[test]
    fn a_hidden_match_stays_hidden_and_unpushed_through_a_rebuild() {
        let fixture = load_fixture("draft_hobbit");
        let dir = TestDir::new("engine-hidden-rebuild");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        // Hours later, so no finished match is rehydrated on open.
        let now = fixture.meta.now_ms() + 3 * ABANDON_AFTER_MS;
        let mut first = open(&config, now);
        run_until_settled(&mut first, now);
        let token: String = store::open_reader(&config.db_path)
            .expect("reader")
            .query_row("SELECT token FROM matches ORDER BY id LIMIT 1", [], |row| {
                row.get(0)
            })
            .expect("a stored match");
        let stored = count(&config, "SELECT COUNT(*) FROM matches");
        assert!(stored > 1, "the fixture has other matches");
        assert_eq!(
            first.edit_match(hide_edit(&token, now)).expect("edit"),
            MatchEditOutcome::Done
        );
        run_until_settled(&mut first, now + 1_000);
        drop(first);

        // A newer projection version re-derives every match from the log.
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute_batch(
                "UPDATE matches SET derived_version = 0;
                 UPDATE derivations SET version = 0;",
            )
            .expect("a newer projection version");
        let mut second = open(&config, now + 2_000);
        let (events, _) = run_until_settled(&mut second, now + 2_000);
        assert_eq!(
            count(
                &config,
                &format!("SELECT derived_version FROM matches WHERE token = '{token}'")
            ),
            i64::from(PROJECTION_VERSION),
            "the rebuild derived the hidden match again"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, TrackerEvent::MatchUpdated { .. })),
            "the other matches were pushed"
        );
        assert!(!pushed_update(&events, &token), "the hidden one was not");
        assert_eq!(
            count(
                &config,
                &format!(
                    "SELECT COUNT(*) FROM matches WHERE token = '{token}' AND hidden_at IS NOT NULL"
                )
            ),
            1
        );
        let reader = store::open_reader(&config.db_path).expect("reader");
        let listed = read::list_matches(&reader, 100, 0).expect("list");
        assert_eq!(listed.len() as i64, stored - 1);
        assert!(listed.iter().all(|row| row.token != token));

        // Hidden after a restart too.
        drop(second);
        let mut third = open(&config, now + 3_000);
        third.record_derived(
            token.clone(),
            Some(Derived::Visible(league_view(&token, None, None))),
            99,
            None,
        );
        let tick = third.tick(now + 3_000).expect("tick");
        assert!(!pushed_update(&tick.events, &token), "{:?}", tick.events);
    }

    // Live events for the overlays (Plan U, U4; overlay spec 6 step 1)

    /// The second game of `draft_hobbit`'s first match (Match ID 289328158).
    const HOBBIT_GAME_TWO: &str = "Game Play Status Update for Game ID: 959762636,";
    const HOBBIT_LIVE_MATCH_ID: i64 = 289_328_158;

    /// The offset of the start of the line holding the `n`th (from 1)
    /// occurrence of `needle`.
    fn nth_line_start(bytes: &[u8], needle: &str, n: usize) -> usize {
        let mut from = 0;
        let mut found = 0;
        for _ in 0..n {
            let at = bytes[from..]
                .windows(needle.len())
                .position(|window| window == needle.as_bytes())
                .expect("the needle occurs often enough");
            found = from + at;
            from = found + needle.len();
        }
        bytes[..found]
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |newline| newline + 1)
    }

    /// The tokens of every `LiveEventsStored` in `events`, one list each.
    fn announcements(events: &[TrackerEvent]) -> Vec<Vec<String>> {
        events
            .iter()
            .filter_map(|event| match event {
                TrackerEvent::LiveEventsStored { tokens, .. } => Some(tokens.clone()),
                _ => None,
            })
            .collect()
    }

    /// The newest event time of `bytes[..cut]`, read by a throwaway run, so
    /// a test can tick just after it (the match still under way).
    fn time_at_cut(fixture: &Fixture, cut: usize, label: &str) -> i64 {
        let dir = TestDir::new(label);
        dir.write("logs/mtgo.log", &fixture.bytes[..cut]);
        let config = config(&dir, fixture);
        let mut engine = open(&config, fixture.meta.now_ms());
        run_until_settled(&mut engine, fixture.meta.now_ms());
        drop(engine);
        value(&config, "SELECT MAX(logged_at) FROM log_events")
    }

    fn token_of_match_id(config: &TrackerConfig, mtgo_match_id: i64) -> String {
        store::open_reader(&config.db_path)
            .expect("reader")
            .query_row(
                "SELECT token FROM matches WHERE mtgo_match_id = ?1",
                [mtgo_match_id],
                |row| row.get(0),
            )
            .expect("the live match")
    }

    /// A tracker caught up on `draft_hobbit` up to game 2 of its first
    /// match, which is under way: the engine, its config, the cut, the time
    /// and the match's token.
    fn mid_game_two(
        dir: &TestDir,
        fixture: &Fixture,
        label: &str,
    ) -> (Engine, TrackerConfig, usize, i64, String) {
        let cut = nth_line_start(&fixture.bytes, HOBBIT_GAME_TWO, 10);
        let now = time_at_cut(fixture, cut, label) + 1_000;
        dir.write("logs/mtgo.log", &fixture.bytes[..cut]);
        let config = config(dir, fixture);
        let mut engine = open(&config, now);
        loop {
            let tick = engine.tick(now).expect("tick");
            assert!(
                announcements(&tick.events).len() <= 1,
                "at most one per tick: {:?}",
                tick.events
            );
            if !tick.more {
                break;
            }
        }
        engine.flush().expect("flush");
        let token = token_of_match_id(&config, HOBBIT_LIVE_MATCH_ID);
        (engine, config, cut, now, token)
    }

    fn game_two_frames(config: &TrackerConfig) -> i64 {
        count(
            config,
            "SELECT COUNT(*) FROM log_events
             WHERE kind = 'game_state_update' AND game_id = 959762636",
        )
    }

    #[test]
    fn live_events_are_announced_once_per_tick_after_commit() {
        let fixture = load_fixture("draft_hobbit");
        let dir = TestDir::new("engine-live-events");
        let (mut engine, config, cut, now, token) =
            mid_game_two(&dir, &fixture, "engine-live-events-time");
        let before = game_two_frames(&config);

        // Ten more frames of game 2 arrive.
        let next = nth_line_start(&fixture.bytes, HOBBIT_GAME_TWO, 20);
        dir.append("logs/mtgo.log", &fixture.bytes[cut..next]);
        let tick = engine.tick(now + 1_000).expect("tick");
        assert_eq!(
            announcements(&tick.events),
            vec![vec![token.clone()]],
            "{:?}",
            tick.events
        );
        // Committed before it was reported: a fresh reader, no flush.
        assert!(
            game_two_frames(&config) > before,
            "the new frames are stored when announced"
        );
        let detected_at = tick
            .events
            .iter()
            .find_map(|event| match event {
                TrackerEvent::LiveEventsStored { detected_at, .. } => Some(*detected_at),
                _ => None,
            })
            .expect("announced");
        assert!(detected_at <= Instant::now());

        // The provisional last event commits at the next tick (and is
        // announced like any other); after that nothing new is stored, and
        // nothing is announced.
        let settle = engine.tick(now + 2_000).expect("tick");
        assert!(announcements(&settle.events).len() <= 1);
        let stored = game_two_frames(&config);
        let quiet = engine.tick(now + 3_000).expect("tick");
        assert_eq!(game_two_frames(&config), stored);
        assert!(
            announcements(&quiet.events).is_empty(),
            "{:?}",
            quiet.events
        );
    }

    #[test]
    fn a_catch_up_of_old_matches_announces_nothing() {
        let fixture = load_fixture("draft_hobbit");
        let dir = TestDir::new("engine-live-events-catch-up");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let now = fixture.meta.now_ms();
        let mut engine = open(&config, now);
        let (events, _) = run_until_settled(&mut engine, now);
        assert!(
            count(&config, "SELECT COUNT(*) FROM matches WHERE source = 'log'") > 0,
            "the matches were read"
        );
        assert!(
            announcements(&events).is_empty(),
            "{:?}",
            announcements(&events)
        );
    }

    /// Another file's backlog (a second client, an old log found mid-match)
    /// does not silence the live match's frames (#303 F7): only the marks
    /// read from a file still catching up are dropped.
    #[test]
    fn a_backlog_in_another_file_does_not_silence_the_live_match() {
        let fixture = load_fixture("draft_hobbit");
        let dir = TestDir::new("engine-live-events-backlog");
        let (mut engine, _config, cut, now, token) =
            mid_game_two(&dir, &fixture, "engine-live-events-backlog-time");
        // More than one read's worth, so it is still catching up after a tick.
        let line = b"12:00:00 [INF] (UI|old)\n";
        let mut backlog = Vec::new();
        while backlog.len() as u64 <= mtgo_log::tail::MAX_READ {
            backlog.extend_from_slice(line);
        }
        dir.write("logs/other/mtgo.log", &backlog);
        let next = nth_line_start(&fixture.bytes, HOBBIT_GAME_TWO, 20);
        dir.append("logs/mtgo.log", &fixture.bytes[cut..next]);
        let tick = engine.tick(now + RESCAN_EVERY_MS).expect("tick");
        assert_eq!(engine.files.len(), 2, "the old log is found");
        assert!(tick.more, "the old log is still being caught up");
        assert_eq!(
            announcements(&tick.events),
            vec![vec![token]],
            "{:?}",
            tick.events
        );
    }

    #[test]
    fn a_hidden_match_is_never_announced() {
        let fixture = load_fixture("draft_hobbit");
        let dir = TestDir::new("engine-live-events-hidden");
        let (mut engine, _config, cut, now, token) =
            mid_game_two(&dir, &fixture, "engine-live-events-hidden-time");
        assert_eq!(
            engine
                .edit_match(MatchEdit::Hide {
                    token: token.clone(),
                    now_ms: now,
                })
                .expect("hide"),
            MatchEditOutcome::Done
        );
        let next = nth_line_start(&fixture.bytes, HOBBIT_GAME_TWO, 20);
        dir.append("logs/mtgo.log", &fixture.bytes[cut..next]);
        let tick = engine.tick(now + 1_000).expect("tick");
        assert!(announcements(&tick.events).is_empty(), "{:?}", tick.events);
    }

    #[test]
    fn live_game_inputs_read_a_live_fixture_match() {
        let fixture = load_fixture("draft_hobbit");
        let dir = TestDir::new("engine-live-game-inputs");
        let (_engine, config, cut, _now, token) =
            mid_game_two(&dir, &fixture, "engine-live-game-inputs-time");
        let reader = store::open_reader(&config.db_path).expect("reader");
        let inputs = store::overlay::live_game_inputs(&reader, &token)
            .expect("read")
            .expect("the live match");
        assert_eq!(inputs.local_username, fixture.meta.local_username);
        assert!(inputs.opponent_id.is_some());
        assert_eq!(inputs.games.len(), 2, "game 1 played, game 2 under way");

        let first = &inputs.games[0];
        assert_eq!(first.mtgo_game_id, Some(959_762_288));
        assert!(!first.deck_used.as_ref().expect("game 1's list").is_empty());
        assert!(first.newest_frame.is_some());

        let second = &inputs.games[1];
        assert_eq!(second.mtgo_game_id, Some(959_762_636));
        assert!(!second.deck_used.as_ref().expect("game 2's list").is_empty());
        let file_id = value(&config, "SELECT id FROM log_files");
        let game_start = nth_line_start(&fixture.bytes, HOBBIT_GAME_TWO, 1);
        assert_eq!(second.first_frame_at, Some((file_id, game_start as i64)));
        // The newest stored frame of game 2: a line of the fixture before
        // the cut, after the game's first frame.
        let newest = second.newest_frame.as_deref().expect("a frame");
        assert!(newest.contains(HOBBIT_GAME_TWO));
        let at = fixture.bytes[..cut]
            .windows(newest.len())
            .rposition(|window| window == newest.as_bytes())
            .expect("the frame is a fixture line");
        assert!(at > game_start);
        let later_frames = nth_line_start(&fixture.bytes, HOBBIT_GAME_TWO, 9);
        assert!(
            at >= later_frames,
            "the newest committed frame, not an older one"
        );
    }
}
