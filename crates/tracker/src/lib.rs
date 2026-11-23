//! The live tracker (spec 3): finds MTGO logs, reads new events, projects matches,
//! stores everything, and reports changes. `Engine` does one tick of work at a
//! time and is tested directly.

pub mod clock;
pub mod engine;
pub mod history;
pub mod supervisor;

use std::fmt;
use std::path::PathBuf;
use std::time::Instant;

use chrono::NaiveDate;
use chrono_tz::Tz;
use mtgo_match::MatchView;
use store::StoreError;

pub use clock::Clock;
pub use engine::{Engine, Tick, TickTiming};
pub use supervisor::{
    ACCOUNT_EDIT_TIMEOUT, ARCHETYPE_EDIT_TIMEOUT, AccountEditor, ArchetypeEditor,
    CARD_IMAGES_TIMEOUT, CARD_STATS_REGENERATE_TIMEOUT, CARD_STUB_SCAN_TIMEOUT,
    CARDS_RESOLVED_TIMEOUT, CardImagesRecorder, CardStubScanner, CardsResolver, Command,
    DECK_EDIT_TIMEOUT, DECK_SYNC_TIMEOUT, DRAFT_NOTE_EDIT_TIMEOUT, DeckEditor, DraftNoteEditor,
    ENDSTEP_CAPTURES_STORED_TIMEOUT, EndstepCaptureImporter, IMPORT_TIMEOUT, Importer,
    LEAGUE_EDIT_TIMEOUT, LeagueEditor, MANUAL_EDIT_TIMEOUT, MATCH_EDIT_TIMEOUT, ManualEditor,
    MatchEditor, OUTBOX_EDIT_TIMEOUT, OutboxEditor, RECORDINGS_STORED_TIMEOUT, RecordingImporter,
    SAVE_TIMEOUT, SYNC_EDIT_TIMEOUT, SettingsSaver, SyncEditor, TrackerHandle, spawn,
    spawn_with_clock,
};

/// Reads the OS timezone and, when it cannot be used, why.
pub type ReadTimezone = fn() -> (Tz, Option<String>);

#[derive(Debug, Clone)]
pub struct TrackerConfig {
    pub db_path: PathBuf,
    /// The timezone for new log files when `os_timezone` is `None`, and the
    /// starting timezone before the OS zone is first read.
    pub timezone: Tz,
    /// Reads the OS timezone (and a warning when it cannot be used) at startup
    /// and at every rescan, so a new log file is dated in the zone the OS has
    /// when the file appears (spec 5.4). `None` (tests, the log player) dates
    /// every new file in `timezone`.
    pub os_timezone: Option<ReadTimezone>,
    /// Folders or files to search instead of the platform defaults (spec 5.1).
    pub log_overrides: Vec<PathBuf>,
    /// Arena `Player.log` files to read instead of the platform defaults
    /// (Arena spec section 3). Each one's `Player-prev.log` sits beside it.
    pub arena_log_overrides: Vec<PathBuf>,
    /// The folder searched for MTGO's account folders (deck files) instead of
    /// the `Data` folders beside the detected log roots (decks spec 3).
    pub mtgo_data_folder: Option<PathBuf>,
    /// Replays only (log player, tests): date new files from this day, and never
    /// re-anchor dates to the read time, because the bytes are old.
    pub replay_day_anchor: Option<NaiveDate>,
    /// The watcher is on (release spec 4.1, the `watcher_active` setting):
    /// off, the tracker reads no log file and no deck XML until
    /// `Command::SetWatching` turns it on, then resumes from each file's
    /// stored cursor.
    pub watching: bool,
    /// Arena is on (the `arena_enabled` setting, resolved by the app: the
    /// stored choice, else whether Arena data exists). Off, the tracker
    /// discovers and reads no Arena log until `Command::SetArenaEnabled`
    /// turns it on.
    pub arena_enabled: bool,
    /// Endstep is on (the `endstep_enabled` setting). Off, the tracker folds
    /// no stored Endstep capture until `Command::SetEndstepEnabled` turns it
    /// on; the captures wait unsynced.
    pub endstep_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackerEvent {
    /// A match's derived result changed. `cause` is the event that changed it.
    /// `manual` is true for a match entered by hand (Plan L), false for one
    /// projected from the log.
    ///
    /// `platform` is the game the match was played on. `opponent_name` is the
    /// opponent's display name where the view's opponent is a registry key
    /// (an Arena `arena:<user id>`); `None` for MTGO, whose opponent is the
    /// name.
    MatchUpdated {
        view: MatchView,
        revision: u64,
        cause: Option<(i64, u64)>,
        manual: bool,
        platform: store::Platform,
        opponent_name: Option<String>,
    },
    /// What a match will look like once the provisional last event commits.
    /// `platform` and `opponent_name` as for `MatchUpdated`.
    MatchPreview {
        view: MatchView,
        platform: store::Platform,
        opponent_name: Option<String>,
    },
    /// A visible match was discarded, or the player removed it from stats.
    MatchRemoved {
        token: String,
    },
    /// Projection failed for a match (stored as `needs_attention`).
    MatchFailed {
        token: String,
        reason: String,
    },
    Status(Status),
    /// The tracker hit an error and is restarting ("tracking paused, restarting").
    Restarting {
        reason: String,
    },
    /// Stored decks changed and are committed: a deck sync wrote something, a
    /// user edit was done, 0.x decks were imported, a match was linked to
    /// a deck version, or a match was removed from stats. At most one per
    /// tick.
    DecksChanged,
    /// League runs or tournaments may have changed and the change is
    /// committed: a match with a league token or a tournament id was derived
    /// or removed, a league join or drop event was stored, a league edit was
    /// done, 0.x leagues were imported, a tournament round adopted its deck,
    /// a match was removed from stats, or the league sync changed a run.
    /// Held while a league sync is pending, so the pages read its rows. At
    /// most one per tick.
    LeaguesChanged,
    /// Limited data changed and the change is committed: a draft event was
    /// stored, a pick note was saved, 0.x drafts, picks, notes or limited
    /// leagues were imported, or the league sync changed a draft (an open
    /// draft past its deadline turned abandoned, say). Held like
    /// `LeaguesChanged`. At most one per tick.
    LimitedChanged,
    /// The event overlay's stored tournaments changed and the change is
    /// committed: the tournament sync changed a standing (a joined event's
    /// round, result, matches left or finish, or one left the 48-hour
    /// window). Never raises `LeaguesChanged`. At most one per tick.
    TournamentsChanged,
    /// Writes committed `cards` stubs (catalog ids met for the first time:
    /// a deck file, a game, a draft, an import or the full scan), so the
    /// cards worker can fill them. `count` is how many rows were added since
    /// the last report. At most one per tick.
    CardStubsAdded {
        count: u32,
    },
    /// Card data was filled (token names or the API's answers) and is
    /// committed: pages that show card names or images refetch. The log
    /// matches whose game rows read a changed card are synced again in the
    /// background (each pushed again when its rows change). At most one per
    /// tick.
    CardsChanged,
    /// The tick stored events that change what a live match's overlay shows
    /// (`game_state_update`, `deck_used`, `game_management_json` or
    /// `match_state_changed`), and they are committed (overlay spec 6 step
    /// 1). `tokens` are the log matches they belong to that are in progress
    /// and not removed from stats, sorted. Frames rarely change a match's
    /// `MatchView`, so `MatchUpdated` alone would miss them. `detected_at`
    /// is when the first of those events was read, for the overlay's speed
    /// budget. Never sent while the tracker catches up on a backlog. At most
    /// one per tick.
    LiveEventsStored {
        tokens: Vec<String>,
        detected_at: Instant,
    },
    /// Archetype rows changed and are committed (a definition, a manual
    /// archetype, a merge): pages that list or name archetypes refetch.
    /// Emitted by the archetype edits that change archetypes (archetypes
    /// spec section 3).
    ArchetypesChanged,
    /// Archetype edits changed these matches' archetype rows or detection
    /// marker, and the change is committed (archetypes spec section 3).
    MatchArchetypesChanged {
        match_ids: Vec<i64>,
    },
    /// The deck sync wrote these decks (by id) and they have no archetype,
    /// committed: the archetypes worker may prefill one (0.x
    /// `SyncDecks::prefillArchetype`, archetypes spec section 6.3). The
    /// sync itself never writes `decks.archetype_id`. At most one per tick.
    DeckNeedsArchetype {
        deck_ids: Vec<i64>,
    },
    /// The current account may have changed, committed (release spec 5): a
    /// Login line was stored, or an account edit (a switch, a tracking
    /// change) was done. Every page refetches. At most one per tick.
    AccountChanged,
    /// Arena rank readings were stored and committed: the rank views (the
    /// dashboard's rank card, an Arena deck's climb) refetch. Held while a
    /// backlog is caught up. At most one per tick.
    RankChanged,
}

/// What the status bar shows (spec 6).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    pub roots: Vec<PathBuf>,
    pub files: Vec<FileStatus>,
    /// Where Arena's `Player.log` is looked for, and each one being read.
    pub arena_searched: Vec<PathBuf>,
    pub arena_files: Vec<FileStatus>,
    pub two_clients: bool,
    pub awaiting_identity: Vec<String>,
    pub quarantined: u32,
    /// Matches whose last projection failed (issue #47). A newer projection
    /// version retries them.
    pub failed_matches: u32,
    /// Set when the OS timezone could not be used (new files are dated in UTC).
    pub timezone_warning: Option<String>,
    /// The watcher is off: logs are found but not read (release spec 4.1).
    pub paused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStatus {
    pub path: PathBuf,
    pub size: u64,
    pub cursor: u64,
    pub last_live_growth_ms: Option<i64>,
    /// Continuation lines dropped because no header came before them (a resume
    /// mid-event or a truncated file), counted since this run began reading it.
    pub dropped_orphan_lines: u64,
    /// The file's stored timezone did not parse, so the tracker's own timezone
    /// is used to date its events instead.
    pub bad_timezone: bool,
}

#[derive(Debug)]
pub enum TrackerError {
    Store(StoreError),
}

impl fmt::Display for TrackerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TrackerError::Store(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for TrackerError {}

impl From<StoreError> for TrackerError {
    fn from(error: StoreError) -> Self {
        TrackerError::Store(error)
    }
}
