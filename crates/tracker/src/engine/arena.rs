//! The Arena source (Arena spec sections 3, 5, 7 and 8): finds Arena's
//! `Player.log`, reads new entries, stores each decoded event as one
//! `arena_log_events` row, and turns the projector's changes into account,
//! opponent, rank, match and deck writes in the engine's batch.
//!
//! Live equals rebuild: at startup every stored event is read back (never the
//! log text) and folded through a fresh projector in (file id, offset, seq)
//! order before tailing resumes, so the projector holds what a live run would.
//!
//! Arena truncates `Player.log` at every launch, keeping the previous one as
//! `Player-prev.log`. A replaced file is sealed and a new row starts at the
//! next rescan. `Player-prev.log` is read once at startup, from where the
//! file it used to be was last read, in case the app missed the end of the
//! last session.
//!
//! Each match's raw game messages (Arena plan 3), Bo1 and Bo3 alike, are
//! stored too, as `arena_game_messages` rows. Once a match result makes the
//! match complete, a writer request after that batch archives them and
//! derives the match's game rows; the match is pushed again when they
//! change. Messages wait in memory until a game info says the match's kind
//! and a room state puts it in a tracked event (`parse_event_name`: a
//! constructed queue, a Brawl queue, or a draft or sealed event); a match
//! that never has both (direct challenges, other events) keeps none. A
//! restart while sideboarding resumes at the next game, as the live capture
//! would.
//!
//! A rebuilt match the store has no row for (a Bo1 tracked before Bo1 had a
//! view) is written by the first batch, as a live change would write it, and
//! its game messages are looked for in the logs still on disk.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

use arena_log::{
    ArenaTailer, DAY_ORDER_BYTES, MAX_READ, arena_log_paths, prev_log_path, read_start,
};
use arena_match::{ArenaChanges, ArenaIdentity, ArenaInput, ArenaProjector, DeckUse, Ladder};
use arena_protocol::{
    ArenaEvent, GameMessage, HeaderClock, MessageSource, RawEntry, decode_all, detect_day_first,
    game_messages, header_time, parse_event_name,
};
use cards::checked_arena_catalog_id;
use chrono::{DateTime, Offset, TimeZone};
use chrono_tz::Tz;
use mtgo_log::Env;
use mtgo_log::identity::{self, Recorded};
use mtgo_match::{MatchState, MatchView};
use store::arena::{
    ArenaFileProgress, NewArenaEvent, NewArenaLogFile, PlatformAccount, PlatformOpponent,
    RankObservation, active_arena_log_files, arena_events_in_order, arena_file_for_head,
    arena_read_offset_for_head,
};
use store::arena_games::{GameMessageSource, NewGameMessage, StoredGameMessage};
use store::decks::link_of;
use store::write::{MatchWrite, WriteBatch};
use store::writer::{ArenaGameSyncReply, WriterHandle};
use store::{Connection, Platform, read};

use super::Engine;
use super::arena_courses::{CourseLog, room_event_name};
use super::deck_use::{DeckLinks, DeckUseIds, deck_sync_write, rename_in_write};
use crate::{FileStatus, TrackerError};

/// The seal reason of a `Player-prev.log` row once it was read to its end.
pub const READ_ONCE: &str = "read_once";

/// The `seq` of an entry's first game message: game messages live in their
/// own table, the offset only keeps them apart from the entry's event seqs.
const GAME_MESSAGE_SEQ: u32 = 1000;

/// The most game messages held while their match's kind is unknown;
/// past it the oldest are dropped.
const HOLD_CAP: usize = 50_000;

/// One Arena log file being read.
struct ArenaFile {
    id: i64,
    path: PathBuf,
    recorded: Recorded,
    tailer: ArenaTailer,
    /// `Player-prev.log`: read once, sealed at its end.
    prev: bool,
    /// The header date order; `None` until the file's start settles it.
    day_first: Option<bool>,
    /// How many bytes of the file's start the date order was looked for in.
    day_scanned: u64,
    /// The time of the file's latest event: an entry with no usable time of
    /// its own takes it.
    last_time_ms: Option<i64>,
    /// (cursor, observed size, head length) last sent to the writer.
    saved: Option<(u64, u64, u64)>,
    /// The file was read to its end once: later growth is Arena writing live.
    caught_up: bool,
    /// When the file last grew after it was caught up (the status bar's
    /// "Last event").
    last_live_growth_ms: Option<i64>,
}

/// What one poll produced.
#[derive(Debug, Default)]
pub(crate) struct ArenaPoll {
    pub matches: Vec<MatchWrite>,
    /// A file still has unread bytes: tick again without sleeping.
    pub more: bool,
    /// The poll queued an Arena account the store did not have yet, or a
    /// changed one: pages that read the context refetch (`AccountChanged`).
    pub account_changed: bool,
    /// The poll queued rank readings: the rank views refetch (`RankChanged`).
    pub rank_changed: bool,
}

/// Which match and game the game messages being read belong to, and the
/// messages held until their match's kind is said.
#[derive(Debug, Default)]
struct GameCapture {
    /// The match the latest room state or game info named.
    current_match: Option<String>,
    /// The game the next message without a game info belongs to: 1 from the
    /// match's start, the next game after an intermission.
    current_game: Option<u32>,
    /// The game number of the current match's latest game info.
    info_game: Option<u32>,
    /// The matches a game info said the kind of (Bo1 or Bo3).
    told: HashSet<String>,
    /// The matches a room state put in a tracked event, the only ones the
    /// projector tracks (`parse_event_name`): direct challenges and other
    /// events are held, then dropped when they end.
    tracked: HashSet<String>,
    /// Messages waiting for their match's kind, oldest first. An empty token
    /// is a message read before any match was named: the next match named
    /// takes it, as game 1.
    held: VecDeque<NewGameMessage>,
    /// The hold reached `HOLD_CAP` and dropped a message since it last had
    /// room: the warning is logged once per overflow, not per message.
    hold_overflowed: bool,
}

impl GameCapture {
    /// Follow a decoded event, live and in a rebuild: a room state names the
    /// current match, and a completed one ends it (the next match's first
    /// messages can come before anything names it). Game infos are followed
    /// message by message in `capture`, in their order within the entry.
    fn note_event(&mut self, event: &ArenaEvent) {
        if let ArenaEvent::RoomState {
            match_id, players, ..
        } = event
            && players
                .iter()
                .any(|seat| parse_event_name(&seat.event_id).is_some())
        {
            self.tracked.insert(match_id.clone());
        }
        match event {
            ArenaEvent::RoomState {
                match_id,
                completed: true,
                ..
            } => self.leave_match(match_id),
            ArenaEvent::RoomState { match_id, .. } => self.enter_match(match_id),
            _ => {}
        }
    }

    /// Follow a stored event in a rebuild, which has no game messages: a game
    /// info also gives the match and game, and whether it said the match's
    /// kind (an event stored before the win condition was kept says only
    /// Bo3: its `false` can also mean "not said").
    fn note_stored_event(&mut self, event: &ArenaEvent) {
        self.note_event(event);
        if let ArenaEvent::GameInfo {
            match_id,
            game_number,
            best_of_three,
            win_condition,
            ..
        } = event
        {
            self.enter_match(match_id);
            if *game_number > 0 {
                self.set_game(*game_number);
            }
            if (*best_of_three || win_condition.is_some())
                && let Some(token) = &self.current_match
            {
                self.told.insert(token.clone());
            }
        }
    }

    /// A new current match starts at game 1 and takes the messages held with
    /// no match (as game 1); the messages still held for an earlier match
    /// are dropped: its kind was never said.
    fn enter_match(&mut self, token: &str) {
        if token.is_empty() || self.current_match.as_deref() == Some(token) {
            return;
        }
        self.current_match = Some(token.to_string());
        self.current_game = Some(1);
        self.info_game = None;
        self.held
            .retain(|message| message.match_token.is_empty() || message.match_token == token);
        for message in &mut self.held {
            if message.match_token.is_empty() {
                message.match_token = token.to_string();
                message.game_number = Some(1);
            }
        }
    }

    /// The match is over: no later message is its, and what it still holds
    /// is dropped (its kind was never said).
    fn leave_match(&mut self, token: &str) {
        self.held.retain(|message| message.match_token != token);
        if self.current_match.as_deref() == Some(token) {
            self.current_match = None;
            self.current_game = None;
            self.info_game = None;
        }
    }

    /// After a rebuild: the stored events give the latest game info's game,
    /// but a restart while sideboarding comes after that game's
    /// intermission, so the messages to come belong to the next game. Only
    /// an intermission after the match's last stored message with a game
    /// info (in log order) moves on, as `capture` does live: Arena's
    /// repeated intermission is stored as the next game, before that game's
    /// own game info, and must not move a restart in that game on.
    fn resume_after_intermission(&mut self, stored: &[StoredGameMessage]) {
        let Some(info_game) = self.info_game else {
            return;
        };
        let mut ended = false;
        for message in stored.iter().rev() {
            if message.source != GameMessageSource::Gre {
                continue;
            }
            if has_game_info(&message.json) {
                break;
            }
            if arena_protocol::is_intermission_json(&message.json) {
                ended = true;
            }
        }
        if ended {
            self.current_game = Some(info_game.saturating_add(1));
        }
    }

    fn set_game(&mut self, game_number: u32) {
        self.current_game = Some(game_number);
        self.info_game = Some(game_number);
    }

    /// The messages to store now that `message` was read: none while its
    /// match's kind is unknown (it is held), else the match's held messages
    /// and this one, in log order. An intermission
    /// ends its game: the messages after it (sideboarding, the next deck)
    /// carry no game info but belong to the next game.
    fn capture(&mut self, message: GameMessage, new: NewGameMessage) -> Vec<NewGameMessage> {
        if let Some(match_id) = &message.match_id {
            self.enter_match(match_id);
        }
        if let Some(game_number) = message.game_number {
            self.set_game(game_number);
        }
        let token = self.current_match.clone().unwrap_or_default();
        if !token.is_empty() && message.best_of_three.is_some() {
            self.told.insert(token.clone());
        }
        let new = NewGameMessage {
            match_token: token.clone(),
            game_number: self.current_game,
            ..new
        };
        if message.is_intermission() {
            // From the game info's number, not the current one: Arena can
            // send the same intermission twice.
            let ended = self.info_game.or(self.current_game);
            if let Some(ended) = ended {
                self.current_game = Some(ended.saturating_add(1));
            }
        }
        if self.is_kept(&token) {
            let mut out = self.release(&token);
            out.push(new);
            out
        } else {
            self.hold(new);
            Vec::new()
        }
    }

    /// An entry's game messages to store, each followed in order.
    fn capture_entry(
        &mut self,
        file_id: i64,
        entry: &RawEntry,
        entry_ms: i64,
    ) -> Vec<NewGameMessage> {
        let mut out = Vec::new();
        for (index, message) in game_messages(entry).into_iter().enumerate() {
            let new = NewGameMessage {
                file_id,
                byte_offset: entry.offset,
                seq: GAME_MESSAGE_SEQ + index as u32,
                match_token: String::new(),
                game_number: None,
                source: match message.source {
                    MessageSource::Gre => GameMessageSource::Gre,
                    MessageSource::Client => GameMessageSource::Client,
                },
                logged_at: message.at_ms.unwrap_or(entry_ms),
                json: message.json.clone(),
            };
            out.extend(self.capture(message, new));
        }
        out
    }

    /// Whether `token`'s messages are stored: its kind was said and it is in
    /// a tracked event.
    fn is_kept(&self, token: &str) -> bool {
        self.told.contains(token) && self.tracked.contains(token)
    }

    /// Take the messages held for `token` out of the hold, in order.
    fn release(&mut self, token: &str) -> Vec<NewGameMessage> {
        if self.held.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for message in std::mem::take(&mut self.held) {
            if message.match_token == token {
                out.push(message);
            } else {
                self.held.push_back(message);
            }
        }
        out
    }

    fn hold(&mut self, message: NewGameMessage) {
        if self.held.len() >= HOLD_CAP {
            self.held.pop_front();
            if !self.hold_overflowed {
                tracing::warn!("too many Arena game messages held; dropping the oldest");
                self.hold_overflowed = true;
            }
        } else {
            self.hold_overflowed = false;
        }
        self.held.push_back(message);
    }
}

/// Whether a stored GRE message carries a game info with a game number (as
/// `arena_protocol::game_messages` reads it).
fn has_game_info(json: &str) -> bool {
    if !json.contains("gameInfo") {
        return false;
    }
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|value| {
            value
                .pointer("/gameStateMessage/gameInfo/gameNumber")
                .and_then(serde_json::Value::as_u64)
        })
        .is_some()
}

/// The projector after a replay of the stored events, and what the replay
/// learned on the way.
pub(crate) struct Rebuilt {
    pub projector: ArenaProjector,
    /// Every match token the projector made visible.
    pub tokens: Vec<String>,
    /// Every deck use, with the time of the event that made it.
    pub deck_uses: Vec<(DeckUse, i64)>,
    /// Each file's latest event time.
    pub last_times: HashMap<i64, i64>,
    pub newest_ms: i64,
    /// Each opponent's display name by registry key (`arena:<user id>`).
    pub opponent_names: HashMap<String, String>,
    /// Each match's `(token, event name)` from its first room state naming
    /// an event, in log order.
    pub match_events: Vec<(String, String)>,
    /// The current match and game, and which matches' kind was said, as the
    /// stored events tell (the held messages of a run are not stored).
    games: GameCapture,
}

pub(crate) struct ArenaSource {
    env: Env,
    overrides: Vec<PathBuf>,
    /// Where the last rescan looked for `Player.log`.
    searched: Vec<PathBuf>,
    files: Vec<ArenaFile>,
    projector: ArenaProjector,
    /// Each match's revision, continued from storage (as MTGO's projector
    /// seeds them), bumped on every change written.
    revisions: HashMap<String, u64>,
    /// The account last written, so it is written again only when it changes.
    written_account: Option<ArenaIdentity>,
    /// `(user id, username)` of the last account written with a user id: a
    /// later one with the same user id under another name is a rename
    /// (#862), and the deck writes queued under the old name follow it.
    named_account: Option<(String, String)>,
    /// Deck writes for the next batch and the matches waiting for their
    /// deck links.
    deck_links: DeckLinks,
    /// `Player-prev.log` was looked at (at the first rescan, and again after
    /// each rotation).
    prev_checked: bool,
    /// Files sealed by a rotation this run, as `(head_hash, head_len,
    /// read_offset)`. The seal reaches the writer later than the rescan can
    /// look for it, so `start_prev` also starts past these.
    sealed: Vec<(u64, u64, u64)>,
    /// The newest event time read, for the time rules while catching up.
    newest_ms: i64,
    /// Each opponent's display name by registry key (`arena:<user id>`), so
    /// a pushed match names its opponent (the view holds the key).
    opponent_names: HashMap<String, String>,
    /// The game messages' match and game, and the messages held.
    games: GameCapture,
    /// Course snapshots waiting for an account, and each account's last one.
    courses: CourseLog,
    /// Match event names for the next batch, in order.
    match_events: Vec<(String, String)>,
    /// Matches whose event name was queued or found stored.
    named_matches: HashSet<String>,
    /// Rebuilt matches with a view and no stored row (a Bo1 tracked before
    /// Bo1 had a view), or a stored row with the wrong Brawl format: written
    /// by the first batch, then looked for by the backfill.
    unstored: Vec<String>,
    /// The accounts the unstored matches name that the store does not have
    /// (an earlier account seen only through fast logins): written as Arena
    /// accounts before the matches, which would otherwise create them as
    /// MTGO ones.
    unstored_accounts: Vec<String>,
    /// Matches settled this tick (by a match result, or without one):
    /// archived and synced in a writer request after the batch with their
    /// result (`Engine::send_arena_syncs`), so the result is not held up by
    /// the sync.
    end_syncs: Vec<String>,
    /// The match-end syncs sent and not answered yet, with their tokens.
    end_sync_replies: Vec<Receiver<ArenaGameSyncReply>>,
    /// Tests only: read as the app did before Arena game data (no game
    /// messages stored, no Arena game sync at a match's end).
    #[cfg(test)]
    game_data_off: bool,
}

impl ArenaSource {
    /// Rebuild the projector from storage and resume every unsealed file.
    pub(crate) fn open(
        reader: &Connection,
        env: Env,
        overrides: Vec<PathBuf>,
        now_ms: i64,
    ) -> Result<Self, TrackerError> {
        let rebuilt = rebuild(reader)?;
        let mut revisions = HashMap::new();
        // Deletes are soft: a match with no revision was never stored.
        let mut unstored = Vec::new();
        let mut unstored_accounts = Vec::new();
        let brawl_tokens = read::arena_brawl_tokens(reader)?;
        for token in &rebuilt.tokens {
            let stored = read::match_revision(reader, token)?;
            // A Brawl ladder match the last release stored as `ARENA_BRAWL`
            // is written again with the view's format, as an unstored one.
            let wrong_format = brawl_tokens.contains(token)
                && rebuilt
                    .projector
                    .view(token)
                    .is_some_and(|view| view.format.as_deref().is_some_and(|f| f != "ARENA_BRAWL"));
            if (stored.is_none() || wrong_format)
                && let Some(view) = rebuilt.projector.view(token)
            {
                unstored.push(token.clone());
                let username = &view.local_username;
                if !unstored_accounts.contains(username)
                    && read::account_id(reader, username)?.is_none()
                {
                    unstored_accounts.push(username.clone());
                }
            }
            revisions.insert(token.clone(), stored.unwrap_or(0));
        }
        // An account the store does not have yet (a fast login before it
        // was read) is written with the next account change or match write.
        let written_account = match rebuilt.projector.account() {
            Some(identity) if read::account_id(reader, &identity.username)?.is_some() => {
                Some(identity.clone())
            }
            _ => None,
        };
        // Event names stored before are not queued again.
        let stored_names = store::arena_courses::match_event_tokens(reader)?;
        let named_matches: HashSet<String> = rebuilt
            .match_events
            .iter()
            .map(|(token, _)| token.clone())
            .chain(stored_names.iter().cloned())
            .collect();
        let match_events: Vec<(String, String)> = rebuilt
            .match_events
            .into_iter()
            .filter(|(token, _)| !stored_names.contains(token))
            .collect();
        let mut source = Self {
            env,
            overrides,
            searched: Vec::new(),
            files: Vec::new(),
            named_account: written_account.as_ref().and_then(|identity| {
                identity
                    .user_id
                    .clone()
                    .map(|user_id| (user_id, identity.username.clone()))
            }),
            written_account,
            projector: rebuilt.projector,
            revisions,
            deck_links: DeckLinks::default(),
            prev_checked: false,
            sealed: Vec::new(),
            newest_ms: rebuilt.newest_ms,
            opponent_names: rebuilt.opponent_names,
            games: rebuilt.games,
            courses: CourseLog::restore(reader)?,
            match_events,
            named_matches,
            unstored,
            unstored_accounts,
            end_syncs: Vec::new(),
            end_sync_replies: Vec::new(),
            #[cfg(test)]
            game_data_off: false,
        };
        if let Some(token) = source.games.current_match.clone() {
            let stored = store::arena_games::game_messages(reader, &token)?;
            source.games.resume_after_intermission(&stored);
        }
        // A match stored without its deck link (the app stopped between the
        // deck write and the link), or not stored yet, is linked.
        for (deck_use, at_ms) in rebuilt.deck_uses {
            if link_of(reader, &deck_use.match_token)?.is_none()
                && let Some(account) = source.view_account(&deck_use.match_token)
            {
                source.queue_deck_use(reader, deck_use, &account, at_ms, now_ms)?;
            }
        }
        for row in active_arena_log_files(reader)? {
            let path = PathBuf::from(&row.path);
            source.files.push(ArenaFile {
                id: row.id,
                recorded: Recorded {
                    last_observed_size: row.last_observed_size,
                    created_ms: row.created_at_fs,
                    head_hash: row.head_hash,
                    head_len: row.head_len,
                    anchor: None,
                },
                tailer: ArenaTailer::new(path.clone(), row.read_offset),
                prev: is_prev_log(&path),
                path,
                day_first: None,
                day_scanned: 0,
                last_time_ms: rebuilt.last_times.get(&row.id).copied(),
                caught_up: false,
                last_live_growth_ms: None,
                saved: Some((row.read_offset, row.last_observed_size, row.head_len)),
            });
        }
        Ok(source)
    }

    /// Start every `Player.log` found that is not read yet (a new row from
    /// offset 0) and, at the first rescan and after a rotation, the unread
    /// part of each `Player-prev.log`.
    pub(crate) fn rescan(
        &mut self,
        reader: &Connection,
        writer: &WriterHandle,
        now_ms: i64,
    ) -> Result<(), TrackerError> {
        let paths = arena_log_paths(&self.env, &self.overrides);
        self.searched = paths.clone();
        if !self.prev_checked {
            self.prev_checked = true;
            for path in &paths {
                self.start_prev(reader, writer, &prev_log_path(path), now_ms)?;
            }
        }
        for path in paths {
            if self.is_reading(&path) || !path.is_file() {
                continue;
            }
            self.start_file(writer, path, 0, now_ms)?;
        }
        Ok(())
    }

    /// Read every file's new entries into `batch`. `tz` gives the machine's
    /// UTC offset for header times.
    pub(crate) fn poll(
        &mut self,
        reader: &Connection,
        batch: &mut WriteBatch,
        now_ms: i64,
        tz: Tz,
    ) -> Result<ArenaPoll, TrackerError> {
        let utc_offset_minutes = utc_offset_minutes(tz, now_ms);
        let mut out = ArenaPoll::default();
        // Snapshots a previous run held, once the account is known.
        self.write_courses(reader, batch, now_ms, &mut out)?;
        let mut index = 0;
        while index < self.files.len() {
            if self.poll_file(index, reader, batch, now_ms, utc_offset_minutes, &mut out)? {
                index += 1;
            } else {
                self.files.remove(index);
            }
        }
        self.deck_links.write_into(batch);
        // Names found by the rebuild (a database from before they were
        // stored) rename their runs: the startup resync may have run without
        // them, and a paused start sends them only now.
        for (token, _) in &self.match_events {
            if let Some(account) = self.view_account(token) {
                queue_arena_runs(batch, &account);
            }
        }
        batch.arena_match_events.append(&mut self.match_events);
        // With the read progress past the held snapshots, so a restart has
        // them.
        batch.settings.extend(self.courses.take_setting());
        Ok(out)
    }

    /// The newest Arena event time read this run (0 before the first).
    pub(crate) fn newest_ms(&self) -> i64 {
        self.newest_ms
    }

    /// The time rules as of `now_ms` (as of the newest event read while
    /// catching up, as MTGO's): a match quiet for an hour is settled.
    pub(crate) fn finalize(&mut self, now_ms: i64, catching_up: bool) -> Vec<MatchWrite> {
        let sweep_ms = if catching_up { self.newest_ms } else { now_ms };
        let tokens = self.projector.finalize(sweep_ms);
        let mut writes = Vec::new();
        for token in tokens {
            if let Some(write) = self.match_write(&token) {
                self.queue_end_sync(&token, &write);
                writes.push(write);
            }
        }
        writes
    }

    /// Queue a settled match's end sync, once a tick: archived, then its
    /// game rows derived, in a writer request after the batch that writes
    /// it. A match settled without a result (quiet, abandoned, ended) is
    /// synced as a decided one is, not at the next start (#571).
    fn queue_end_sync(&mut self, token: &str, write: &MatchWrite) {
        if is_settled(write)
            && self.stores_game_data()
            && !self.end_syncs.iter().any(|t| t == token)
        {
            self.end_syncs.push(token.to_string());
        }
    }

    /// Where `Player.log` is looked for, and each one being read (the status
    /// bar's; `Player-prev.log` is a catch-up, never shown).
    pub(crate) fn status(&self) -> (Vec<PathBuf>, Vec<FileStatus>) {
        let files = self
            .files
            .iter()
            .filter(|file| !file.prev)
            .map(|file| FileStatus {
                path: file.path.clone(),
                size: file.recorded.last_observed_size,
                cursor: file.tailer.cursor(),
                last_live_growth_ms: file.last_live_growth_ms,
                dropped_orphan_lines: 0,
                bad_timezone: false,
            })
            .collect();
        (self.searched.clone(), files)
    }

    /// Whether a match waits for its deck link: the engine commits the batch
    /// first, then calls `link`.
    pub(crate) fn has_pending_links(&self) -> bool {
        self.deck_links.has_pending_links()
    }

    /// Link each waiting match to its deck version by exact signature among
    /// the account's versions (its own deck's first). The deck writes were
    /// committed before this is called, so a version that is not there now
    /// will not come: the link is dropped. Deck writes held behind a link go
    /// into the batch after it.
    pub(crate) fn link(
        &mut self,
        reader: &Connection,
        batch: &mut WriteBatch,
    ) -> Result<(), TrackerError> {
        self.deck_links.link(reader, batch, Platform::Arena)
    }

    /// The deck ids synced since the last call, for the archetype prefill.
    pub(crate) fn take_written_deck_ids(&mut self) -> Vec<String> {
        self.deck_links.take_written()
    }

    /// Read one file. Returns `false` when the file is done with (sealed).
    fn poll_file(
        &mut self,
        index: usize,
        reader: &Connection,
        batch: &mut WriteBatch,
        now_ms: i64,
        utc_offset_minutes: i32,
        out: &mut ArenaPoll,
    ) -> Result<bool, TrackerError> {
        // A file that vanished or is locked is tried again next tick.
        let Ok(probe) = identity::probe(&self.files[index].path, None) else {
            return Ok(true);
        };
        if let Some(reason) = identity::detect_rotation(&self.files[index].recorded, &probe) {
            let file = &self.files[index];
            tracing::info!(path = %file.path.display(), reason = reason.as_str(), "Arena log replaced; sealing it");
            batch.arena_files.push(ArenaFileProgress {
                id: file.id,
                last_observed_size: file.recorded.last_observed_size,
                read_offset: file.tailer.cursor(),
                head_hash: file.recorded.head_hash,
                head_len: file.recorded.head_len,
                sealed: Some((now_ms, reason.as_str().to_string())),
            });
            out.more = true;
            // Arena relaunched: the old file's unread tail is in
            // `Player-prev.log` now, so the next rescan looks at it again.
            self.prev_checked = false;
            self.sealed.push((
                file.recorded.head_hash,
                file.recorded.head_len,
                file.tailer.cursor(),
            ));
            return Ok(false);
        }
        identity::observe(&mut self.files[index].recorded, &probe);
        self.detect_day_order(index, probe.size);
        let cursor_before = self.files[index].tailer.cursor();
        let Ok(read) = self.files[index].tailer.poll(probe.size) else {
            return Ok(true);
        };
        // Past the file's start, every read is looked at until the order is
        // known.
        if self.files[index].day_first.is_none() {
            self.files[index].day_first = day_first_of_entries(&read.entries);
        }
        let clock = HeaderClock {
            day_first: self.files[index].day_first,
            utc_offset_minutes,
        };
        for entry in &read.entries {
            let entry_ms = entry_time(
                &clock,
                entry,
                self.files[index].last_time_ms,
                probe.modified_ms,
                now_ms,
            );
            for (seq, event) in decode_all(entry).into_iter().enumerate() {
                // A course snapshot is not a log event: it never reaches
                // `arena_log_events` or the projector.
                if let ArenaEvent::Courses { courses } = event {
                    let file_id = self.files[index].id;
                    self.courses.hold(file_id, entry.offset, entry_ms, courses);
                    self.write_courses(reader, batch, now_ms, out)?;
                    continue;
                }
                let logged_at_ms = own_time(&event).unwrap_or(entry_ms);
                self.files[index].last_time_ms = Some(logged_at_ms);
                let input = ArenaInput {
                    file_id: self.files[index].id as u64,
                    offset: entry.offset,
                    seq: seq as u32,
                    at_ms: logged_at_ms,
                    event,
                };
                self.ingest(reader, batch, &input, now_ms, out)?;
                self.write_courses(reader, batch, now_ms, out)?;
            }
            let file_id = self.files[index].id;
            self.capture_game_messages(batch, file_id, entry, entry_ms);
        }
        let file = &mut self.files[index];
        if file.caught_up && read.cursor > cursor_before {
            file.last_live_growth_ms = Some(now_ms);
        }
        file.caught_up |= read.at_eof;
        let sealed = file.prev && read.at_eof;
        // The head is in the mark: while the file is shorter than the full
        // head it grows with it, and the stored head must follow.
        let mark = (
            read.cursor,
            file.recorded.last_observed_size,
            file.recorded.head_len,
        );
        if file.saved != Some(mark) || sealed {
            file.saved = Some(mark);
            batch.arena_files.push(ArenaFileProgress {
                id: file.id,
                last_observed_size: file.recorded.last_observed_size,
                read_offset: read.cursor,
                head_hash: file.recorded.head_hash,
                head_len: file.recorded.head_len,
                sealed: sealed.then(|| (now_ms, READ_ONCE.to_string())),
            });
        }
        if !read.at_eof {
            out.more = true;
        }
        Ok(!sealed)
    }

    /// Store and project one decoded event. Its stored time is the time the
    /// projector is given, so a rebuild folds it the same way.
    fn ingest(
        &mut self,
        reader: &Connection,
        batch: &mut WriteBatch,
        input: &ArenaInput,
        now_ms: i64,
        out: &mut ArenaPoll,
    ) -> Result<(), TrackerError> {
        let raw = match serde_json::to_string(&input.event) {
            Ok(raw) => raw,
            Err(error) => {
                tracing::warn!(%error, "an Arena event could not be stored");
                return Ok(());
            }
        };
        batch.arena_events.push(NewArenaEvent {
            file_id: input.file_id as i64,
            byte_offset: input.offset,
            seq: input.seq,
            kind: input.event.kind(),
            match_token: event_match_token(&input.event),
            logged_at_ms: input.at_ms,
            raw,
        });
        self.newest_ms = self.newest_ms.max(input.at_ms);
        if let Some((token, event_name)) = room_event_name(&input.event)
            && self.named_matches.insert(token.clone())
        {
            batch.arena_match_events.push((token, event_name));
        }
        self.games.note_event(&input.event);
        let changes = self.projector.apply(input);
        self.absorb(reader, batch, changes, input.at_ms, now_ms, out)
    }

    /// Turn one event's changes into writes. The account goes first, so the
    /// store has it before any match, rank or deck names it.
    fn absorb(
        &mut self,
        reader: &Connection,
        batch: &mut WriteBatch,
        changes: ArenaChanges,
        at_ms: i64,
        now_ms: i64,
        out: &mut ArenaPoll,
    ) -> Result<(), TrackerError> {
        self.write_account(batch, now_ms, out);
        let account = self.projector.account().cloned();
        for (username, display_name) in changes.opponents {
            self.opponent_names
                .insert(username.clone(), display_name.clone());
            batch.platform_opponents.push(PlatformOpponent {
                platform: Platform::Arena,
                username,
                display_name,
            });
        }
        // Rank responses name no account: the latest login's (trap 6).
        if let Some(identity) = &account {
            for point in changes.rank {
                batch.rank_observations.push(RankObservation {
                    account_username: identity.username.clone(),
                    account_user_id: identity.user_id.clone(),
                    ladder: match point.ladder {
                        Ladder::Constructed => "constructed",
                        Ladder::Limited => "limited",
                    },
                    season: point.reading.season,
                    observed_at_ms: point.at_ms,
                    class: point.reading.class,
                    level: point.reading.level,
                    step: point.reading.step,
                    won: point.reading.won,
                    lost: point.reading.lost,
                    mythic_percentile: point.reading.mythic_percentile,
                    mythic_placement: point.reading.mythic_placement,
                    file_id: point.file_id as i64,
                    byte_offset: point.offset,
                });
                out.rank_changed = true;
            }
        }
        for token in changes.matches {
            if let Some(write) = self.match_write(&token) {
                self.queue_end_sync(&token, &write);
                out.matches.push(write);
            }
        }
        for deck_use in changes.deck_uses {
            if let Some(account) = self.view_account(&deck_use.match_token) {
                self.queue_deck_use(reader, deck_use, &account, at_ms, now_ms)?;
            }
        }
        Ok(())
    }

    /// The held course snapshots, once the projector has an account: under
    /// that account (written first if it is new), each one that differs
    /// from the account's previous snapshot. A snapshot waits for a login
    /// in its own file or a later one: a new `Player.log` may be another
    /// account's, and its courses can come before its login.
    fn write_courses(
        &mut self,
        reader: &Connection,
        batch: &mut WriteBatch,
        now_ms: i64,
        out: &mut ArenaPoll,
    ) -> Result<(), TrackerError> {
        if !self.courses.has_held() {
            return Ok(());
        }
        let Some(username) = self.projector.account().map(|a| a.username.clone()) else {
            return Ok(());
        };
        let Some(login_file) = self.projector.login_file() else {
            return Ok(());
        };
        if !self.courses.has_held_up_to(login_file as i64) {
            return Ok(());
        }
        self.write_account(batch, now_ms, out);
        let rows = self.courses.release(reader, &username, login_file as i64)?;
        if !rows.is_empty() {
            queue_arena_runs(batch, &username);
        }
        batch.arena_course_observations.extend(rows);
        Ok(())
    }

    /// The projector's account, when it is not the one last written.
    fn write_account(&mut self, batch: &mut WriteBatch, now_ms: i64, out: &mut ArenaPoll) {
        let account = self.projector.account().cloned();
        if account.is_none() || account == self.written_account {
            return;
        }
        if let Some(identity) = &account {
            batch.platform_accounts.push(PlatformAccount {
                platform: Platform::Arena,
                username: identity.username.clone(),
                user_id: identity.user_id.clone(),
                display_name: identity.display_name.clone(),
                seen_at_ms: now_ms,
            });
            // An untagged name (trap 29) is no rename: the next tagged login
            // would undo it.
            if let Some(user_id) = &identity.user_id
                && !identity.is_untagged()
            {
                self.follow_rename(batch, out, user_id, &identity.username);
            }
        }
        self.written_account = account;
        out.account_changed = true;
    }

    /// The account with `user_id` is written as `username`: when it was
    /// written under another name before, everything still queued under that
    /// name takes the new one, since the batch writes the rename before any
    /// of it (#862): the Arena match writes (this poll's and the batch's),
    /// the course snapshots and run syncs in the batch, and the deck writes
    /// (in the batch or waiting). A match already stored needs nothing: its
    /// row points at the account row the rename moves.
    fn follow_rename(
        &mut self,
        batch: &mut WriteBatch,
        out: &mut ArenaPoll,
        user_id: &str,
        username: &str,
    ) {
        if let Some((named_id, old_name)) = &self.named_account
            && named_id == user_id
            && old_name != username
        {
            for write in &mut out.matches {
                rename_match_write(write, old_name, username);
            }
            for write in &mut batch.matches {
                let arena = batch.platform_matches.iter().any(|(token, platform)| {
                    token == write.token() && *platform == Platform::Arena
                });
                if arena {
                    rename_match_write(write, old_name, username);
                }
            }
            for observation in &mut batch.arena_course_observations {
                if &observation.account_username == old_name {
                    observation.account_username = username.to_string();
                }
            }
            if batch
                .arena_run_accounts
                .iter()
                .any(|queued| queued == old_name)
            {
                batch.arena_run_accounts.retain(|queued| queued != old_name);
                queue_arena_runs(batch, username);
            }
            self.courses.rename_account(old_name, username);
            self.deck_links.rename_account(old_name, username);
            for write in &mut batch.decks {
                rename_in_write(write, old_name, username);
            }
        }
        self.named_account = Some((user_id.to_string(), username.to_string()));
    }

    /// The writes of the rebuilt matches the store has no row for, as a live
    /// change writes them: the accounts, each match's opponent, then the
    /// match (its deck link was queued by `open`). An account other than the
    /// current one is written by name only (the projector keeps the current
    /// account's user id alone).
    fn write_unstored(&mut self, batch: &mut WriteBatch, now_ms: i64) -> ArenaPoll {
        let mut out = ArenaPoll::default();
        if self.unstored.is_empty() {
            return out;
        }
        self.write_account(batch, now_ms, &mut out);
        let current = self
            .written_account
            .as_ref()
            .map(|identity| identity.username.clone());
        for username in std::mem::take(&mut self.unstored_accounts) {
            if current.as_ref() == Some(&username) {
                continue;
            }
            batch.platform_accounts.push(PlatformAccount {
                platform: Platform::Arena,
                username,
                user_id: None,
                display_name: None,
                seen_at_ms: now_ms,
            });
            out.account_changed = true;
        }
        for token in self.unstored.clone() {
            let Some(write) = self.match_write(&token) else {
                continue;
            };
            if let MatchWrite::Upsert { view, .. } = &write
                && let Some(key) = &view.opponent
                && let Some(display_name) = self.opponent_names.get(key)
            {
                batch.platform_opponents.push(PlatformOpponent {
                    platform: Platform::Arena,
                    username: key.clone(),
                    display_name: display_name.clone(),
                });
            }
            out.matches.push(write);
        }
        out
    }

    /// Store an entry's game messages (and hold those whose match's kind is
    /// not said yet). A message's time is its own, else the entry's.
    fn capture_game_messages(
        &mut self,
        batch: &mut WriteBatch,
        file_id: i64,
        entry: &RawEntry,
        entry_ms: i64,
    ) {
        let ready = self.games.capture_entry(file_id, entry, entry_ms);
        if self.stores_game_data() {
            batch.arena_game_messages.extend(ready);
        }
    }

    /// Whether the projector tracks `token` (it has a view, Bo1 or Bo3), so
    /// its game messages are worth looking for.
    fn is_tracked(&self, token: &str) -> bool {
        self.projector.view(token).is_some()
    }

    /// The game messages of the `wanted` matches in the Arena logs still on
    /// disk: each `Player-prev.log`, then its `Player.log`, as a live run
    /// reads them.
    fn backfill(
        &self,
        reader: &Connection,
        wanted: &HashSet<String>,
        utc_offset_minutes: i32,
        now_ms: i64,
    ) -> Result<Vec<NewGameMessage>, TrackerError> {
        let mut capture = GameCapture::default();
        let mut found = Vec::new();
        for path in arena_log_paths(&self.env, &self.overrides) {
            for file in [prev_log_path(&path), path] {
                if let Some(messages) = backfill_file(
                    reader,
                    &file,
                    wanted,
                    &mut capture,
                    utc_offset_minutes,
                    now_ms,
                )? {
                    tracing::info!(
                        path = %file.display(),
                        messages = messages.len(),
                        "looked for game data of earlier Arena matches"
                    );
                    found.extend(messages);
                }
            }
        }
        Ok(found)
    }

    #[cfg(test)]
    fn stores_game_data(&self) -> bool {
        !self.game_data_off
    }

    #[cfg(not(test))]
    fn stores_game_data(&self) -> bool {
        true
    }

    /// A match's current view at its current revision, to push again once
    /// its game rows are written.
    fn current_view(&self, token: &str) -> Option<(MatchView, u64)> {
        let view = self.projector.view(token)?.clone();
        let revision = self.revisions.get(token).copied().unwrap_or(0);
        Some((view, revision))
    }

    /// The upsert of a match's current view, at its next revision.
    fn match_write(&mut self, token: &str) -> Option<MatchWrite> {
        let view = self.projector.view(token)?.clone();
        let revision = self.revisions.entry(token.to_string()).or_insert(0);
        *revision += 1;
        Some(MatchWrite::Upsert {
            view: Box::new(view),
            revision: *revision,
        })
    }

    /// The display name of the opponent a match view names by its registry
    /// key, once the projector told it.
    pub(crate) fn opponent_name(&self, key: Option<&str>) -> Option<String> {
        self.opponent_names.get(key?).cloned()
    }

    /// The local account a match view names.
    fn view_account(&self, token: &str) -> Option<String> {
        self.projector
            .view(token)
            .map(|view| view.local_username.clone())
    }

    /// The deck a match used becomes a deck file sync (Arena cards at their
    /// catalog id offset), and the match waits to be linked to it.
    fn queue_deck_use(
        &mut self,
        reader: &Connection,
        deck_use: DeckUse,
        account: &str,
        at_ms: i64,
        now_ms: i64,
    ) -> Result<(), TrackerError> {
        // A commander is a sideboard row, as MTGO's `.dek` holds it.
        let mut side = deck_use.side;
        side.extend(deck_use.commanders.iter().map(|grp| (*grp, 1)));
        let deck = DeckUseIds {
            match_token: deck_use.match_token,
            deck_id: deck_use.deck_id,
            deck_name: deck_use.deck_name,
            format: deck_use.format_code,
            main: catalog_pairs(&deck_use.main),
            side: catalog_pairs(&side),
        };
        let (write, link) = deck_sync_write(reader, deck, account, at_ms, now_ms, Platform::Arena)?;
        self.deck_links.push(write, link);
        Ok(())
    }

    /// Learn the header date order from the file's first `DAY_ORDER_BYTES`,
    /// looking again while the file is shorter than that and still unsettled
    /// (each read is looked at too, in `poll_file`).
    fn detect_day_order(&mut self, index: usize, size: u64) {
        let file = &mut self.files[index];
        if file.day_first.is_some()
            || file.day_scanned >= DAY_ORDER_BYTES
            || size <= file.day_scanned
        {
            return;
        }
        if let Ok(bytes) = read_start(&file.path, DAY_ORDER_BYTES) {
            file.day_scanned = bytes.len() as u64;
            file.day_first = detect_day_first(&bytes);
        }
    }

    /// Whether a file at `path` is being read.
    fn is_reading(&self, path: &Path) -> bool {
        self.files.iter().any(|file| file.path == path)
    }

    /// The unread part of `Player-prev.log`: from where a file with the same
    /// start was last read, or all of it when no file had it.
    fn start_prev(
        &mut self,
        reader: &Connection,
        writer: &WriterHandle,
        path: &Path,
        now_ms: i64,
    ) -> Result<(), TrackerError> {
        if self.is_reading(path) || !path.is_file() {
            return Ok(());
        }
        let Ok(probe) = identity::probe(path, None) else {
            return Ok(());
        };
        let start = arena_read_offset_for_head(reader, probe.head_hash, probe.head_len)?;
        let start = start.unwrap_or(0).max(self.sealed_offset(&probe));
        if start >= probe.size {
            return Ok(());
        }
        self.start_file(writer, path.to_path_buf(), start, now_ms)
    }

    /// How far this run read a sealed file whose head `probe` carries.
    fn sealed_offset(&self, probe: &identity::Probe) -> u64 {
        self.sealed
            .iter()
            .filter(|(hash, len, _)| *hash == probe.head_hash && *len == probe.head_len)
            .map(|(_, _, offset)| *offset)
            .max()
            .unwrap_or(0)
    }

    /// A new `arena_log_files` row for `path`, read from `start`. A file that
    /// vanished or is locked is tried again at the next rescan.
    fn start_file(
        &mut self,
        writer: &WriterHandle,
        path: PathBuf,
        start: u64,
        now_ms: i64,
    ) -> Result<(), TrackerError> {
        let Ok(probe) = identity::probe(&path, None) else {
            return Ok(());
        };
        let id = writer.create_arena_log_file(NewArenaLogFile {
            path: path.to_string_lossy().to_string(),
            identity_hash: identity::identity_hash(&probe),
            head_hash: probe.head_hash,
            head_len: probe.head_len,
            created_at_fs: probe.created_ms,
            seen_at_ms: now_ms,
        })?;
        if start > 0 {
            // Stored at once, so a restart never reads the part before it.
            writer.write(WriteBatch {
                arena_files: vec![ArenaFileProgress {
                    id,
                    last_observed_size: 0,
                    read_offset: start,
                    head_hash: probe.head_hash,
                    head_len: probe.head_len,
                    sealed: None,
                }],
                now_ms,
                ..WriteBatch::default()
            })?;
        }
        tracing::info!(path = %path.display(), start, "reading an Arena log");
        self.files.push(ArenaFile {
            id,
            recorded: Recorded::first(&probe),
            tailer: ArenaTailer::new(path.clone(), start),
            prev: is_prev_log(&path),
            path,
            day_first: None,
            day_scanned: 0,
            last_time_ms: None,
            saved: Some((start, 0, probe.head_len)),
            caught_up: false,
            last_live_growth_ms: None,
        });
        Ok(())
    }
}

/// Fold every stored Arena event through a fresh projector, in (file id,
/// offset, seq) order, each at its stored time. An event that no longer
/// reads back is skipped with a warning.
pub(crate) fn rebuild(reader: &Connection) -> Result<Rebuilt, TrackerError> {
    let mut rebuilt = Rebuilt {
        projector: ArenaProjector::new(),
        tokens: Vec::new(),
        deck_uses: Vec::new(),
        last_times: HashMap::new(),
        newest_ms: 0,
        opponent_names: HashMap::new(),
        match_events: Vec::new(),
        games: GameCapture::default(),
    };
    let mut named: HashSet<String> = HashSet::new();
    for stored in arena_events_in_order(reader)? {
        let event: ArenaEvent = match serde_json::from_str(&stored.raw) {
            Ok(event) => event,
            Err(error) => {
                tracing::warn!(
                    file_id = stored.file_id,
                    byte_offset = stored.byte_offset,
                    %error,
                    "a stored Arena event does not read back; skipped"
                );
                continue;
            }
        };
        let input = ArenaInput {
            file_id: stored.file_id as u64,
            offset: stored.byte_offset,
            seq: stored.seq,
            at_ms: stored.logged_at_ms,
            event,
        };
        rebuilt.games.note_stored_event(&input.event);
        if let Some((token, event_name)) = room_event_name(&input.event)
            && named.insert(token.clone())
        {
            rebuilt.match_events.push((token, event_name));
        }
        let changes = rebuilt.projector.apply(&input);
        rebuilt
            .last_times
            .insert(stored.file_id, stored.logged_at_ms);
        rebuilt.newest_ms = rebuilt.newest_ms.max(stored.logged_at_ms);
        for token in changes.matches {
            if !rebuilt.tokens.contains(&token) {
                rebuilt.tokens.push(token);
            }
        }
        for deck_use in changes.deck_uses {
            rebuilt.deck_uses.push((deck_use, stored.logged_at_ms));
        }
        rebuilt.opponent_names.extend(changes.opponents);
    }
    Ok(rebuilt)
}

impl Engine {
    /// At start, write the rebuilt Arena matches the store has no row for
    /// (Bo1 matches tracked before Bo1 had a view) in the first batch, as
    /// the live path writes a changed match, and push them.
    pub(super) fn plan_arena_unstored(&mut self, now_ms: i64) {
        let written = self.arena.write_unstored(&mut self.batch, now_ms);
        self.push_arena(&written.matches);
        self.queue_limited_runs(&written.matches);
        for write in written.matches {
            self.batch.push_match(write, Platform::Arena);
        }
        if written.account_changed {
            self.account_changed = true;
        }
    }

    /// At start, sync every Arena account's limited runs once, after the
    /// rebuild, so runs reach a database whose limited matches came before
    /// the runs were stored.
    pub(super) fn plan_arena_runs_resync(&mut self) {
        self.batch.arena_runs_resync = true;
    }

    /// Queue the run sync of the account that played `token`, when it is
    /// a stored limited Arena match: hidden, shown again or removed, it
    /// leaves or rejoins its run (#631). The MTGO league sync skips Arena
    /// accounts, so marking the match's owner alone does not do it.
    pub(super) fn queue_arena_runs_of_match(&mut self, token: &str) {
        match read::arena_limited_match_username(&self.reader, token) {
            Ok(Some(username)) => queue_arena_runs(&mut self.batch, &username),
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(%token, %error, "could not read a match's Arena account; its runs sync later");
            }
        }
    }

    /// Queue the run sync of each limited Arena match's account in this
    /// batch.
    pub(super) fn queue_limited_runs(&mut self, writes: &[MatchWrite]) {
        for write in writes {
            if let MatchWrite::Upsert { view, .. } = write
                && view.match_type.as_deref() == Some("limited")
            {
                queue_arena_runs(&mut self.batch, &view.local_username);
            }
        }
    }

    /// At start, look for the game messages of the Arena matches tracked
    /// before game messages were stored (Arena plan 3).
    ///
    /// The list is the stored matches without messages, and the settled
    /// matches `plan_arena_unstored` queued in this batch (one still in
    /// progress is looked for too, never archived or synced here). Each tracked
    /// one's messages are read once from the Arena logs still on disk; the
    /// first batch stores and archives them, then syncs every match on the
    /// list. A match whose messages are not found (or that the projector
    /// does not track) is stamped `no_messages` by that sync and is never
    /// looked for again. The logs are only read when a tracked match is on
    /// the list, which after the first start is rare. Runs on the tracker's
    /// thread, as `Engine::open` does.
    pub(super) fn plan_arena_backfill(&mut self, now_ms: i64) -> Result<(), TrackerError> {
        let mut tokens = store::arena_games::arena_tokens_without_messages(&self.reader)?;
        let mut wanted: HashSet<String> = tokens
            .iter()
            .filter(|token| self.arena.is_tracked(token))
            .cloned()
            .collect();
        // An unstored match still in progress gets its messages so far; it
        // is archived and synced when it ends, as a live one is.
        for token in std::mem::take(&mut self.arena.unstored) {
            let settled = self
                .arena
                .projector
                .view(&token)
                .is_some_and(|view| view.state != MatchState::InProgress);
            wanted.insert(token.clone());
            if settled && !tokens.contains(&token) {
                tokens.push(token);
            }
        }
        if tokens.is_empty() && wanted.is_empty() {
            return Ok(());
        }
        if !wanted.is_empty() {
            let utc_offset = utc_offset_minutes(self.timezone, now_ms);
            let messages = self
                .arena
                .backfill(&self.reader, &wanted, utc_offset, now_ms)?;
            self.batch.arena_game_messages.extend(messages);
        }
        for token in tokens {
            self.batch.arena_archive_tokens.push(token.clone());
            self.batch.arena_sync_tokens.push(token);
        }
        Ok(())
    }

    /// At start, derive the game rows of every Arena match the Arena game
    /// sync has not stamped (or stamped at an older version, or found no
    /// messages for that it has now). They go with the first batch.
    pub(super) fn plan_arena_game_sync(&mut self) -> Result<(), TrackerError> {
        for match_id in store::arena_game_sync::arena_matches_to_sync(&self.reader)? {
            let Some(token) = store::manual::match_token(&self.reader, match_id)? else {
                continue;
            };
            // Queued by the backfill already.
            if self.batch.arena_sync_tokens.contains(&token) {
                continue;
            }
            // Live messages that came after the match was archived (or a
            // match settled without a result, never archived) go into the
            // archive first, so the match is not due again.
            self.batch.arena_archive_tokens.push(token.clone());
            self.batch.arena_sync_tokens.push(token);
        }
        Ok(())
    }
}

impl Engine {
    /// Send the match-end syncs queued this tick, in one writer request
    /// after the batch with their results (sent already).
    pub(super) fn send_arena_syncs(&mut self) -> Result<(), TrackerError> {
        let tokens = std::mem::take(&mut self.arena.end_syncs);
        if tokens.is_empty() {
            return Ok(());
        }
        let reply = self.writer.sync_arena_games(tokens, self.batch.now_ms)?;
        self.arena.end_sync_replies.push(reply);
        Ok(())
    }

    /// Take the match-end syncs' answers: log the skipped matches and push
    /// again each visible match whose game rows changed, so the pages that
    /// read it at its result read the game rows now. Whether any is still
    /// in flight.
    pub(super) fn take_arena_sync_replies(&mut self) -> bool {
        let mut waiting = Vec::new();
        for reply in std::mem::take(&mut self.arena.end_sync_replies) {
            match reply.try_recv() {
                Ok(Ok((outcome, work))) => {
                    tracing::debug!(
                        target: "mymtgo::timing",
                        rows = work.rows,
                        micros = work.micros,
                        "arena_game_sync"
                    );
                    for token in &outcome.skipped {
                        tracing::warn!(%token, "an Arena match's games could not be synced: its messages could not be read");
                    }
                    for token in &outcome.changed_tokens {
                        if self.hidden.contains(token) {
                            continue;
                        }
                        if let Some((view, revision)) = self.arena.current_view(token) {
                            let opponent_name = self.arena.opponent_name(view.opponent.as_deref());
                            self.push_platform_view(view, revision, Platform::Arena, opponent_name);
                        }
                    }
                }
                Ok(Err(error)) => tracing::warn!(%error, "an Arena game sync failed"),
                Err(std::sync::mpsc::TryRecvError::Empty) => waiting.push(reply),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
            }
        }
        self.arena.end_sync_replies = waiting;
        !self.arena.end_sync_replies.is_empty()
    }
}

/// An Arena match write's account, `from` renamed to `to` (#862).
fn rename_match_write(write: &mut MatchWrite, from: &str, to: &str) {
    match write {
        MatchWrite::Upsert { view, .. } if view.local_username == from => {
            view.local_username = to.to_string();
        }
        MatchWrite::Skip { username, .. } if username == from => {
            *username = to.to_string();
        }
        _ => {}
    }
}

/// Queue `username`'s limited run sync in `batch`, once.
fn queue_arena_runs(batch: &mut WriteBatch, username: &str) {
    if !batch
        .arena_run_accounts
        .iter()
        .any(|queued| queued == username)
    {
        batch.arena_run_accounts.push(username.to_string());
    }
}

/// Whether a match write is of a settled match (decided by its result, or
/// settled without one): its game messages are archived and its game rows
/// derived after its batch.
fn is_settled(write: &MatchWrite) -> bool {
    match write {
        MatchWrite::Upsert { view, .. } => view.state != MatchState::InProgress,
        _ => false,
    }
}

/// Arena cards as `(catalog id, count)`: each grpId at its catalog id
/// offset. An id that cannot be one (a hostile log) is left out.
fn catalog_pairs(pairs: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let mut cards = Vec::new();
    for (grp, quantity) in pairs {
        if let Some(catalog_id) = checked_arena_catalog_id(*grp) {
            cards.push((catalog_id, *quantity));
        }
    }
    cards
}

/// The header date order the entries settle, if any: each entry is looked at
/// as the lines it was read from.
fn day_first_of_entries(entries: &[RawEntry]) -> Option<bool> {
    for entry in entries {
        let mut lines = entry.head.clone();
        lines.push('\n');
        if let Some(json) = &entry.json {
            lines.push_str(json);
            lines.push('\n');
        }
        if let Some(day_first) = detect_day_first(lines.as_bytes()) {
            return Some(day_first);
        }
    }
    None
}

/// An entry's time: its header time, else the file's last event time, else
/// now. An ambiguous date with the order still unknown takes the reading
/// nearest the file's last event, else its last write (a backlog), else now.
fn entry_time(
    clock: &HeaderClock,
    entry: &RawEntry,
    last_time_ms: Option<i64>,
    modified_ms: Option<i64>,
    now_ms: i64,
) -> i64 {
    let reference_ms = last_time_ms.or(modified_ms).unwrap_or(now_ms);
    header_time(clock, &entry.head, reference_ms)
        .or(last_time_ms)
        .unwrap_or(now_ms)
}

/// The game messages of the `wanted` matches in one Arena log, read from its
/// start as `poll_file` reads it (the same reads, times and capture). Only a
/// file some `arena_log_files` row started with is read, and its messages
/// take that row's id, so they carry the keys a live read gave them. `None`
/// when the file is missing, unreadable or unknown.
fn backfill_file(
    reader: &Connection,
    path: &Path,
    wanted: &HashSet<String>,
    capture: &mut GameCapture,
    utc_offset_minutes: i32,
    now_ms: i64,
) -> Result<Option<Vec<NewGameMessage>>, TrackerError> {
    let Ok(probe) = identity::probe(path, None) else {
        return Ok(None);
    };
    let Some(file_id) = arena_file_for_head(reader, probe.head_hash, probe.head_len)? else {
        return Ok(None);
    };
    let mut clock = HeaderClock {
        day_first: read_start(path, DAY_ORDER_BYTES)
            .ok()
            .and_then(|bytes| detect_day_first(&bytes)),
        utc_offset_minutes,
    };
    let mut tailer = ArenaTailer::new(path.to_path_buf(), 0);
    let mut last_time_ms = None;
    let mut found = Vec::new();
    // Each read moves at least one byte on or ends the loop; the cap only
    // guards against a file that shrinks while it is read.
    for _ in 0..=(probe.size / MAX_READ + 1) {
        let Ok(read) = tailer.poll(probe.size) else {
            tracing::warn!(path = %path.display(), "an Arena log could not be read for game data");
            return Ok(None);
        };
        if clock.day_first.is_none() {
            clock.day_first = day_first_of_entries(&read.entries);
        }
        for entry in &read.entries {
            let entry_ms = entry_time(&clock, entry, last_time_ms, probe.modified_ms, now_ms);
            for event in decode_all(entry) {
                last_time_ms = Some(own_time(&event).unwrap_or(entry_ms));
                capture.note_event(&event);
            }
            for message in capture.capture_entry(file_id, entry, entry_ms) {
                if wanted.contains(&message.match_token) {
                    found.push(message);
                }
            }
        }
        if read.at_eof {
            break;
        }
    }
    Ok(Some(found))
}

/// The event's own time (a GRE or client message time), when it has one.
fn own_time(event: &ArenaEvent) -> Option<i64> {
    match event {
        ArenaEvent::RoomState { at_ms, .. }
        | ArenaEvent::DeckSubmitted { at_ms, .. }
        | ArenaEvent::Mulligan { at_ms, .. } => *at_ms,
        ArenaEvent::GameInfo { at_ms, .. } if *at_ms > 0 => Some(*at_ms),
        _ => None,
    }
}

/// The match an event names, for `arena_log_events.match_token`.
fn event_match_token(event: &ArenaEvent) -> Option<String> {
    let match_id = match event {
        ArenaEvent::RoomState { match_id, .. }
        | ArenaEvent::GameInfo { match_id, .. }
        | ArenaEvent::FirstTurn { match_id, .. } => match_id,
        _ => return None,
    };
    (!match_id.is_empty()).then(|| match_id.clone())
}

/// Minutes `tz` is ahead of UTC at `now_ms`.
pub(crate) fn utc_offset_minutes(tz: Tz, now_ms: i64) -> i32 {
    let Some(now) = DateTime::from_timestamp_millis(now_ms) else {
        return 0;
    };
    tz.offset_from_utc_datetime(&now.naive_utc())
        .fix()
        .local_minus_utc()
        / 60
}

/// `true` for a `Player-prev.log` path.
fn is_prev_log(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy() == arena_log::PLAYER_PREV_LOG)
}

#[cfg(test)]
mod tests {
    use fixture_tools::TestDir;

    use super::*;
    use cards::ARENA_ID_OFFSET;

    use crate::engine::test_support::open;
    use crate::engine::{Engine, RESCAN_EVERY_MS};
    use crate::{TrackerConfig, TrackerEvent};
    use arena_protocol::Seat;
    use decks::DeckCard;
    use store::decks::DeckWrite;
    use store::match_edits::{MatchEdit, MatchEditOutcome};

    /// After the fixture's last event, so its matches are settled.
    const NOW: i64 = 1_793_720_000_000;

    /// How many events of an entry reach `arena_log_events` (course
    /// snapshots do not).
    fn stored_event_count(entry: &RawEntry) -> usize {
        decode_all(entry)
            .iter()
            .filter(|event| !matches!(event, ArenaEvent::Courses { .. }))
            .count()
    }
    const PLAYER_LOG: &str = "arena/Player.log";
    const PREV_LOG: &str = "arena/Player-prev.log";

    fn fixture_bytes() -> Vec<u8> {
        fixture_tools::arena_fixture("bo3-loss-and-match-concede").bytes
    }

    fn config(dir: &TestDir) -> TrackerConfig {
        TrackerConfig {
            db_path: dir.join("data/mymtgo.sqlite"),
            timezone: chrono_tz::UTC,
            os_timezone: None,
            log_overrides: vec![dir.join("logs")],
            arena_log_overrides: vec![dir.join(PLAYER_LOG)],
            mtgo_data_folder: Some(dir.join("mtgo-data")),
            replay_day_anchor: None,
            watching: true,
            arena_enabled: true,
            endstep_enabled: false,
        }
    }

    /// Tick until every file is read, then a few rescans more, and commit.
    fn run(engine: &mut Engine, start_ms: i64) -> i64 {
        let mut now = start_ms;
        for _ in 0..1_000 {
            let tick = engine.tick(now).expect("tick");
            now += 1;
            if !tick.more {
                break;
            }
        }
        for _ in 0..3 {
            now += RESCAN_EVERY_MS;
            engine.tick(now).expect("tick");
        }
        engine.flush().expect("flush");
        now
    }

    fn reader(config: &TrackerConfig) -> Connection {
        store::open_reader(&config.db_path).expect("reader")
    }

    fn number(config: &TrackerConfig, sql: &str) -> i64 {
        reader(config)
            .query_row(sql, [], |row| row.get(0))
            .expect("number")
    }

    fn texts(config: &TrackerConfig, sql: &str) -> Vec<String> {
        let conn = reader(config);
        let mut stmt = conn.prepare(sql).expect("prepare");
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query");
        rows.collect::<Result<_, _>>().expect("rows")
    }

    /// Every row of `table` as text, every column, in id order.
    fn dump(config: &TrackerConfig, table: &str) -> Vec<String> {
        let columns = texts(
            config,
            &format!("SELECT 'quote(\"' || name || '\")' FROM pragma_table_info('{table}')"),
        );
        texts(
            config,
            &format!(
                "SELECT {} FROM {table} ORDER BY id",
                columns.join(" || '|' || ")
            ),
        )
    }

    /// Every row of `table` as text, every column, sorted as text (for
    /// tables without an `id`).
    fn dump_rows(config: &TrackerConfig, table: &str) -> Vec<String> {
        let mut rows = texts(
            config,
            &format!(
                "SELECT {} FROM {table}",
                texts(
                    config,
                    &format!(
                        "SELECT 'quote(\"' || name || '\")' FROM pragma_table_info('{table}')"
                    ),
                )
                .join(" || '|' || ")
            ),
        );
        rows.sort();
        rows
    }

    /// A tracker that read the fixture as `Player.log`.
    fn tracked_fixture(label: &str) -> (TestDir, TrackerConfig, i64) {
        tracked_bytes(label, &fixture_bytes())
    }

    /// A tracker that read `bytes` as `Player.log`.
    fn tracked_bytes(label: &str, bytes: &[u8]) -> (TestDir, TrackerConfig, i64) {
        let dir = TestDir::new(label);
        dir.write(PLAYER_LOG, bytes);
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let now = run(&mut engine, NOW);
        engine.finish().expect("finish");
        (dir, config, now)
    }

    #[test]
    fn arena_fixture_produces_two_matches() {
        let (_dir, config, _) = tracked_fixture("arena-two-matches");
        assert_eq!(
            texts(
                &config,
                "SELECT m.outcome || ' ' || m.format || ' ' || a.platform || ' ' || a.username
                 FROM matches m JOIN accounts a ON a.id = m.account_id ORDER BY m.started_at"
            ),
            vec![
                "loss CPIONEER arena Player#00001".to_string(),
                "loss CPIONEER arena Player#00001".to_string(),
            ]
        );
        assert_eq!(
            number(
                &config,
                "SELECT COUNT(*) FROM opponents o JOIN matches m ON m.opponent_id = o.id
                 WHERE o.platform = 'arena' AND o.username LIKE 'arena:%'"
            ),
            2
        );
    }

    /// Tick like `run`, counting the `AccountChanged` events pushed.
    fn run_counting_account_changes(engine: &mut Engine, start_ms: i64) -> usize {
        let mut count = 0;
        let mut now = start_ms;
        for _ in 0..1_000 {
            let tick = engine.tick(now).expect("tick");
            now += 1;
            count += tick
                .events
                .iter()
                .filter(|event| matches!(event, crate::TrackerEvent::AccountChanged))
                .count();
            if !tick.more {
                break;
            }
        }
        for _ in 0..3 {
            now += RESCAN_EVERY_MS;
            let tick = engine.tick(now).expect("tick");
            count += tick
                .events
                .iter()
                .filter(|event| matches!(event, crate::TrackerEvent::AccountChanged))
                .count();
        }
        engine.flush().expect("flush");
        count
    }

    /// A hidden Arena match shown again is pushed live as stored, on Arena,
    /// with its opponent's shown name (#546).
    #[test]
    fn an_arena_match_shown_again_is_pushed() {
        let (_dir, config, now) = tracked_fixture("arena-shown-again");
        let token = texts(
            &config,
            "SELECT token FROM matches ORDER BY started_at LIMIT 1",
        )
        .remove(0);
        let mut engine = open(&config, now);
        let now = run(&mut engine, now);
        let outcome = engine
            .edit_match(MatchEdit::Hide {
                token: token.clone(),
                now_ms: now,
            })
            .expect("hide");
        assert_eq!(outcome, MatchEditOutcome::Done);
        engine.tick(now).expect("tick");
        let outcome = engine
            .edit_match(MatchEdit::Restore {
                token: token.clone(),
                now_ms: now,
            })
            .expect("restore");
        assert_eq!(outcome, MatchEditOutcome::Done);
        let tick = engine.tick(now + 1).expect("tick");
        let pushed: Vec<(String, Platform, bool, bool)> = tick
            .events
            .iter()
            .filter_map(|event| match event {
                TrackerEvent::MatchUpdated {
                    view,
                    platform,
                    manual,
                    opponent_name,
                    ..
                } => Some((
                    view.token.clone(),
                    *platform,
                    *manual,
                    opponent_name.is_some(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(pushed, vec![(token, Platform::Arena, false, true)]);
        engine.finish().expect("finish");
    }

    /// Arena off: the log is never discovered or read, so no event is stored.
    /// Switched on, the next tick rescans and reads it.
    #[test]
    fn a_switched_off_arena_reads_nothing() {
        let dir = TestDir::new("arena-switched-off");
        dir.write(PLAYER_LOG, &fixture_bytes());
        let mut config = config(&dir);
        config.arena_enabled = false;
        let mut engine = open(&config, NOW);
        run(&mut engine, NOW);
        assert_eq!(number(&config, "SELECT COUNT(*) FROM arena_log_events"), 0);
        assert_eq!(number(&config, "SELECT COUNT(*) FROM arena_log_files"), 0);
        assert!(!engine.arena_enabled());

        engine.set_arena_enabled(true);
        run(&mut engine, NOW + 100_000);
        assert!(number(&config, "SELECT COUNT(*) FROM arena_log_events") > 0);
        engine.finish().expect("finish");
    }

    /// #862: the fixture's matches, then the player renamed (a login line
    /// under the new name and the same user id), read in one go. The
    /// matches and their decks follow the renamed account: no account is
    /// made under the old name, on any platform.
    #[test]
    fn matches_read_with_a_later_rename_follow_the_renamed_account() {
        let mut bytes = fixture_bytes();
        bytes.extend_from_slice(
            b"[Accounts - Login] Logged in successfully. Display Name: Qrenamed#00002\n\
[UnityCrossThreadLogger]03/11/2026 15:00:00: Match to PLAYER0000000000000000000A: AuthenticateResponse\n\
{ \"transactionId\": \"00000000-0000-4000-8000-000000000009\", \"requestId\": 1, \"timestamp\": \"639293123000000000\", \"authenticateResponse\": { \"clientId\": \"PLAYER0000000000000000000A\", \"sessionId\": \"00000000-0000-4000-8000-000000000009\", \"screenName\":\"Qrenamed\" } }\n",
        );
        let (_dir, config, _) = tracked_bytes("arena-rename-in-one-go", &bytes);
        assert_eq!(
            texts(
                &config,
                "SELECT a.platform || ' ' || a.username || ' ' || COALESCE(a.arena_user_id, '-')
                 FROM matches m JOIN accounts a ON a.id = m.account_id ORDER BY m.started_at"
            ),
            vec![
                "arena Qrenamed#00002 PLAYER0000000000000000000A".to_string(),
                "arena Qrenamed#00002 PLAYER0000000000000000000A".to_string(),
            ],
        );
        assert_eq!(
            texts(
                &config,
                "SELECT platform || ' ' || username || ' ' || COALESCE(arena_user_id, '-')
                 FROM accounts ORDER BY id"
            ),
            vec!["arena Qrenamed#00002 PLAYER0000000000000000000A".to_string()],
            "one account, renamed"
        );
        assert_eq!(
            texts(
                &config,
                "SELECT DISTINCT a.username FROM decks d JOIN accounts a ON a.id = d.account_id"
            ),
            vec!["Qrenamed#00002".to_string()],
        );
        assert_eq!(
            number(
                &config,
                "SELECT COUNT(*) FROM matches WHERE deck_version_id IS NULL"
            ),
            0,
            "every match is linked to its deck"
        );
    }

    /// A new Arena account flips the platform context, so pages refetch: one
    /// `AccountChanged` when it appears, none while it stays the same.
    #[test]
    fn an_arena_account_appearing_reports_account_changed_once() {
        let dir = TestDir::new("arena-account-changed");
        let bytes = fixture_bytes();
        dir.write(PLAYER_LOG, &bytes);
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        assert_eq!(run_counting_account_changes(&mut engine, NOW), 1);

        // More of the same log, the same account: nothing to report.
        let mut more = bytes.clone();
        more.extend_from_slice(&bytes);
        dir.write(PLAYER_LOG, &more);
        assert_eq!(run_counting_account_changes(&mut engine, NOW + 100_000), 0);
        engine.finish().expect("finish");

        // A restart rebuilds the account from the stored events: still none.
        let mut engine = open(&config, NOW + 200_000);
        assert_eq!(run_counting_account_changes(&mut engine, NOW + 200_000), 0);
        engine.finish().expect("finish");
    }

    /// The `RankChanged` events of one tick: never more than one.
    fn rank_changes(tick: &crate::engine::Tick) -> usize {
        let count = tick
            .events
            .iter()
            .filter(|event| matches!(event, crate::TrackerEvent::RankChanged))
            .count();
        assert!(count <= 1, "at most one per tick");
        count
    }

    /// Tick like `run`, counting the `RankChanged` events pushed.
    fn run_counting_rank_changes(engine: &mut Engine, start_ms: i64) -> usize {
        let mut count = 0;
        let mut now = start_ms;
        for _ in 0..1_000 {
            let tick = engine.tick(now).expect("tick");
            now += 1;
            count += rank_changes(&tick);
            if !tick.more {
                break;
            }
        }
        for _ in 0..3 {
            now += RESCAN_EVERY_MS;
            count += rank_changes(&engine.tick(now).expect("tick"));
        }
        engine.flush().expect("flush");
        count
    }

    /// Stored rank readings tell the rank views to refetch: once after the
    /// backlog, none when nothing new is read.
    #[test]
    fn stored_rank_observations_push_rank_changed() {
        let dir = TestDir::new("arena-rank-changed");
        dir.write(PLAYER_LOG, &fixture_bytes());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        assert_eq!(run_counting_rank_changes(&mut engine, NOW), 1);
        assert!(number(&config, "SELECT COUNT(*) FROM arena_rank_observations") > 0);
        engine.finish().expect("finish");

        // A restart reads nothing new: no rank change.
        let mut engine = open(&config, NOW + 200_000);
        assert_eq!(run_counting_rank_changes(&mut engine, NOW + 200_000), 0);
        engine.finish().expect("finish");
    }

    #[test]
    fn arena_matches_are_pushed_with_their_platform() {
        let dir = TestDir::new("arena-pushed");
        dir.write(PLAYER_LOG, &fixture_bytes());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let mut pushed: Vec<(String, store::Platform, Option<String>)> = Vec::new();
        for now in (NOW..).take(1_000) {
            let tick = engine.tick(now).expect("tick");
            for event in tick.events {
                if let crate::TrackerEvent::MatchUpdated {
                    view,
                    platform,
                    opponent_name,
                    ..
                } = event
                {
                    pushed.push((view.token, platform, opponent_name));
                }
            }
            if !tick.more {
                break;
            }
        }
        engine.finish().expect("finish");
        let mut tokens: Vec<String> = pushed.iter().map(|(token, _, _)| token.clone()).collect();
        tokens.sort();
        tokens.dedup();
        assert_eq!(
            tokens,
            texts(&config, "SELECT token FROM matches ORDER BY token"),
            "every stored match is pushed"
        );
        assert_eq!(tokens.len(), 2);
        // Each push names the opponent as stored (the fixture's two are
        // `Opponent` and `Opponent2`).
        let stored = texts(
            &config,
            "SELECT m.token || ' ' || o.display_name FROM matches m
             JOIN opponents o ON o.id = m.opponent_id ORDER BY m.token",
        );
        assert!(stored.iter().any(|row| row.ends_with(" Opponent")));
        for (token, platform, name) in &pushed {
            assert_eq!(*platform, store::Platform::Arena, "{token}");
            let expected = format!("{token} {}", name.as_deref().unwrap_or("none"));
            assert!(stored.contains(&expected), "{expected}");
        }
    }

    #[test]
    fn arena_game_cards_use_the_offset() {
        let (_dir, config, _) = tracked_fixture("arena-offset");
        let signatures = texts(&config, "SELECT signature FROM deck_versions");
        assert!(!signatures.is_empty());
        for signature in signatures {
            let cards = decks::decode_signature(&signature);
            assert!(!cards.is_empty());
            assert!(cards.iter().all(|card| card.catalog_id >= ARENA_ID_OFFSET));
        }
        assert_eq!(
            number(
                &config,
                &format!("SELECT COUNT(*) FROM cards WHERE catalog_id < {ARENA_ID_OFFSET}")
            ),
            0,
            "no raw grpId is ever a card row"
        );
        assert!(number(&config, "SELECT COUNT(*) FROM cards") > 0);
    }

    /// #690: an Arena deck the sync wrote without an archetype is told to
    /// the archetypes worker, as MTGO's deck files are.
    #[test]
    fn a_synced_arena_deck_asks_for_an_archetype() {
        let dir = TestDir::new("arena-deck-needs-archetype");
        dir.write(PLAYER_LOG, &fixture_bytes());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let mut asked = Vec::new();
        let mut now = NOW;
        for _ in 0..1_000 {
            let tick = engine.tick(now).expect("tick");
            for event in tick.events {
                if let TrackerEvent::DeckNeedsArchetype { deck_ids } = event {
                    asked.extend(deck_ids);
                }
            }
            now += 1;
            if !tick.more {
                break;
            }
        }
        for _ in 0..3 {
            now += RESCAN_EVERY_MS;
            for event in engine.tick(now).expect("tick").events {
                if let TrackerEvent::DeckNeedsArchetype { deck_ids } = event {
                    asked.extend(deck_ids);
                }
            }
        }
        engine.finish().expect("finish");
        asked.sort_unstable();
        asked.dedup();
        let decks: Vec<i64> = texts(
            &config,
            "SELECT CAST(d.id AS TEXT) FROM decks d JOIN accounts a ON a.id = d.account_id
             WHERE a.platform = 'arena' ORDER BY d.id",
        )
        .iter()
        .map(|id| id.parse().expect("id"))
        .collect();
        assert!(!decks.is_empty(), "the matches wrote their decks");
        assert_eq!(asked, decks);
    }

    #[test]
    fn arena_match_links_its_deck() {
        let (_dir, config, _) = tracked_fixture("arena-link");
        assert_eq!(
            number(
                &config,
                "SELECT COUNT(*) FROM matches m JOIN deck_versions v ON v.id = m.deck_version_id
                 JOIN decks d ON d.id = v.deck_id JOIN accounts a ON a.id = d.account_id
                 WHERE a.platform = 'arena'"
            ),
            2
        );
    }

    #[test]
    fn arena_rank_observations_are_stored() {
        let (_dir, config, _) = tracked_fixture("arena-rank");
        assert!(number(&config, "SELECT COUNT(*) FROM arena_rank_observations") > 0);
        assert_eq!(
            texts(
                &config,
                "SELECT DISTINCT CAST(season AS TEXT) FROM arena_rank_observations"
            ),
            vec!["95".to_string()]
        );
    }

    #[test]
    fn arena_rebuild_equals_live() {
        let (_dir, config, now) = tracked_fixture("arena-rebuild");
        let matches = dump(&config, "matches");
        let games = dump(&config, "games");
        let events = number(&config, "SELECT COUNT(*) FROM arena_log_events");
        assert_eq!(matches.len(), 2);

        let mut engine = open(&config, now);
        run(&mut engine, now);
        engine.finish().expect("finish");
        assert_eq!(dump(&config, "matches"), matches);
        assert_eq!(dump(&config, "games"), games);
        assert_eq!(
            number(&config, "SELECT COUNT(*) FROM arena_log_events"),
            events
        );
    }

    #[test]
    fn a_rebuilt_projector_equals_the_live_one() {
        let (_dir, config, _) = tracked_fixture("arena-rebuild-views");
        let rebuilt = rebuild(&reader(&config)).expect("rebuild");
        assert_eq!(rebuilt.tokens.len(), 2);
        let mut live = ArenaProjector::new();
        let bytes = fixture_bytes();
        let clock = HeaderClock {
            day_first: detect_day_first(&bytes[..DAY_ORDER_BYTES as usize]),
            utc_offset_minutes: 0,
        };
        let mut last = None;
        for entry in arena_protocol::split_complete(&bytes) {
            let entry_ms = header_time(&clock, &entry.head, last.unwrap_or(NOW))
                .or(last)
                .unwrap_or(NOW);
            for (seq, event) in decode_all(&entry).into_iter().enumerate() {
                let at_ms = own_time(&event).unwrap_or(entry_ms);
                last = Some(at_ms);
                live.apply(&ArenaInput {
                    file_id: 1,
                    offset: entry.offset,
                    seq: seq as u32,
                    at_ms,
                    event,
                });
            }
        }
        for token in &rebuilt.tokens {
            assert_eq!(rebuilt.projector.view(token), live.view(token));
        }
        assert_eq!(rebuilt.projector.account(), live.account());
    }

    #[test]
    fn truncated_player_log_starts_a_new_file() {
        let (dir, config, now) = tracked_fixture("arena-truncate");
        let old_events = number(&config, "SELECT COUNT(*) FROM arena_log_events");
        let bytes = fixture_bytes();
        let head = &bytes[..2048];
        dir.write(PLAYER_LOG, head);

        let mut engine = open(&config, now);
        run(&mut engine, now);
        engine.finish().expect("finish");

        assert_eq!(
            texts(
                &config,
                "SELECT COALESCE(seal_reason, 'open') FROM arena_log_files ORDER BY id"
            ),
            vec!["truncated".to_string(), "open".to_string()]
        );
        assert_eq!(
            number(
                &config,
                "SELECT COUNT(*) FROM arena_log_events WHERE arena_log_file_id = 1"
            ),
            old_events,
            "the old file's events stay as they were"
        );
        let expected: usize = arena_protocol::split_complete(head)
            .iter()
            .map(stored_event_count)
            .sum();
        assert_eq!(
            number(
                &config,
                "SELECT COUNT(*) FROM arena_log_events WHERE arena_log_file_id = 2"
            ),
            expected as i64,
            "the new file is read once from its start"
        );
        assert_eq!(number(&config, "SELECT COUNT(*) FROM matches"), 2);
    }

    #[test]
    fn prev_log_is_read_once_at_startup() {
        let dir = TestDir::new("arena-prev");
        dir.write(PREV_LOG, &fixture_bytes());
        dir.write(PLAYER_LOG, b"");
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let now = run(&mut engine, NOW);
        engine.finish().expect("finish");
        assert_eq!(
            texts(&config, "SELECT outcome FROM matches ORDER BY started_at"),
            vec!["loss".to_string(), "loss".to_string()]
        );
        let events = number(&config, "SELECT COUNT(*) FROM arena_log_events");
        let files = dump(&config, "arena_log_files");
        assert!(
            files.iter().any(|row| row.contains("read_once")),
            "{files:?}"
        );

        let mut engine = open(&config, now);
        run(&mut engine, now);
        engine.finish().expect("finish");
        assert_eq!(
            number(&config, "SELECT COUNT(*) FROM arena_log_events"),
            events
        );
        assert_eq!(dump(&config, "arena_log_files").len(), files.len());
    }

    #[test]
    fn a_prev_log_continues_where_its_old_file_stopped() {
        let dir = TestDir::new("arena-prev-continue");
        let bytes = fixture_bytes();
        let cut = bytes.len() / 2;
        let cut = cut + bytes[cut..].iter().position(|b| *b == b'\n').expect("line") + 1;
        dir.write(PLAYER_LOG, &bytes[..cut]);
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let now = run(&mut engine, NOW);
        engine.finish().expect("finish");
        let read_first = number(&config, "SELECT COUNT(*) FROM arena_log_events");

        // Arena relaunched while the app was closed: the old log, now whole,
        // is Player-prev.log and Player.log starts again.
        dir.write(PREV_LOG, &bytes);
        dir.write(PLAYER_LOG, b"");
        let mut engine = open(&config, now);
        run(&mut engine, now);
        engine.finish().expect("finish");
        let all: usize = arena_protocol::split_complete(&bytes)
            .iter()
            .map(stored_event_count)
            .sum();
        assert!(read_first < all as i64);
        assert_eq!(
            number(&config, "SELECT COUNT(*) FROM arena_log_events"),
            all as i64,
            "every event once: the first part from Player.log, the rest from Player-prev.log"
        );
        assert_eq!(number(&config, "SELECT COUNT(*) FROM matches"), 2);
    }

    #[test]
    fn a_log_first_seen_short_is_known_again_as_player_prev_log() {
        let dir = TestDir::new("arena-short-head");
        let bytes = fixture_bytes();
        let cut = 1024
            + bytes[1024..]
                .iter()
                .position(|b| *b == b'\n')
                .expect("line")
            + 1;
        dir.write(PLAYER_LOG, &bytes[..cut]);
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let now = run(&mut engine, NOW);
        // The session goes on past the 4 KiB head.
        dir.append(PLAYER_LOG, &bytes[cut..]);
        let now = run(&mut engine, now);
        engine.finish().expect("finish");
        let events = number(&config, "SELECT COUNT(*) FROM arena_log_events");
        let ranks = number(&config, "SELECT COUNT(*) FROM arena_rank_observations");
        assert!(ranks > 0);

        // Arena relaunched while the app was closed.
        dir.write(PREV_LOG, &bytes);
        dir.write(PLAYER_LOG, b"");
        let mut engine = open(&config, now);
        run(&mut engine, now);
        engine.finish().expect("finish");
        assert_eq!(
            number(&config, "SELECT COUNT(*) FROM arena_log_events"),
            events,
            "the session is not read twice"
        );
        assert_eq!(
            number(&config, "SELECT COUNT(*) FROM arena_rank_observations"),
            ranks
        );
    }

    #[test]
    fn a_backlog_keeps_its_header_times() {
        // Read long after the session: the fixture's first 64 KiB settle no
        // date order and every date is 03/11, so each rank response's header
        // time comes from the reading nearest the file's own times.
        let dir = TestDir::new("arena-prev-header-times");
        let bytes = fixture_bytes();
        dir.write(PREV_LOG, &bytes);
        dir.write(PLAYER_LOG, b"");
        let config = config(&dir);
        let later = NOW + 40 * 24 * 3_600_000;
        let mut engine = open(&config, later);
        run(&mut engine, later);
        engine.finish().expect("finish");

        let clock = HeaderClock {
            day_first: Some(true),
            utc_offset_minutes: 0,
        };
        let expected: Vec<String> = arena_protocol::split_complete(&bytes)
            .iter()
            .filter(|entry| {
                decode_all(entry)
                    .iter()
                    .any(|event| matches!(event, ArenaEvent::Rank { constructed, .. } if constructed.season == 95))
            })
            .map(|entry| {
                let at_ms = header_time(&clock, &entry.head, 0).expect("a header time");
                format!("{} {at_ms}", entry.offset)
            })
            .collect();
        assert!(!expected.is_empty());
        assert_eq!(
            texts(
                &config,
                "SELECT DISTINCT byte_offset || ' ' || observed_at FROM arena_rank_observations
                 WHERE season = 95 ORDER BY byte_offset"
            ),
            expected
        );
    }

    #[test]
    fn a_restart_while_paused_reads_the_rest_from_player_prev_log() {
        let dir = TestDir::new("arena-restart-paused");
        let bytes = fixture_bytes();
        let cut = bytes.len() / 2;
        let cut = cut + bytes[cut..].iter().position(|b| *b == b'\n').expect("line") + 1;
        dir.write(PLAYER_LOG, &bytes[..cut]);
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let now = run(&mut engine, NOW);
        let read_first = number(&config, "SELECT COUNT(*) FROM arena_log_events");

        // Paused while Arena finishes the session, quits and starts again.
        engine.set_watching(false);
        let now = run(&mut engine, now);
        dir.write(PREV_LOG, &bytes);
        dir.write(PLAYER_LOG, b"[UnityCrossThreadLogger]Qrestarted\n");
        engine.set_watching(true);
        run(&mut engine, now);
        engine.finish().expect("finish");

        let all: usize = arena_protocol::split_complete(&bytes)
            .iter()
            .map(stored_event_count)
            .sum();
        assert!(read_first < all as i64);
        assert_eq!(
            number(&config, "SELECT COUNT(*) FROM arena_log_events"),
            all as i64,
            "every event once: the rest of the session from Player-prev.log"
        );
        assert_eq!(number(&config, "SELECT COUNT(*) FROM matches"), 2);
    }

    #[test]
    fn player_prev_log_then_player_log_equals_one_read_and_a_rebuild() {
        let (_whole_dir, whole, _) = tracked_fixture("arena-prev-whole");
        let dir = TestDir::new("arena-prev-then-player");
        let bytes = fixture_bytes();
        let cut = bytes.len() / 2;
        let cut = cut + bytes[cut..].iter().position(|b| *b == b'\n').expect("line") + 1;
        dir.write(PLAYER_LOG, &bytes[..cut]);
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let now = run(&mut engine, NOW);
        engine.set_watching(false);
        let now = run(&mut engine, now);
        dir.write(PREV_LOG, &bytes);
        dir.write(PLAYER_LOG, b"[UnityCrossThreadLogger]Qrestarted\n");
        engine.set_watching(true);
        let now = run(&mut engine, now);
        engine.finish().expect("finish");

        // The columns a read decides; stamps, revisions and file ids differ.
        let facts = |config: &TrackerConfig| {
            texts(
                config,
                "SELECT m.token || '|' || m.state || '|' || m.outcome || '|' || m.format || '|'
                     || m.started_at || '|' || COALESCE(m.ended_at, '') || '|'
                     || (SELECT group_concat(g.game_number || g.result || COALESCE(g.on_play, ''), ',')
                         FROM games g WHERE g.match_id = m.id)
                 FROM matches m ORDER BY m.started_at",
            )
        };
        let before = facts(&config);
        assert_eq!(before.len(), 2);
        assert_eq!(before, facts(&whole), "the one-file read");
        let events = number(&config, "SELECT COUNT(*) FROM arena_log_events");

        // A start rebuilds from the stored events and changes nothing.
        let mut engine = open(&config, now);
        run(&mut engine, now);
        engine.finish().expect("finish");
        assert_eq!(facts(&config), before, "the rebuild");
        assert_eq!(
            number(&config, "SELECT COUNT(*) FROM arena_log_events"),
            events
        );
    }

    #[test]
    fn a_relaunch_while_the_writer_is_behind_stores_each_event_once() {
        let dir = TestDir::new("arena-relaunch-writer-behind");
        let bytes = fixture_bytes();
        let line_after =
            |at: usize| at + bytes[at..].iter().position(|b| *b == b'\n').expect("line") + 1;
        let cut = line_after(bytes.len() / 3);
        let later = line_after(2 * bytes.len() / 3);
        dir.write(PLAYER_LOG, &bytes[..cut]);
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let now = run(&mut engine, NOW);

        // More of the session is read, then Arena relaunches, all before the
        // writer has stored that read or the seal.
        dir.write(PLAYER_LOG, &bytes[..later]);
        let mut behind = WriteBatch::default();
        engine
            .arena
            .poll(&engine.reader, &mut behind, now, engine.timezone)
            .expect("read more");
        dir.write(PREV_LOG, &bytes);
        dir.write(PLAYER_LOG, b"[UnityCrossThreadLogger]Qrestarted\n");
        engine
            .arena
            .poll(&engine.reader, &mut behind, now, engine.timezone)
            .expect("seal");
        engine
            .arena
            .rescan(&engine.reader, &engine.writer, now)
            .expect("rescan");
        engine.writer.write(behind).expect("write");
        run(&mut engine, now);
        engine.finish().expect("finish");

        let all: usize = arena_protocol::split_complete(&bytes)
            .iter()
            .map(stored_event_count)
            .sum();
        assert_eq!(
            number(&config, "SELECT COUNT(*) FROM arena_log_events"),
            all as i64,
            "every event once"
        );
    }

    #[test]
    fn a_paused_startup_keeps_its_links_until_watching() {
        let (_dir, mut config, now) = tracked_fixture("arena-paused-links");
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute_batch("UPDATE matches SET deck_version_id = NULL")
            .expect("unlink");
        config.watching = false;
        let mut engine = open(&config, now);
        let now = run(&mut engine, now);
        assert_eq!(engine.arena.deck_links.links.len(), 2, "the links wait");
        assert_eq!(
            number(
                &config,
                "SELECT COUNT(*) FROM matches WHERE deck_version_id IS NULL"
            ),
            2
        );

        engine.set_watching(true);
        run(&mut engine, now);
        engine.finish().expect("finish");
        assert_eq!(
            number(
                &config,
                "SELECT COUNT(*) FROM matches WHERE deck_version_id IS NOT NULL"
            ),
            2
        );
    }

    #[test]
    fn no_arena_log_means_no_arena_rows() {
        let dir = TestDir::new("arena-none");
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        run(&mut engine, NOW);
        engine.finish().expect("finish");
        assert_eq!(number(&config, "SELECT COUNT(*) FROM arena_log_files"), 0);
        assert_eq!(
            number(
                &config,
                "SELECT COUNT(*) FROM accounts WHERE platform = 'arena'"
            ),
            0
        );
    }

    /// The newest status a tick reported, ticking until the backlog is read.
    fn latest_status(engine: &mut Engine, start_ms: i64) -> crate::Status {
        let mut status = None;
        for now in (start_ms..).take(1_000) {
            let tick = engine.tick(now).expect("tick");
            for event in tick.events {
                if let crate::TrackerEvent::Status(reported) = event {
                    status = Some(reported);
                }
            }
            if !tick.more {
                break;
            }
        }
        status.expect("a status")
    }

    #[test]
    fn the_status_reports_player_log_and_its_live_growth() {
        let dir = TestDir::new("arena-status");
        let bytes = fixture_bytes();
        let cut = entry_ends(&bytes)[1];
        dir.write(PLAYER_LOG, &bytes[..cut]);
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let caught_up = latest_status(&mut engine, NOW);
        assert_eq!(caught_up.arena_searched, vec![dir.join(PLAYER_LOG)]);
        assert_eq!(caught_up.arena_files.len(), 1);
        assert_eq!(caught_up.arena_files[0].path, dir.join(PLAYER_LOG));
        assert_eq!(
            caught_up.arena_files[0].last_live_growth_ms, None,
            "reading the backlog is not a live event"
        );
        dir.append(PLAYER_LOG, &bytes[cut..]);
        let live_at = NOW + 10_000;
        let live = latest_status(&mut engine, live_at);
        assert!(
            live.arena_files[0]
                .last_live_growth_ms
                .is_some_and(|at| at >= live_at),
            "growth after the catch-up is live"
        );
        assert!(live.files.is_empty(), "no mtgo.log in this folder");
        engine.finish().expect("finish");
    }

    fn deck_use(token: &str, grp: u32) -> DeckUse {
        DeckUse {
            match_token: token.to_string(),
            deck_id: "deck-guid".to_string(),
            deck_name: "Qdeck".to_string(),
            format_code: "CPIONEER".to_string(),
            main: vec![(grp, 4)],
            side: Vec::new(),
            commanders: Vec::new(),
        }
    }

    #[test]
    fn a_deck_used_again_waits_until_the_earlier_match_is_linked() {
        let dir = TestDir::new("arena-held-deck");
        let config = config(&dir);
        // Creates the database.
        open(&config, NOW).finish().expect("finish");
        let reader = reader(&config);
        let mut source =
            ArenaSource::open(&reader, Env::current(), Vec::new(), NOW).expect("source");
        source
            .queue_deck_use(&reader, deck_use("q-match-1", 1), "Qplayer#1", 10, NOW)
            .expect("queue");
        source
            .queue_deck_use(&reader, deck_use("q-match-2", 2), "Qplayer#1", 20, NOW)
            .expect("queue");
        assert_eq!(
            source.deck_links.deck_writes.len(),
            1,
            "the second list waits"
        );
        assert_eq!(source.deck_links.held.len(), 1);

        // After the first write commits: the first link (not found in this
        // empty database, so dropped), then the held write.
        source.deck_links.deck_writes.clear();
        let mut batch = WriteBatch::default();
        source.link(&reader, &mut batch).expect("link");
        assert!(source.deck_links.held.is_empty());
        assert_eq!(source.deck_links.links.len(), 1);
        assert_eq!(source.deck_links.links[0].token, "q-match-2");
        match batch.decks.as_slice() {
            [DeckWrite::SyncFile { file, .. }] => {
                assert_eq!(file.cards[0].catalog_id, ARENA_ID_OFFSET + 2);
            }
            other => panic!("expected the held write, got {other:?}"),
        }
    }

    /// Every stored game message of every match, live or archived, as text.
    fn all_game_messages(config: &TrackerConfig) -> Vec<String> {
        let conn = reader(config);
        let mut lines = Vec::new();
        for token in texts(config, "SELECT token FROM matches ORDER BY token") {
            for message in store::arena_games::game_messages(&conn, &token).expect("messages") {
                lines.push(format!(
                    "{token} {} {} {:?} {:?} {} {}",
                    message.byte_offset,
                    message.seq,
                    message.game_number,
                    message.source,
                    message.logged_at,
                    message.json
                ));
            }
        }
        lines
    }

    /// Every match has messages from game 1 to at most `games`, all archived.
    fn assert_archived_game_messages(config: &TrackerConfig, games: u32) {
        let conn = reader(config);
        let tokens = texts(config, "SELECT token FROM matches ORDER BY token");
        assert!(!tokens.is_empty());
        for token in &tokens {
            let messages = store::arena_games::game_messages(&conn, token).expect("messages");
            assert!(!messages.is_empty(), "{token} has game messages");
            // The connect response (game 1's deck) comes before anything
            // names the match: it is still the match's first message.
            assert!(
                messages[0].json.contains("GREMessageType_ConnectResp"),
                "{token} starts at its connect response"
            );
            assert_eq!(messages[0].game_number, Some(1), "{token} starts at game 1");
            assert!(
                messages.iter().all(|message| message.game_number.is_some()),
                "every message of {token} has a game"
            );
            let last = messages
                .iter()
                .filter_map(|message| message.game_number)
                .max();
            assert!(last <= Some(games), "{token} has at most {games} games");
        }
        // Each match is archived, and no message names another match.
        assert_eq!(
            number(config, "SELECT COUNT(*) FROM arena_game_messages"),
            0
        );
        assert_eq!(
            texts(
                config,
                "SELECT match_token FROM arena_game_archives ORDER BY match_token"
            ),
            tokens
        );
    }

    fn archive_bytes(config: &TrackerConfig) -> i64 {
        number(
            config,
            "SELECT SUM(LENGTH(messages)) FROM arena_game_archives",
        )
    }

    #[test]
    fn stores_game_messages_of_the_fixture_match() {
        let (_dir, config, _) = tracked_fixture("arena-game-messages");
        assert_archived_game_messages(&config, 3);
        let size = archive_bytes(&config);
        println!("bo3-loss-and-match-concede archive bytes: {size}");
        assert!(size < 1_000_000);
    }

    #[test]
    fn stores_game_messages_of_a_three_game_match() {
        let bytes = fixture_tools::arena_fixture("bo3-three-games").bytes;
        let (_dir, config, _) = tracked_bytes("arena-game-messages-three", &bytes);
        assert_archived_game_messages(&config, 3);
        assert!(
            all_game_messages(&config)
                .iter()
                .any(|line| line.contains(" Some(3) ")),
            "the third game is stored"
        );
        let size = archive_bytes(&config);
        println!("bo3-three-games archive bytes: {size}");
        assert!(size < 1_000_000);
    }

    /// The game number in a stored message's own game info, if any.
    fn info_game_number(json: &str) -> Option<u64> {
        let value: serde_json::Value = serde_json::from_str(json).expect("json");
        value
            .pointer("/gameStateMessage/gameInfo/gameNumber")
            .and_then(serde_json::Value::as_u64)
    }

    #[test]
    fn between_game_messages_take_the_next_game() {
        let bytes = fixture_tools::arena_fixture("bo3-three-games").bytes;
        let (_dir, config, _) = tracked_bytes("arena-between-games", &bytes);
        let token = texts(&config, "SELECT token FROM matches").remove(0);
        let messages =
            store::arena_games::game_messages(&reader(&config), &token).expect("messages");
        let mut decks = Vec::new();
        for (index, message) in messages.iter().enumerate() {
            if !message.json.contains("SubmitDeckReq") && !message.json.contains("SubmitDeckResp") {
                continue;
            }
            // The game the deck is for: the next game info's number.
            let next = messages[index..]
                .iter()
                .find_map(|later| info_game_number(&later.json))
                .expect("a later game info") as u32;
            assert_eq!(message.game_number, Some(next), "{}", message.byte_offset);
            decks.push(next);
        }
        decks.dedup();
        assert_eq!(decks, vec![2, 3]);
    }

    fn bo1_fixture_bytes() -> Vec<u8> {
        fixture_tools::arena_fixture("bo1-wins-and-concede").bytes
    }

    #[test]
    fn bo1_messages_are_stored() {
        let text = String::from_utf8(fixture_bytes()).expect("utf-8");
        let bo1 = text.replace(
            "MatchWinCondition_Best2of3",
            "MatchWinCondition_SingleElimination",
        );
        let (_dir, config, _) = tracked_bytes("arena-bo1-messages", bo1.as_bytes());
        assert!(number(&config, "SELECT COUNT(*) FROM arena_log_events") > 0);
        assert_eq!(
            texts(&config, "SELECT DISTINCT game_structure FROM matches"),
            vec!["bo1".to_string()]
        );
        assert!(!all_game_messages(&config).is_empty());
        assert_eq!(
            number(&config, "SELECT COUNT(*) FROM arena_game_archives"),
            number(&config, "SELECT COUNT(*) FROM matches")
        );
    }

    /// The Bo1 fixture with its first match's room states naming `event`.
    fn bo1_fixture_with_first_event(event: &str) -> (Vec<u8>, String) {
        let text = String::from_utf8(bo1_fixture_bytes()).expect("utf-8");
        let first = "114cd4b1-0788-4702-bfa4-e3b1fe2c17d2";
        let mut out = String::new();
        for line in text.split_inclusive('\n') {
            if line.contains("matchGameRoomStateChangedEvent") && line.contains(first) {
                out.push_str(&line.replace(
                    "\"eventId\": \"Explorer_Play\"",
                    &format!("\"eventId\": \"{event}\""),
                ));
            } else {
                out.push_str(line);
            }
        }
        assert_ne!(out, text, "the first match's event was renamed");
        (out.into_bytes(), first.to_string())
    }

    #[test]
    fn an_untracked_event_keeps_no_game_messages() {
        let event = "DirectGame";
        let (bytes, token) = bo1_fixture_with_first_event(event);
        let (_dir, config, _) = tracked_bytes("arena-bo1-untracked", &bytes);
        assert_eq!(
            number(&config, "SELECT COUNT(*) FROM matches"),
            3,
            "{event}"
        );
        assert_eq!(
            number(
                &config,
                &format!("SELECT COUNT(*) FROM arena_game_messages WHERE match_token = '{token}'")
            ),
            0,
            "{event}"
        );
        assert_archived_game_messages(&config, 1);
    }

    fn brawl_bytes(name: &str) -> Vec<u8> {
        fixture_tools::arena_fixture(name).bytes
    }

    #[test]
    fn a_brawl_match_keeps_its_game_messages() {
        let (_dir, config, _) =
            tracked_bytes("arena-brawl-queue", &brawl_bytes("brawl/brawl_queue"));
        let token = texts(&config, "SELECT token FROM matches").remove(0);
        let messages =
            store::arena_games::game_messages(&reader(&config), &token).expect("messages");
        assert!(!messages.is_empty());
        assert_eq!(
            number(&config, "SELECT COUNT(*) FROM arena_game_archives"),
            1
        );
        assert_eq!(
            texts(
                &config,
                "SELECT format || ' ' || match_type || ' ' || game_structure FROM matches"
            ),
            vec!["ARENA_HISTORIC_BRAWL unranked bo1".to_string()]
        );
    }

    #[test]
    fn a_brawl_deck_stores_the_commander_as_sideboard() {
        let (_dir, config, _) =
            tracked_bytes("arena-brawl-deck", &brawl_bytes("brawl/brawl_queue"));
        let signatures = texts(&config, "SELECT signature FROM deck_versions");
        assert_eq!(signatures.len(), 1);
        let cards = decks::decode_signature(&signatures[0]);
        let side: Vec<&DeckCard> = cards.iter().filter(|card| card.sideboard).collect();
        assert_eq!(side.len(), 1);
        assert_eq!(side[0].catalog_id, ARENA_ID_OFFSET + 83902);
        assert_eq!(side[0].quantity, 1);
        let main: u32 = cards
            .iter()
            .filter(|card| !card.sideboard)
            .map(|card| card.quantity)
            .sum();
        assert_eq!(main, 99);
    }

    /// Turn the database into what the last release left for a Brawl ladder
    /// match: format `ARENA_BRAWL`, detected, no commander rows, `deck_set`
    /// events without `commanders` and `format` (not read then), and a deck
    /// version without the commander row.
    fn age_to_last_release(conn: &Connection) {
        conn.execute_batch(
            "UPDATE matches SET format = 'ARENA_BRAWL', archetypes_detected_at = 5;
             DELETE FROM match_commanders;",
        )
        .expect("age the match");
        let events: Vec<(i64, i64, i64, Vec<u8>, bool)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT arena_log_file_id, byte_offset, seq, raw, compressed
                     FROM arena_log_events WHERE kind = 'deck_set'",
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
        };
        assert!(!events.is_empty(), "the fixture has a deck_set event");
        for (file_id, offset, seq, raw, compressed) in events {
            let text = store::write::decode_raw(raw, compressed).expect("decode");
            let mut value: serde_json::Value = serde_json::from_str(&text).expect("json");
            let object = value.as_object_mut().expect("object");
            object.remove("commanders");
            object.remove("format");
            let (raw, compressed) = store::write::encode_raw(&value.to_string()).expect("encode");
            conn.execute(
                "UPDATE arena_log_events SET raw = ?4, compressed = ?5
                 WHERE arena_log_file_id = ?1 AND byte_offset = ?2 AND seq = ?3",
                store::params![file_id, offset, seq, raw, compressed],
            )
            .expect("strip the event");
        }
        let versions: Vec<(i64, String)> = {
            let mut stmt = conn
                .prepare("SELECT id, signature FROM deck_versions")
                .expect("prepare");
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .expect("query")
                .map(|row| row.expect("row"))
                .collect()
        };
        for (id, signature) in versions {
            let main: Vec<DeckCard> = decks::decode_signature(&signature)
                .into_iter()
                .filter(|card| !card.sideboard)
                .collect();
            conn.execute(
                "UPDATE deck_versions SET signature = ?2 WHERE id = ?1",
                store::params![id, decks::signature(&main)],
            )
            .expect("drop the commander row");
        }
    }

    #[test]
    fn brawl_ladder_rows_from_the_last_release_are_repaired() {
        let bytes = brawl_bytes("brawl/brawl_ladder");
        let (_dir, config, now) = tracked_bytes("arena-brawl-repair", &bytes);
        assert_eq!(
            texts(&config, "SELECT format || ' ' || match_type FROM matches"),
            vec!["ARENA_HISTORIC_BRAWL ranked".to_string()]
        );
        {
            let conn = store::open_writer(&config.db_path).expect("writer");
            age_to_last_release(&conn);
        }
        assert_eq!(
            number(&config, "SELECT COUNT(*) FROM match_commanders"),
            0,
            "aged"
        );
        let mut engine = open(&config, now + 10 * RESCAN_EVERY_MS);
        run(&mut engine, now + 10 * RESCAN_EVERY_MS);
        engine.finish().expect("finish");
        assert_eq!(
            texts(&config, "SELECT format || ' ' || match_type FROM matches"),
            vec!["ARENA_HISTORIC_BRAWL ranked".to_string()]
        );
        assert_eq!(
            number(
                &config,
                "SELECT COUNT(*) FROM matches WHERE archetypes_detected_at IS NOT NULL"
            ),
            0
        );
        // Both seats from game 1's command zone (the game messages are kept).
        assert_eq!(
            texts(
                &config,
                "SELECT seat || ' ' || catalog_id FROM match_commanders ORDER BY seat, position"
            ),
            vec![
                format!("opponent {}", ARENA_ID_OFFSET + 91522),
                format!("player {}", ARENA_ID_OFFSET + 83902),
            ]
        );
        // The commanders have `cards` stubs for the cards worker; the match
        // waits for them, then is due for detection.
        let match_id = number(&config, "SELECT id FROM matches");
        assert_eq!(
            number(
                &config,
                "SELECT COUNT(*) FROM match_commanders mc
                 JOIN cards c ON c.catalog_id = mc.catalog_id"
            ),
            2
        );
        let due = |config: &TrackerConfig| {
            store::archetypes::due_matches(&reader(config), 10).expect("due")
        };
        assert!(!due(&config).contains(&match_id), "commanders unresolved");
        {
            let conn = store::open_writer(&config.db_path).expect("writer");
            conn.execute(
                "UPDATE cards SET oracle_id = 'q-' || catalog_id, name = 'Q' || catalog_id
                 WHERE catalog_id IN (SELECT catalog_id FROM match_commanders)",
                [],
            )
            .expect("resolve");
        }
        assert_eq!(due(&config), vec![match_id]);
    }

    #[test]
    fn rows_that_need_no_brawl_repair_are_not_rewritten() {
        // A stored ARENA_BRAWL row whose view is also ARENA_BRAWL, and a
        // non-Brawl Arena row, keep their detection stamp and revision.
        let (_dir, config, now) = tracked_bytes("arena-no-repair", &fixture_bytes());
        let before = texts(
            &config,
            "SELECT token || ' ' || revision FROM matches ORDER BY token",
        );
        {
            let conn = store::open_writer(&config.db_path).expect("writer");
            conn.execute("UPDATE matches SET archetypes_detected_at = 5", [])
                .expect("stamp");
        }
        let mut engine = open(&config, now + 10 * RESCAN_EVERY_MS);
        run(&mut engine, now + 10 * RESCAN_EVERY_MS);
        engine.finish().expect("finish");
        assert_eq!(
            texts(
                &config,
                "SELECT token || ' ' || revision FROM matches ORDER BY token"
            ),
            before
        );
        assert_eq!(
            number(
                &config,
                "SELECT COUNT(*) FROM matches WHERE archetypes_detected_at IS NULL"
            ),
            0
        );
    }

    #[test]
    fn a_stored_brawl_row_whose_view_is_brawl_is_not_rewritten() {
        let text = String::from_utf8(brawl_bytes("brawl/brawl_ladder")).expect("utf-8");
        let standard = text.replace("HistoricBrawl", "Brawl");
        assert_ne!(standard, text);
        let (_dir, config, now) = tracked_bytes("arena-brawl-same", standard.as_bytes());
        assert_eq!(
            texts(&config, "SELECT format FROM matches"),
            vec!["ARENA_BRAWL".to_string()]
        );
        let revision = number(&config, "SELECT revision FROM matches");
        {
            let conn = store::open_writer(&config.db_path).expect("writer");
            conn.execute("UPDATE matches SET archetypes_detected_at = 5", [])
                .expect("stamp");
        }
        let mut engine = open(&config, now + 10 * RESCAN_EVERY_MS);
        run(&mut engine, now + 10 * RESCAN_EVERY_MS);
        engine.finish().expect("finish");
        assert_eq!(number(&config, "SELECT revision FROM matches"), revision);
        assert_eq!(
            number(&config, "SELECT archetypes_detected_at FROM matches"),
            5
        );
    }

    #[test]
    fn a_tracked_fixture_stores_every_bo1_game() {
        let (_dir, config, _) = tracked_bytes("arena-bo1-fixture", &bo1_fixture_bytes());
        assert_eq!(number(&config, "SELECT COUNT(*) FROM matches"), 4);
        assert_eq!(
            texts(&config, "SELECT DISTINCT game_structure FROM matches"),
            vec!["bo1".to_string()]
        );
        assert_archived_game_messages(&config, 1);
        assert_eq!(
            texts(&config, "SELECT DISTINCT status FROM arena_game_syncs"),
            vec!["derived".to_string()]
        );
        assert_eq!(number(&config, "SELECT COUNT(*) FROM arena_game_syncs"), 4);
    }

    #[test]
    fn a_sealed_direct_is_tracked_for_an_untagged_account() {
        let bytes = fixture_tools::arena_fixture("sealed-direct-fast-login").bytes;
        let (_dir, config, _) = tracked_bytes("arena-sealed-direct", &bytes);
        assert_eq!(
            texts(
                &config,
                "SELECT username || ' ' || COALESCE(display_name, '') FROM accounts
                 WHERE platform = 'arena'"
            ),
            vec!["arena:PLAYER0000000000000000000A Player".to_string()]
        );
        assert_eq!(
            texts(
                &config,
                "SELECT format || ' ' || match_type || ' ' || outcome FROM matches ORDER BY started_at"
            ),
            [
                vec!["S6EOE limited win".to_string(); 5],
                vec!["S6EOE limited unknown".to_string()]
            ]
            .concat()
        );
        assert_eq!(
            number(
                &config,
                "SELECT COUNT(*) FROM decks WHERE mtgo_id NOT LIKE 'limited:arena\\_%' ESCAPE '\\'"
            ),
            0,
            "no constructed deck"
        );
        // The run's course deck (lead ruling R5), every match on it.
        assert_eq!(
            number(
                &config,
                "SELECT COUNT(*) FROM decks WHERE mtgo_id LIKE 'limited:arena\\_%' ESCAPE '\\'"
            ),
            1
        );
        assert_eq!(
            number(
                &config,
                "SELECT COUNT(*) FROM matches WHERE deck_version_id IS NULL"
            ),
            0,
            "every match of the run plays its course deck"
        );
        let conn = reader(&config);
        for token in texts(&config, "SELECT token FROM matches") {
            let messages = store::arena_games::game_messages(&conn, &token).expect("messages");
            assert!(!messages.is_empty(), "{token} keeps its game messages");
        }
    }

    fn sealed_bytes() -> Vec<u8> {
        fixture_tools::arena_fixture("sealed-direct-fast-login").bytes
    }

    /// Each courses response of `bytes` as `(entry offset, courses sorted
    /// by course id)`, in log order.
    fn course_snapshots(bytes: &[u8]) -> Vec<(u64, Vec<arena_protocol::Course>)> {
        let mut snapshots = Vec::new();
        for entry in arena_protocol::split_complete(bytes) {
            for event in decode_all(&entry) {
                if let ArenaEvent::Courses { mut courses } = event {
                    courses.sort_by(|a, b| a.course_id.cmp(&b.course_id));
                    snapshots.push((entry.offset, courses));
                }
            }
        }
        snapshots
    }

    /// The stored snapshots' places, as `file:offset`, in log order.
    fn stored_snapshot_places(config: &TrackerConfig) -> Vec<String> {
        texts(
            config,
            "SELECT DISTINCT arena_log_file_id || ':' || byte_offset
             FROM arena_course_observations ORDER BY arena_log_file_id, byte_offset",
        )
    }

    /// A tagged fast login (`authenticateResponse` naming the account) for
    /// the fixture's account under `screen_name`, at `header`'s time.
    fn tagged_authenticate(header: &str, screen_name: &str) -> Vec<u8> {
        format!(
            "{header}: Match to PLAYER0000000000000000000A: AuthenticateResponse\n\
             {{ \"transactionId\": \"00000000-0000-4000-8000-00000000000a\", \"requestId\": 1, \
             \"authenticateResponse\": {{ \"clientId\": \"PLAYER0000000000000000000A\", \
             \"sessionId\": \"00000000-0000-4000-8000-00000000000a\", \
             \"screenName\":\"{screen_name}\" }} }}\n"
        )
        .into_bytes()
    }

    /// #862: the sealed fixture played as `Player#00001`, renamed right
    /// after its first course snapshot, in the same read (one batch). The
    /// snapshots and the run follow the renamed account, as without the
    /// rename, and no account is left under the old name.
    #[test]
    fn course_snapshots_and_runs_read_with_a_rename_follow_it() {
        let fixture = sealed_bytes();
        let first_snapshot = course_snapshots(&fixture)[0].0 as usize;
        let entry = b"\n[UnityCrossThreadLogger]";
        let cut = first_snapshot
            + fixture[first_snapshot..]
                .windows(entry.len())
                .position(|window| window == entry)
                .expect("an entry after the first snapshot")
            + 1;
        // The time of the last timed header before the cut.
        let before = String::from_utf8_lossy(&fixture[..cut]).to_string();
        let timed = before
            .lines()
            .rev()
            .find(|line| {
                line.strip_prefix("[UnityCrossThreadLogger]")
                    .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
            })
            .expect("a timed header");
        let header = &timed[..timed.find(": ").unwrap_or(timed.len())];
        let mut bytes = tagged_authenticate(header, "Player#00001");
        bytes.extend_from_slice(&fixture[..cut]);
        bytes.extend_from_slice(&tagged_authenticate(header, "Qrenamed#00002"));
        assert!(
            (bytes.len() as u64) < MAX_READ,
            "the first snapshot and the rename are read in one poll"
        );
        bytes.extend_from_slice(&fixture[cut..]);
        let (_dir, config, _) = tracked_bytes("arena-course-rename", &bytes);
        let (_plain_dir, plain, _) = tracked_bytes("arena-course-plain", &fixture);

        assert_eq!(
            texts(
                &config,
                "SELECT platform || ' ' || username || ' ' || COALESCE(arena_user_id, '-')
                 FROM accounts ORDER BY id"
            ),
            vec!["arena Qrenamed#00002 PLAYER0000000000000000000A".to_string()],
        );
        // The same snapshots, each once (by their times: the offsets moved).
        let snapshots = "SELECT DISTINCT CAST(observed_at AS TEXT) FROM arena_course_observations
                         ORDER BY observed_at";
        assert_eq!(texts(&config, snapshots), texts(&plain, snapshots));
        let runs = "SELECT COUNT(*) FROM leagues";
        assert!(number(&plain, runs) > 0);
        assert_eq!(number(&config, runs), number(&plain, runs));
        let linked = "SELECT COUNT(*) FROM matches WHERE league_id IS NOT NULL";
        assert_eq!(number(&config, linked), number(&plain, linked));
    }

    #[test]
    fn the_fixture_stores_its_course_snapshots() {
        let bytes = sealed_bytes();
        let (_dir, config, _) = tracked_bytes("arena-course-snapshots", &bytes);

        let mut changed: Vec<String> = Vec::new();
        let mut previous: Option<Vec<arena_protocol::Course>> = None;
        for (offset, courses) in course_snapshots(&bytes) {
            if previous.as_ref() != Some(&courses) {
                changed.push(format!("1:{offset}"));
            }
            previous = Some(courses);
        }
        assert_eq!(
            stored_snapshot_places(&config),
            changed,
            "every snapshot that differs from the one before it, once"
        );
        assert_eq!(changed.len(), 7);
        assert_eq!(
            texts(
                &config,
                "SELECT DISTINCT a.username FROM arena_course_observations o
                 JOIN accounts a ON a.id = o.account_id"
            ),
            vec!["arena:PLAYER0000000000000000000A".to_string()]
        );
        let conn = reader(&config);
        let account_id = read::account_id(&conn, "arena:PLAYER0000000000000000000A")
            .expect("read")
            .expect("account");
        let observations = store::arena_courses::observations(&conn, account_id).expect("read");
        let direct = observations
            .iter()
            .rfind(|row| row.event_name == "ArenaDirect_EOE_Play_Sealed_20261106")
            .expect("the Direct");
        assert_eq!(direct.wins, 5);
        assert_eq!(direct.pool.len(), 78);
        assert!(
            observations
                .iter()
                .flat_map(|row| row.pool.iter().chain(row.main.iter().map(|(id, _)| id)))
                .all(|id| *id >= ARENA_ID_OFFSET),
            "card ids are catalog ids"
        );
        assert_eq!(
            texts(
                &config,
                "SELECT event_name FROM arena_match_events ORDER BY match_token"
            ),
            vec!["ArenaDirect_EOE_Play_Sealed_20261106".to_string(); 6]
        );
        assert_eq!(
            store::arena_courses::match_event_names(&conn, account_id)
                .expect("names")
                .len(),
            6
        );
    }

    #[test]
    fn courses_before_the_account_are_held() {
        let bytes = sealed_bytes();
        let entries = arena_protocol::split_complete(&bytes);
        let authenticate = entries
            .iter()
            .find(|entry| {
                decode_all(entry)
                    .iter()
                    .any(|event| matches!(event, ArenaEvent::Authenticated { .. }))
            })
            .expect("an authenticate response");
        let first_courses = course_snapshots(&bytes)[0].0;
        assert!(first_courses < authenticate.offset);

        let dir = TestDir::new("arena-courses-held");
        let cut = authenticate.offset as usize;
        dir.write(PLAYER_LOG, &bytes[..cut]);
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let now = run(&mut engine, NOW);
        assert_eq!(
            number(&config, "SELECT COUNT(*) FROM arena_course_observations"),
            0,
            "no account to store them under yet"
        );
        let end = (authenticate.offset + authenticate.len) as usize;
        dir.append(PLAYER_LOG, &bytes[cut..end]);
        run(&mut engine, now);
        engine.finish().expect("finish");
        let places = stored_snapshot_places(&config);
        assert_eq!(places.first(), Some(&format!("1:{first_courses}")));
        assert_eq!(
            texts(
                &config,
                "SELECT DISTINCT a.username FROM arena_course_observations o
                 JOIN accounts a ON a.id = o.account_id"
            ),
            vec!["arena:PLAYER0000000000000000000A".to_string()]
        );
    }

    /// The Bo3 fixture as another account's log: both fixtures are one
    /// account's, so its user id and login name are swapped for others of
    /// the same length.
    fn other_account_bytes() -> Vec<u8> {
        let text = String::from_utf8(fixture_bytes()).expect("utf-8 fixture");
        text.replace("PLAYER0000000000000000000A", "PLAYER0000000000000000000B")
            .replace("Player#00001", "Qother#00002")
            .into_bytes()
    }

    /// A new `Player.log` is a new launch, maybe of another account: its
    /// snapshots before its own login wait for that login, and are not
    /// credited to the account the earlier file logged in.
    #[test]
    fn courses_wait_for_a_login_in_their_own_file() {
        let dir = TestDir::new("arena-courses-own-file");
        dir.write(PREV_LOG, &other_account_bytes());
        dir.write(PLAYER_LOG, &sealed_bytes());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        run(&mut engine, NOW);
        engine.finish().expect("finish");
        let sealed_file = "(SELECT id FROM arena_log_files WHERE path LIKE '%Player.log')";
        let owners = |table: &str| {
            texts(
                &config,
                &format!(
                    "SELECT DISTINCT a.username FROM {table} r
                     JOIN accounts a ON a.id = r.account_id
                     WHERE r.arena_log_file_id = {sealed_file}"
                ),
            )
        };
        let sealed_account = vec!["arena:PLAYER0000000000000000000A".to_string()];
        assert_eq!(owners("arena_course_observations"), sealed_account);
        assert_eq!(
            owners("arena_rank_observations"),
            sealed_account,
            "rank readings wait for their own file's login too"
        );
        let sealed_places = texts(
            &config,
            &format!(
                "SELECT DISTINCT CAST(byte_offset AS TEXT) FROM arena_course_observations
                 WHERE arena_log_file_id = {sealed_file}"
            ),
        );
        assert_eq!(
            sealed_places.len(),
            7,
            "what a read of the file alone stores"
        );
    }

    /// A restart while snapshots wait for an account keeps them: the
    /// fixture cut just before its authenticate response, then read on,
    /// stores what one read stores.
    #[test]
    fn a_restart_keeps_held_course_snapshots() {
        let bytes = sealed_bytes();
        let (_whole_dir, whole, _) = tracked_bytes("arena-courses-whole", &bytes);
        let authenticate = arena_protocol::split_complete(&bytes)
            .into_iter()
            .find(|entry| {
                decode_all(entry)
                    .iter()
                    .any(|event| matches!(event, ArenaEvent::Authenticated { .. }))
            })
            .expect("an authenticate response");
        let (_dir, config) =
            restarted_at("arena-courses-cut", &bytes, authenticate.offset as usize);
        assert_eq!(
            dump_rows(&config, "arena_course_observations"),
            dump_rows(&whole, "arena_course_observations")
        );
        assert_eq!(stored_snapshot_places(&config).len(), 7);
    }

    #[test]
    fn startup_stores_the_event_names_of_earlier_matches() {
        let (_dir, config, now) = tracked_bytes("arena-match-events-backfill", &sealed_bytes());
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute("DELETE FROM arena_match_events", [])
            .expect("delete");
        restart(&config, now);
        assert_eq!(
            number(&config, "SELECT COUNT(*) FROM arena_match_events"),
            6
        );
    }

    #[test]
    fn a_restart_stores_no_repeat_snapshot() {
        let bytes = sealed_bytes();
        let (dir, config, now) = tracked_bytes("arena-courses-restart", &bytes);
        let before = number(&config, "SELECT COUNT(*) FROM arena_course_observations");
        let last = course_snapshots(&bytes).last().expect("a snapshot").0;
        let entry = arena_protocol::split_complete(&bytes)
            .into_iter()
            .find(|entry| entry.offset == last)
            .expect("the last courses entry");
        let start = entry.offset as usize;
        dir.append(PLAYER_LOG, &bytes[start..start + entry.len as usize]);
        restart(&config, now);
        assert_eq!(
            number(&config, "SELECT COUNT(*) FROM arena_course_observations"),
            before
        );
    }

    const DIRECT_RUN: &str = "arena:1dc93d86-9f08-485c-ab08-457aa1167d48";
    const DRAFT_RUN: &str = "arena:dfed50e8-11c3-431e-a6c2-d7b4efdfed8e";

    /// Each Arena run as `key kind state format set matches snapshots`.
    fn arena_runs(config: &TrackerConfig) -> Vec<String> {
        texts(
            config,
            "SELECT l.run_key || ' ' || l.kind || ' ' || l.state || ' ' || l.format || ' '
                 || l.set_code || ' '
                 || (SELECT COUNT(*) FROM matches m WHERE m.league_id = l.id) || ' '
                 || (SELECT COUNT(*) FROM limited_deck_snapshots s
                     JOIN matches m ON m.id = s.match_id
                     WHERE m.league_id = l.id AND s.source = 'registered')
             FROM leagues l ORDER BY l.run_key",
        )
    }

    #[test]
    fn the_fixture_gives_two_limited_runs() {
        let (_dir, config, _) = tracked_bytes("arena-limited-runs", &sealed_bytes());
        assert_eq!(
            arena_runs(&config),
            vec![
                format!("{DIRECT_RUN} sealed active S6EOE EOE 6 6"),
                format!("{DRAFT_RUN} draft active DFRAFRAFRA FRA 0 0"),
            ]
        );
        // The Direct's own record agrees with its linked matches: five won,
        // none lost, the sixth still in progress.
        let observed = texts(
            &config,
            &format!(
                "SELECT wins || '-' || losses FROM arena_course_observations
                 WHERE course_id = '{}'
                 ORDER BY arena_log_file_id DESC, byte_offset DESC LIMIT 1",
                DIRECT_RUN.trim_start_matches("arena:")
            ),
        );
        let counted = texts(
            &config,
            &format!(
                "SELECT SUM(m.outcome = 'win') || '-' || SUM(m.outcome = 'loss')
                 FROM matches m JOIN leagues l ON l.id = m.league_id
                 WHERE l.run_key = '{DIRECT_RUN}'"
            ),
        );
        assert_eq!(observed, vec!["5-0".to_string()]);
        assert_eq!(counted, observed);
    }

    #[test]
    fn a_restart_resyncs_runs() {
        let (_dir, config, now) = tracked_bytes("arena-limited-runs-restart", &sealed_bytes());
        let before = arena_runs(&config);
        assert_eq!(before.len(), 2, "{before:?}");
        let conn = store::open_writer(&config.db_path).expect("writer");
        conn.execute_batch(
            "UPDATE matches SET league_id = NULL;
             DELETE FROM limited_deck_snapshots;
             DELETE FROM leagues;",
        )
        .expect("delete");
        drop(conn);
        assert!(arena_runs(&config).is_empty());

        restart(&config, now);
        assert_eq!(arena_runs(&config), before);
    }

    /// Deleting a limited match takes it out of its run, and restoring it
    /// puts it back, without waiting for another batch or a restart (#631).
    #[test]
    fn deleting_and_restoring_a_limited_match_resyncs_its_run() {
        let (_dir, config, now) = tracked_bytes("arena-limited-runs-hide", &sealed_bytes());
        let token = texts(
            &config,
            &format!(
                "SELECT m.token FROM matches m JOIN leagues l ON l.id = m.league_id
                 WHERE l.run_key = '{DIRECT_RUN}' ORDER BY m.started_at LIMIT 1"
            ),
        )
        .remove(0);
        let mut engine = open(&config, now);
        let now = run(&mut engine, now);
        let outcome = engine
            .edit_match(MatchEdit::Hide {
                token: token.clone(),
                now_ms: now,
            })
            .expect("hide");
        assert_eq!(outcome, MatchEditOutcome::Done);
        engine.flush().expect("flush");
        assert_eq!(
            arena_runs(&config),
            vec![
                format!("{DIRECT_RUN} sealed active S6EOE EOE 5 5"),
                format!("{DRAFT_RUN} draft active DFRAFRAFRA FRA 0 0"),
            ]
        );

        let outcome = engine
            .edit_match(MatchEdit::Restore { token, now_ms: now })
            .expect("restore");
        assert_eq!(outcome, MatchEditOutcome::Done);
        engine.flush().expect("flush");
        assert_eq!(
            arena_runs(&config),
            vec![
                format!("{DIRECT_RUN} sealed active S6EOE EOE 6 6"),
                format!("{DRAFT_RUN} draft active DFRAFRAFRA FRA 0 0"),
            ]
        );
    }

    /// A database from before match event names were stored: the names are
    /// gone and the runs were synced without them.
    fn runs_without_event_names(label: &str) -> (TestDir, TrackerConfig, i64, Vec<String>) {
        let (dir, config, now) = tracked_bytes(label, &sealed_bytes());
        let before = arena_runs(&config);
        let mut conn = store::open_writer(&config.db_path).expect("writer");
        conn.execute("DELETE FROM arena_match_events", [])
            .expect("delete");
        let tx = conn.transaction().expect("transaction");
        store::arena_run_sync::sync_all_arena_runs(&tx, now).expect("sync");
        tx.commit().expect("commit");
        drop(conn);
        assert_ne!(arena_runs(&config), before, "the runs lost their names");
        (dir, config, now, before)
    }

    #[test]
    fn a_restart_names_the_runs_of_backfilled_match_events() {
        let (_dir, config, now, before) = runs_without_event_names("arena-runs-named-backfill");
        restart(&config, now);
        assert_eq!(arena_runs(&config), before);
    }

    #[test]
    fn a_paused_restart_names_the_runs_once_watching() {
        let (_dir, mut config, now, before) =
            runs_without_event_names("arena-runs-named-backfill-paused");
        config.watching = false;
        let mut engine = open(&config, now);
        let now = run(&mut engine, now);
        engine.set_watching(true);
        run(&mut engine, now);
        engine.finish().expect("finish");
        assert_eq!(arena_runs(&config), before);
    }

    #[test]
    fn the_backfill_fills_a_tracked_bo1_match() {
        let (_live_dir, live, _) = tracked_bytes("arena-bo1-backfill-live", &bo1_fixture_bytes());
        let dir = TestDir::new("arena-bo1-backfill");
        dir.write(PLAYER_LOG, &bo1_fixture_bytes());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        engine.arena.game_data_off = true;
        let now = run(&mut engine, NOW);
        engine.finish().expect("finish");
        assert_eq!(number(&config, "SELECT COUNT(*) FROM matches"), 4);
        assert!(all_game_messages(&config).is_empty());

        restart(&config, now);
        assert!(!all_game_messages(&live).is_empty());
        assert_eq!(all_game_messages(&config), all_game_messages(&live));
        assert_eq!(synced_games(&config), synced_games(&live));
        assert_eq!(stamps(&config), vec!["derived 2".to_string(); 4]);
    }

    /// Each match's type and the constructed reading it stores as its rank.
    fn match_ranks(config: &TrackerConfig) -> Vec<String> {
        texts(
            config,
            "SELECT m.match_type || ' ' || COALESCE(r.class || ' ' || r.level || ' ' || r.step, '-')
             FROM matches m
             LEFT JOIN arena_rank_observations r ON r.arena_log_file_id = m.rank_file_id
                 AND r.byte_offset = m.rank_byte_offset AND r.ladder = 'constructed'
             ORDER BY m.started_at",
        )
    }

    #[test]
    fn a_ranked_bo1_match_stores_the_rank_before_it_live_and_on_rebuild() {
        let (_dir, config, now) = tracked_bytes("arena-bo1-rank", &bo1_fixture_bytes());
        let ranks = match_ranks(&config);
        assert!(
            ranks.contains(&"ranked Bronze 2 0".to_string()),
            "the ranked match has the reading before it: {ranks:?}"
        );
        assert!(
            ranks
                .iter()
                .filter(|rank| rank.starts_with("unranked"))
                .all(|rank| rank.ends_with(" -")),
            "casual matches have no rank: {ranks:?}"
        );
        let matches = dump(&config, "matches");

        // Rebuilt from the stored events, and read in chunks.
        let mut engine = open(&config, now);
        run(&mut engine, now);
        engine.finish().expect("finish");
        assert_eq!(dump(&config, "matches"), matches);

        let dir = TestDir::new("arena-bo1-rank-chunks");
        dir.write(PLAYER_LOG, b"");
        let chunked = self::config(&dir);
        let mut engine = open(&chunked, NOW);
        let mut now = run(&mut engine, NOW);
        for chunk in bo1_fixture_bytes().chunks(4096) {
            dir.append(PLAYER_LOG, chunk);
            loop {
                now += 1;
                if !engine.tick(now).expect("tick").more {
                    break;
                }
            }
        }
        run(&mut engine, now);
        engine.finish().expect("finish");
        assert_eq!(match_ranks(&chunked), ranks);
    }

    /// The matches as the pages read them: token, kind, result, deck linked.
    fn match_rows(config: &TrackerConfig) -> Vec<String> {
        texts(
            config,
            "SELECT m.token || ' ' || m.game_structure || ' ' || m.outcome || ' '
                 || a.platform || ' ' || COALESCE(o.platform, '-') || ' '
                 || (m.deck_version_id IS NOT NULL)
             FROM matches m JOIN accounts a ON a.id = m.account_id
             LEFT JOIN opponents o ON o.id = m.opponent_id
             ORDER BY m.token",
        )
    }

    /// An install that stored the events of `bytes` (read into `live`)
    /// but never a match for them (Bo1 had no view before): its own
    /// database, holding only the reference's Arena log files and events,
    /// the file at its path (its creation time forgotten: the copy is
    /// newer, not a new log).
    fn events_only(label: &str, live: &TrackerConfig, bytes: &[u8]) -> (TestDir, TrackerConfig) {
        let dir = TestDir::new(label);
        let config = config(&dir);
        open(&config, NOW).finish().expect("create the database");
        dir.write(PLAYER_LOG, bytes);
        {
            let conn = store::open_writer(&config.db_path).expect("writer");
            conn.execute(
                "ATTACH DATABASE ?1 AS live",
                [live.db_path.to_string_lossy().to_string()],
            )
            .expect("attach");
            conn.execute_batch(
                "INSERT INTO arena_log_files SELECT * FROM live.arena_log_files;
                 INSERT INTO arena_log_events SELECT * FROM live.arena_log_events;",
            )
            .expect("copy the events");
            conn.execute(
                "UPDATE arena_log_files SET path = ?1, created_at_fs = NULL",
                [dir.join(PLAYER_LOG).to_string_lossy().to_string()],
            )
            .expect("the file is here");
            conn.execute_batch("DETACH DATABASE live").expect("detach");
        }
        assert_eq!(number(&config, "SELECT COUNT(*) FROM matches"), 0);
        (dir, config)
    }

    #[test]
    fn open_writes_stored_bo1_matches_that_have_no_row() {
        // What the tracker stores for the Bo1 fixture, as a reference.
        let (_live_dir, live, _) = tracked_bytes("arena-bo1-unstored-live", &bo1_fixture_bytes());
        assert_eq!(match_rows(&live).len(), 4);
        let (_dir, config) = events_only("arena-bo1-unstored", &live, &bo1_fixture_bytes());

        // One tick writes the matches; a deck used again waits for the
        // earlier match's link, so the links take the ticks after it.
        let mut engine = open(&config, NOW);
        engine.tick(NOW).expect("tick");
        engine.flush().expect("flush");
        assert_eq!(number(&config, "SELECT COUNT(*) FROM matches"), 4);
        run(&mut engine, NOW + 1);
        assert_eq!(match_rows(&config), match_rows(&live));
        assert!(
            match_rows(&config).iter().all(|row| row.ends_with(" 1")),
            "every match is linked to its deck"
        );
        assert_archived_game_messages(&config, 1);
        assert_eq!(all_game_messages(&config), all_game_messages(&live));
        assert_eq!(synced_games(&config), synced_games(&live));
        assert_eq!(stamps(&config), vec!["derived 2".to_string(); 4]);
        assert_eq!(
            dump_rows(&config, "arena_log_events"),
            dump_rows(&live, "arena_log_events"),
            "the log is not read again"
        );
        engine.finish().expect("finish");
    }

    #[test]
    fn an_unstored_match_of_an_earlier_account_is_stored_as_arena() {
        // The first two matches' sessions log in as another account (the
        // fixture's sessions are fast logins): the current account is the
        // later one.
        let text = String::from_utf8(bo1_fixture_bytes()).expect("utf-8");
        let split = text
            .find("\"matchId\": \"03d3921b")
            .expect("the third match");
        let split = text[..split]
            .rfind("authenticateResponse")
            .expect("its login");
        let earlier = text[..split]
            .replace("Player#00001", "Player#00002")
            .replace("PLAYER0000000000000000000A", "PLAYER0000000000000000000B");
        let bytes = format!("{earlier}{}", &text[split..]).into_bytes();
        let (_live_dir, live, _) = tracked_bytes("arena-bo1-two-accounts-live", &bytes);
        let accounts = "SELECT a.username || ' ' || a.platform || ' ' || COUNT(m.id)
             FROM accounts a LEFT JOIN matches m ON m.account_id = a.id
             GROUP BY a.id ORDER BY a.username";
        assert_eq!(
            texts(&live, accounts),
            vec![
                "Player#00001 arena 2".to_string(),
                "Player#00002 arena 2".to_string()
            ]
        );
        let (_dir, config) = events_only("arena-bo1-two-accounts", &live, &bytes);

        let mut engine = open(&config, NOW);
        run(&mut engine, NOW);
        engine.finish().expect("finish");
        assert_eq!(texts(&config, accounts), texts(&live, accounts));
        assert_eq!(match_rows(&config), match_rows(&live));
    }

    #[test]
    fn unstored_matches_of_a_bare_fast_login_join_the_account_named_later() {
        // The first two sessions are fast logins with a bare screen name;
        // a login line names the account before the third.
        let text = String::from_utf8(bo1_fixture_bytes()).expect("utf-8");
        let split = text
            .find("\"matchId\": \"03d3921b")
            .expect("the third match");
        let split = text[..split]
            .rfind("authenticateResponse")
            .expect("its login");
        let split = text[..split].rfind('\n').expect("its line") + 1;
        let earlier = text[..split].replace(
            "\"screenName\":\"Player#00001\"",
            "\"screenName\":\"Player\"",
        );
        let login = "[Accounts - Login] Logged in successfully. Display Name: Player#00001\n";
        let bytes = format!("{earlier}{login}{}", &text[split..]).into_bytes();
        let (_live_dir, live, _) = tracked_bytes("arena-bo1-bare-login-live", &bytes);
        let accounts = "SELECT a.username || ' ' || a.platform || ' ' || COUNT(m.id)
             FROM accounts a LEFT JOIN matches m ON m.account_id = a.id
             GROUP BY a.id ORDER BY a.username";
        assert_eq!(
            texts(&live, accounts),
            vec!["Player#00001 arena 4".to_string()]
        );
        // The tracker before untagged names stored the account under its
        // login line's name, and never the Bo1 matches.
        let (_dir, config) = events_only("arena-bo1-bare-login", &live, &bytes);
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute(
                "INSERT INTO accounts (username, first_seen_at, platform, arena_user_id)
                 VALUES ('Player#00001', 1, 'arena', 'PLAYER0000000000000000000A')",
                [],
            )
            .expect("the stored account");

        let mut engine = open(&config, NOW);
        run(&mut engine, NOW);
        engine.finish().expect("finish");
        assert_eq!(texts(&config, accounts), texts(&live, accounts));
        assert_eq!(match_rows(&config), match_rows(&live));
    }

    #[test]
    fn an_unstored_match_still_in_progress_gets_its_messages() {
        // The log stops halfway into the fourth match: it is still in
        // progress.
        let full = bo1_fixture_bytes();
        let text = String::from_utf8(full.clone()).expect("utf-8");
        let begins = text
            .find("\"matchId\": \"2e4743ce")
            .expect("the fourth match");
        let ends = text
            .rfind("MatchGameRoomStateType_MatchCompleted")
            .expect("the last match ends");
        let middle = begins + (ends - begins) / 2;
        let cut = entry_ends(&full)
            .into_iter()
            .filter(|end| *end <= middle)
            .max()
            .expect("an entry before the middle");
        let bytes = full[..cut].to_vec();
        let (_live_dir, live, _) = tracked_bytes("arena-bo1-in-progress-live", &bytes);
        let (_dir, config) = events_only("arena-bo1-in-progress", &live, &bytes);

        let token = "2e4743ce-abe6-4184-b8b4-8551e5cd4c8d";
        let mut engine = open(&config, NOW);
        assert_eq!(
            engine.arena.projector.view(token).map(|view| view.state),
            Some(MatchState::InProgress)
        );
        engine.tick(NOW).expect("tick");
        engine.flush().expect("flush");
        let conn = reader(&config);
        let messages = store::arena_games::game_messages(&conn, token).expect("messages");
        assert!(
            !messages.is_empty(),
            "the match in progress has its messages"
        );
        engine.finish().expect("finish");
    }

    #[test]
    fn live_equals_whole_file() {
        let (_whole_dir, whole, _) = tracked_fixture("arena-messages-whole");
        let expected = all_game_messages(&whole);
        assert!(!expected.is_empty());

        let dir = TestDir::new("arena-messages-chunks");
        dir.write(PLAYER_LOG, b"");
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let mut now = run(&mut engine, NOW);
        for chunk in fixture_bytes().chunks(4096) {
            dir.append(PLAYER_LOG, chunk);
            loop {
                now += 1;
                if !engine.tick(now).expect("tick").more {
                    break;
                }
            }
        }
        run(&mut engine, now);
        engine.finish().expect("finish");
        assert_eq!(all_game_messages(&config), expected);
        assert_eq!(
            number(&config, "SELECT COUNT(*) FROM arena_game_messages"),
            0
        );
    }

    #[test]
    fn a_restart_mid_match_keeps_capturing() {
        let (_whole_dir, whole, _) = tracked_fixture("arena-messages-restart-whole");
        let expected = all_game_messages(&whole);

        let dir = TestDir::new("arena-messages-restart");
        let bytes = fixture_bytes();
        let cut = bytes.len() / 2;
        let cut = cut + bytes[cut..].iter().position(|b| *b == b'\n').expect("line") + 1;
        dir.write(PLAYER_LOG, &bytes[..cut]);
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let now = run(&mut engine, NOW);
        engine.finish().expect("finish");
        // Quiet since the cut, the match settled and its messages so far
        // were archived (#571); the rest join them after the restart.
        assert!(!all_game_messages(&config).is_empty());

        dir.append(PLAYER_LOG, &bytes[cut..]);
        let mut engine = open(&config, now);
        run(&mut engine, now);
        engine.finish().expect("finish");
        assert_eq!(all_game_messages(&config), expected);
    }

    /// The app restarts after a game's intermission and before the next
    /// game's game info (sideboarding): the next game's deck messages still
    /// go to the next game, as a run without the restart stores them.
    #[test]
    fn a_restart_while_sideboarding_keeps_the_next_games_deck() {
        let bytes = fixture_tools::arena_fixture("bo3-three-games").bytes;
        let (_whole_dir, whole, _) = tracked_bytes("arena-sideboard-restart-whole", &bytes);
        // Cut right after game 1's intermission.
        let cut = arena_protocol::split_complete(&bytes)
            .iter()
            .find(|entry| {
                game_messages(entry)
                    .iter()
                    .any(GameMessage::is_intermission)
            })
            .map(|entry| (entry.offset + entry.len) as usize)
            .expect("an intermission");
        let (_dir, config) = restarted_at("arena-sideboard-restart", &bytes, cut);
        assert_eq!(all_game_messages(&config), all_game_messages(&whole));
        assert_eq!(synced_games(&config), synced_games(&whole));
    }

    /// The log read up to byte `cut`, then (after a restart) to its end.
    fn restarted_at(label: &str, bytes: &[u8], cut: usize) -> (TestDir, TrackerConfig) {
        let dir = TestDir::new(label);
        dir.write(PLAYER_LOG, &bytes[..cut]);
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let now = run(&mut engine, NOW);
        engine.finish().expect("finish");
        dir.append(PLAYER_LOG, &bytes[cut..]);
        let mut engine = open(&config, now);
        run(&mut engine, now);
        engine.finish().expect("finish");
        (dir, config)
    }

    /// The end offsets of the fixture's entries.
    fn entry_ends(bytes: &[u8]) -> Vec<usize> {
        arena_protocol::split_complete(bytes)
            .iter()
            .map(|entry| (entry.offset + entry.len) as usize)
            .collect()
    }

    /// The entry end offsets `after` entries past the first entry whose
    /// game messages name game `game`.
    fn cut_into_game(bytes: &[u8], game: u32, after: usize) -> usize {
        let entries = arena_protocol::split_complete(bytes);
        let first = entries
            .iter()
            .position(|entry| {
                game_messages(entry)
                    .iter()
                    .any(|message| message.game_number == Some(game))
            })
            .expect("the game");
        let entry = &entries[first + after];
        (entry.offset + entry.len) as usize
    }

    /// A restart in game 3, after Arena's repeated intermission (stored as
    /// game 3) and game 3's own game info: game 3's messages stay game 3.
    #[test]
    fn a_restart_mid_game_three_keeps_its_game() {
        let bytes = fixture_bytes();
        let (_whole_dir, whole, _) = tracked_bytes("arena-mid-three-whole", &bytes);
        let cut = cut_into_game(&bytes, 3, 5);
        let (_dir, config) = restarted_at("arena-mid-three", &bytes, cut);
        assert_eq!(all_game_messages(&config), all_game_messages(&whole));
        assert_eq!(synced_games(&config), synced_games(&whole));
    }

    /// Restarting and reading on stores what one read stores, wherever the
    /// restart falls: after each intermission, a few entries into each
    /// game, and at evenly spread points, in both fixtures.
    #[test]
    fn a_restart_anywhere_equals_one_read() {
        for name in ["bo3-three-games", "bo3-loss-and-match-concede"] {
            let bytes = fixture_tools::arena_fixture(name).bytes;
            let (_whole_dir, whole, _) = tracked_bytes(&format!("arena-any-{name}"), &bytes);
            let expected = all_game_messages(&whole);
            let entries = arena_protocol::split_complete(&bytes);
            let ends = entry_ends(&bytes);
            let mut cuts: Vec<usize> = Vec::new();
            for (index, entry) in entries.iter().enumerate() {
                let messages = game_messages(entry);
                if messages.iter().any(GameMessage::is_intermission) {
                    cuts.push(ends[index]);
                }
                if messages.iter().any(|message| message.game_number.is_some())
                    && let Some(end) = ends.get(index + 2)
                {
                    cuts.push(*end);
                }
            }
            for step in 1..8 {
                cuts.push(ends[ends.len() * step / 8]);
            }
            cuts.sort_unstable();
            cuts.dedup();
            // Every game info repeats in its game: keep the first few.
            let mut picked: Vec<usize> = Vec::new();
            for cut in cuts {
                if picked.last().is_none_or(|last| cut > last + 200_000) {
                    picked.push(cut);
                }
            }
            for cut in picked {
                let (_dir, config) = restarted_at(&format!("arena-any-{name}-{cut}"), &bytes, cut);
                assert_eq!(
                    all_game_messages(&config),
                    expected,
                    "{name} restarted at byte {cut}"
                );
            }
        }
    }

    /// The batch that writes a match result carries no archive or game
    /// sync: those wait for a writer request after it, so the result
    /// reaches the pages without waiting on the sync.
    #[test]
    fn the_result_batch_carries_no_arena_sync() {
        let dir = TestDir::new("arena-result-batch");
        dir.write(PLAYER_LOG, &fixture_bytes());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        engine.flush().expect("startup batch");
        let reader = reader(&config);
        engine
            .arena
            .rescan(&reader, &engine.writer, NOW)
            .expect("rescan");
        let mut batch = WriteBatch::default();
        let mut decided = 0;
        for _ in 0..1_000 {
            let polled = engine
                .arena
                .poll(&reader, &mut batch, NOW, chrono_tz::UTC)
                .expect("poll");
            decided += polled.matches.iter().filter(|w| is_settled(w)).count();
            if !polled.more {
                break;
            }
        }
        assert!(decided >= 2, "both matches are decided");
        assert!(batch.arena_archive_tokens.is_empty());
        assert!(batch.arena_sync_tokens.is_empty());
        assert_eq!(
            engine.arena.end_syncs.len(),
            2,
            "queued for after the batch"
        );
    }

    /// Once a decided match's game rows are written, it is pushed again
    /// (the pages that read it at its result read the game rows).
    #[test]
    fn a_decided_match_is_pushed_again_after_its_game_sync() {
        let dir = TestDir::new("arena-pushed-again");
        dir.write(PLAYER_LOG, &fixture_bytes());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let mut complete_pushes: HashMap<String, usize> = HashMap::new();
        for now in (NOW..).take(1_000) {
            let tick = engine.tick(now).expect("tick");
            for event in tick.events {
                if let crate::TrackerEvent::MatchUpdated { view, .. } = event
                    && view.state == MatchState::Complete
                {
                    *complete_pushes.entry(view.token).or_insert(0) += 1;
                }
            }
            if !tick.more && !tick.syncing {
                break;
            }
            if tick.syncing {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        engine.finish().expect("finish");
        assert_eq!(complete_pushes.len(), 2);
        for (token, pushes) in &complete_pushes {
            assert!(*pushes >= 2, "{token} pushed at its result and again");
        }
        assert_eq!(number(&config, "SELECT COUNT(*) FROM arena_game_syncs"), 2);
    }

    fn held_message(index: u64) -> (GameMessage, NewGameMessage) {
        let message = GameMessage {
            source: MessageSource::Client,
            json: format!("{{\"n\":{index}}}"),
            match_id: None,
            game_number: None,
            best_of_three: None,
            at_ms: None,
        };
        let new = NewGameMessage {
            file_id: 1,
            byte_offset: index,
            seq: GAME_MESSAGE_SEQ,
            match_token: String::new(),
            game_number: None,
            source: GameMessageSource::Client,
            logged_at: 0,
            json: message.json.clone(),
        };
        (message, new)
    }

    #[test]
    fn held_game_messages_are_capped_and_go_to_the_next_bo3_match() {
        let mut capture = tracked_capture(&["m1"]);
        for index in 0..(HOLD_CAP as u64 + 2) {
            let (message, new) = held_message(index);
            assert!(capture.capture(message, new).is_empty());
        }
        assert_eq!(capture.held.len(), HOLD_CAP);
        assert!(capture.hold_overflowed, "warned once for the overflow");
        let (mut message, new) = held_message(HOLD_CAP as u64 + 2);
        message.match_id = Some("m1".to_string());
        message.game_number = Some(1);
        message.best_of_three = Some(true);
        let stored = capture.capture(message, new);
        assert_eq!(stored.len(), HOLD_CAP + 1);
        assert_eq!(stored[0].byte_offset, 2, "the two oldest were dropped");
        assert!(stored.iter().all(|m| m.match_token == "m1"));
        assert!(capture.held.is_empty());
        let (_, new) = held_message(HOLD_CAP as u64 + 3);
        capture.hold(new);
        assert!(
            !capture.hold_overflowed,
            "room again: a later overflow warns again"
        );
    }

    #[test]
    fn a_completed_room_state_ends_the_current_match() {
        let mut capture = tracked_capture(&["m1", "m2"]);
        let (mut message, new) = held_message(0);
        message.match_id = Some("m1".to_string());
        message.best_of_three = Some(true);
        assert_eq!(capture.capture(message, new).len(), 1);
        capture.note_event(&ArenaEvent::RoomState {
            match_id: "m1".to_string(),
            completed: true,
            players: Vec::new(),
            results: Vec::new(),
            at_ms: None,
        });
        let (message, new) = held_message(1);
        assert!(
            capture.capture(message, new).is_empty(),
            "held for the next match"
        );
        let (mut message, new) = held_message(2);
        message.match_id = Some("m2".to_string());
        message.best_of_three = Some(false);
        let stored = capture.capture(message, new);
        assert_eq!(stored.len(), 2, "a Bo1's messages are stored too");
        assert!(stored.iter().all(|m| m.match_token == "m2"));
        assert!(capture.held.is_empty());
    }

    /// A capture that a room state already put `tokens` in a tracked
    /// queue, before anything named them current.
    fn tracked_capture(tokens: &[&str]) -> GameCapture {
        let mut capture = GameCapture::default();
        for token in tokens {
            capture.tracked.insert(token.to_string());
        }
        capture
    }

    fn room_state(match_id: &str, event_id: &str, completed: bool) -> ArenaEvent {
        let seat = |seat: u32, user_id: &str| Seat {
            seat,
            team: seat,
            user_id: user_id.to_string(),
            name: String::new(),
            event_id: event_id.to_string(),
        };
        ArenaEvent::RoomState {
            match_id: match_id.to_string(),
            completed,
            players: vec![seat(1, "local"), seat(2, "opponent")],
            results: Vec::new(),
            at_ms: None,
        }
    }

    #[test]
    fn an_untracked_event_holds_its_messages_until_it_ends() {
        for event in ["DirectGame", "Traditional_Draft"] {
            let mut capture = GameCapture::default();
            capture.note_event(&room_state("m1", event, false));
            let (mut message, new) = held_message(0);
            message.match_id = Some("m1".to_string());
            message.best_of_three = Some(false);
            assert!(capture.capture(message, new).is_empty(), "{event}");
            let (message, new) = held_message(1);
            assert!(capture.capture(message, new).is_empty(), "{event}");
            assert_eq!(capture.held.len(), 2);
            capture.note_event(&room_state("m1", event, true));
            assert!(capture.held.is_empty(), "{event}: dropped at its end");
        }
    }

    fn gre_entry(offset: u64, messages: &[&str]) -> RawEntry {
        RawEntry {
            offset,
            len: 1,
            head: "[UnityCrossThreadLogger]Match to PLAYER: GreToClientEvent".to_string(),
            json: Some(format!(
                "{{\"timestamp\":\"1793711206806\",\"greToClientEvent\":{{\"greToClientMessages\":[{}]}}}}",
                messages.join(",")
            )),
        }
    }

    fn game_state(game_number: u32) -> String {
        format!(
            "{{\"type\":\"GREMessageType_GameStateMessage\",\"gameStateMessage\":{{\"gameInfo\":{{\"matchID\":\"m1\",\"gameNumber\":{game_number},\"matchWinCondition\":\"MatchWinCondition_Best2of3\"}}}}}}"
        )
    }

    const NO_INFO: &str = "{\"type\":\"GREMessageType_GameStateMessage\",\"gameStateMessage\":{}}";
    const INTERMISSION: &str = "{\"type\":\"GREMessageType_IntermissionReq\"}";

    /// As `poll_file` does: the entry's events first, then its messages.
    fn read_entry(capture: &mut GameCapture, entry: &RawEntry) -> Vec<Option<u32>> {
        for event in decode_all(entry) {
            capture.note_event(&event);
        }
        capture
            .capture_entry(1, entry, 0)
            .iter()
            .map(|message| message.game_number)
            .collect()
    }

    #[test]
    fn an_entry_message_before_a_game_info_keeps_the_earlier_game() {
        let mut capture = tracked_capture(&["m1"]);
        assert_eq!(
            read_entry(&mut capture, &gre_entry(1, &[&game_state(1)])),
            vec![Some(1)]
        );
        let entry = gre_entry(2, &[NO_INFO, &game_state(2), NO_INFO]);
        assert_eq!(
            read_entry(&mut capture, &entry),
            vec![Some(1), Some(2), Some(2)]
        );
    }

    #[test]
    fn an_intermission_moves_later_messages_to_the_next_game() {
        let mut capture = tracked_capture(&["m1"]);
        // A message before anything names the match is game 1 once named.
        let first = read_entry(&mut capture, &gre_entry(1, &[NO_INFO]));
        assert!(first.is_empty(), "held");
        let entry = gre_entry(2, &[&game_state(1), INTERMISSION, NO_INFO]);
        assert_eq!(
            read_entry(&mut capture, &entry),
            vec![Some(1), Some(1), Some(1), Some(2)]
        );
        // Arena can repeat the intermission: still the next game.
        let entry = gre_entry(3, &[INTERMISSION, NO_INFO]);
        assert_eq!(read_entry(&mut capture, &entry), vec![Some(2), Some(2)]);
        let entry = gre_entry(4, &[&game_state(2), INTERMISSION, NO_INFO]);
        assert_eq!(
            read_entry(&mut capture, &entry),
            vec![Some(2), Some(2), Some(3)]
        );
    }

    /// Each match's games as `game_number:local rows:opponent rows:turn_count`,
    /// matches in start order.
    fn synced_games(config: &TrackerConfig) -> Vec<String> {
        texts(
            config,
            "SELECT m.token || ' ' || g.game_number || ':'
                 || (SELECT COUNT(*) FROM game_cards c
                     WHERE c.game_id = g.id AND c.side = 'local' AND c.has_zone_data = 1)
                 || ':' || (SELECT COUNT(*) FROM game_cards c
                     WHERE c.game_id = g.id AND c.side = 'opponent')
                 || ':' || COALESCE(g.turn_count, 'none')
             FROM games g JOIN matches m ON m.id = g.match_id
             ORDER BY m.started_at, g.game_number",
        )
    }

    fn match_tokens(config: &TrackerConfig) -> Vec<String> {
        texts(config, "SELECT token FROM matches ORDER BY started_at")
    }

    #[test]
    fn fixture_match_gets_game_rows() {
        let (_dir, config, _) = tracked_fixture("arena-game-sync-rows");
        let tokens = match_tokens(&config);
        let first = &tokens[0];
        let games: Vec<String> = synced_games(&config)
            .into_iter()
            .filter(|line| line.starts_with(first.as_str()))
            .collect();
        assert!(!games.is_empty());
        for line in &games {
            let parts: Vec<&str> = line.split(' ').nth(1).expect("game").split(':').collect();
            assert_ne!(parts[1], "0", "local rows: {line}");
            assert_ne!(parts[2], "0", "opponent rows: {line}");
            assert_ne!(parts[3], "none", "turn count: {line}");
        }
        assert_eq!(
            texts(
                &config,
                "SELECT status || ' ' || derived_version FROM arena_game_syncs ORDER BY match_id"
            ),
            vec!["derived 2".to_string(), "derived 2".to_string()]
        );
        assert_eq!(
            number(
                &config,
                "SELECT COUNT(*) FROM matches WHERE games_synced_at IS NOT NULL"
            ),
            2
        );
        let reader = reader(&config);
        assert!(
            store::arena_game_sync::arena_matches_to_sync(&reader)
                .expect("to sync")
                .is_empty(),
            "every match is stamped"
        );
    }

    /// A match that goes quiet mid-game settles without a result in
    /// `finalize`: it is archived and synced straight away, not at the next
    /// start (#571).
    #[test]
    fn a_match_settled_without_a_result_is_synced_straight_away() {
        let bytes = fixture_bytes();
        let cut = cut_into_game(&bytes, 2, 5);
        let (_dir, config, _) = tracked_bytes("arena-settled-no-result", &bytes[..cut]);
        assert_eq!(
            texts(&config, "SELECT state FROM matches"),
            vec!["abandoned".to_string()]
        );
        assert_eq!(
            texts(&config, "SELECT status FROM arena_game_syncs"),
            vec!["derived".to_string()],
            "synced before any restart"
        );
        assert_eq!(
            number(&config, "SELECT COUNT(*) FROM arena_game_messages"),
            0,
            "archived"
        );
    }

    /// A start rebuilds a quietly settled match in progress, and the first
    /// tick settles it again: with no new messages and a current stamp it
    /// is not derived again (review of #571).
    #[test]
    fn a_settled_match_is_not_derived_again_at_each_start() {
        let bytes = fixture_bytes();
        let cut = cut_into_game(&bytes, 2, 5);
        let (_dir, config, now) = tracked_bytes("arena-settled-no-rederive", &bytes[..cut]);
        let stamped = texts(
            &config,
            "SELECT CAST(synced_at AS TEXT) FROM arena_game_syncs",
        );
        assert_eq!(stamped.len(), 1);
        restart(&config, now + 60_000);
        assert_eq!(
            texts(
                &config,
                "SELECT CAST(synced_at AS TEXT) FROM arena_game_syncs"
            ),
            stamped
        );
    }

    /// A quiet spell shorter than the abandon hour settles nothing: the
    /// cut match stays in progress with its messages live, and the rest of
    /// the log after a restart stores what one read stores.
    #[test]
    fn a_short_quiet_spell_keeps_the_match_live() {
        let bytes = fixture_bytes();
        let cut = cut_into_game(&bytes, 2, 5);
        let (_probe_dir, probe, _) = tracked_bytes("arena-short-quiet-probe", &bytes[..cut]);
        // The probe settled the match at its last activity.
        let start = number(&probe, "SELECT ended_at FROM matches") + 60_000;
        let (_whole_dir, whole, _) = tracked_bytes("arena-short-quiet-whole", &bytes);

        let dir = TestDir::new("arena-short-quiet");
        dir.write(PLAYER_LOG, &bytes[..cut]);
        let config = config(&dir);
        let mut engine = open(&config, start);
        let now = run(&mut engine, start);
        engine.finish().expect("finish");
        assert_eq!(
            texts(&config, "SELECT state FROM matches"),
            vec!["in_progress".to_string()]
        );
        assert!(number(&config, "SELECT COUNT(*) FROM arena_game_messages") > 0);
        assert_eq!(number(&config, "SELECT COUNT(*) FROM arena_game_syncs"), 0);

        dir.append(PLAYER_LOG, &bytes[cut..]);
        let mut engine = open(&config, now);
        run(&mut engine, now);
        engine.finish().expect("finish");
        assert_eq!(all_game_messages(&config), all_game_messages(&whole));
        assert_eq!(synced_games(&config), synced_games(&whole));
    }

    /// A match settled by silence and synced, then taken up again by the
    /// rest of the log in the same run (no restart), ends as one read
    /// stores it.
    #[test]
    fn a_settled_match_that_resumes_ends_as_one_read() {
        let bytes = fixture_bytes();
        let cut = cut_into_game(&bytes, 2, 5);
        let (_whole_dir, whole, _) = tracked_bytes("arena-resumed-whole", &bytes);

        let dir = TestDir::new("arena-resumed");
        dir.write(PLAYER_LOG, &bytes[..cut]);
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let now = run(&mut engine, NOW);
        assert_eq!(
            texts(&config, "SELECT state FROM matches"),
            vec!["abandoned".to_string()]
        );
        assert_eq!(number(&config, "SELECT COUNT(*) FROM arena_game_syncs"), 1);

        dir.append(PLAYER_LOG, &bytes[cut..]);
        run(&mut engine, now);
        engine.finish().expect("finish");
        assert_eq!(all_game_messages(&config), all_game_messages(&whole));
        assert_eq!(synced_games(&config), synced_games(&whole));
    }

    #[test]
    fn match_concede_after_game_one_has_one_game() {
        let (_dir, config, _) = tracked_fixture("arena-game-sync-concede");
        let tokens = match_tokens(&config);
        let second = &tokens[1];
        let games: Vec<String> = synced_games(&config)
            .into_iter()
            .filter(|line| line.starts_with(second.as_str()))
            .collect();
        assert_eq!(games.len(), 1, "{games:?}");
        let parts: Vec<&str> = games[0]
            .split(' ')
            .nth(1)
            .expect("game")
            .split(':')
            .collect();
        assert_eq!(parts[0], "1");
        assert_ne!(parts[1], "0", "the deck's local rows: {}", games[0]);
    }

    #[test]
    fn startup_syncs_unstamped_matches() {
        let (_dir, config, now) = tracked_fixture("arena-game-sync-startup");
        let synced = synced_games(&config);
        {
            let conn = store::open_writer(&config.db_path).expect("writer");
            conn.execute_batch(
                "DELETE FROM game_cards; DELETE FROM game_mulligan_cards;
                 DELETE FROM arena_game_syncs;",
            )
            .expect("forget the sync");
        }
        assert_ne!(synced_games(&config), synced);
        let mut engine = open(&config, now);
        run(&mut engine, now);
        engine.finish().expect("finish");
        assert_eq!(synced_games(&config), synced);
        assert_eq!(number(&config, "SELECT COUNT(*) FROM arena_game_syncs"), 2);
    }

    /// A tracker that read `bytes` as `log` (relative to the test folder)
    /// the way the app did before Arena game data: no game messages, no
    /// Arena game sync.
    fn tracked_before_game_data(
        label: &str,
        log: &str,
        bytes: &[u8],
    ) -> (TestDir, TrackerConfig, i64) {
        let dir = TestDir::new(label);
        dir.write(log, bytes);
        if log != PLAYER_LOG {
            dir.write(PLAYER_LOG, b"");
        }
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        engine.arena.game_data_off = true;
        let now = run(&mut engine, NOW);
        engine.finish().expect("finish");
        assert_eq!(number(&config, "SELECT COUNT(*) FROM matches"), 2);
        assert!(all_game_messages(&config).is_empty());
        assert_eq!(number(&config, "SELECT COUNT(*) FROM arena_game_syncs"), 0);
        (dir, config, now)
    }

    /// Start again (game data on) and read what there is.
    fn restart(config: &TrackerConfig, now: i64) -> i64 {
        let mut engine = open(config, now);
        let now = run(&mut engine, now);
        engine.finish().expect("finish");
        now
    }

    fn stamps(config: &TrackerConfig) -> Vec<String> {
        texts(
            config,
            "SELECT status || ' ' || derived_version FROM arena_game_syncs ORDER BY match_id",
        )
    }

    #[test]
    fn backfill_fills_a_match_tracked_before_plan_3() {
        let (_live_dir, live, _) = tracked_fixture("arena-backfill-live");
        let (_dir, config, now) =
            tracked_before_game_data("arena-backfill", PLAYER_LOG, &fixture_bytes());
        let events = dump_rows(&config, "arena_log_events");
        let files = dump(&config, "arena_log_files");
        restart(&config, now);
        assert_eq!(all_game_messages(&config), all_game_messages(&live));
        assert_eq!(synced_games(&config), synced_games(&live));
        assert_eq!(
            stamps(&config),
            vec!["derived 2".to_string(), "derived 2".to_string()]
        );
        assert_eq!(
            number(&config, "SELECT COUNT(*) FROM arena_game_messages"),
            0,
            "archived"
        );
        assert_eq!(
            dump_rows(&config, "arena_log_events"),
            events,
            "events untouched"
        );
        assert_eq!(
            dump(&config, "arena_log_files"),
            files,
            "read offsets untouched"
        );
    }

    #[test]
    fn backfill_reads_player_prev_log() {
        let dir = TestDir::new("arena-backfill-prev-live");
        dir.write(PREV_LOG, &fixture_bytes());
        dir.write(PLAYER_LOG, b"");
        let live = config(&dir);
        restart(&live, NOW);
        let (_dir, config, now) =
            tracked_before_game_data("arena-backfill-prev", PREV_LOG, &fixture_bytes());
        restart(&config, now);
        assert!(!all_game_messages(&live).is_empty());
        assert_eq!(all_game_messages(&config), all_game_messages(&live));
        assert_eq!(synced_games(&config), synced_games(&live));
        assert_eq!(
            stamps(&config),
            vec!["derived 2".to_string(), "derived 2".to_string()]
        );
    }

    #[test]
    fn backfill_marks_missing_matches() {
        let (dir, config, now) =
            tracked_before_game_data("arena-backfill-missing", PLAYER_LOG, &fixture_bytes());
        std::fs::remove_file(dir.join(PLAYER_LOG)).expect("the log is gone");
        let now = restart(&config, now);
        assert_eq!(
            stamps(&config),
            vec!["no_messages 2".to_string(), "no_messages 2".to_string()]
        );
        let syncs = dump_rows(&config, "arena_game_syncs");

        // The log is back as Player-prev.log, which the live read skips (it
        // was read to its end) and a backfill would find the messages in.
        // The matches are not looked for again.
        dir.write(PREV_LOG, &fixture_bytes());
        let events = dump_rows(&config, "arena_log_events");
        restart(&config, now);
        assert_eq!(dump_rows(&config, "arena_log_events"), events);
        assert!(all_game_messages(&config).is_empty());
        assert_eq!(dump_rows(&config, "arena_game_syncs"), syncs);
    }

    #[test]
    fn backfill_does_not_duplicate_live_messages() {
        let (_live_dir, live, _) = tracked_fixture("arena-backfill-dup-live");
        let bytes = fixture_bytes();
        // Cut after the first match's completed room state: that match is
        // tracked without game data, the second one live.
        let cut = arena_protocol::split_complete(&bytes)
            .iter()
            .find(|entry| {
                decode_all(entry).iter().any(|event| {
                    matches!(
                        event,
                        ArenaEvent::RoomState {
                            completed: true,
                            ..
                        }
                    )
                })
            })
            .map(|entry| (entry.offset + entry.len) as usize)
            .expect("a completed match");
        let (dir, config, now) = tracked_before_game_data_part("arena-backfill-dup", &bytes[..cut]);
        dir.append(PLAYER_LOG, &bytes[cut..]);
        let now = restart(&config, now);
        assert_eq!(all_game_messages(&config), all_game_messages(&live));
        assert_eq!(synced_games(&config), synced_games(&live));
        assert_eq!(
            number(
                &config,
                "SELECT SUM(message_count) FROM arena_game_archives"
            ),
            number(&live, "SELECT SUM(message_count) FROM arena_game_archives"),
        );

        // A live track is never backfilled: nothing changes.
        let archives = dump_rows(&config, "arena_game_archives");
        restart(&config, now);
        assert_eq!(dump_rows(&config, "arena_game_archives"), archives);
    }

    /// As `tracked_before_game_data`, for a log that holds one match.
    fn tracked_before_game_data_part(label: &str, bytes: &[u8]) -> (TestDir, TrackerConfig, i64) {
        let dir = TestDir::new(label);
        dir.write(PLAYER_LOG, bytes);
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        engine.arena.game_data_off = true;
        let now = run(&mut engine, NOW);
        engine.finish().expect("finish");
        assert_eq!(number(&config, "SELECT COUNT(*) FROM matches"), 1);
        (dir, config, now)
    }

    #[test]
    fn startup_archives_and_derives_late_messages() {
        let (_dir, config, now) = tracked_fixture("arena-late-messages");
        let token = match_tokens(&config).remove(0);
        let before = store::arena_games::game_messages(&reader(&config), &token)
            .expect("messages")
            .len();
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute(
                "INSERT INTO arena_game_messages (arena_log_file_id, byte_offset, seq,
                     match_token, game_number, source, logged_at, raw, compressed)
                 VALUES (1, 999999999, 0, ?1, 3, 'client', 1, CAST('{}' AS BLOB), 0)",
                [&token],
            )
            .expect("late message");
        restart(&config, now);
        assert_eq!(
            number(&config, "SELECT COUNT(*) FROM arena_game_messages"),
            0,
            "archived at start"
        );
        assert_eq!(
            store::arena_games::game_messages(&reader(&config), &token)
                .expect("messages")
                .len(),
            before + 1
        );
        assert!(
            store::arena_game_sync::arena_matches_to_sync(&reader(&config))
                .expect("due")
                .is_empty()
        );
    }

    #[test]
    fn utc_offsets_follow_the_zone_at_the_time() {
        let london: Tz = "Europe/London".parse().expect("zone");
        // 2026-07-01 (summer time) and 2026-01-01.
        assert_eq!(utc_offset_minutes(london, 1_782_864_000_000), 60);
        assert_eq!(utc_offset_minutes(london, 1_767_225_600_000), 0);
        assert_eq!(utc_offset_minutes(chrono_tz::UTC, NOW), 0);
    }

    /// A card data answer that names `ids` (and changes what the game sync
    /// reads: the name and oracle id).
    fn card_answers(ids: &[u32]) -> store::cards::CardsUpdate {
        let resolved = ids
            .iter()
            .map(|&id| mymtgo_api::Resolution {
                mtgo_id: id,
                card: mymtgo_api::CardAnswer {
                    scryfall_id: Some(format!("q-scryfall-{id}")),
                    oracle_id: Some(format!("q-oracle-{id}")),
                    name: Some(format!("Qcard {id}")),
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

    #[test]
    fn card_data_keeps_arena_game_rows() {
        let dir = TestDir::new("arena-card-data-keeps-rows");
        dir.write(PLAYER_LOG, &fixture_bytes());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let now = run(&mut engine, NOW);
        let cards = dump_rows(&config, "game_cards");
        let mulligans = dump_rows(&config, "game_mulligan_cards");
        assert!(!cards.is_empty(), "the Arena sync wrote game cards");
        assert!(!mulligans.is_empty(), "the Arena sync wrote mulligan cards");
        let ids: Vec<u32> = texts(
            &config,
            "SELECT DISTINCT CAST(catalog_id AS TEXT) FROM game_cards ORDER BY catalog_id",
        )
        .iter()
        .map(|id| id.parse().expect("catalog id"))
        .collect();
        let outcome = engine.resolve_cards(card_answers(&ids)).expect("resolved");
        assert!(!outcome.changed.is_empty(), "the answers change card data");
        run(&mut engine, now + 10_000);
        engine.finish().expect("finish");
        assert_eq!(dump_rows(&config, "game_cards"), cards);
        assert_eq!(dump_rows(&config, "game_mulligan_cards"), mulligans);
        assert_eq!(
            number(
                &config,
                "SELECT COUNT(*) FROM matches WHERE games_synced_at IS NOT NULL"
            ),
            2
        );
    }
}
