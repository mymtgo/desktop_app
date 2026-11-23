//! Runs the engine on its own thread and keeps it running (spec 3): an error or
//! panic restarts it from the committed state with backoff, and an event that
//! crashes it `MAX_CRASHES` times is quarantined.

use std::collections::{BTreeSet, HashMap};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cards::tokens::TokenCard;
use mtgo_log::Env;
use mtgo_match::panic_message;
use mymtgo_api::Resolutions;
use store::accounts::{AccountEdit, AccountEditOutcome};
use store::archetype_edit::{ArchetypeEdit, ArchetypeEditOutcome};
use store::cards::ResolveOutcome;
use store::decks::{DeckEdit, EditOutcome};
use store::history_import::{HistoryImportCounts, HistoryMatch};
use store::import::{AccountImportCounts, ImportCounts, ImportedAccount, ImportedMatch};
use store::import_api::{ApiImportCounts, ImportedApiState};
use store::import_archetypes::{ArchetypeImportCounts, ImportedArchetypes};
use store::import_decks::{DeckImportCounts, ImportedDecks};
use store::import_leagues::{ImportedLeague, LeagueImportCounts};
use store::import_limited::{ImportedLimited, LimitedImportCounts};
use store::import_overlay::{LegacyDeckLabel, OverlayImportCounts};
use store::imported_detail::{DetailImport, ImportedDetail};
use store::leagues::{LeagueEdit, LeagueEditOutcome};
use store::limited::{DraftNoteEdit, DraftNoteOutcome};
use store::manual::{ManualEdit, ManualEditOutcome};
use store::match_edits::{MatchEdit, MatchEditOutcome};
use store::outbox::{OutboxEdit, OutboxOutcome};
use store::sync::SyncEdit;

use crate::clock::{Clock, TrackerClock};
use crate::engine::{Engine, InFlight, InFlightEvent, POLL_ACTIVE_MS, POLL_IDLE_MS};
use crate::{TrackerConfig, TrackerError, TrackerEvent};

/// Crashes on the same event before it is quarantined (spec 3).
pub const MAX_CRASHES: u32 = 3;
const FIRST_BACKOFF_MS: u64 = 100;
const MAX_BACKOFF_MS: u64 = 5_000;
/// A run at least this long was healthy: the next failure restarts the backoff.
const HEALTHY_RUN: Duration = Duration::from_secs(60);
/// How long `SettingsSaver::save` waits for its answer: longer than the maximum
/// backoff, so a save sent during a restart is answered after the reopen.
pub const SAVE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long an import batch waits for its answer: a batch takes milliseconds,
/// so this only runs out when the tracker cannot work at all.
pub const IMPORT_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a deck sync waits for its answer: reading a few dozen small files
/// takes milliseconds, so this only runs out when tracking cannot work.
pub const DECK_SYNC_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a deck edit waits for its answer: one small transaction, so like
/// `SAVE_TIMEOUT` this only runs out while the tracker restarts or is stuck.
pub const DECK_EDIT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a league edit waits for its answer: one small transaction (a drop
/// or a note folds one account's league inputs first), like `DECK_EDIT_TIMEOUT`.
pub const LEAGUE_EDIT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a match edit waits for its answer: one small transaction, like
/// `DECK_EDIT_TIMEOUT`.
pub const MATCH_EDIT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long an account edit (a switch, a tracking change) waits for its
/// answer: one small transaction, like `MATCH_EDIT_TIMEOUT`.
pub const ACCOUNT_EDIT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a manual match edit waits for its answer: one small
/// transaction, like `DECK_EDIT_TIMEOUT`.
pub const MANUAL_EDIT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a pick note edit waits for its answer: one small transaction
/// (folding one draft's events first), like `DECK_EDIT_TIMEOUT`.
pub const DRAFT_NOTE_EDIT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long an archetype edit waits for its answer: one small transaction
/// (a detection slice of 10 matches stays under 100 ms), like
/// `DECK_EDIT_TIMEOUT`.
pub const ARCHETYPE_EDIT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a card stub scan waits for its answer: the full scan reads every
/// stored list and timeline (under 1 s for 10,000 matches, cards spec
/// section 12), with room for pending writes to commit first.
pub const CARD_STUB_SCAN_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the cards worker waits for a population run to be applied: one
/// transaction over the rows it learned, with room for pending writes to
/// commit first, like `CARD_STUB_SCAN_TIMEOUT`.
pub const CARDS_RESOLVED_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the cards worker waits for a card's image files to be recorded
/// or for the local files to be forgotten: one small transaction.
pub const CARD_IMAGES_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the recorder import worker waits for its files to be stored:
/// one transaction over a batch of files, with room for pending writes.
pub const RECORDINGS_STORED_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the Endstep capture import worker waits for its files to be
/// stored: one transaction over a batch of files, with room for pending
/// writes.
pub const ENDSTEP_CAPTURES_STORED_TIMEOUT: Duration = Duration::from_secs(30);

/// How long "Regenerate" waits for its answer: one transaction that applies
/// the imported rule to the deck's imported matches, with room for pending
/// writes to commit first, like `CARD_STUB_SCAN_TIMEOUT`. The log matches'
/// syncs run after the answer.
pub const CARD_STATS_REGENERATE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the API worker waits for an outbox edit.
pub const OUTBOX_EDIT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the API worker waits for a sync edit: most are one small
/// transaction, but a pulled deck commits the pending live writes first.
pub const SYNC_EDIT_TIMEOUT: Duration = Duration::from_secs(30);
/// Commands queued for the engine before `SettingsSaver::save` reports it busy.
const INBOX: usize = 16;

/// Work the app asks the engine to do on its own thread.
pub enum Command {
    /// Store settings rows, then answer. Skipped, with an error answer, if
    /// `deadline` has passed: the caller has already told the user it failed.
    SaveSettings {
        rows: Vec<(String, String)>,
        reply: Sender<Result<(), String>>,
        deadline: Instant,
    },
    /// Store settings rows (the `watcher_active` row), then switch the
    /// watcher on or off (release spec 4.1) and answer. Off, the tracker
    /// reads no log file and no deck XML; on, it resumes from each file's
    /// stored cursor. The switch holds across error restarts. Skipped, with
    /// an error answer, if `deadline` has passed, like `SaveSettings`.
    SetWatching {
        watching: bool,
        rows: Vec<(String, String)>,
        reply: Sender<Result<(), String>>,
        deadline: Instant,
    },
    /// Store settings rows (the `arena_enabled` row), then switch Arena on
    /// or off and answer, like `SetWatching`. The switch holds across error
    /// restarts. Skipped, with an error answer, if `deadline` has passed.
    SetArenaEnabled {
        enabled: bool,
        rows: Vec<(String, String)>,
        reply: Sender<Result<(), String>>,
        deadline: Instant,
    },
    /// Store settings rows (the `endstep_enabled` row), then switch the
    /// Endstep sync on or off and answer, like `SetArenaEnabled`.
    SetEndstepEnabled {
        enabled: bool,
        rows: Vec<(String, String)>,
        reply: Sender<Result<(), String>>,
        deadline: Instant,
    },
    /// Store imported 0.x matches, then answer with the counts. No deadline:
    /// an import the caller gave up on is still safe to store (the next
    /// import finds its matches unchanged).
    Import {
        matches: Vec<ImportedMatch>,
        reply: Sender<Result<ImportCounts, String>>,
    },
    /// Register the 0.x accounts and their login ids (Plan O), then answer
    /// with the counts. No deadline, like `Import`.
    ImportAccounts {
        accounts: Vec<ImportedAccount>,
        reply: Sender<Result<AccountImportCounts, String>>,
    },
    /// Store imported 0.x cards, decks, versions and match links, then answer
    /// with the counts. Sent after the matches; no deadline, like `Import`.
    ImportDecks {
        decks: ImportedDecks,
        reply: Sender<Result<DeckImportCounts, String>>,
    },
    /// Store imported 0.x leagues, then answer with the counts. Sent after the
    /// decks; no deadline, like `Import`.
    ImportLeagues {
        leagues: Vec<ImportedLeague>,
        reply: Sender<Result<LeagueImportCounts, String>>,
    },
    /// Store imported 0.x drafts, picks, notes and limited registered decks
    /// (Plan N), then answer with the counts. Sent after the leagues; no
    /// deadline, like `Import`.
    ImportLimited {
        data: Box<ImportedLimited>,
        reply: Sender<Result<LimitedImportCounts, String>>,
    },
    /// Store 0.x's API state (07 spec section 11), then answer with the
    /// counts. Sent after the leagues; skipped with an error answer once its
    /// deadline passed, like `SyncEdit`.
    ImportApi {
        state: Box<ImportedApiState>,
        reply: Sender<Result<ApiImportCounts, String>>,
        deadline: Instant,
    },
    /// Store 0.x's archetype data (archetypes spec sections 15 and 21.5),
    /// then answer with the counts. Sent after the matches and decks;
    /// skipped with an error answer once its deadline passed, like
    /// `ImportApi`.
    ImportArchetypes {
        data: Box<ImportedArchetypes>,
        reply: Sender<Result<ArchetypeImportCounts, String>>,
        deadline: Instant,
    },
    /// Store 0.x's overlay deck labels (overlay spec section 12), then
    /// answer with the counts. Sent after the decks; no deadline, like
    /// `ImportDecks`.
    ImportOverlayLabels {
        labels: Vec<LegacyDeckLabel>,
        reply: Sender<Result<OverlayImportCounts, String>>,
    },
    /// Store one imported 0.x match's detail (Plan O), then answer with what
    /// it did. Sent after the matches; no deadline, like `Import`.
    ImportDetail {
        token: String,
        detail: Box<ImportedDetail>,
        reply: Sender<Result<DetailImport, String>>,
    },
    /// Store a batch of MTGO history matches (Plan M), then answer with the
    /// counts. No deadline, like `Import`: a batch the caller gave up on is
    /// still safe to store (a rerun skips its matches).
    ImportHistory {
        matches: Vec<HistoryMatch>,
        reply: Sender<Result<HistoryImportCounts, String>>,
    },
    /// Remove every history import ("Delete all imported"), then answer with
    /// the removed tokens. No deadline: a delete the caller gave up on is
    /// still what the player asked for.
    DeleteHistoryImports {
        reply: Sender<Result<Vec<String>, String>>,
    },
    /// Read the deck files now (the "Sync decks" button), commit what changed,
    /// then answer with how many decks were written. While the watcher is
    /// stopped it reads nothing and answers 0 (release spec 4.1).
    SyncDecks { reply: Sender<Result<u32, String>> },
    /// Apply a user's deck edit (rename, colour identity, cover, archive,
    /// restore) in its own transaction, then answer whether it was done or
    /// refused. Skipped, with an error answer, if `deadline` has passed: the
    /// caller has already told the user it failed, so it must not apply later.
    DeckEdit {
        edit: DeckEdit,
        reply: Sender<Result<EditOutcome, String>>,
        deadline: Instant,
    },
    /// Apply a user's league edit (manual league, link, unlink, notes, drop)
    /// in its own transaction, then answer whether it was done or refused.
    /// Skipped, with an error answer, if `deadline` has passed, like
    /// `DeckEdit`.
    LeagueEdit {
        edit: LeagueEdit,
        reply: Sender<Result<LeagueEditOutcome, String>>,
        deadline: Instant,
    },
    /// Apply a user's match edit (notes, "Remove from stats") in its own
    /// transaction, then answer whether it was done or refused. Skipped, with
    /// an error answer, if `deadline` has passed, like `DeckEdit`.
    MatchEdit {
        edit: MatchEdit,
        reply: Sender<Result<MatchEditOutcome, String>>,
        deadline: Instant,
    },
    /// Apply a user's account edit (a switch, a tracking change; release
    /// spec 5) in its own transaction, then answer whether it was done or
    /// refused. Skipped, with an error answer, if `deadline` has passed,
    /// like `DeckEdit`.
    AccountEdit {
        edit: AccountEdit,
        reply: Sender<Result<AccountEditOutcome, String>>,
        deadline: Instant,
    },
    /// Apply a user's manual match edit (create, hand, sideboard, reveals)
    /// in its own transaction, then answer whether it was done or refused.
    /// Skipped, with an error answer, if `deadline` has passed, like
    /// `DeckEdit`.
    ManualEdit {
        edit: ManualEdit,
        reply: Sender<Result<ManualEditOutcome, String>>,
        deadline: Instant,
    },
    /// Apply a user's pick note edit (Plan N) in its own transaction, then
    /// answer whether it was done or refused. Skipped, with an error answer,
    /// if `deadline` has passed, like `DeckEdit`.
    DraftNoteEdit {
        edit: DraftNoteEdit,
        reply: Sender<Result<DraftNoteOutcome, String>>,
        deadline: Instant,
    },
    /// Apply an archetype edit (archetypes spec section 3: the player's or
    /// the archetypes worker's) in its own transaction, then answer whether
    /// it was done or refused. Skipped, with an error answer, if `deadline`
    /// has passed, like `DraftNoteEdit`: a caller that gave up never sees a
    /// late write.
    ArchetypeEdit {
        edit: Box<ArchetypeEdit>,
        reply: Sender<Result<ArchetypeEditOutcome, String>>,
        deadline: Instant,
    },
    /// Apply an API worker's outbox edit (07 spec section 7) in its own
    /// transaction, then answer what it did. Skipped, with an error answer,
    /// if `deadline` has passed, like `DeckEdit`: a claim the worker gave up
    /// on never marks rows `sending`.
    OutboxEdit {
        edit: OutboxEdit,
        reply: Sender<Result<OutboxOutcome, String>>,
        deadline: Instant,
    },
    /// Apply an API worker's sync edit (07 spec section 10) in its own
    /// transaction, then answer how many rows it changed. Skipped, with an
    /// error answer, if `deadline` has passed, like `OutboxEdit`.
    SyncEdit {
        edit: SyncEdit,
        reply: Sender<Result<u32, String>>,
        deadline: Instant,
    },
    /// Write a stub for every stored catalog id without a `cards` row (the
    /// full scan, cards spec section 5) in its own transaction, then answer
    /// how many were added. Skipped, with an error answer, if `deadline` has
    /// passed, like `DraftNoteEdit`.
    ScanCardStubs {
        reply: Sender<Result<u32, String>>,
        deadline: Instant,
    },
    /// Write a stub for each given catalog id without a `cards` row (an
    /// archetype decklist's cards the user never met, issue #326) in its
    /// own transaction, then answer how many were added. Skipped, with an
    /// error answer, if `deadline` has passed.
    EnsureCardStubs {
        catalog_ids: Vec<u32>,
        reply: Sender<Result<u32, String>>,
        deadline: Instant,
    },
    /// Write a named token row for each token an Arena replay found no art
    /// for (`store::cards::ensure_token_stubs`) in its own transaction, then
    /// answer how many rows were added. Skipped, with an error answer, if
    /// `deadline` has passed.
    EnsureTokenStubs {
        tokens: Vec<TokenCard>,
        reply: Sender<Result<u32, String>>,
        deadline: Instant,
    },
    /// Apply a population run (cards spec section 6): the token names, then
    /// the API's answers, in one transaction; then sync again the log
    /// matches whose game rows read a changed card and report
    /// `CardsChanged`. Skipped, with an error answer, if `deadline` has
    /// passed: the worker has given up on it and its next run asks again,
    /// so a late answer never writes.
    CardsResolved {
        tokens: Vec<TokenCard>,
        resolutions: Resolutions,
        /// When the lookup ran: the ids it found unknown are marked with it.
        unknown_at_ms: i64,
        reply: Sender<Result<ResolveOutcome, String>>,
        deadline: Instant,
    },
    /// Store the recorder files the import worker read, in one transaction,
    /// then answer what each did. Skipped, with an error answer, if
    /// `deadline` has passed: the next poll reads the files again.
    RecordingsStored {
        recordings: Vec<store::recorded_replays::NewRecording>,
        now_ms: i64,
        reply: Sender<Result<store::recorded_replays::ImportReport, String>>,
        deadline: Instant,
    },
    /// Store the Endstep capture files the import worker read, in one
    /// transaction, then answer what each did. Skipped, with an error
    /// answer, if `deadline` has passed: the next poll reads the files again.
    EndstepCapturesStored {
        captures: Vec<store::endstep_captures::NewCapture>,
        now_ms: i64,
        reply: Sender<Result<store::endstep_captures::CaptureImportReport, String>>,
        deadline: Instant,
    },
    /// Record a card's downloaded or copied image files (cards spec section
    /// 7): only the given columns are set. Skipped, with an error answer, if
    /// `deadline` has passed: the file stays unrecorded and the next run
    /// fetches it again.
    CardImagesStored {
        catalog_id: u32,
        local_image: Option<String>,
        local_art_crop: Option<String>,
        reply: Sender<Result<u32, String>>,
        deadline: Instant,
    },
    /// Forget the local files not in `files` (the names now in the images
    /// folder, after a move) so they are downloaded again; then report
    /// `CardsChanged` when any was forgotten. Skipped, with an error answer,
    /// if `deadline` has passed.
    CardImagesPresent {
        files: BTreeSet<String>,
        reply: Sender<Result<u32, String>>,
        deadline: Instant,
    },
    /// "Regenerate" a deck's card stats (cards spec section 8): sync its
    /// complete log matches again and apply the imported rule again to its
    /// imported and history matches, then answer how many matches that is.
    /// Skipped, with an error answer, if `deadline` has passed, like
    /// `DeckEdit`.
    RegenerateCardStats {
        deck_id: i64,
        reply: Sender<Result<u32, String>>,
        deadline: Instant,
    },
}

pub struct TrackerHandle {
    stop: Arc<AtomicBool>,
    commands: SyncSender<Command>,
    thread: Option<JoinHandle<()>>,
}

/// The answer when a command cannot be queued: the inbox is full or the
/// tracker has stopped.
const BUSY: &str = "the tracker is busy or stopped; try again";
/// `BUSY` for the API worker, whose messages go to the log.
const BUSY_FOR_LOG: &str = "the tracker is busy or stopped";
/// A settings save's answer when it times out or the tracker stops first.
const SAVE_LATE: &str = "the tracker did not answer in time; try again";

/// Send one command to the tracker and wait for its answer: the round trip
/// every caller below makes. `command` builds the command from the reply
/// channel and the deadline (`timeout` from now; a command without a
/// deadline ignores it). Fails with `busy` when the command cannot be
/// queued, with `late` after `timeout`, and with `stopped` when the tracker
/// drops the command unanswered (it stopped meanwhile).
fn ask<T>(
    commands: &SyncSender<Command>,
    timeout: Duration,
    busy: &str,
    late: &str,
    stopped: &str,
    command: impl FnOnce(Sender<Result<T, String>>, Instant) -> Command,
) -> Result<T, String> {
    let (reply, answer) = mpsc::channel();
    commands
        .try_send(command(reply, Instant::now() + timeout))
        .map_err(|_| busy.to_string())?;
    match answer.recv_timeout(timeout) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => Err(late.to_string()),
        Err(RecvTimeoutError::Disconnected) => Err(stopped.to_string()),
    }
}

impl TrackerHandle {
    /// A handle that stores settings rows through this tracker's writer,
    /// like `deck_editor`: the caller waits on it without holding the
    /// `TrackerHandle`.
    pub fn settings_saver(&self) -> SettingsSaver {
        SettingsSaver {
            commands: self.commands.clone(),
        }
    }

    /// A handle that asks this tracker for card stub scans (the cards
    /// worker's), like `deck_editor`.
    pub fn card_stub_scanner(&self) -> CardStubScanner {
        CardStubScanner {
            commands: self.commands.clone(),
        }
    }

    /// A handle that sends population runs to this tracker (the cards
    /// worker's), like `deck_editor`.
    pub fn cards_resolver(&self) -> CardsResolver {
        CardsResolver {
            commands: self.commands.clone(),
            settings: self.settings_saver(),
        }
    }

    /// A handle that records the cards worker's image files, like
    /// `deck_editor`.
    pub fn card_images(&self) -> CardImagesRecorder {
        CardImagesRecorder {
            commands: self.commands.clone(),
        }
    }

    /// A handle that stores the recorder import worker's files, like
    /// `deck_editor`.
    pub fn recording_importer(&self) -> RecordingImporter {
        RecordingImporter {
            commands: self.commands.clone(),
        }
    }

    /// A handle that stores the Endstep capture import worker's files, like
    /// `recording_importer`.
    pub fn endstep_capture_importer(&self) -> EndstepCaptureImporter {
        EndstepCaptureImporter {
            commands: self.commands.clone(),
        }
    }

    /// A handle that sends pick note edits to this tracker, like
    /// `deck_editor`.
    pub fn draft_note_editor(&self) -> DraftNoteEditor {
        DraftNoteEditor {
            commands: self.commands.clone(),
        }
    }

    /// A handle that sends archetype edits to this tracker (the pages' and
    /// the archetypes worker's), like `deck_editor`.
    pub fn archetype_editor(&self) -> ArchetypeEditor {
        ArchetypeEditor {
            commands: self.commands.clone(),
        }
    }

    /// A handle that sends the API worker's outbox edits to this tracker,
    /// like `deck_editor`.
    pub fn outbox_editor(&self) -> OutboxEditor {
        OutboxEditor {
            commands: self.commands.clone(),
        }
    }

    /// A handle that sends the API worker's sync edits to this tracker,
    /// like `deck_editor`.
    pub fn sync_editor(&self) -> SyncEditor {
        SyncEditor {
            commands: self.commands.clone(),
        }
    }

    /// A handle that sends manual match edits to this tracker, like
    /// `deck_editor`.
    pub fn manual_editor(&self) -> ManualEditor {
        ManualEditor {
            commands: self.commands.clone(),
        }
    }

    /// A handle that sends match edits to this tracker, like `deck_editor`.
    pub fn match_editor(&self) -> MatchEditor {
        MatchEditor {
            commands: self.commands.clone(),
        }
    }

    /// A handle that sends account edits to this tracker, like `match_editor`.
    pub fn account_editor(&self) -> AccountEditor {
        AccountEditor {
            commands: self.commands.clone(),
        }
    }

    /// A handle that sends league edits to this tracker, like `deck_editor`.
    pub fn league_editor(&self) -> LeagueEditor {
        LeagueEditor {
            commands: self.commands.clone(),
        }
    }

    /// A handle that sends deck edits and syncs to this tracker. Like
    /// `importer`, the caller can wait on it without holding the
    /// `TrackerHandle`.
    pub fn deck_editor(&self) -> DeckEditor {
        DeckEditor {
            commands: self.commands.clone(),
        }
    }

    /// A handle that sends imports to this tracker. The caller can keep it
    /// without holding the `TrackerHandle`, so a slow import never blocks
    /// whoever needs the handle to stop or restart tracking.
    pub fn importer(&self) -> Importer {
        Importer {
            commands: self.commands.clone(),
        }
    }

    /// Stop tracking: the current tick finishes, pending writes commit, and the
    /// thread is joined.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Stores settings rows through the tracker (see
/// `TrackerHandle::settings_saver`).
#[derive(Clone)]
pub struct SettingsSaver {
    commands: SyncSender<Command>,
}

impl SettingsSaver {
    /// Store settings rows through the tracker's writer. Returns once they are
    /// committed, or an error the UI can show.
    pub fn save(&self, rows: Vec<(String, String)>) -> Result<(), String> {
        self.save_within(rows, SAVE_TIMEOUT)
    }

    pub(crate) fn save_within(
        &self,
        rows: Vec<(String, String)>,
        timeout: Duration,
    ) -> Result<(), String> {
        ask(
            &self.commands,
            timeout,
            BUSY,
            SAVE_LATE,
            SAVE_LATE,
            |reply, deadline| Command::SaveSettings {
                rows,
                reply,
                deadline,
            },
        )
    }

    /// Store `rows` and switch Arena on or off (the integrations switch).
    /// Returns once the rows are committed and the switch is made, or an
    /// error the UI can show.
    pub fn set_arena_enabled(
        &self,
        enabled: bool,
        rows: Vec<(String, String)>,
    ) -> Result<(), String> {
        ask(
            &self.commands,
            SAVE_TIMEOUT,
            BUSY,
            SAVE_LATE,
            SAVE_LATE,
            |reply, deadline| Command::SetArenaEnabled {
                enabled,
                rows,
                reply,
                deadline,
            },
        )
    }

    /// Store `rows` and switch the Endstep sync on or off (the integrations
    /// switch). Returns once the rows are committed and the switch is made,
    /// or an error the UI can show.
    pub fn set_endstep_enabled(
        &self,
        enabled: bool,
        rows: Vec<(String, String)>,
    ) -> Result<(), String> {
        ask(
            &self.commands,
            SAVE_TIMEOUT,
            BUSY,
            SAVE_LATE,
            SAVE_LATE,
            |reply, deadline| Command::SetEndstepEnabled {
                enabled,
                rows,
                reply,
                deadline,
            },
        )
    }

    /// Store `rows` and switch the watcher on or off (release spec 4.1).
    /// Returns once the rows are committed and the switch is made, or an
    /// error the UI can show.
    pub fn set_watching(&self, watching: bool, rows: Vec<(String, String)>) -> Result<(), String> {
        ask(
            &self.commands,
            SAVE_TIMEOUT,
            BUSY,
            SAVE_LATE,
            SAVE_LATE,
            |reply, deadline| Command::SetWatching {
                watching,
                rows,
                reply,
                deadline,
            },
        )
    }
}

/// Sends imported 0.x matches to the tracker (see `TrackerHandle::importer`).
#[derive(Clone)]
pub struct Importer {
    commands: SyncSender<Command>,
}

impl Importer {
    /// Store imported matches through the tracker's writer and return the
    /// counts, or a message the UI can show. Waits up to `IMPORT_TIMEOUT`,
    /// long enough for a restart in between. A tracker that stops meanwhile
    /// drops the command, which answers at once.
    pub fn import(&self, matches: Vec<ImportedMatch>) -> Result<ImportCounts, String> {
        self.import_within(matches, IMPORT_TIMEOUT)
    }

    pub(crate) fn import_within(
        &self,
        matches: Vec<ImportedMatch>,
        timeout: Duration,
    ) -> Result<ImportCounts, String> {
        ask(
            &self.commands,
            timeout,
            BUSY,
            "tracking did not answer the import in time; try again",
            "tracking stopped before the import was stored; try again",
            |reply, _deadline| Command::Import { matches, reply },
        )
    }

    /// Register the 0.x accounts through the tracker's writer and return the
    /// counts, or a message the UI can show. Waits up to `IMPORT_TIMEOUT`,
    /// like `import`.
    pub fn import_accounts(
        &self,
        accounts: Vec<ImportedAccount>,
    ) -> Result<AccountImportCounts, String> {
        ask(
            &self.commands,
            IMPORT_TIMEOUT,
            BUSY,
            "tracking did not answer the account import in time; try again",
            "tracking stopped before the accounts were stored; try again",
            |reply, _deadline| Command::ImportAccounts { accounts, reply },
        )
    }

    /// Store imported 0.x deck data through the tracker's writer and return
    /// the counts, or a message the UI can show. Waits up to
    /// `IMPORT_TIMEOUT`, like `import`.
    pub fn import_decks(&self, decks: ImportedDecks) -> Result<DeckImportCounts, String> {
        ask(
            &self.commands,
            IMPORT_TIMEOUT,
            BUSY,
            "tracking did not answer the deck import in time; try again",
            "tracking stopped before the decks were stored; try again",
            |reply, _deadline| Command::ImportDecks { decks, reply },
        )
    }

    /// Store imported 0.x leagues through the tracker's writer and return the
    /// counts, or a message the UI can show. Waits up to `IMPORT_TIMEOUT`,
    /// like `import`.
    pub fn import_leagues(
        &self,
        leagues: Vec<ImportedLeague>,
    ) -> Result<LeagueImportCounts, String> {
        ask(
            &self.commands,
            IMPORT_TIMEOUT,
            BUSY,
            "tracking did not answer the league import in time; try again",
            "tracking stopped before the leagues were stored; try again",
            |reply, _deadline| Command::ImportLeagues { leagues, reply },
        )
    }

    /// Store imported 0.x drafts, picks, notes and limited registered decks
    /// through the tracker's writer and return the counts, or a message the
    /// UI can show. Waits up to `IMPORT_TIMEOUT`, like `import`.
    pub fn import_limited(&self, data: ImportedLimited) -> Result<LimitedImportCounts, String> {
        ask(
            &self.commands,
            IMPORT_TIMEOUT,
            BUSY,
            "tracking did not answer the Limited import in time; try again",
            "tracking stopped before the Limited data was stored; try again",
            |reply, _deadline| Command::ImportLimited {
                data: Box::new(data),
                reply,
            },
        )
    }

    /// Store 0.x's API state through the tracker's writer and return the
    /// counts, or a message the UI can show. Waits up to `IMPORT_TIMEOUT`;
    /// a command still queued past that is skipped, not applied late.
    pub fn import_api(&self, state: ImportedApiState) -> Result<ApiImportCounts, String> {
        self.import_api_within(state, IMPORT_TIMEOUT)
    }

    pub(crate) fn import_api_within(
        &self,
        state: ImportedApiState,
        timeout: Duration,
    ) -> Result<ApiImportCounts, String> {
        ask(
            &self.commands,
            timeout,
            BUSY,
            "tracking did not answer the API state import in time; try again",
            "tracking stopped before the API state was stored; try again",
            |reply, deadline| Command::ImportApi {
                state: Box::new(state),
                reply,
                deadline,
            },
        )
    }

    /// Store 0.x's archetype data through the tracker's writer and return the
    /// counts, or a message the UI can show. Waits up to `IMPORT_TIMEOUT`;
    /// a command still queued past that is skipped, not applied late.
    pub fn import_archetypes(
        &self,
        data: ImportedArchetypes,
    ) -> Result<ArchetypeImportCounts, String> {
        self.import_archetypes_within(data, IMPORT_TIMEOUT)
    }

    pub(crate) fn import_archetypes_within(
        &self,
        data: ImportedArchetypes,
        timeout: Duration,
    ) -> Result<ArchetypeImportCounts, String> {
        ask(
            &self.commands,
            timeout,
            BUSY,
            "tracking did not answer the archetype import in time; try again",
            "tracking stopped before the archetypes were stored; try again",
            |reply, deadline| Command::ImportArchetypes {
                data: Box::new(data),
                reply,
                deadline,
            },
        )
    }

    /// Store 0.x's overlay deck labels through the tracker's writer and
    /// return the counts, or a message the UI can show. Waits up to
    /// `IMPORT_TIMEOUT`, like `import`.
    pub fn import_overlay_labels(
        &self,
        labels: Vec<LegacyDeckLabel>,
    ) -> Result<OverlayImportCounts, String> {
        ask(
            &self.commands,
            IMPORT_TIMEOUT,
            BUSY,
            "tracking did not answer the deck label import in time; try again",
            "tracking stopped before the deck labels were stored; try again",
            |reply, _deadline| Command::ImportOverlayLabels { labels, reply },
        )
    }

    /// Store one imported 0.x match's detail through the tracker's writer
    /// and return what it did, or a message the UI can show. Waits up to
    /// `IMPORT_TIMEOUT`, like `import`.
    pub fn import_detail(
        &self,
        token: String,
        detail: ImportedDetail,
    ) -> Result<DetailImport, String> {
        ask(
            &self.commands,
            IMPORT_TIMEOUT,
            BUSY,
            "tracking did not answer the detail import in time; try again",
            "tracking stopped before the match detail was stored; try again",
            |reply, _deadline| Command::ImportDetail {
                token,
                detail: Box::new(detail),
                reply,
            },
        )
    }

    /// Store a batch of MTGO history matches through the tracker's writer
    /// and return the counts, or a message the UI can show. Waits up to
    /// `IMPORT_TIMEOUT`, like `import`.
    pub fn import_history(
        &self,
        matches: Vec<HistoryMatch>,
    ) -> Result<HistoryImportCounts, String> {
        ask(
            &self.commands,
            IMPORT_TIMEOUT,
            BUSY,
            "tracking did not answer the history import in time; try again",
            "tracking stopped before the history matches were stored; try again",
            |reply, _deadline| Command::ImportHistory { matches, reply },
        )
    }

    /// Remove every history import through the tracker's writer and return
    /// the removed tokens, or a message the UI can show. Waits up to
    /// `IMPORT_TIMEOUT`, like `import`.
    pub fn delete_history_imports(&self) -> Result<Vec<String>, String> {
        ask(
            &self.commands,
            IMPORT_TIMEOUT,
            BUSY,
            "tracking did not answer the delete in time; try again",
            "tracking stopped before the imported matches were deleted; try again",
            |reply, _deadline| Command::DeleteHistoryImports { reply },
        )
    }
}

/// Sends deck edits and deck syncs to the tracker (see
/// `TrackerHandle::deck_editor`).
#[derive(Clone)]
pub struct DeckEditor {
    commands: SyncSender<Command>,
}

impl DeckEditor {
    /// Read the deck files now and return how many decks a changed file
    /// updated, once committed, or a message the UI can show.
    pub fn sync(&self) -> Result<u32, String> {
        ask(
            &self.commands,
            DECK_SYNC_TIMEOUT,
            BUSY,
            "tracking did not answer the deck sync in time; try again",
            "tracking stopped before the decks were synced; try again",
            |reply, _deadline| Command::SyncDecks { reply },
        )
    }

    /// Apply a user's deck edit through the tracker's writer and return
    /// whether it was done or refused, or a message the UI can show. Waits up
    /// to `DECK_EDIT_TIMEOUT`.
    pub fn edit(&self, edit: DeckEdit) -> Result<EditOutcome, String> {
        self.edit_within(edit, DECK_EDIT_TIMEOUT)
    }

    pub(crate) fn edit_within(
        &self,
        edit: DeckEdit,
        timeout: Duration,
    ) -> Result<EditOutcome, String> {
        ask(
            &self.commands,
            timeout,
            BUSY,
            "tracking did not answer the deck edit in time; try again",
            "tracking stopped before the deck was edited; try again",
            |reply, deadline| Command::DeckEdit {
                edit,
                reply,
                deadline,
            },
        )
    }

    /// "Regenerate" a deck's card stats through the tracker and return how
    /// many matches it regenerates, or a message the UI can show. Waits up
    /// to `CARD_STATS_REGENERATE_TIMEOUT`.
    pub fn regenerate_card_stats(&self, deck_id: i64) -> Result<u32, String> {
        ask(
            &self.commands,
            CARD_STATS_REGENERATE_TIMEOUT,
            BUSY,
            "tracking did not regenerate the card stats in time; try again",
            "tracking stopped before the card stats were regenerated; try again",
            |reply, deadline| Command::RegenerateCardStats {
                deck_id,
                reply,
                deadline,
            },
        )
    }
}

/// Sends league edits to the tracker (see `TrackerHandle::league_editor`).
#[derive(Clone)]
pub struct LeagueEditor {
    commands: SyncSender<Command>,
}

impl LeagueEditor {
    /// Apply a user's league edit through the tracker's writer and return
    /// whether it was done or refused, or a message the UI can show. Waits up
    /// to `LEAGUE_EDIT_TIMEOUT`.
    pub fn edit(&self, edit: LeagueEdit) -> Result<LeagueEditOutcome, String> {
        self.edit_within(edit, LEAGUE_EDIT_TIMEOUT)
    }

    pub(crate) fn edit_within(
        &self,
        edit: LeagueEdit,
        timeout: Duration,
    ) -> Result<LeagueEditOutcome, String> {
        ask(
            &self.commands,
            timeout,
            BUSY,
            "tracking did not answer the league edit in time; try again",
            "tracking stopped before the league was edited; try again",
            |reply, deadline| Command::LeagueEdit {
                edit,
                reply,
                deadline,
            },
        )
    }
}

/// Sends match edits to the tracker (see `TrackerHandle::match_editor`).
#[derive(Clone)]
pub struct MatchEditor {
    commands: SyncSender<Command>,
}

impl MatchEditor {
    /// Apply a user's match edit through the tracker's writer and return
    /// whether it was done or refused, or a message the UI can show. Waits up
    /// to `MATCH_EDIT_TIMEOUT`.
    pub fn edit(&self, edit: MatchEdit) -> Result<MatchEditOutcome, String> {
        self.edit_within(edit, MATCH_EDIT_TIMEOUT)
    }

    pub(crate) fn edit_within(
        &self,
        edit: MatchEdit,
        timeout: Duration,
    ) -> Result<MatchEditOutcome, String> {
        ask(
            &self.commands,
            timeout,
            BUSY,
            "tracking did not answer the match edit in time; try again",
            "tracking stopped before the match was edited; try again",
            |reply, deadline| Command::MatchEdit {
                edit,
                reply,
                deadline,
            },
        )
    }
}

/// Sends account edits to the tracker (see `TrackerHandle::account_editor`).
#[derive(Clone)]
pub struct AccountEditor {
    commands: SyncSender<Command>,
}

impl AccountEditor {
    /// Apply a user's account edit through the tracker's writer and return
    /// whether it was done or refused, or a message the UI can show. Waits up
    /// to `ACCOUNT_EDIT_TIMEOUT`.
    pub fn edit(&self, edit: AccountEdit) -> Result<AccountEditOutcome, String> {
        self.edit_within(edit, ACCOUNT_EDIT_TIMEOUT)
    }

    pub(crate) fn edit_within(
        &self,
        edit: AccountEdit,
        timeout: Duration,
    ) -> Result<AccountEditOutcome, String> {
        ask(
            &self.commands,
            timeout,
            BUSY,
            "tracking did not answer the account edit in time; try again",
            "tracking stopped before the account was edited; try again",
            |reply, deadline| Command::AccountEdit {
                edit,
                reply,
                deadline,
            },
        )
    }
}

/// Sends manual match edits to the tracker (see
/// `TrackerHandle::manual_editor`).
#[derive(Clone)]
pub struct ManualEditor {
    commands: SyncSender<Command>,
}

impl ManualEditor {
    /// Apply a user's manual match edit through the tracker's writer and
    /// return whether it was done or refused, or a message the UI can show.
    /// Waits up to `MANUAL_EDIT_TIMEOUT`.
    pub fn edit(&self, edit: ManualEdit) -> Result<ManualEditOutcome, String> {
        self.edit_within(edit, MANUAL_EDIT_TIMEOUT)
    }

    pub(crate) fn edit_within(
        &self,
        edit: ManualEdit,
        timeout: Duration,
    ) -> Result<ManualEditOutcome, String> {
        ask(
            &self.commands,
            timeout,
            BUSY,
            "tracking did not answer the manual match edit in time; try again",
            "tracking stopped before the manual match was saved; try again",
            |reply, deadline| Command::ManualEdit {
                edit,
                reply,
                deadline,
            },
        )
    }
}

/// Sends pick note edits to the tracker (see
/// `TrackerHandle::draft_note_editor`).
#[derive(Clone)]
pub struct DraftNoteEditor {
    commands: SyncSender<Command>,
}

impl DraftNoteEditor {
    /// Apply a user's pick note edit through the tracker's writer and return
    /// whether it was done or refused, or a message the UI can show. Waits
    /// up to `DRAFT_NOTE_EDIT_TIMEOUT`.
    pub fn edit(&self, edit: DraftNoteEdit) -> Result<DraftNoteOutcome, String> {
        self.edit_within(edit, DRAFT_NOTE_EDIT_TIMEOUT)
    }

    pub(crate) fn edit_within(
        &self,
        edit: DraftNoteEdit,
        timeout: Duration,
    ) -> Result<DraftNoteOutcome, String> {
        ask(
            &self.commands,
            timeout,
            BUSY,
            "tracking did not answer the note edit in time; try again",
            "tracking stopped before the note was saved; try again",
            |reply, deadline| Command::DraftNoteEdit {
                edit,
                reply,
                deadline,
            },
        )
    }
}

/// Sends archetype edits to the tracker (see
/// `TrackerHandle::archetype_editor`).
#[derive(Clone)]
pub struct ArchetypeEditor {
    commands: SyncSender<Command>,
}

impl ArchetypeEditor {
    /// Apply an archetype edit through the tracker's writer and return
    /// whether it was done or refused, or a message the UI can show. Waits
    /// up to `ARCHETYPE_EDIT_TIMEOUT`.
    pub fn edit(&self, edit: ArchetypeEdit) -> Result<ArchetypeEditOutcome, String> {
        self.edit_within(edit, ARCHETYPE_EDIT_TIMEOUT)
    }

    pub(crate) fn edit_within(
        &self,
        edit: ArchetypeEdit,
        timeout: Duration,
    ) -> Result<ArchetypeEditOutcome, String> {
        ask(
            &self.commands,
            timeout,
            BUSY,
            "tracking did not answer the archetype edit in time; try again",
            "tracking stopped before the archetype was saved; try again",
            |reply, deadline| Command::ArchetypeEdit {
                edit: Box::new(edit),
                reply,
                deadline,
            },
        )
    }
}

/// Asks the tracker for card stub scans (see
/// `TrackerHandle::card_stub_scanner`).
#[derive(Clone)]
pub struct CardStubScanner {
    commands: SyncSender<Command>,
}

impl CardStubScanner {
    /// Run the full card stub scan through the tracker's writer and return
    /// how many stubs were added, or a message to show. Waits up to
    /// `CARD_STUB_SCAN_TIMEOUT`.
    pub fn scan(&self) -> Result<u32, String> {
        self.scan_within(CARD_STUB_SCAN_TIMEOUT)
    }

    pub(crate) fn scan_within(&self, timeout: Duration) -> Result<u32, String> {
        ask(
            &self.commands,
            timeout,
            BUSY,
            "tracking did not answer the card scan in time; try again",
            "tracking stopped before the card scan finished; try again",
            |reply, deadline| Command::ScanCardStubs { reply, deadline },
        )
    }

    /// Write a stub for each given catalog id without a `cards` row and
    /// return how many were added, or a message to show. Waits up to
    /// `CARD_STUB_SCAN_TIMEOUT`. The next tick reports `CardStubsAdded`.
    pub fn ensure(&self, catalog_ids: Vec<u32>) -> Result<u32, String> {
        ask(
            &self.commands,
            CARD_STUB_SCAN_TIMEOUT,
            BUSY,
            "tracking did not add the card stubs in time; try again",
            "tracking stopped before the card stubs were added; try again",
            |reply, deadline| Command::EnsureCardStubs {
                catalog_ids,
                reply,
                deadline,
            },
        )
    }

    /// Queue named token rows for the API's token-by-name fetch (an Arena
    /// replay's tokens with no art) without waiting for the answer. False
    /// when the tracker is busy or stopped (the next open asks again). The
    /// next tick reports `CardStubsAdded`.
    pub fn request_tokens(&self, tokens: Vec<TokenCard>) -> bool {
        self.try_queue(|reply, deadline| Command::EnsureTokenStubs {
            tokens,
            reply,
            deadline,
        })
    }

    /// `ensure` without waiting for the answer, for a page read that must
    /// not wait on the tracker. False when the tracker is busy or stopped
    /// (the next read asks again).
    pub fn request(&self, catalog_ids: Vec<u32>) -> bool {
        self.try_queue(|reply, deadline| Command::EnsureCardStubs {
            catalog_ids,
            reply,
            deadline,
        })
    }

    /// Send a stub command without waiting for its answer. False when the
    /// tracker's queue is full or the tracker has stopped.
    fn try_queue(
        &self,
        build: impl FnOnce(mpsc::Sender<Result<u32, String>>, Instant) -> Command,
    ) -> bool {
        let (reply, _answer) = mpsc::channel();
        let command = build(reply, Instant::now() + CARD_STUB_SCAN_TIMEOUT);
        self.commands.try_send(command).is_ok()
    }
}

/// Records the cards worker's image files through the tracker (see
/// `TrackerHandle::card_images`).
#[derive(Clone)]
pub struct CardImagesRecorder {
    commands: SyncSender<Command>,
}

/// A card images command's answer when it times out.
const IMAGES_LATE: &str = "tracking did not store the card images in time; try again";
/// A card images command's answer when the tracker stops first.
const IMAGES_STOPPED: &str = "tracking stopped before the card images were stored; try again";

impl CardImagesRecorder {
    /// Record a card's local files (`None` leaves a column as it is) and
    /// return the rows written. Waits up to `CARD_IMAGES_TIMEOUT`.
    pub fn stored(
        &self,
        catalog_id: u32,
        local_image: Option<String>,
        local_art_crop: Option<String>,
    ) -> Result<u32, String> {
        self.stored_within(catalog_id, local_image, local_art_crop, CARD_IMAGES_TIMEOUT)
    }

    pub(crate) fn stored_within(
        &self,
        catalog_id: u32,
        local_image: Option<String>,
        local_art_crop: Option<String>,
        timeout: Duration,
    ) -> Result<u32, String> {
        ask(
            &self.commands,
            timeout,
            BUSY,
            IMAGES_LATE,
            IMAGES_STOPPED,
            |reply, deadline| Command::CardImagesStored {
                catalog_id,
                local_image,
                local_art_crop,
                reply,
                deadline,
            },
        )
    }

    /// Forget the local files not in `files` and return the columns
    /// nulled. Waits up to `CARD_IMAGES_TIMEOUT`.
    pub fn present(&self, files: BTreeSet<String>) -> Result<u32, String> {
        ask(
            &self.commands,
            CARD_IMAGES_TIMEOUT,
            BUSY,
            IMAGES_LATE,
            IMAGES_STOPPED,
            |reply, deadline| Command::CardImagesPresent {
                files,
                reply,
                deadline,
            },
        )
    }
}

/// Stores the recorder import worker's files through the tracker (see
/// `TrackerHandle::recording_importer`).
#[derive(Clone)]
pub struct RecordingImporter {
    commands: SyncSender<Command>,
}

impl RecordingImporter {
    /// Store the files and return what each did. Waits up to
    /// `RECORDINGS_STORED_TIMEOUT`.
    pub fn store(
        &self,
        recordings: Vec<store::recorded_replays::NewRecording>,
        now_ms: i64,
    ) -> Result<store::recorded_replays::ImportReport, String> {
        ask(
            &self.commands,
            RECORDINGS_STORED_TIMEOUT,
            BUSY,
            "tracking did not store the recordings in time; try again",
            "tracking stopped before the recordings were stored; try again",
            |reply, deadline| Command::RecordingsStored {
                recordings,
                now_ms,
                reply,
                deadline,
            },
        )
    }
}

/// Stores the Endstep capture import worker's files through the tracker (see
/// `TrackerHandle::endstep_capture_importer`).
#[derive(Clone)]
pub struct EndstepCaptureImporter {
    commands: SyncSender<Command>,
}

impl EndstepCaptureImporter {
    /// Store the files and return what each did. Waits up to
    /// `ENDSTEP_CAPTURES_STORED_TIMEOUT`.
    pub fn store(
        &self,
        captures: Vec<store::endstep_captures::NewCapture>,
        now_ms: i64,
    ) -> Result<store::endstep_captures::CaptureImportReport, String> {
        ask(
            &self.commands,
            ENDSTEP_CAPTURES_STORED_TIMEOUT,
            BUSY,
            "tracking did not store the captures in time; try again",
            "tracking stopped before the captures were stored; try again",
            |reply, deadline| Command::EndstepCapturesStored {
                captures,
                now_ms,
                reply,
                deadline,
            },
        )
    }
}

/// Sends the cards worker's population runs to the tracker (see
/// `TrackerHandle::cards_resolver`).
#[derive(Clone)]
pub struct CardsResolver {
    commands: SyncSender<Command>,
    /// The population run's markers go through the same saver as the app's.
    settings: SettingsSaver,
}

impl CardsResolver {
    /// Apply a population run through the tracker's writer and return what
    /// it did, or a message to show. Waits up to `CARDS_RESOLVED_TIMEOUT`.
    pub fn resolve(
        &self,
        tokens: Vec<TokenCard>,
        resolutions: Resolutions,
        unknown_at_ms: i64,
    ) -> Result<ResolveOutcome, String> {
        self.resolve_within(tokens, resolutions, unknown_at_ms, CARDS_RESOLVED_TIMEOUT)
    }

    /// Store settings rows through the tracker's writer, as
    /// `SettingsSaver::save` (the population run's markers).
    pub fn save_settings(&self, rows: Vec<(String, String)>) -> Result<(), String> {
        self.settings.save(rows)
    }

    pub(crate) fn resolve_within(
        &self,
        tokens: Vec<TokenCard>,
        resolutions: Resolutions,
        unknown_at_ms: i64,
        timeout: Duration,
    ) -> Result<ResolveOutcome, String> {
        ask(
            &self.commands,
            timeout,
            BUSY,
            "tracking did not store the card data in time; try again",
            "tracking stopped before the card data was stored; try again",
            |reply, deadline| Command::CardsResolved {
                tokens,
                resolutions,
                unknown_at_ms,
                reply,
                deadline,
            },
        )
    }
}

/// Sends the API worker's outbox edits to the tracker (see
/// `TrackerHandle::outbox_editor`).
#[derive(Clone)]
pub struct OutboxEditor {
    commands: SyncSender<Command>,
}

impl OutboxEditor {
    /// Apply an outbox edit through the tracker's writer and return what it
    /// did, or a message for the log. Waits up to `OUTBOX_EDIT_TIMEOUT`.
    pub fn edit(&self, edit: OutboxEdit) -> Result<OutboxOutcome, String> {
        self.edit_within(edit, OUTBOX_EDIT_TIMEOUT)
    }

    pub(crate) fn edit_within(
        &self,
        edit: OutboxEdit,
        timeout: Duration,
    ) -> Result<OutboxOutcome, String> {
        ask(
            &self.commands,
            timeout,
            BUSY_FOR_LOG,
            "tracking did not answer the outbox edit in time",
            "tracking stopped before the outbox edit was answered",
            |reply, deadline| Command::OutboxEdit {
                edit,
                reply,
                deadline,
            },
        )
    }
}

/// Sends the API worker's sync edits to the tracker (see
/// `TrackerHandle::sync_editor`).
#[derive(Clone)]
pub struct SyncEditor {
    commands: SyncSender<Command>,
}

impl SyncEditor {
    /// Apply a sync edit through the tracker's writer and return how many
    /// rows it changed, or a message for the log. Waits up to
    /// `SYNC_EDIT_TIMEOUT`.
    pub fn edit(&self, edit: SyncEdit) -> Result<u32, String> {
        self.edit_within(edit, SYNC_EDIT_TIMEOUT)
    }

    pub(crate) fn edit_within(&self, edit: SyncEdit, timeout: Duration) -> Result<u32, String> {
        ask(
            &self.commands,
            timeout,
            BUSY_FOR_LOG,
            "tracking did not answer the sync edit in time",
            "tracking stopped before the sync edit was answered",
            |reply, deadline| Command::SyncEdit {
                edit,
                reply,
                deadline,
            },
        )
    }
}

impl Drop for TrackerHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Start the tracker on its own thread. Events go to `sink`.
pub fn spawn(config: TrackerConfig, sink: Sender<TrackerEvent>) -> std::io::Result<TrackerHandle> {
    spawn_with_clock(config, Clock::Real, sink)
}

/// `spawn` on `clock`: `Clock::Log` for a replay (the log player, a
/// headless test), so the time rules run on the log's time (#858).
pub fn spawn_with_clock(
    config: TrackerConfig,
    clock: Clock,
    sink: Sender<TrackerEvent>,
) -> std::io::Result<TrackerHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let (commands, inbox) = mpsc::sync_channel(INBOX);
    let thread = std::thread::Builder::new()
        .name("tracker".to_string())
        .spawn(move || supervise(&config, clock, &sink, &inbox, &thread_stop))?;
    Ok(TrackerHandle {
        stop,
        commands,
        thread: Some(thread),
    })
}

/// Run the engine until stopped, restarting it after every error or panic.
fn supervise(
    config: &TrackerConfig,
    clock: Clock,
    sink: &Sender<TrackerEvent>,
    inbox: &Receiver<Command>,
    stop: &AtomicBool,
) {
    let in_flight: InFlight = Arc::new(Mutex::new(None));
    let mut crashes = Crashes::new();
    let mut backoff_ms = FIRST_BACKOFF_MS;
    // The watcher switch and the clock outlive each engine: a restart keeps
    // them (and so the log time).
    let mut switches = Switches {
        watching: config.watching,
        arena_enabled: config.arena_enabled,
        endstep_enabled: config.endstep_enabled,
        clock: TrackerClock::new(clock),
    };
    while !stop.load(Ordering::SeqCst) {
        let quarantine = quarantined(&crashes);
        let started = Instant::now();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            run(
                config,
                sink,
                inbox,
                stop,
                &in_flight,
                quarantine,
                &mut switches,
            )
        }));
        let reason = match outcome {
            Ok(Ok(())) => return,
            Ok(Err(error)) => error.to_string(),
            Err(payload) => format!("tracker panicked: {}", panic_message(payload.as_ref())),
        };
        // A failure while stopping (the final flush) is not followed by a
        // restart, so it must not be announced as one.
        if stop.load(Ordering::SeqCst) {
            tracing::warn!(reason = %reason, "the tracker failed while stopping");
            return;
        }
        // Only a crash inside an event's processing leaves a key here; any other
        // failure blames no event.
        if let Some(event) = take_in_flight(&in_flight)
            && note_crash(&mut crashes, event)
        {
            tracing::error!(
                file = event.key.0,
                offset = event.key.1,
                "event quarantined"
            );
        }
        tracing::warn!(reason = %reason, "tracker restarting");
        if sink.send(TrackerEvent::Restarting { reason }).is_err() {
            return;
        }
        if started.elapsed() >= HEALTHY_RUN {
            backoff_ms = FIRST_BACKOFF_MS;
        }
        sleep_unless_stopped(stop, backoff_ms);
        backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
        #[cfg(test)]
        crate::engine::test_hooks::wait_while_reopen_held(&config.db_path, stop);
    }
}

/// Crashes per event version (`InFlightEvent`).
type Crashes = HashMap<InFlightEvent, u32>;

/// Count a crash on this event version; true when it reaches
/// `MAX_CRASHES` and quarantines the event.
fn note_crash(crashes: &mut Crashes, event: InFlightEvent) -> bool {
    let count = crashes.entry(event).or_insert(0);
    *count += 1;
    *count == MAX_CRASHES
}

/// The events quarantined: a version of each crashed `MAX_CRASHES` times.
/// Crashes on other versions of a growing event do not add up (#50).
fn quarantined(crashes: &Crashes) -> Vec<(i64, u64)> {
    let mut keys: Vec<(i64, u64)> = crashes
        .iter()
        .filter(|(_, count)| **count >= MAX_CRASHES)
        .map(|(event, _)| event.key)
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// Take the event that was being processed, even if a panic poisoned the lock.
fn take_in_flight(in_flight: &InFlight) -> Option<InFlightEvent> {
    match in_flight.lock() {
        Ok(mut slot) => slot.take(),
        Err(poisoned) => poisoned.into_inner().take(),
    }
}

/// The switches that outlive each engine, and the clock (a restart keeps
/// the log time): a restart keeps them.
struct Switches {
    watching: bool,
    arena_enabled: bool,
    endstep_enabled: bool,
    clock: TrackerClock,
}

impl Switches {
    fn remember(&mut self, engine: &Engine) {
        self.watching = engine.watching();
        self.arena_enabled = engine.arena_enabled();
        self.endstep_enabled = engine.endstep_enabled();
    }
}

/// One engine lifetime: open, tick until stopped or failed. Commands are handled
/// between ticks; a command that arrives while the engine sleeps wakes it.
fn run(
    config: &TrackerConfig,
    sink: &Sender<TrackerEvent>,
    inbox: &Receiver<Command>,
    stop: &AtomicBool,
    in_flight: &InFlight,
    quarantine: Vec<(i64, u64)>,
    switches: &mut Switches,
) -> Result<(), TrackerError> {
    // No engine has read anything yet: the clock keeps the newest time.
    let mut engine = Engine::open(
        config.clone(),
        Env::current(),
        Arc::clone(in_flight),
        quarantine,
        switches.clock.now_ms(0),
    )?;
    engine.set_watching(switches.watching);
    engine.set_arena_enabled(switches.arena_enabled);
    engine.set_endstep_enabled(switches.endstep_enabled);
    while !stop.load(Ordering::SeqCst) {
        while let Ok(command) = inbox.try_recv() {
            let result = handle_command(&mut engine, command, &mut switches.clock);
            switches.remember(&engine);
            result?;
        }
        let tick = engine.tick(now(&mut switches.clock, &engine))?;
        if tick.worked {
            log_tick(&tick);
        }
        for event in tick.events {
            if sink.send(event).is_err() {
                // Nobody is listening any more: stop cleanly.
                return engine.finish();
            }
        }
        if !tick.more
            && let Some(command) =
                wait_after_tick(&mut engine, tick.active, tick.syncing, stop, inbox)
        {
            let result = handle_command(&mut engine, command, &mut switches.clock);
            switches.remember(&engine);
            result?;
        }
    }
    #[cfg(test)]
    if crate::engine::test_hooks::take_final_flush_failure(&config.db_path) {
        engine.inject_broken_event();
    }
    engine.finish()
}

fn log_tick(tick: &crate::Tick) {
    let t = tick.timing;
    tracing::debug!(
        target: "mymtgo::timing",
        rescan_us = t.rescan_us,
        poll_us = t.poll_us,
        lookups_us = t.lookups_us,
        finalize_us = t.finalize_us,
        rebuild_us = t.rebuild_us,
        decks_us = t.decks_us,
        write_us = t.write_us,
        games_us = t.games_us,
        leagues_us = t.leagues_us,
        tournaments_us = t.tournaments_us,
        total_us = t.total_us,
        events = tick.events.len(),
        "tick"
    );
}

/// Do one command and answer it. A storage failure is answered, then restarts
/// the tracker like any other failure.
fn handle_command(
    engine: &mut Engine,
    command: Command,
    clock: &mut TrackerClock,
) -> Result<(), TrackerError> {
    match command {
        Command::SaveSettings {
            rows,
            reply,
            deadline,
        } => {
            if Instant::now() >= deadline {
                let _ = reply.send(Err("the save took too long and was skipped".to_string()));
                return Ok(());
            }
            let result = engine.save_settings(rows);
            let answer = match &result {
                Ok(()) => Ok(()),
                Err(error) => Err(format!("could not save settings: {error}")),
            };
            // The caller may have given up just now; the save still happened.
            let _ = reply.send(answer);
            return result;
        }
        Command::SetWatching {
            watching,
            rows,
            reply,
            deadline,
        } => {
            if Instant::now() >= deadline {
                let _ = reply.send(Err("the switch took too long and was skipped".to_string()));
                return Ok(());
            }
            let result = engine.save_settings(rows);
            let answer = match &result {
                Ok(()) => {
                    // Switched only once its row is stored, so a restart
                    // reads the same state the engine runs with.
                    engine.set_watching(watching);
                    Ok(())
                }
                Err(error) => Err(format!("could not switch the watcher: {error}")),
            };
            let _ = reply.send(answer);
            return result;
        }
        Command::SetArenaEnabled {
            enabled,
            rows,
            reply,
            deadline,
        } => {
            if Instant::now() >= deadline {
                let _ = reply.send(Err("the switch took too long and was skipped".to_string()));
                return Ok(());
            }
            let result = engine.save_settings(rows);
            let answer = match &result {
                Ok(()) => {
                    engine.set_arena_enabled(enabled);
                    Ok(())
                }
                Err(error) => Err(format!("could not switch Arena: {error}")),
            };
            let _ = reply.send(answer);
            return result;
        }
        Command::SetEndstepEnabled {
            enabled,
            rows,
            reply,
            deadline,
        } => {
            if Instant::now() >= deadline {
                let _ = reply.send(Err("the switch took too long and was skipped".to_string()));
                return Ok(());
            }
            let result = engine.save_settings(rows);
            let answer = match &result {
                Ok(()) => {
                    engine.set_endstep_enabled(enabled);
                    Ok(())
                }
                Err(error) => Err(format!("could not switch Endstep: {error}")),
            };
            let _ = reply.send(answer);
            return result;
        }
        Command::Import { matches, reply } => {
            let result = engine.import(matches, now(clock, engine));
            answer(reply, "could not import the 0.x matches", result);
        }
        Command::ImportAccounts { accounts, reply } => {
            let result = engine.import_accounts(accounts, now(clock, engine));
            answer(reply, "could not import the 0.x accounts", result);
        }
        Command::ImportDecks { decks, reply } => {
            let result = engine.import_decks(decks, now(clock, engine));
            answer(reply, "could not import the 0.x decks", result);
        }
        Command::ImportLeagues { leagues, reply } => {
            let result = engine.import_leagues(leagues, now(clock, engine));
            answer(reply, "could not import the 0.x leagues", result);
        }
        Command::ImportLimited { data, reply } => {
            let result = engine.import_limited(*data, now(clock, engine));
            answer(reply, "could not import the 0.x Limited data", result);
        }
        Command::ImportApi {
            state,
            reply,
            deadline,
        } => answer_by(
            reply,
            deadline,
            "the API state import took too long and was skipped",
            "could not import the 0.x API state",
            || engine.import_api(*state, now(clock, engine)),
        ),
        Command::ImportArchetypes {
            data,
            reply,
            deadline,
        } => answer_by(
            reply,
            deadline,
            "the archetype import took too long and was skipped",
            "could not import the 0.x archetypes",
            || engine.import_archetypes(*data, now(clock, engine)),
        ),
        Command::ImportOverlayLabels { labels, reply } => {
            let result = engine.import_overlay_labels(labels);
            answer(reply, "could not import the 0.x deck labels", result);
        }
        Command::ImportDetail {
            token,
            detail,
            reply,
        } => {
            let result = engine.import_detail(token, *detail);
            answer(reply, "could not import a 0.x match's detail", result);
        }
        Command::ImportHistory { matches, reply } => {
            let result = engine.import_history(matches, now(clock, engine));
            answer(reply, "could not import the history matches", result);
        }
        Command::DeleteHistoryImports { reply } => {
            let result = engine.delete_history_imports();
            answer(reply, "could not delete the imported matches", result);
        }
        Command::SyncDecks { reply } => {
            // Stopped, the tracker reads no deck XML (release spec 4.1):
            // 0.x `MtgoManager::syncDecks` does nothing unless `canRun()`,
            // and says nothing either.
            if !engine.watching() {
                let _ = reply.send(Ok(0));
                return Ok(());
            }
            let now = now(clock, engine);
            let result = engine
                .sync_deck_files(now)
                .and_then(|written| engine.flush().map(|()| written));
            let answer = match &result {
                Ok(written) => Ok(*written),
                Err(error) => Err(format!("could not sync the decks: {error}")),
            };
            let _ = reply.send(answer);
            // A storage failure restarts the tracker like any other.
            return result.map(|_| ());
        }
        Command::DeckEdit {
            edit,
            reply,
            deadline,
        } => answer_by(
            reply,
            deadline,
            EDIT_LATE,
            "could not edit the deck",
            || engine.edit_deck(edit),
        ),
        Command::LeagueEdit {
            edit,
            reply,
            deadline,
        } => answer_by(
            reply,
            deadline,
            EDIT_LATE,
            "could not edit the league",
            || engine.edit_league(edit),
        ),
        Command::MatchEdit {
            edit,
            reply,
            deadline,
        } => answer_by(
            reply,
            deadline,
            EDIT_LATE,
            "could not edit the match",
            || engine.edit_match(edit),
        ),
        Command::AccountEdit {
            edit,
            reply,
            deadline,
        } => answer_by(
            reply,
            deadline,
            EDIT_LATE,
            "could not edit the account",
            || engine.edit_account(edit),
        ),
        Command::ManualEdit {
            edit,
            reply,
            deadline,
        } => answer_by(
            reply,
            deadline,
            EDIT_LATE,
            "could not save the manual match",
            || engine.edit_manual(edit),
        ),
        Command::DraftNoteEdit {
            edit,
            reply,
            deadline,
        } => answer_by(
            reply,
            deadline,
            EDIT_LATE,
            "could not save the note",
            || engine.edit_draft_note(edit),
        ),
        Command::ArchetypeEdit {
            edit,
            reply,
            deadline,
        } => answer_by(
            reply,
            deadline,
            EDIT_LATE,
            "could not save the archetype",
            || engine.edit_archetypes(*edit),
        ),
        Command::OutboxEdit {
            edit,
            reply,
            deadline,
        } => answer_by(
            reply,
            deadline,
            "the outbox edit took too long and was skipped",
            "could not update the outbox",
            || engine.edit_outbox(edit),
        ),
        Command::SyncEdit {
            edit,
            reply,
            deadline,
        } => answer_by(
            reply,
            deadline,
            "the sync edit took too long and was skipped",
            "could not update the sync state",
            || engine.edit_sync(edit),
        ),
        Command::ScanCardStubs { reply, deadline } => answer_by(
            reply,
            deadline,
            "the card scan took too long and was skipped",
            "could not scan for missing cards",
            || engine.scan_card_stubs(),
        ),
        Command::EnsureCardStubs {
            catalog_ids,
            reply,
            deadline,
        } => answer_by(
            reply,
            deadline,
            "adding the card stubs took too long and was skipped",
            "could not add the card stubs",
            || {
                let result = engine.ensure_card_stubs(catalog_ids);
                if let Err(error) = &result {
                    tracing::warn!(%error, "card stubs for a decklist were not added");
                }
                result
            },
        ),
        Command::EnsureTokenStubs {
            tokens,
            reply,
            deadline,
        } => answer_by(
            reply,
            deadline,
            "adding the token stubs took too long and was skipped",
            "could not add the token stubs",
            || {
                let result = engine.ensure_token_stubs(tokens);
                if let Err(error) = &result {
                    tracing::warn!(%error, "token stubs for a replay were not added");
                }
                result
            },
        ),
        Command::CardsResolved {
            tokens,
            resolutions,
            unknown_at_ms,
            reply,
            deadline,
        } => answer_by(
            reply,
            deadline,
            "storing the card data took too long and was skipped",
            "could not store the card data",
            || {
                engine.resolve_cards(store::cards::CardsUpdate {
                    tokens,
                    resolutions,
                    unknown_at_ms,
                })
            },
        ),
        Command::CardImagesStored {
            catalog_id,
            local_image,
            local_art_crop,
            reply,
            deadline,
        } => answer_by(
            reply,
            deadline,
            "storing the card images took too long and was skipped",
            "could not store the card images",
            || {
                engine.store_card_images(store::writer::StoredImages {
                    catalog_id,
                    local_image,
                    local_art_crop,
                })
            },
        ),
        Command::RecordingsStored {
            recordings,
            now_ms,
            reply,
            deadline,
        } => answer_by(
            reply,
            deadline,
            "storing the recordings took too long and was skipped",
            "could not store the recordings",
            || engine.store_recordings(recordings, now_ms),
        ),
        Command::EndstepCapturesStored {
            captures,
            now_ms,
            reply,
            deadline,
        } => answer_by(
            reply,
            deadline,
            "storing the captures took too long and was skipped",
            "could not store the captures",
            || engine.store_endstep_captures(captures, now_ms),
        ),
        Command::CardImagesPresent {
            files,
            reply,
            deadline,
        } => answer_by(
            reply,
            deadline,
            "checking the card images took too long and was skipped",
            "could not update the card images",
            || engine.keep_present_card_images(files),
        ),
        Command::RegenerateCardStats {
            deck_id,
            reply,
            deadline,
        } => answer_by(
            reply,
            deadline,
            "regenerating the card stats took too long and was skipped",
            "could not regenerate the card stats",
            || engine.regenerate_card_stats(deck_id),
        ),
    }
    Ok(())
}

/// The answer to a user's edit still queued past its deadline.
const EDIT_LATE: &str = "the edit took too long and was skipped";

/// Send a command's result to its caller, an error as `failed: {error}`.
/// For a command that is its own transaction: its failure leaves the writer
/// running, so tracking goes on, and a writer that failed for another reason
/// stops the next tick, which restarts the tracker. A caller that gave up
/// just now no longer listens; the work still happened.
fn answer<T>(reply: Sender<Result<T, String>>, failed: &str, result: Result<T, TrackerError>) {
    let _ = reply.send(result.map_err(|error| format!("{failed}: {error}")));
}

/// `answer` for a command with a deadline: once `deadline` has passed the
/// caller has already told the user it failed, so `work` is skipped and the
/// answer is `late`; otherwise `work` runs and its result is answered.
fn answer_by<T>(
    reply: Sender<Result<T, String>>,
    deadline: Instant,
    late: &str,
    failed: &str,
    work: impl FnOnce() -> Result<T, TrackerError>,
) {
    if Instant::now() >= deadline {
        let _ = reply.send(Err(late.to_string()));
        return;
    }
    answer(reply, failed, work());
}

/// Sleep up to `ms`, returning early with a command if one arrives, or with
/// nothing if the tracker is stopping.
/// The wait after a tick that read everything: the poll interval, ended by
/// a command, or while game syncs remain (`syncing`) as soon as the
/// slice in flight is answered, so the next one goes at once.
fn wait_after_tick(
    engine: &mut Engine,
    active: bool,
    syncing: bool,
    stop: &AtomicBool,
    inbox: &Receiver<Command>,
) -> Option<Command> {
    let pause = if active || syncing {
        POLL_ACTIVE_MS
    } else {
        POLL_IDLE_MS
    };
    if !syncing {
        return wait_for_command(stop, inbox, pause);
    }
    let deadline = Instant::now() + Duration::from_millis(pause);
    while !stop.load(Ordering::SeqCst) {
        let now = Instant::now();
        if now >= deadline {
            return None;
        }
        let slice = (deadline - now).min(Duration::from_millis(SYNC_CHECK_MS));
        match inbox.recv_timeout(slice) {
            Ok(command) => return Some(command),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => std::thread::sleep(slice),
        }
        if engine.game_slice_answered() {
            return None;
        }
    }
    None
}

/// How often the wait checks for a game sync slice's answer.
const SYNC_CHECK_MS: u64 = 1;

fn wait_for_command(stop: &AtomicBool, inbox: &Receiver<Command>, ms: u64) -> Option<Command> {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while !stop.load(Ordering::SeqCst) {
        let now = Instant::now();
        if now >= deadline {
            return None;
        }
        let slice = (deadline - now).min(Duration::from_millis(25));
        match inbox.recv_timeout(slice) {
            Ok(command) => return Some(command),
            Err(RecvTimeoutError::Timeout) => {}
            // The handle is gone: the stop flag is already set or about to be.
            Err(RecvTimeoutError::Disconnected) => std::thread::sleep(slice),
        }
    }
    None
}

fn sleep_unless_stopped(stop: &AtomicBool, ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while !stop.load(Ordering::SeqCst) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(ms.min(25)));
    }
}

/// The engine's `now` on `clock`.
fn now(clock: &mut TrackerClock, engine: &Engine) -> i64 {
    clock.now_ms(engine.newest_log_ms())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::wall_now_ms as now_ms;

    /// `handle_command` on the real clock, as the app runs it.
    fn handle_real(engine: &mut Engine, command: Command) -> Result<(), TrackerError> {
        handle_command(engine, command, &mut TrackerClock::new(Clock::Real))
    }
    use crate::engine::test_hooks;
    use fixture_tools::{Fixture, TestDir, load_fixture, prepared_events};
    use mtgo_protocol::{EventKind, split_complete};
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
    use store::manual::{ManualGame, ManualMatch, ManualPlacement};
    use store::open_reader;
    use store::outbox::{NewOutboxRow, OutboxKind};
    use store::sync::SyncEdit;

    /// The stored settings, without the tracker's own bookkeeping (the zone
    /// the league sync folded in, and the pending sync markers).
    fn stored_settings(config: &TrackerConfig) -> Vec<(String, String)> {
        let reader = open_reader(&config.db_path).expect("reader");
        let mut rows = store::read::settings(&reader).expect("settings");
        rows.retain(|(key, _)| {
            key != store::league_sync::LEAGUE_FOLD_TIMEZONE
                && key != crate::engine::LEAGUE_SYNC_PENDING
                && key != crate::engine::GAME_SYNC_PENDING
        });
        rows
    }

    /// Log matches in the scheduling timing test.
    const TIMED_MATCHES: usize = 3_000;
    /// State frames per game in it (up to 40 cards each).
    const TIMED_FRAMES: usize = 20;

    /// `TIMED_MATCHES` compacted log matches of three games, each with
    /// `TIMED_FRAMES` state frames, and game card rows of an older build.
    fn timed_database(dir: &TestDir) -> TrackerConfig {
        use mtgo_match::{EndReason, GameResult, GameView, MatchState, MatchView, Outcome};
        use store::write::{MatchWrite, NewAccount, NewEvent, NewLogFile, WriteBatch, commit};

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
        dir.write("logs/.keep", b"");
        dir.write("mtgo-data/.keep", b"");
        let mut conn = store::open_writer(&config.db_path).expect("open");
        let start = 1_735_689_600_000_i64;
        commit(
            &mut conn,
            &WriteBatch {
                accounts: vec![NewAccount {
                    username: "Qtimed".to_string(),
                    login_id: Some(4_000_000),
                    seen_at_ms: start,
                    logged_at_ms: None,
                }],
                now_ms: start,
                ..WriteBatch::default()
            },
        )
        .expect("account");
        let file = store::write::insert_log_file(
            &conn,
            &NewLogFile {
                path: "Q/timed/mtgo.log".to_string(),
                identity_hash: 1,
                head_hash: 2,
                head_len: 10,
                created_at_fs: Some(start),
                day_anchor: chrono::NaiveDate::from_ymd_opt(2025, 1, 1).expect("date"),
                timezone: "UTC".to_string(),
                inherited_username: Some("Qtimed".to_string()),
                seen_at_ms: start,
            },
        )
        .expect("log file");
        conn.execute(
            "UPDATE log_files SET sealed_at = 1, seal_reason = 'truncated'",
            [],
        )
        .expect("sealed: nothing to tail");
        let mut offset = 0_u64;
        for chunk in 0..TIMED_MATCHES / 10 {
            let mut batch = WriteBatch::default();
            for slot in 0..10 {
                let n = chunk * 10 + slot;
                let started = start + n as i64 * 3_600_000;
                let match_id = 300_000_000 + n as u64;
                let mut games = Vec::new();
                for number in 0..3_u32 {
                    let game_id = 900_000_000 + (n as u64) * 3 + u64::from(number);
                    for frame in 0..TIMED_FRAMES {
                        let visible = 7 + frame * 33 / (TIMED_FRAMES - 1);
                        let cards: Vec<String> = (0..visible)
                            .map(|index| {
                                let (owner, zone) = if index % 4 == 3 {
                                    (2, "Battlefield")
                                } else if index < 7 {
                                    (1, "Hand")
                                } else {
                                    (1, "Battlefield")
                                };
                                format!(
                                    "{{\"Id\":{},\"CatalogID\":{},\"Zone\":\"{zone}\",\"ActualZone\":\"{zone}\",\"Owner\":{owner},\"Controller\":{owner}}}",
                                    100 + index,
                                    10_000 + index
                                )
                            })
                            .collect();
                        let raw = format!(
                            "12:00:00 [INF] (Twitch Info|Game Play Status Update for Game ID: {game_id}, Match ID: {match_id}, Event ID: {match_id}) {{\"Players\":[{{\"Id\":1,\"Name\":\"Qtimed\",\"LibraryCount\":53,\"HandCount\":7,\"Life\":20}},{{\"Id\":2,\"Name\":\"Qopponent\",\"LibraryCount\":53,\"HandCount\":7,\"Life\":20}}],\"Cards\":[{}]}}",
                            cards.join(",")
                        );
                        let at = started + i64::from(number) * 900_000 + frame as i64 * 5_000;
                        let prepared =
                            mtgo_protocol::prepare_at(file as u64, offset, at, &raw, None)
                                .expect("a frame classifies");
                        batch.events.push(NewEvent {
                            log_file_id: file,
                            byte_offset: offset,
                            kind: Some(prepared.kind),
                            subjects: prepared.subjects,
                            logged_at_ms: at,
                            raw: raw.clone(),
                            quarantined: false,
                        });
                        offset += raw.len() as u64 + 2;
                    }
                    let game_start = started + i64::from(number) * 900_000;
                    games.push(GameView {
                        mtgo_game_id: Some(game_id),
                        game_number: number + 1,
                        result: GameResult::Win,
                        end_reason: EndReason::Win,
                        on_play: Some(number % 2 == 0),
                        local_mulligans: Some(0),
                        opp_mulligans: Some(0),
                        local_die_roll: Some(3),
                        opp_die_roll: Some(5),
                        turn_count: Some(9),
                        started_at_ms: Some(game_start),
                        ended_at_ms: Some(game_start + 840_000),
                    });
                }
                let token = format!("00000001-0000-4000-8000-{n:012x}");
                batch.compact.push(token.clone());
                batch.now_ms = started + 4 * 3_600_000;
                batch.matches.push(MatchWrite::Upsert {
                    view: Box::new(MatchView {
                        token,
                        mtgo_match_id: Some(match_id),
                        local_username: "Qtimed".to_string(),
                        opponent: Some("Qopponent".to_string()),
                        format: Some("CMODERN".to_string()),
                        match_type: Some("Modern".to_string()),
                        tournament_event_id: None,
                        tournament_round: None,
                        league_token: None,
                        game_structure: Some("Modern".to_string()),
                        state: MatchState::Complete,
                        resolution: None,
                        outcome: Outcome::Win,
                        tentative_outcome: None,
                        wins: 2,
                        losses: 0,
                        started_at_ms: started,
                        ended_at_ms: Some(started + 3_000_000),
                        last_activity_ms: started + 3_000_000,
                        games,
                    }),
                    revision: 1,
                });
            }
            commit(&mut conn, &batch).expect("timed matches");
        }
        config
    }

    /// Tick as `run` does until no game sync remains; the wall-clock time.
    fn tick_until_synced(engine: &mut Engine, now: i64) -> Duration {
        let (_commands, inbox) = mpsc::channel::<Command>();
        let stop = AtomicBool::new(false);
        let started = Instant::now();
        loop {
            let tick = engine.tick(now).expect("tick");
            if !tick.more && !tick.syncing && !engine.game_backfill_running() {
                break;
            }
            if !tick.more {
                assert!(
                    wait_after_tick(engine, tick.active, tick.syncing, &stop, &inbox).is_none(),
                    "no command is sent"
                );
            }
        }
        engine.flush().expect("flush");
        started.elapsed()
    }

    /// Wall-clock time of the game backfill and of a large queue (a
    /// rebuild's), against the writer time of their slices: the next slice
    /// goes as soon as the last is answered, not one per idle tick.
    #[test]
    #[ignore = "timing check, run in release"]
    fn game_syncs_take_about_their_work_time() {
        let dir = TestDir::new("supervisor-game-sync-timing");
        let config = timed_database(&dir);
        let now = 1_735_689_600_000_i64 + 400 * 24 * 3_600_000;
        let slices = TIMED_MATCHES.div_ceil(crate::engine::GAME_SYNCS_PER_TICK);
        let one_per_idle_tick = slices as f64 * POLL_IDLE_MS as f64 / 1_000.0;
        let mut engine = Engine::open(
            config.clone(),
            Env::current(),
            Arc::default(),
            Vec::new(),
            now,
        )
        .expect("open");
        assert!(engine.game_backfill_running(), "a fresh database backfills");
        let backfill = tick_until_synced(&mut engine, now);
        let (slowest, work) = engine.game_sync_work();
        println!(
            "| Game backfill, {TIMED_MATCHES} matches, {slices} slices | wall {:.2} s | slice work {:.2} s | slowest slice {:.1} ms | one slice per idle tick: {one_per_idle_tick:.0} s |",
            backfill.as_secs_f64(),
            work as f64 / 1e6,
            slowest as f64 / 1e3
        );

        let tokens: Vec<String> = (0..TIMED_MATCHES)
            .map(|n| format!("00000001-0000-4000-8000-{n:012x}"))
            .collect();
        engine.queue_game_syncs_for_test(tokens);
        let queue = tick_until_synced(&mut engine, now + 1_000);
        let (slowest_after, work_after) = engine.game_sync_work();
        let queue_work = work_after - work;
        println!(
            "| Game sync queue, {TIMED_MATCHES} matches, {slices} slices | wall {:.2} s | slice work {:.2} s | slowest slice {:.1} ms | one transaction before |",
            queue.as_secs_f64(),
            queue_work as f64 / 1e6,
            slowest_after as f64 / 1e3
        );
        assert!(slowest_after < 100_000, "every slice under 100 ms");
        // About the work time: at most 20 ms of scheduling per slice.
        let allowance = Duration::from_millis(20 * slices as u64);
        assert!(
            backfill < Duration::from_micros(work) + allowance,
            "backfill {backfill:?} for {work} us of work"
        );
        assert!(
            queue < Duration::from_micros(queue_work) + allowance,
            "queue {queue:?} for {queue_work} us of work"
        );
    }

    #[test]
    fn a_saved_setting_is_committed_before_the_answer() {
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("supervisor-settings");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let (sink, _receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");

        let rows = vec![("close_to_tray".to_string(), "false".to_string())];
        handle.settings_saver().save(rows.clone()).expect("saved");
        assert_eq!(stored_settings(&config), rows, "stored when answered");
        handle.stop();
    }

    #[test]
    fn a_save_sent_while_the_tracker_restarts_is_committed_after_it_reopens() {
        let fixture = load_fixture("league_join_drop");
        let events = prepared_events(&fixture);
        let cut = events[events.len() / 2].offset as usize;
        let dir = TestDir::new("supervisor-settings-restart");
        dir.write("logs/mtgo.log", &fixture.bytes[..cut]);
        let config = config(&dir, &fixture);
        let (sink, receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        assert!(wait_for_cursor(&config, cut as i64), "first half committed");

        // Fail the next batch and hold the restarted engine before it opens.
        test_hooks::FAIL_NEXT_BATCH
            .lock()
            .expect("hooks")
            .push(config.db_path.clone());
        test_hooks::HOLD_REOPEN
            .lock()
            .expect("hooks")
            .push(config.db_path.clone());
        dir.append("logs/mtgo.log", &fixture.bytes[cut..]);
        wait_for(&receiver, Duration::from_secs(60), |events| {
            restarts(events) >= 1
        });

        let rows = vec![("log_folder".to_string(), "\"C:/logs\"".to_string())];
        let releaser = {
            let db = config.db_path.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                test_hooks::release_reopen(&db);
            })
        };
        handle
            .settings_saver()
            .save(rows.clone())
            .expect("answered after the reopen");
        releaser.join().expect("releaser");
        assert_eq!(stored_settings(&config), rows);
        handle.stop();
    }

    /// `SetWatching` stores its rows before it answers, and the watcher
    /// stays off when an error restarts the engine (release spec 4.1).
    #[test]
    fn set_watching_stores_its_rows_and_stays_paused_across_a_restart() {
        let fixture = load_fixture("league_join_drop");
        let events = prepared_events(&fixture);
        let cut = events[events.len() / 2].offset as usize;
        let dir = TestDir::new("supervisor-watching");
        dir.write("logs/mtgo.log", &fixture.bytes[..cut]);
        let config = config(&dir, &fixture);
        let (sink, receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        assert!(wait_for_cursor(&config, cut as i64), "first half committed");

        let off = vec![("watcher_active".to_string(), "false".to_string())];
        handle
            .settings_saver()
            .set_watching(false, off.clone())
            .expect("paused");
        assert_eq!(stored_settings(&config), off, "committed before the answer");

        test_hooks::FAIL_PAUSED_TICK
            .lock()
            .expect("hooks")
            .push(config.db_path.clone());
        dir.append("logs/mtgo.log", &fixture.bytes[cut..]);
        let mut seen = wait_for(&receiver, Duration::from_secs(60), |events| {
            restarts(events) >= 1
        });
        // The reopened engine ticks a few times, still paused.
        std::thread::sleep(Duration::from_millis(1_500));
        seen.extend(receiver.try_iter());
        assert_eq!(
            committed_cursor(&config),
            Some(cut as i64),
            "nothing read while paused, before or after the restart"
        );
        let last = seen.iter().rev().find_map(|event| match event {
            TrackerEvent::Status(status) => Some(status.clone()),
            _ => None,
        });
        assert!(last.expect("a status").paused);

        let on = vec![("watcher_active".to_string(), "true".to_string())];
        handle
            .settings_saver()
            .set_watching(true, on.clone())
            .expect("resumed");
        assert!(
            wait_for_cursor(&config, fixture.bytes.len() as i64),
            "the rest was read from the cursor"
        );
        handle.stop();
        assert_eq!(stored_settings(&config), on);
        assert_eq!(
            stored_event_count(&config),
            logged_event_count(&events),
            "every whitelisted event stored exactly once"
        );
    }

    #[test]
    fn a_save_that_timed_out_is_never_committed_later() {
        let fixture = load_fixture("league_join_drop");
        let events = prepared_events(&fixture);
        let cut = events[events.len() / 2].offset as usize;
        let dir = TestDir::new("supervisor-settings-expired");
        dir.write("logs/mtgo.log", &fixture.bytes[..cut]);
        let config = config(&dir, &fixture);
        let (sink, receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        assert!(wait_for_cursor(&config, cut as i64), "first half committed");

        // Hold the restarted engine so the save has to wait in the inbox.
        test_hooks::FAIL_NEXT_BATCH
            .lock()
            .expect("hooks")
            .push(config.db_path.clone());
        test_hooks::HOLD_REOPEN
            .lock()
            .expect("hooks")
            .push(config.db_path.clone());
        dir.append("logs/mtgo.log", &fixture.bytes[cut..]);
        wait_for(&receiver, Duration::from_secs(60), |events| {
            restarts(events) >= 1
        });

        let late = vec![("log_folder".to_string(), "\"C:/late\"".to_string())];
        let result = handle
            .settings_saver()
            .save_within(late, Duration::from_millis(200));
        assert!(result.is_err(), "the caller gave up");
        test_hooks::release_reopen(&config.db_path);

        // Commands run in order, so once this one is answered the late one has
        // been handled (or skipped).
        let later = vec![("close_to_tray".to_string(), "false".to_string())];
        handle.settings_saver().save(later.clone()).expect("saved");
        assert_eq!(stored_settings(&config), later);
        handle.stop();
    }

    #[test]
    fn the_tracker_answers_an_import_with_counts() {
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("supervisor-import");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let (sink, _receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");

        let token = "f5e33a1f-c2e7-4678-b30d-309b63f17a44";
        let matches = vec![crate::engine::tests::imported(token, 4_242)];
        let first = handle.importer().import(matches.clone()).expect("imported");
        assert_eq!(first.added, 1);
        let again = handle.importer().import(matches).expect("imported again");
        assert_eq!((again.added, again.unchanged), (0, 1));
        handle.stop();
    }

    #[test]
    fn the_tracker_answers_a_deck_import_with_counts() {
        use store::import_decks::{ImportedDeck, ImportedDecks, ImportedLink, ImportedVersion};
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("supervisor-import-decks");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let (sink, _receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");

        let token = "f5e33a1f-c2e7-4678-b30d-309b63f17a45";
        let importer = handle.importer();
        importer
            .import(vec![crate::engine::tests::imported(token, 4_244)])
            .expect("matches");
        let signature = "MTI6NDpmYWxzZQ==".to_string();
        let data = ImportedDecks {
            cards: Vec::new(),
            decks: vec![ImportedDeck {
                mtgo_id: "1001".to_string(),
                account: None,
                name: "Q Burn".to_string(),
                original_name: None,
                format: "CMODERN".to_string(),
                color_identity: None,
                cover_catalog_id: None,
                archived_at_ms: None,
                created_at_ms: None,
                updated_at_ms: None,
                versions: vec![ImportedVersion {
                    signature: signature.clone(),
                    modified_at_ms: 1,
                }],
            }],
            links: vec![ImportedLink {
                token: token.to_string(),
                mtgo_match_id: Some(4_244),
                deck_mtgo_id: "1001".to_string(),
                signature,
            }],
        };
        let first = importer.import_decks(data.clone()).expect("decks");
        assert_eq!((first.decks_added, first.links_added), (1, 1));
        let again = importer.import_decks(data).expect("decks again");
        assert_eq!((again.decks_unchanged, again.links_unchanged), (1, 1));
        handle.stop();
    }

    // Overlay spec section 12: 0.x's deck labels go through the writer
    // after the decks, and a new label tells the pages (`DecksChanged`).
    #[test]
    fn the_tracker_answers_a_deck_label_import_with_counts_and_tells_the_pages() {
        use store::import_decks::{ImportedDeck, ImportedDecks};
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("supervisor-import-labels");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let (sink, receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");

        let importer = handle.importer();
        let deck = ImportedDeck {
            mtgo_id: "1001".to_string(),
            account: None,
            name: "Q Burn".to_string(),
            original_name: None,
            format: "CMODERN".to_string(),
            color_identity: None,
            cover_catalog_id: None,
            archived_at_ms: None,
            created_at_ms: None,
            updated_at_ms: None,
            versions: Vec::new(),
        };
        importer
            .import_decks(ImportedDecks {
                cards: Vec::new(),
                decks: vec![deck],
                links: Vec::new(),
            })
            .expect("decks");
        // The deck import's own notice first, so the next one is the labels'.
        let told = |receiver: &mpsc::Receiver<TrackerEvent>| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline {
                if let Ok(event) = receiver.recv_timeout(Duration::from_millis(100))
                    && event == TrackerEvent::DecksChanged
                {
                    return true;
                }
            }
            false
        };
        assert!(told(&receiver), "the deck import tells the pages");
        let labels = vec![
            LegacyDeckLabel {
                deck_mtgo_id: "1001".to_string(),
                label: "Q stream deck".to_string(),
            },
            LegacyDeckLabel {
                deck_mtgo_id: "q-unknown".to_string(),
                label: "Q nobody".to_string(),
            },
        ];
        let first = importer
            .import_overlay_labels(labels.clone())
            .expect("labels");
        assert_eq!((first.labels_added, first.labels_unknown), (1, 1));
        assert!(
            told(&receiver),
            "a new label tells the deck pages and the league card"
        );
        let again = importer.import_overlay_labels(labels).expect("again");
        assert_eq!((again.labels_added, again.labels_kept), (0, 1));
        handle.stop();
    }

    #[test]
    fn the_tracker_answers_a_league_import_with_counts() {
        use store::import_leagues::{ImportedLeague, LeagueMember};
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("supervisor-import-leagues");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let (sink, _receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");

        let token = "q-imported-league-match";
        let importer = handle.importer();
        importer
            .import(vec![crate::engine::tests::imported(token, 4_245)])
            .expect("matches");
        let leagues = vec![ImportedLeague {
            legacy_id: 31,
            manual: false,
            kind: "constructed".to_string(),
            set_code: None,
            course_id: None,
            token: "q-league".to_string(),
            event_id: Some(310),
            name: "Q League".to_string(),
            format: "CMODERN".to_string(),
            state: "complete".to_string(),
            started_at_ms: 1,
            joined_at_ms: None,
            completed_at_ms: Some(2),
            dropped_at_ms: None,
            created_at_ms: None,
            deck: None,
            notes: None,
            matches: vec![LeagueMember {
                token: token.to_string(),
                mtgo_match_id: Some(4_245),
            }],
        }];
        let first = importer.import_leagues(leagues.clone()).expect("leagues");
        assert_eq!(first.added, 1);
        let again = importer.import_leagues(leagues).expect("leagues again");
        assert_eq!(again.unchanged, 1);
        handle.stop();
    }

    #[test]
    fn the_tracker_answers_a_detail_import() {
        use store::imported_detail::{DetailChange, DetailLogEntry};
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("supervisor-import-detail");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let (sink, _receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");

        let token = "q-imported-detail-match";
        let importer = handle.importer();
        importer
            .import(vec![crate::engine::tests::imported(token, 4_246)])
            .expect("matches");
        let detail = ImportedDetail {
            games: Vec::new(),
            log: vec![DetailLogEntry {
                at_ms: 1,
                message: "Q line".to_string(),
            }],
        };
        let first = importer
            .import_detail(token.to_string(), detail.clone())
            .expect("detail");
        assert_eq!(first.change, DetailChange::Added);
        let again = importer
            .import_detail(token.to_string(), detail)
            .expect("detail again");
        assert_eq!(again.change, DetailChange::Unchanged);
        handle.stop();
    }

    #[test]
    fn an_import_the_tracker_cannot_answer_times_out_and_the_tracker_still_stops() {
        let fixture = load_fixture("league_join_drop");
        let events = prepared_events(&fixture);
        let cut = events[events.len() / 2].offset as usize;
        let dir = TestDir::new("supervisor-import-stuck");
        dir.write("logs/mtgo.log", &fixture.bytes[..cut]);
        let config = config(&dir, &fixture);
        let (sink, receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        assert!(wait_for_cursor(&config, cut as i64), "first half committed");

        // Fail the next batch and keep the restarted engine from opening, as
        // an engine that cannot open would: no command is answered.
        test_hooks::FAIL_NEXT_BATCH
            .lock()
            .expect("hooks")
            .push(config.db_path.clone());
        test_hooks::HOLD_REOPEN
            .lock()
            .expect("hooks")
            .push(config.db_path.clone());
        dir.append("logs/mtgo.log", &fixture.bytes[cut..]);
        wait_for(&receiver, Duration::from_secs(60), |events| {
            restarts(events) >= 1
        });

        let importer = handle.importer();
        let token = "f5e33a1f-c2e7-4678-b30d-309b63f17a45";
        let matches = vec![crate::engine::tests::imported(token, 4_243)];
        let started = Instant::now();
        let late = importer.import_within(matches.clone(), Duration::from_millis(200));
        assert!(late.is_err(), "the caller gave up: {late:?}");
        assert!(started.elapsed() < Duration::from_secs(5));

        // An import still waiting does not keep the tracker from stopping,
        // and learns that it stopped.
        let pending =
            std::thread::spawn(move || importer.import_within(matches, Duration::from_secs(60)));
        std::thread::sleep(Duration::from_millis(100));
        let stopping = Instant::now();
        handle.stop();
        let answer = pending.join().expect("import thread");
        assert!(answer.is_err(), "the tracker stopped: {answer:?}");
        assert!(stopping.elapsed() < Duration::from_secs(30));
        test_hooks::release_reopen(&config.db_path);
    }

    #[test]
    fn a_history_import_and_its_delete_are_answered_once_committed() {
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("supervisor-history-import");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let (sink, _receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        let importer = handle.importer();
        let matches = vec![crate::engine::tests::history_match("q-h1", 4_501)];
        let counts = importer.import_history(matches).expect("import");
        assert_eq!(counts.imported, vec!["q-h1".to_string()]);
        let imported: i64 = open_reader(&config.db_path)
            .expect("reader")
            .query_row(
                "SELECT COUNT(*) FROM matches WHERE token = 'q-h1' AND source = 'history'",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(imported, 1, "committed before the answer");
        let removed = importer.delete_history_imports().expect("delete");
        assert_eq!(removed, vec!["q-h1".to_string()]);
        handle.stop();
    }

    #[test]
    fn waiting_for_work_returns_as_soon_as_a_command_arrives() {
        let stop = AtomicBool::new(false);
        let (commands, inbox) = mpsc::sync_channel(4);
        let (reply, _answer) = mpsc::channel();
        commands
            .send(Command::SaveSettings {
                rows: Vec::new(),
                reply,
                deadline: Instant::now() + SAVE_TIMEOUT,
            })
            .expect("send");
        let started = Instant::now();
        let command = wait_for_command(&stop, &inbox, 5_000);
        assert!(command.is_some());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_deck_sync_answers_with_the_decks_written_once_committed() {
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("supervisor-deck-sync");
        dir.write("logs/mtgo.log", &fixture.bytes);
        dir.write("mtgo-data/Q1/acct/user_settings", b"x");
        let config = config(&dir, &fixture);
        let (sink, _receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        // Once the log is read, its logins no longer ask for a sync of their own.
        assert!(wait_for_cursor(&config, fixture.bytes.len() as i64));
        assert_eq!(handle.deck_editor().sync(), Ok(0), "no deck files yet");

        const DECK: &str = "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d";
        dir.write(
            &format!("mtgo-data/Q1/acct/grouping {DECK}.xml"),
            format!(
                "<Grouping Name=\"Qdeck\" NetDeckId=\"{DECK}\" GroupingType=\"Deck\" \
                 Timestamp=\"2026-06-30T10:00:00\" FormatCode=\"CMODERN\">\
                 <Item CatId=\"1001\" Quantity=\"4\" IsSideboard=\"False\" /></Grouping>"
            )
            .as_bytes(),
        );
        assert_eq!(handle.deck_editor().sync(), Ok(1));
        let reader = open_reader(&config.db_path).expect("reader");
        assert!(
            store::decks::deck_by_mtgo_id(&reader, DECK)
                .expect("read")
                .is_some(),
            "stored when answered"
        );
        assert_eq!(handle.deck_editor().sync(), Ok(0), "unchanged");
        handle.stop();
    }

    /// Spec 4.1 and 0.x `MtgoManager::syncDecks` (a no-op unless
    /// `canRun()`): while the watcher is stopped, "Sync decks" reads no deck
    /// file and writes nothing; once started it syncs again.
    #[test]
    fn a_deck_sync_while_the_watcher_is_stopped_reads_nothing() {
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("supervisor-deck-sync-paused");
        dir.write("logs/mtgo.log", &fixture.bytes);
        dir.write("mtgo-data/Q1/acct/user_settings", b"x");
        const DECK: &str = "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d";
        dir.write(
            &format!("mtgo-data/Q1/acct/grouping {DECK}.xml"),
            format!(
                "<Grouping Name=\"Qdeck\" NetDeckId=\"{DECK}\" GroupingType=\"Deck\" \
                 Timestamp=\"2026-06-30T10:00:00\" FormatCode=\"CMODERN\">\
                 <Item CatId=\"1001\" Quantity=\"4\" IsSideboard=\"False\" /></Grouping>"
            )
            .as_bytes(),
        );
        let config = TrackerConfig {
            watching: false,
            arena_enabled: true,
            endstep_enabled: false,
            ..config(&dir, &fixture)
        };
        let (sink, _receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        assert_eq!(
            handle.deck_editor().sync(),
            Ok(0),
            "nothing read while stopped"
        );
        let reader = open_reader(&config.db_path).expect("reader");
        assert!(
            store::decks::deck_by_mtgo_id(&reader, DECK)
                .expect("read")
                .is_none(),
            "no deck stored while stopped"
        );
        handle
            .settings_saver()
            .set_watching(true, Vec::new())
            .expect("started");
        // Started, the tick's own sync may get there first: either way the
        // deck is stored once the button's sync answers.
        handle.deck_editor().sync().expect("synced once started");
        assert!(
            store::decks::deck_by_mtgo_id(&reader, DECK)
                .expect("read")
                .is_some(),
            "stored once started"
        );
        handle.stop();
    }

    #[test]
    fn a_deck_edit_is_answered_with_its_outcome_and_reported() {
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("supervisor-deck-edit");
        dir.write("logs/mtgo.log", &fixture.bytes);
        dir.write("mtgo-data/Q1/acct/user_settings", b"x");
        const DECK: &str = "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d";
        dir.write(
            &format!("mtgo-data/Q1/acct/grouping {DECK}.xml"),
            format!(
                "<Grouping Name=\"Qdeck\" NetDeckId=\"{DECK}\" GroupingType=\"Deck\" \
                 Timestamp=\"2026-06-30T10:00:00\" FormatCode=\"CMODERN\">\
                 <Item CatId=\"1001\" Quantity=\"4\" IsSideboard=\"False\" /></Grouping>"
            )
            .as_bytes(),
        );
        let config = config(&dir, &fixture);
        let (sink, receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        assert!(wait_for_cursor(&config, fixture.bytes.len() as i64));
        handle.deck_editor().sync().expect("synced");
        let reader = open_reader(&config.db_path).expect("reader");
        let deck_id = store::decks::deck_by_mtgo_id(&reader, DECK)
            .expect("read")
            .expect("stored")
            .id;
        // Drain what the sync reported.
        wait_for(&receiver, Duration::from_secs(5), |events| {
            events
                .iter()
                .any(|event| matches!(event, TrackerEvent::DecksChanged))
        });

        let editor = handle.deck_editor();
        let renamed = editor.edit(DeckEdit::Rename {
            deck_id,
            name: "Qrenamed".to_string(),
        });
        assert_eq!(renamed, Ok(EditOutcome::Done));
        let name: String = reader
            .query_row("SELECT name FROM decks WHERE id = ?1", [deck_id], |row| {
                row.get(0)
            })
            .expect("name");
        assert_eq!(name, "Qrenamed", "committed when answered");
        let seen = wait_for(&receiver, Duration::from_secs(30), |events| {
            events
                .iter()
                .any(|event| matches!(event, TrackerEvent::DecksChanged))
        });
        assert!(
            seen.iter()
                .any(|event| matches!(event, TrackerEvent::DecksChanged))
        );

        assert_eq!(
            handle
                .deck_editor()
                .edit(DeckEdit::Archive { deck_id, at_ms: 1 }),
            Ok(EditOutcome::Done)
        );
        assert_eq!(
            handle.deck_editor().edit(DeckEdit::SetCover {
                deck_id,
                catalog_id: Some(1001)
            }),
            Ok(EditOutcome::Archived)
        );
        assert_eq!(
            handle
                .deck_editor()
                .edit(DeckEdit::Restore { deck_id: 9_999 }),
            Ok(EditOutcome::NotFound)
        );
        handle.stop();
    }

    #[test]
    fn a_deck_edit_the_tracker_cannot_answer_times_out() {
        let (commands, _inbox) = mpsc::sync_channel(1);
        let editor = DeckEditor { commands };
        let started = Instant::now();
        let answer =
            editor.edit_within(DeckEdit::Restore { deck_id: 1 }, Duration::from_millis(100));
        assert!(answer.is_err(), "{answer:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_deck_edit_that_timed_out_is_never_applied_later() {
        let dir = TestDir::new("supervisor-edit-expired");
        dir.write("logs/mtgo.log", b"12:00:00 [INF] (UI|x)\n");
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
        {
            let conn = store::open_writer(&config.db_path).expect("migrate");
            conn.execute_batch(
                "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source)
                 VALUES (1, 'Q-deck', 'Qdeck', 'CMODERN', 1, 1, 'file');",
            )
            .expect("deck");
        }
        let mut engine = Engine::open(
            config.clone(),
            Env::current(),
            Arc::default(),
            Vec::new(),
            now_ms(),
        )
        .expect("engine");

        // Nothing reads the inbox, so the caller gives up and is told so.
        let (commands, inbox) = mpsc::sync_channel(1);
        let editor = DeckEditor { commands };
        let rename = DeckEdit::Rename {
            deck_id: 1,
            name: "Qlate".to_string(),
        };
        assert!(
            editor
                .edit_within(rename, Duration::from_millis(50))
                .is_err()
        );

        // The tracker gets to it afterwards: skipped, not applied.
        let late = inbox.try_recv().expect("queued");
        handle_real(&mut engine, late).expect("handled");
        let reader = open_reader(&config.db_path).expect("reader");
        let name: String = reader
            .query_row("SELECT name FROM decks WHERE id = 1", [], |row| row.get(0))
            .expect("deck");
        assert_eq!(name, "Qdeck");
    }

    /// A tracker over a one-line log, with Qlocal (account 1) and its deck 1
    /// already stored.
    fn league_config(dir: &TestDir) -> TrackerConfig {
        dir.write("logs/mtgo.log", b"12:00:00 [INF] (UI|x)\n");
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
        let conn = store::open_writer(&config.db_path).expect("migrate");
        conn.execute_batch(
            "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);
             INSERT INTO decks (id, mtgo_id, account_id, name, format, created_at, updated_at,
                 source)
             VALUES (1, 'Q-deck', 1, 'Qdeck', 'CMODERN', 1, 1, 'file');",
        )
        .expect("deck");
        config
    }

    fn create_league(name: &str) -> LeagueEdit {
        LeagueEdit::CreateManual {
            account_id: 1,
            deck_id: 1,
            name: name.to_string(),
            token: format!("manual_Q{name}"),
            started_at: 5_000,
            now_ms: 6_000,
        }
    }

    fn manual_league_names(config: &TrackerConfig) -> Vec<String> {
        let reader = open_reader(&config.db_path).expect("reader");
        let mut stmt = reader
            .prepare("SELECT name FROM leagues WHERE source = 'manual' ORDER BY id")
            .expect("prepare");
        stmt.query_map([], |row| row.get::<_, String>(0))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect()
    }

    #[test]
    fn a_league_edit_is_answered_with_its_outcome_once_committed() {
        let dir = TestDir::new("supervisor-league-edit");
        let config = league_config(&dir);
        let (sink, _receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");

        assert_eq!(
            handle.league_editor().edit(create_league("Q Friday")),
            Ok(LeagueEditOutcome::Done)
        );
        assert_eq!(
            manual_league_names(&config),
            vec!["Q Friday".to_string()],
            "committed when answered"
        );
        let editor = handle.league_editor();
        assert_eq!(
            editor.edit(LeagueEdit::SetStatus {
                run_key: "manual:1".to_string(),
                status: Some(store::leagues::UserState::Dropped),
                now_ms: 7_000,
            }),
            Ok(LeagueEditOutcome::Done)
        );
        handle.stop();
    }

    #[test]
    fn a_league_edit_the_tracker_cannot_answer_times_out() {
        let (commands, _inbox) = mpsc::sync_channel(1);
        let editor = LeagueEditor { commands };
        let started = Instant::now();
        let answer = editor.edit_within(create_league("Qlate"), Duration::from_millis(100));
        assert!(answer.is_err(), "{answer:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_league_edit_that_timed_out_is_never_applied_later() {
        let dir = TestDir::new("supervisor-league-edit-expired");
        let config = league_config(&dir);
        let mut engine = Engine::open(
            config.clone(),
            Env::current(),
            Arc::default(),
            Vec::new(),
            now_ms(),
        )
        .expect("engine");

        // Nothing reads the inbox, so the caller gives up and is told so.
        let (commands, inbox) = mpsc::sync_channel(1);
        let editor = LeagueEditor { commands };
        assert!(
            editor
                .edit_within(create_league("Qlate"), Duration::from_millis(50))
                .is_err()
        );

        // The tracker gets to it afterwards: skipped, not applied.
        let late = inbox.try_recv().expect("queued");
        handle_real(&mut engine, late).expect("handled");
        assert!(manual_league_names(&config).is_empty());
    }

    /// `league_config` with one complete match of Qlocal stored.
    fn match_config(dir: &TestDir) -> TrackerConfig {
        let config = league_config(dir);
        let conn = store::open_writer(&config.db_path).expect("migrate");
        conn.execute(
            "INSERT INTO matches (token, account_id, source, state, outcome, started_at,
                 last_activity_at, revision, derived_version)
             VALUES (?1, 1, 'log', 'complete', 'win', 1, 1, 1, 1)",
            [MATCH],
        )
        .expect("match");
        config
    }

    const MATCH: &str = "f5e33a1f-c2e7-4678-b30d-309b63f17a40";

    fn hide_match() -> MatchEdit {
        MatchEdit::Hide {
            token: MATCH.to_string(),
            now_ms: 6_000,
        }
    }

    fn hidden_rows(config: &TrackerConfig) -> i64 {
        open_reader(&config.db_path)
            .expect("reader")
            .query_row(
                "SELECT COUNT(*) FROM matches WHERE hidden_at IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .expect("count")
    }

    #[test]
    fn a_match_edit_is_answered_with_its_outcome_once_committed() {
        let dir = TestDir::new("supervisor-match-edit");
        let config = match_config(&dir);
        let (sink, _receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");

        assert_eq!(
            handle.match_editor().edit(hide_match()),
            Ok(MatchEditOutcome::Done)
        );
        assert_eq!(hidden_rows(&config), 1, "committed when answered");
        let editor = handle.match_editor();
        assert_eq!(
            editor.edit(MatchEdit::SetNotes {
                token: "Q-missing".to_string(),
                notes: "Q note".to_string(),
            }),
            Ok(MatchEditOutcome::NotFound)
        );
        handle.stop();
    }

    #[test]
    fn a_match_edit_the_tracker_cannot_answer_times_out() {
        let (commands, _inbox) = mpsc::sync_channel(1);
        let editor = MatchEditor { commands };
        let started = Instant::now();
        let answer = editor.edit_within(hide_match(), Duration::from_millis(100));
        assert!(answer.is_err(), "{answer:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_match_edit_that_timed_out_is_never_applied_later() {
        let dir = TestDir::new("supervisor-match-edit-expired");
        let config = match_config(&dir);
        let mut engine = Engine::open(
            config.clone(),
            Env::current(),
            Arc::default(),
            Vec::new(),
            now_ms(),
        )
        .expect("engine");

        // Nothing reads the inbox, so the caller gives up and is told so.
        let (commands, inbox) = mpsc::sync_channel(1);
        let editor = MatchEditor { commands };
        assert!(
            editor
                .edit_within(hide_match(), Duration::from_millis(50))
                .is_err()
        );

        // The tracker gets to it afterwards: skipped, not applied.
        let late = inbox.try_recv().expect("queued");
        handle_real(&mut engine, late).expect("handled");
        assert_eq!(hidden_rows(&config), 0);
    }

    fn untrack(username: &str) -> AccountEdit {
        AccountEdit::SetTracked {
            username: username.to_string(),
            tracked: false,
        }
    }

    fn tracked_flag(config: &TrackerConfig) -> i64 {
        open_reader(&config.db_path)
            .expect("reader")
            .query_row("SELECT MIN(tracked) FROM accounts", [], |row| row.get(0))
            .expect("tracked")
    }

    #[test]
    fn an_account_edit_is_answered_with_its_outcome_once_committed() {
        let dir = TestDir::new("supervisor-account-edit");
        let config = match_config(&dir);
        let username: String = open_reader(&config.db_path)
            .expect("reader")
            .query_row("SELECT username FROM accounts WHERE id = 1", [], |row| {
                row.get(0)
            })
            .expect("account");
        let (sink, _receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        assert_eq!(
            handle.account_editor().edit(untrack(&username)),
            Ok(AccountEditOutcome::Done {
                retrack: Vec::new()
            })
        );
        assert_eq!(tracked_flag(&config), 0, "committed when answered");
        assert_eq!(
            handle.account_editor().edit(untrack("Qnobody")),
            Ok(AccountEditOutcome::NotFound)
        );
        handle.stop();
    }

    #[test]
    fn an_account_edit_that_timed_out_is_never_applied_later() {
        let dir = TestDir::new("supervisor-account-edit-expired");
        let config = match_config(&dir);
        let mut engine = Engine::open(
            config.clone(),
            Env::current(),
            Arc::default(),
            Vec::new(),
            now_ms(),
        )
        .expect("engine");
        let username: String = open_reader(&config.db_path)
            .expect("reader")
            .query_row("SELECT username FROM accounts WHERE id = 1", [], |row| {
                row.get(0)
            })
            .expect("account");
        let (commands, inbox) = mpsc::sync_channel(1);
        let handle_side = AccountEditor { commands };
        assert!(
            handle_side
                .edit_within(untrack(&username), Duration::from_millis(50))
                .is_err()
        );
        let late = inbox.try_recv().expect("queued");
        handle_real(&mut engine, late).expect("handled");
        assert_eq!(tracked_flag(&config), 1, "skipped, not applied");
    }

    /// `league_config` with a 0.x draft `Q-draft` whose pick 1 exists.
    fn note_config(dir: &TestDir) -> TrackerConfig {
        let config = league_config(dir);
        store::open_writer(&config.db_path)
            .expect("migrate")
            .execute_batch(
                "INSERT INTO drafts (id, draft_token, source, state, imported_from)
                 VALUES (1, 'Q-draft', 'imported', 'finished', 1);
                 INSERT INTO draft_picks (draft_id, ordinal, pack_number, pick_number,
                     cards_available, reservations)
                 VALUES (1, 1, 1, 1, '[1]', '[]');",
            )
            .expect("draft");
        config
    }

    fn pick_note(ordinal: u32) -> DraftNoteEdit {
        DraftNoteEdit::Set {
            draft_token: "Q-draft".to_string(),
            ordinal,
            note: "Q note".to_string(),
            now_ms: 6_000,
        }
    }

    fn note_rows(config: &TrackerConfig) -> i64 {
        open_reader(&config.db_path)
            .expect("reader")
            .query_row(
                "SELECT COUNT(*) FROM draft_picks WHERE note IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .expect("count")
    }

    #[test]
    fn a_pick_note_edit_is_answered_with_its_outcome_once_committed() {
        let dir = TestDir::new("supervisor-note-edit");
        let config = note_config(&dir);
        let (sink, _receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        assert_eq!(
            handle.draft_note_editor().edit(pick_note(1)),
            Ok(DraftNoteOutcome::Done)
        );
        assert_eq!(note_rows(&config), 1, "committed when answered");
        assert_eq!(
            handle.draft_note_editor().edit(pick_note(2)),
            Ok(DraftNoteOutcome::NotFound)
        );
        handle.stop();
    }

    #[test]
    fn a_pick_note_edit_the_tracker_cannot_answer_times_out() {
        let (commands, _inbox) = mpsc::sync_channel(1);
        let editor = DraftNoteEditor { commands };
        let started = Instant::now();
        let answer = editor.edit_within(pick_note(1), Duration::from_millis(100));
        assert!(answer.is_err(), "{answer:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_pick_note_edit_that_timed_out_is_never_applied_later() {
        let dir = TestDir::new("supervisor-note-edit-expired");
        let config = note_config(&dir);
        let mut engine = Engine::open(
            config.clone(),
            Env::current(),
            Arc::default(),
            Vec::new(),
            now_ms(),
        )
        .expect("engine");
        let (commands, inbox) = mpsc::sync_channel(1);
        let editor = DraftNoteEditor { commands };
        assert!(
            editor
                .edit_within(pick_note(1), Duration::from_millis(50))
                .is_err()
        );
        let late = inbox.try_recv().expect("queued");
        handle_real(&mut engine, late).expect("handled");
        assert_eq!(note_rows(&config), 0);
    }

    /// `league_config` with a complete match 1 against `Qopponent`.
    fn archetype_config(dir: &TestDir) -> TrackerConfig {
        let config = league_config(dir);
        store::open_writer(&config.db_path)
            .expect("migrate")
            .execute_batch(
                "INSERT INTO opponents (id, username, first_seen_at) VALUES (1, 'Qopponent', 0);
                 INSERT INTO matches (id, token, account_id, opponent_id, source, format,
                     state, outcome, started_at, last_activity_at, revision, derived_version)
                 VALUES (1, 'Q-match', 1, 1, 'imported', 'CMODERN', 'complete', 'win', 1, 1,
                     1, 0);",
            )
            .expect("match");
        config
    }

    fn homebrew_opponent() -> ArchetypeEdit {
        ArchetypeEdit::SetOpponent {
            match_ids: vec![1],
            archetype_id: Some(1),
            now_ms: 6_000,
        }
    }

    fn archetype_rows(config: &TrackerConfig) -> i64 {
        open_reader(&config.db_path)
            .expect("reader")
            .query_row("SELECT COUNT(*) FROM match_archetypes", [], |row| {
                row.get(0)
            })
            .expect("count")
    }

    #[test]
    fn an_archetype_edit_is_answered_with_its_outcome_once_committed() {
        let dir = TestDir::new("supervisor-archetype-edit");
        let config = archetype_config(&dir);
        let (sink, receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        assert_eq!(
            handle.archetype_editor().edit(homebrew_opponent()),
            Ok(ArchetypeEditOutcome::Done {
                matches: vec![1],
                decks: Vec::new()
            })
        );
        assert_eq!(archetype_rows(&config), 1, "committed when answered");
        let events = wait_for(&receiver, Duration::from_secs(5), |events| {
            events.iter().any(|event| {
                matches!(event, TrackerEvent::MatchArchetypesChanged { match_ids }
                    if match_ids == &[1])
            })
        });
        assert!(
            events
                .iter()
                .any(|event| matches!(event, TrackerEvent::MatchArchetypesChanged { .. })),
            "{events:?}"
        );
        assert_eq!(
            handle.archetype_editor().edit(ArchetypeEdit::SetOpponent {
                match_ids: vec![1],
                archetype_id: Some(999),
                now_ms: 6_000,
            }),
            Ok(ArchetypeEditOutcome::UnknownArchetype)
        );
        handle.stop();
    }

    /// A downloaded definition is an archetype change the pages hear of.
    #[test]
    fn a_downloaded_definition_reports_archetypes_changed() {
        let dir = TestDir::new("supervisor-archetype-download");
        let config = archetype_config(&dir);
        let (sink, receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        let definition = archetypes::Definition {
            uuid: "q-new".to_string(),
            name: "Qnew".to_string(),
            ..archetypes::Definition::default()
        };
        assert_eq!(
            handle
                .archetype_editor()
                .edit(ArchetypeEdit::EnsureDownloaded {
                    definition: Box::new(definition),
                    format: "modern".to_string(),
                    now_ms: 6_000,
                }),
            Ok(ArchetypeEditOutcome::Done {
                matches: Vec::new(),
                decks: Vec::new()
            })
        );
        let events = wait_for(&receiver, Duration::from_secs(5), |events| {
            events.contains(&TrackerEvent::ArchetypesChanged)
        });
        assert!(
            events.contains(&TrackerEvent::ArchetypesChanged),
            "{events:?}"
        );
        handle.stop();
    }

    /// Overlay spec 14 (U12 review I1): a manual archetype edit is an
    /// archetype change too, so the game overlay reads its candidates
    /// again. Removing a variant moves neither the row count nor
    /// `updated_at`, so only the event tells it.
    #[test]
    fn a_manual_archetype_edit_reports_archetypes_changed() {
        let dir = TestDir::new("supervisor-archetype-manual");
        let config = archetype_config(&dir);
        store::open_writer(&config.db_path)
            .expect("open")
            .execute_batch(
                "INSERT INTO archetypes (id, uuid, name, format, source, created_at, updated_at)
                 VALUES (500, 'q-manual', 'Qmanual', 'modern', 'manual', 1, 1);
                 INSERT INTO archetype_cards (archetype_id, list, catalog_id, oracle_id,
                     quantity, sideboard)
                 VALUES (500, 1, 11, 'oracle-11', 4, 0), (500, 2, 12, 'oracle-12', 4, 0);",
            )
            .expect("a manual archetype with two variants");
        let (sink, receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        assert_eq!(
            handle
                .archetype_editor()
                .edit(ArchetypeEdit::RemoveVariant { id: 500, list: 2 }),
            Ok(ArchetypeEditOutcome::Done {
                matches: Vec::new(),
                decks: Vec::new()
            })
        );
        let events = wait_for(&receiver, Duration::from_secs(5), |events| {
            events.contains(&TrackerEvent::ArchetypesChanged)
        });
        assert!(
            events.contains(&TrackerEvent::ArchetypesChanged),
            "{events:?}"
        );
        // A refused edit changed nothing and says nothing.
        assert_eq!(
            handle
                .archetype_editor()
                .edit(ArchetypeEdit::RemoveVariant { id: 500, list: 1 }),
            Ok(ArchetypeEditOutcome::Refused(
                "Cannot delete the last variant.".to_string()
            ))
        );
        handle.stop();
    }

    #[test]
    fn an_archetype_edit_the_tracker_cannot_answer_times_out() {
        let (commands, _inbox) = mpsc::sync_channel(1);
        let editor = ArchetypeEditor { commands };
        let started = Instant::now();
        let answer = editor.edit_within(homebrew_opponent(), Duration::from_millis(100));
        assert!(answer.is_err(), "{answer:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn an_edit_past_its_deadline_changes_nothing() {
        let dir = TestDir::new("supervisor-archetype-edit-expired");
        let config = archetype_config(&dir);
        let mut engine = Engine::open(
            config.clone(),
            Env::current(),
            Arc::default(),
            Vec::new(),
            now_ms(),
        )
        .expect("engine");
        let (commands, inbox) = mpsc::sync_channel(1);
        let editor = ArchetypeEditor { commands };
        assert!(
            editor
                .edit_within(homebrew_opponent(), Duration::from_millis(50))
                .is_err()
        );
        let late = inbox.try_recv().expect("queued");
        handle_real(&mut engine, late).expect("handled");
        assert_eq!(archetype_rows(&config), 0);
    }

    /// A deck version whose signature lists catalog id 101, with no
    /// `cards` row.
    fn version_without_cards(config: &TrackerConfig) {
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
    }

    fn card_rows(config: &TrackerConfig) -> i64 {
        open_reader(&config.db_path)
            .expect("reader")
            .query_row("SELECT COUNT(*) FROM cards", [], |row| row.get(0))
            .expect("count")
    }

    #[test]
    fn a_card_stub_scan_runs_through_the_tracker() {
        let dir = TestDir::new("supervisor-card-stub-scan");
        let config = note_config(&dir);
        version_without_cards(&config);
        let (sink, _receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        // 101 from the version and 1 from `note_config`'s pick.
        assert_eq!(handle.card_stub_scanner().scan(), Ok(2));
        assert_eq!(handle.card_stub_scanner().scan(), Ok(0));
        assert_eq!(card_rows(&config), 2);
        handle.stop();
    }

    // Issue #326: an archetype decklist's unknown cards get stubs through
    // the tracker, reported as `CardStubsAdded` so the cards worker looks
    // them up.
    #[test]
    fn card_stubs_for_given_ids_are_written_through_the_tracker_and_reported() {
        let dir = TestDir::new("supervisor-ensure-card-stubs");
        let config = note_config(&dir);
        let (sink, receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        let before = card_rows(&config);
        let scanner = handle.card_stub_scanner();
        assert_eq!(scanner.ensure(vec![70_001, 70_002]), Ok(2));
        assert_eq!(scanner.ensure(vec![70_001]), Ok(0));
        assert!(scanner.request(vec![70_003]));
        let added = |events: &[TrackerEvent]| {
            events
                .iter()
                .filter_map(|event| match event {
                    TrackerEvent::CardStubsAdded { count } => Some(*count),
                    _ => None,
                })
                .sum::<u32>()
                >= 3
        };
        let seen = wait_for(&receiver, Duration::from_secs(10), added);
        assert!(added(&seen), "CardStubsAdded for every stub: {seen:?}");
        assert_eq!(card_rows(&config), before + 3);
        handle.stop();
    }

    // An Arena replay's tokens with no art are queued the same way, named
    // and marked as tokens, for the API's token-by-name fetch.
    #[test]
    fn token_stubs_are_written_through_the_tracker_and_reported() {
        let dir = TestDir::new("supervisor-token-stubs");
        let config = note_config(&dir);
        let (sink, receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        let before = card_rows(&config);
        let scanner = handle.card_stub_scanner();
        assert!(scanner.request_tokens(vec![TokenCard {
            catalog_id: 100_090_001,
            name: "Map".to_string(),
            card_type: "Token".to_string(),
            sub_type: Some("Map".to_string()),
            color_identity: "C".to_string(),
        }]));
        let added = |events: &[TrackerEvent]| {
            events
                .iter()
                .any(|event| matches!(event, TrackerEvent::CardStubsAdded { count: 1 }))
        };
        let seen = wait_for(&receiver, Duration::from_secs(10), added);
        assert!(added(&seen), "CardStubsAdded for the token: {seen:?}");
        assert_eq!(card_rows(&config), before + 1);
        handle.stop();
    }

    fn bolt_resolution() -> Resolutions {
        Resolutions {
            resolved: vec![mymtgo_api::Resolution {
                mtgo_id: 101,
                card: mymtgo_api::CardAnswer {
                    scryfall_id: Some("q-scryfall".to_string()),
                    name: Some("Qbolt".to_string()),
                    ..mymtgo_api::CardAnswer::default()
                },
                is_token: false,
            }],
            unknown: Vec::new(),
        }
    }

    fn resolved_names(config: &TrackerConfig) -> i64 {
        open_reader(&config.db_path)
            .expect("reader")
            .query_row(
                "SELECT COUNT(*) FROM cards WHERE scryfall_id IS NOT NULL OR name IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .expect("count")
    }

    #[test]
    fn a_population_run_is_stored_through_the_tracker_and_reported() {
        let dir = TestDir::new("supervisor-cards-resolved");
        let config = note_config(&dir);
        version_without_cards(&config);
        let (sink, receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        assert_eq!(handle.card_stub_scanner().scan(), Ok(2));
        let outcome = handle
            .cards_resolver()
            .resolve(Vec::new(), bolt_resolution(), 0)
            .expect("resolved");
        assert_eq!(outcome.updated, 1);
        assert_eq!(outcome.changed, vec![101]);
        assert_eq!(resolved_names(&config), 1);
        let has = |events: &[TrackerEvent]| {
            events
                .iter()
                .any(|event| matches!(event, TrackerEvent::CardsChanged))
        };
        let seen = wait_for(&receiver, Duration::from_secs(10), has);
        assert!(has(&seen), "CardsChanged after the run is stored");
        handle.stop();
    }

    #[test]
    fn a_resolution_past_its_deadline_changes_nothing() {
        let dir = TestDir::new("supervisor-cards-resolved-expired");
        let config = note_config(&dir);
        version_without_cards(&config);
        let mut engine = Engine::open(
            config.clone(),
            Env::current(),
            Arc::default(),
            Vec::new(),
            now_ms(),
        )
        .expect("engine");
        engine.scan_card_stubs().expect("stubs");
        let (commands, inbox) = mpsc::sync_channel(1);
        let resolver = CardsResolver {
            settings: SettingsSaver {
                commands: commands.clone(),
            },
            commands,
        };
        assert!(
            resolver
                .resolve_within(Vec::new(), bolt_resolution(), 0, Duration::from_millis(50))
                .is_err()
        );
        let late = inbox.try_recv().expect("queued");
        handle_real(&mut engine, late).expect("handled");
        engine.flush().expect("flush");
        assert_eq!(resolved_names(&config), 0, "a late run never writes");
    }

    fn local_files(config: &TrackerConfig, catalog_id: i64) -> (Option<String>, Option<String>) {
        open_reader(&config.db_path)
            .expect("reader")
            .query_row(
                "SELECT local_image, local_art_crop FROM cards WHERE catalog_id = ?1",
                [catalog_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("row")
    }

    #[test]
    fn card_images_are_recorded_through_the_tracker() {
        let dir = TestDir::new("supervisor-card-images");
        let config = note_config(&dir);
        version_without_cards(&config);
        let (sink, receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        assert_eq!(handle.card_stub_scanner().scan(), Ok(2));
        let images = handle.card_images();
        assert_eq!(images.stored(101, Some("101.jpg".to_string()), None), Ok(1));
        assert_eq!(
            images.stored(101, None, Some("101_art.jpg".to_string())),
            Ok(1)
        );
        assert_eq!(
            local_files(&config, 101),
            (Some("101.jpg".to_string()), Some("101_art.jpg".to_string()))
        );
        let present: BTreeSet<String> = ["101.jpg".to_string()].into();
        assert_eq!(images.present(present), Ok(1), "the art file is gone");
        assert_eq!(
            local_files(&config, 101),
            (Some("101.jpg".to_string()), None)
        );
        let has = |events: &[TrackerEvent]| {
            events
                .iter()
                .any(|event| matches!(event, TrackerEvent::CardsChanged))
        };
        let seen = wait_for(&receiver, Duration::from_secs(10), has);
        assert!(has(&seen), "pages drop the cleared local names");
        handle.stop();
    }

    #[test]
    fn recordings_are_stored_through_the_tracker() {
        use store::recorded_replays::{ImportReport, NewRecording};
        let dir = TestDir::new("supervisor-recordings");
        let config = note_config(&dir);
        let (sink, _receiver) = mpsc::channel();
        let handle = spawn(config, sink).expect("spawn");
        let batch = vec![NewRecording {
            header: mtgr::RecordingHeader {
                match_token: "6f1c2d3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f".to_string(),
                mtgo_match_id: None,
                recorder: None,
                games: vec![mtgr::HeaderGame {
                    position: 0,
                    game_number: 1,
                    mtgo_game_id: None,
                }],
            },
            text: "a".to_string(),
            file_hash: "h1".to_string(),
        }];
        let importer = handle.recording_importer();
        assert_eq!(
            importer.store(batch.clone(), 1),
            Ok(ImportReport {
                added: 1,
                replaced: 0,
                unchanged: 0
            })
        );
        assert_eq!(
            importer.store(batch, 2),
            Ok(ImportReport {
                added: 0,
                replaced: 0,
                unchanged: 1
            })
        );
        handle.stop();
    }

    #[test]
    fn endstep_captures_are_stored_through_the_tracker() {
        use endstep_capture::{CaptureHeader, Classification, UnitKind};
        use store::endstep_captures::{CaptureImportReport, NewCapture};
        let dir = TestDir::new("supervisor-endstep-captures");
        let config = note_config(&dir);
        let (sink, _receiver) = mpsc::channel();
        let handle = spawn(config, sink).expect("spawn");
        let batch = vec![NewCapture {
            header: CaptureHeader {
                format: 1,
                unit_id: "0f8fad5b-d9cb-469f-a165-70867728950e".to_string(),
                kind: UnitKind::Match,
                started_at: 1,
                counter: 1,
                extension: "0.1.0".to_string(),
                browser: "chrome".to_string(),
                developer: false,
                classification: Classification::Human,
            },
            frame_count: 0,
            text: "a".to_string(),
            file_hash: "h1".to_string(),
        }];
        let importer = handle.endstep_capture_importer();
        assert_eq!(
            importer.store(batch.clone(), 1),
            Ok(CaptureImportReport {
                added: 1,
                replaced: 0,
                unchanged: 0
            })
        );
        assert_eq!(
            importer.store(batch, 2),
            Ok(CaptureImportReport {
                added: 0,
                replaced: 0,
                unchanged: 1
            })
        );
        handle.stop();
    }

    #[test]
    fn card_images_past_their_deadline_are_never_written() {
        let dir = TestDir::new("supervisor-card-images-expired");
        let config = note_config(&dir);
        version_without_cards(&config);
        let mut engine = Engine::open(
            config.clone(),
            Env::current(),
            Arc::default(),
            Vec::new(),
            now_ms(),
        )
        .expect("engine");
        engine.scan_card_stubs().expect("stubs");
        let (commands, inbox) = mpsc::sync_channel(2);
        let images = CardImagesRecorder { commands };
        assert!(
            images
                .stored_within(
                    101,
                    Some("101.jpg".to_string()),
                    None,
                    Duration::from_millis(50)
                )
                .is_err()
        );
        let late = inbox.try_recv().expect("queued");
        handle_real(&mut engine, late).expect("handled");
        engine.flush().expect("flush");
        assert_eq!(
            local_files(&config, 101),
            (None, None),
            "a late record never writes"
        );
    }

    #[test]
    fn a_card_stub_scan_that_timed_out_is_never_run_later() {
        let dir = TestDir::new("supervisor-card-stub-scan-expired");
        let config = note_config(&dir);
        version_without_cards(&config);
        let mut engine = Engine::open(
            config.clone(),
            Env::current(),
            Arc::default(),
            Vec::new(),
            now_ms(),
        )
        .expect("engine");
        let (commands, inbox) = mpsc::sync_channel(1);
        let scanner = CardStubScanner { commands };
        assert!(scanner.scan_within(Duration::from_millis(50)).is_err());
        let late = inbox.try_recv().expect("queued");
        handle_real(&mut engine, late).expect("handled");
        assert_eq!(card_rows(&config), 0);
    }

    #[test]
    fn a_card_stats_regenerate_past_its_deadline_is_skipped() {
        let dir = TestDir::new("supervisor-regenerate-expired");
        let config = note_config(&dir);
        version_without_cards(&config);
        let mut engine = Engine::open(
            config.clone(),
            Env::current(),
            Arc::default(),
            Vec::new(),
            now_ms(),
        )
        .expect("engine");
        let (reply, answer) = mpsc::channel();
        let late = Command::RegenerateCardStats {
            deck_id: 900,
            reply,
            deadline: Instant::now(),
        };
        handle_real(&mut engine, late).expect("handled");
        assert!(answer.recv().expect("answer").is_err());
        let (reply, answer) = mpsc::channel();
        let timely = Command::RegenerateCardStats {
            deck_id: 900,
            reply,
            deadline: Instant::now() + Duration::from_secs(10),
        };
        handle_real(&mut engine, timely).expect("handled");
        assert_eq!(answer.recv().expect("answer"), Ok(0));
    }

    #[test]
    fn a_card_stats_regenerate_runs_through_the_tracker() {
        let dir = TestDir::new("supervisor-regenerate");
        let config = note_config(&dir);
        version_without_cards(&config);
        let (sink, _receiver) = mpsc::channel();
        let handle = spawn(config, sink).expect("spawn");
        assert_eq!(handle.deck_editor().regenerate_card_stats(900), Ok(0));
        handle.stop();
    }

    #[test]
    fn limited_data_imports_through_the_tracker_and_a_rerun_is_unchanged() {
        let dir = TestDir::new("supervisor-import-limited");
        let config = note_config(&dir);
        let (sink, _receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        let importer = handle.importer();
        let data = ImportedLimited {
            drafts: Vec::new(),
            decks: vec![store::import_limited::LegacyLimitedDeck {
                league_id: 404,
                match_token: "Q-match".to_string(),
                mtgo_match_id: None,
                cards: Vec::new(),
                captured_at_ms: 1,
            }],
            league_keys: None,
        };
        let first = importer.import_limited(data.clone()).expect("limited");
        assert_eq!(first.decks_without_run, 1, "league 404 was not imported");
        let again = importer.import_limited(data).expect("again");
        assert_eq!(again, first);
        handle.stop();
    }

    /// `league_config` with a version of deck 1, so a manual match can be
    /// created.
    fn manual_config(dir: &TestDir) -> TrackerConfig {
        let config = league_config(dir);
        store::open_writer(&config.db_path)
            .expect("migrate")
            .execute(
                "INSERT INTO deck_versions (id, deck_id, signature, modified_at)
                 VALUES (10, 1, 'Q-sig', 1)",
                [],
            )
            .expect("version");
        config
    }

    fn create_manual() -> ManualEdit {
        ManualEdit::Create(ManualMatch {
            deck_id: 1,
            opponent: "Qkai".to_string(),
            started_at_ms: 5_000,
            ended_at_ms: 65_000,
            games: vec![ManualGame {
                won: true,
                on_play: true,
                turns: None,
            }],
            placement: ManualPlacement::None,
            opponent_archetype_id: None,
            ranked: false,
            best_of: store::BestOf::Three,
            rank_gap: None,
            now_ms: 6_000,
        })
    }

    fn manual_rows(config: &TrackerConfig) -> i64 {
        open_reader(&config.db_path)
            .expect("reader")
            .query_row(
                "SELECT COUNT(*) FROM matches WHERE source = 'manual'",
                [],
                |row| row.get(0),
            )
            .expect("count")
    }

    #[test]
    fn a_manual_edit_is_answered_with_its_outcome_once_committed() {
        let dir = TestDir::new("supervisor-manual-edit");
        let config = manual_config(&dir);
        let (sink, _receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");

        let created = handle.manual_editor().edit(create_manual());
        assert!(
            matches!(created, Ok(ManualEditOutcome::Created { .. })),
            "{created:?}"
        );
        assert_eq!(manual_rows(&config), 1, "committed when answered");
        let editor = handle.manual_editor();
        assert_eq!(
            editor.edit(ManualEdit::SetReveals {
                token: "Q-missing".to_string(),
                game: 1,
                cards: Vec::new(),
            }),
            Ok(ManualEditOutcome::NotFound)
        );
        handle.stop();
    }

    #[test]
    fn a_manual_edit_the_tracker_cannot_answer_times_out() {
        let (commands, _inbox) = mpsc::sync_channel(1);
        let editor = ManualEditor { commands };
        let started = Instant::now();
        let answer = editor.edit_within(create_manual(), Duration::from_millis(100));
        assert!(answer.is_err(), "{answer:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_manual_edit_that_timed_out_is_never_applied_later() {
        let dir = TestDir::new("supervisor-manual-edit-expired");
        let config = manual_config(&dir);
        let mut engine = Engine::open(
            config.clone(),
            Env::current(),
            Arc::default(),
            Vec::new(),
            now_ms(),
        )
        .expect("engine");

        // Nothing reads the inbox, so the caller gives up and is told so.
        let (commands, inbox) = mpsc::sync_channel(1);
        let editor = ManualEditor { commands };
        assert!(
            editor
                .edit_within(create_manual(), Duration::from_millis(50))
                .is_err()
        );

        // The tracker gets to it afterwards: skipped, not applied.
        let late = inbox.try_recv().expect("queued");
        handle_real(&mut engine, late).expect("handled");
        assert_eq!(manual_rows(&config), 0);
    }

    #[test]
    fn waiting_for_work_gives_up_at_the_pause() {
        let stop = AtomicBool::new(false);
        let (_commands, inbox) = mpsc::sync_channel::<Command>(4);
        let started = Instant::now();
        assert!(wait_for_command(&stop, &inbox, 60).is_none());
        assert!(started.elapsed() >= Duration::from_millis(60));
    }

    fn config(dir: &TestDir, fixture: &Fixture) -> TrackerConfig {
        TrackerConfig {
            db_path: dir.join("data/mymtgo.sqlite"),
            timezone: fixture.meta.tz(),
            os_timezone: None,
            log_overrides: vec![dir.join("logs")],
            arena_log_overrides: vec![dir.join("arena/Player.log")],
            mtgo_data_folder: Some(dir.join("mtgo-data")),
            replay_day_anchor: Some(fixture.meta.day_anchor()),
            watching: true,
            arena_enabled: true,
            endstep_enabled: false,
        }
    }

    fn wait_for(
        receiver: &Receiver<TrackerEvent>,
        timeout: Duration,
        done: impl Fn(&[TrackerEvent]) -> bool,
    ) -> Vec<TrackerEvent> {
        let deadline = Instant::now() + timeout;
        let mut seen = Vec::new();
        while Instant::now() < deadline {
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(event) => {
                    seen.push(event);
                    if done(&seen) {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        seen
    }

    fn committed_cursor(config: &TrackerConfig) -> Option<i64> {
        let reader = open_reader(&config.db_path).ok()?;
        reader
            .query_row(
                "SELECT cursor_offset FROM log_files WHERE sealed_at IS NULL",
                [],
                |row| row.get(0),
            )
            .ok()
    }

    /// Wait (up to a minute) until the committed cursor is `target`.
    fn wait_for_cursor(config: &TrackerConfig, target: i64) -> bool {
        for _ in 0..1_200 {
            if committed_cursor(config) == Some(target) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    fn stored_event_count(config: &TrackerConfig) -> i64 {
        let reader = open_reader(&config.db_path).expect("reader");
        let rows: i64 = reader
            .query_row("SELECT COUNT(*) FROM log_events", [], |row| row.get(0))
            .expect("count");
        let archived: i64 = reader
            .query_row(
                "SELECT COALESCE(SUM(event_count), 0) FROM match_event_archives",
                [],
                |row| row.get(0),
            )
            .expect("sum");
        rows + archived
    }

    /// The prepared events that go to `log_events`: `InPlayoffs` lines go to
    /// `tournament_player_states`, a fact table the tournament sync prunes
    /// outside its window (these fixtures are older than it).
    fn logged_event_count(events: &[mtgo_protocol::PreparedEvent]) -> i64 {
        events
            .iter()
            .filter(|event| event.kind != mtgo_protocol::EventKind::TournamentPlayerState)
            .count() as i64
    }

    fn quarantined_rows(config: &TrackerConfig) -> i64 {
        let reader = open_reader(&config.db_path).expect("reader");
        reader
            .query_row(
                "SELECT COUNT(*) FROM log_events WHERE quarantined = 1",
                [],
                |row| row.get(0),
            )
            .expect("count")
    }

    /// `(kind, quarantined)` of the stored row for `key`.
    fn stored_row(config: &TrackerConfig, key: (i64, u64)) -> (Option<String>, bool) {
        let reader = open_reader(&config.db_path).expect("reader");
        reader
            .query_row(
                "SELECT kind, quarantined FROM log_events WHERE log_file_id = ?1 AND byte_offset = ?2",
                [key.0, key.1 as i64],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("stored row")
    }

    fn restarts(events: &[TrackerEvent]) -> usize {
        events
            .iter()
            .filter(|event| matches!(event, TrackerEvent::Restarting { .. }))
            .count()
    }

    fn reported_quarantine(events: &[TrackerEvent]) -> bool {
        events
            .iter()
            .any(|event| matches!(event, TrackerEvent::Status(status) if status.quarantined == 1))
    }

    fn poison(config: &TrackerConfig, key: (i64, u64)) {
        test_hooks::PANIC_AT
            .lock()
            .expect("hooks")
            .push((config.db_path.clone(), key));
    }

    fn in_flight(offset: u64, text_len: usize) -> InFlightEvent {
        InFlightEvent {
            key: (1, offset),
            text_len,
        }
    }

    /// A partial event (the file's provisional last event, still growing)
    /// that crashes the preview at each length counts once per version: only
    /// one version crashing `MAX_CRASHES` times quarantines it (#50).
    #[test]
    fn crashes_on_a_growing_event_count_per_version() {
        let mut crashes = Crashes::new();
        for text_len in [10, 20, 30, 40] {
            note_crash(&mut crashes, in_flight(7, text_len));
        }
        assert!(
            quarantined(&crashes).is_empty(),
            "every version crashed once"
        );

        for _ in 1..MAX_CRASHES {
            note_crash(&mut crashes, in_flight(7, 40));
        }
        assert_eq!(quarantined(&crashes), vec![(1, 7)]);
    }

    #[test]
    fn a_poison_event_is_quarantined_after_repeated_crashes() {
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("supervisor-poison");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        // A league event: skipping it changes no match.
        let key = prepared_events(&fixture)
            .iter()
            .find(|event| {
                matches!(
                    event.kind,
                    EventKind::LeagueJoinRequest
                        | EventKind::LeagueJoined
                        | EventKind::LeaguePoolGranted
                        | EventKind::LeagueDropped
                )
            })
            .map(|event| (1_i64, event.offset))
            .expect("the fixture has a league event");
        poison(&config, key);

        let (sink, receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        let seen = wait_for(&receiver, Duration::from_secs(60), |events| {
            reported_quarantine(events) || restarts(events) > MAX_CRASHES as usize
        });
        handle.stop();

        assert_eq!(restarts(&seen), MAX_CRASHES as usize);
        assert_eq!(stored_row(&config, key), (None, true));
    }

    #[test]
    fn a_poison_provisional_event_is_quarantined_too() {
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("supervisor-poison-provisional");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        // The file's last event stays provisional (previewed) until the log has
        // been quiet for a moment, so it crashes the preview first.
        let last = split_complete(&fixture.bytes)
            .last()
            .map(|raw| raw.offset)
            .expect("the fixture has events");
        let key = (1_i64, last);
        poison(&config, key);

        let (sink, receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        let seen = wait_for(&receiver, Duration::from_secs(60), |events| {
            reported_quarantine(events) || restarts(events) > MAX_CRASHES as usize
        });
        handle.stop();

        assert_eq!(restarts(&seen), MAX_CRASHES as usize);
        assert_eq!(stored_row(&config, key), (None, true));
        assert_eq!(committed_cursor(&config), Some(fixture.bytes.len() as i64));
    }

    #[test]
    fn a_storage_error_restarts_from_the_committed_cursor() {
        let fixture = load_fixture("league_join_drop");
        let events = prepared_events(&fixture);
        let size = fixture.bytes.len() as i64;
        // Commit the first half, then fail the batch that carries the second.
        let cut = events[events.len() / 2].offset as usize;
        let dir = TestDir::new("supervisor-storage");
        dir.write("logs/mtgo.log", &fixture.bytes[..cut]);
        let config = config(&dir, &fixture);

        let (sink, receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        assert!(
            wait_for_cursor(&config, cut as i64),
            "the first half committed"
        );
        test_hooks::FAIL_NEXT_BATCH
            .lock()
            .expect("hooks")
            .push(config.db_path.clone());
        // Keep the next engine from opening until the cursor has been checked.
        test_hooks::HOLD_REOPEN
            .lock()
            .expect("hooks")
            .push(config.db_path.clone());
        dir.append("logs/mtgo.log", &fixture.bytes[cut..]);

        let mut seen = wait_for(&receiver, Duration::from_secs(60), |events| {
            restarts(events) >= 1
        });
        // The failed engine is gone and the next one is held before it opens.
        assert_eq!(
            committed_cursor(&config),
            Some(cut as i64),
            "the failed batch committed nothing, so the cursor stayed put"
        );
        test_hooks::release_reopen(&config.db_path);
        assert!(wait_for_cursor(&config, size), "the restart read the rest");
        handle.stop();
        seen.extend(receiver.try_iter());

        // A second restart would mean the restarted tracker re-stored events
        // (the primary key refuses duplicates) or failed again.
        assert_eq!(restarts(&seen), 1, "one restart, for the injected failure");
        assert_eq!(
            stored_event_count(&config),
            logged_event_count(&events),
            "every whitelisted event stored exactly once"
        );
        assert_eq!(quarantined_rows(&config), 0, "no event was blamed");
    }

    #[test]
    fn a_dropped_receiver_stops_the_tracker() {
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("supervisor-dropped");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);

        let (sink, receiver) = mpsc::channel();
        drop(receiver);
        let handle = spawn(config, sink).expect("spawn");
        let thread = handle.thread.as_ref().expect("thread");
        let deadline = Instant::now() + Duration::from_secs(30);
        while !thread.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(thread.is_finished(), "the tracker stopped on its own");
    }

    #[test]
    fn dropping_the_handle_commits_what_was_read() {
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("supervisor-drop-handle");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let size = fixture.bytes.len() as u64;

        let (sink, receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        let seen = wait_for(&receiver, Duration::from_secs(60), |events| {
            events.iter().any(|event| {
                matches!(event, TrackerEvent::Status(status)
                    if status.files.iter().any(|file| file.cursor == size))
            })
        });
        drop(handle);

        assert_eq!(restarts(&seen), 0);
        assert_eq!(committed_cursor(&config), Some(size as i64));
    }

    #[test]
    fn a_failure_while_stopping_is_not_reported_as_a_restart() {
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("supervisor-stop-failure");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let size = fixture.bytes.len() as i64;

        let (sink, receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        assert!(wait_for_cursor(&config, size), "caught up");
        test_hooks::FAIL_FINAL_FLUSH
            .lock()
            .expect("hooks")
            .push(config.db_path.clone());
        handle.stop();

        let seen: Vec<TrackerEvent> = receiver.try_iter().collect();
        assert!(
            !test_hooks::FAIL_FINAL_FLUSH
                .lock()
                .expect("hooks")
                .contains(&config.db_path),
            "the final flush failed"
        );
        assert_eq!(restarts(&seen), 0, "a stopping tracker does not restart");
    }

    fn outbox_row(subject: &str) -> NewOutboxRow {
        NewOutboxRow {
            kind: OutboxKind::TournamentObservation,
            subject: subject.to_string(),
            payload: Some(b"Qpayload".to_vec()),
            now_ms: 5_000,
        }
    }

    fn claim_edit() -> OutboxEdit {
        OutboxEdit::Claim {
            kind: OutboxKind::TournamentObservation,
            now_ms: 6_000,
            limit: 200,
        }
    }

    fn outbox_statuses(config: &TrackerConfig) -> Vec<String> {
        store::outbox::rows_for_test(&open_reader(&config.db_path).expect("reader"))
            .expect("rows")
            .into_iter()
            .map(|row| row.status)
            .collect()
    }

    #[test]
    fn an_outbox_edit_is_answered_with_its_outcome_once_committed() {
        let dir = TestDir::new("supervisor-outbox-edit");
        let config = league_config(&dir);
        let (sink, _receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        let editor = handle.outbox_editor();
        let rows = vec![outbox_row("1:10"), outbox_row("1:10")];
        assert_eq!(
            editor.edit(OutboxEdit::Enqueue { rows }),
            Ok(OutboxOutcome::Enqueued(1))
        );
        assert_eq!(outbox_statuses(&config), vec!["pending".to_string()]);
        let Ok(OutboxOutcome::Claimed(claimed)) = editor.edit(claim_edit()) else {
            panic!("claimed");
        };
        assert_eq!(claimed.len(), 1);
        assert_eq!(outbox_statuses(&config), vec!["sending".to_string()]);
        handle.stop();
    }

    #[test]
    fn an_outbox_edit_the_tracker_cannot_answer_times_out() {
        let (commands, _inbox) = mpsc::sync_channel(1);
        let editor = OutboxEditor { commands };
        let started = Instant::now();
        let answer = editor.edit_within(claim_edit(), Duration::from_millis(100));
        assert!(answer.is_err(), "{answer:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(OUTBOX_EDIT_TIMEOUT, Duration::from_secs(10));
    }

    // Review Focus 3: a claim the worker gave up on is never applied later,
    // so no row is left `sending` by a worker that is not waiting for it.
    #[test]
    fn a_claim_past_its_deadline_changes_nothing() {
        let dir = TestDir::new("supervisor-outbox-expired");
        let config = league_config(&dir);
        let mut engine = Engine::open(
            config.clone(),
            Env::current(),
            Arc::default(),
            Vec::new(),
            now_ms(),
        )
        .expect("engine");
        let rows = vec![outbox_row("1:10")];
        assert_eq!(
            engine
                .edit_outbox(OutboxEdit::Enqueue { rows })
                .expect("enqueue"),
            OutboxOutcome::Enqueued(1)
        );
        let (commands, inbox) = mpsc::sync_channel(1);
        let editor = OutboxEditor { commands };
        assert!(
            editor
                .edit_within(claim_edit(), Duration::from_millis(50))
                .is_err()
        );
        let late = inbox.try_recv().expect("queued");
        handle_real(&mut engine, late).expect("handled");
        assert_eq!(outbox_statuses(&config), vec!["pending".to_string()]);
    }

    fn last_error_edit(error: &str) -> SyncEdit {
        SyncEdit::LastError {
            error: Some(error.to_string()),
        }
    }

    fn stored_last_error(config: &TrackerConfig) -> Option<String> {
        store::sync::last_error(&open_reader(&config.db_path).expect("reader")).expect("error")
    }

    #[test]
    fn a_sync_edit_is_answered_once_committed() {
        let dir = TestDir::new("supervisor-sync-edit");
        let config = league_config(&dir);
        let (sink, _receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        let editor = handle.sync_editor();
        assert_eq!(editor.edit(last_error_edit("Qfailure")), Ok(1));
        assert_eq!(stored_last_error(&config).as_deref(), Some("Qfailure"));
        handle.stop();
    }

    #[test]
    fn a_sync_edit_the_tracker_cannot_answer_times_out() {
        let (commands, _inbox) = mpsc::sync_channel(1);
        let editor = SyncEditor { commands };
        let started = Instant::now();
        let answer = editor.edit_within(last_error_edit("Q"), Duration::from_millis(100));
        assert!(answer.is_err(), "{answer:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(SYNC_EDIT_TIMEOUT, Duration::from_secs(30));
    }

    #[test]
    fn a_sync_edit_past_its_deadline_changes_nothing() {
        let dir = TestDir::new("supervisor-sync-expired");
        let config = league_config(&dir);
        let mut engine = Engine::open(
            config.clone(),
            Env::current(),
            Arc::default(),
            Vec::new(),
            now_ms(),
        )
        .expect("engine");
        let (commands, inbox) = mpsc::sync_channel(1);
        let editor = SyncEditor { commands };
        assert!(
            editor
                .edit_within(last_error_edit("Qlate"), Duration::from_millis(50))
                .is_err()
        );
        let late = inbox.try_recv().expect("queued");
        handle_real(&mut engine, late).expect("handled");
        assert_eq!(stored_last_error(&config), None);
    }

    fn api_state() -> store::import_api::ImportedApiState {
        store::import_api::ImportedApiState {
            submitted: vec![store::import_api::LegacySubmitted {
                token: "q-0x".to_string(),
                mtgo_match_id: None,
                submitted_at_ms: 1_000,
            }],
            ..Default::default()
        }
    }

    fn report_status(config: &TrackerConfig) -> Option<String> {
        open_reader(&config.db_path)
            .expect("reader")
            .query_row(
                "SELECT status FROM outbox WHERE kind = 'match_report' AND subject = 'q-0x'",
                [],
                |row| row.get(0),
            )
            .ok()
    }

    #[test]
    fn an_api_import_is_answered_once_committed() {
        let dir = TestDir::new("supervisor-api-import");
        let config = league_config(&dir);
        let (sink, _receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        let counts = handle.importer().import_api(api_state()).expect("imported");
        assert_eq!(counts.reports_sent, 1);
        assert_eq!(report_status(&config).as_deref(), Some("sent"));
        handle.stop();
    }

    /// A 0.x manual archetype (Plan T task 14).
    fn archetype_data() -> ImportedArchetypes {
        ImportedArchetypes {
            archetypes: vec![store::import_archetypes::LegacyArchetype {
                legacy_id: 1,
                uuid: "qdev0001-q".to_string(),
                name: "Q Manual".to_string(),
                format: Some("modern".to_string()),
                format_hint: None,
                color_identity: None,
                source: store::import_archetypes::LegacySource::Manual,
                merged_into: None,
                source_match_token: None,
                incomplete: false,
                created_at_ms: Some(1_000),
                updated_at_ms: Some(1_000),
                lists: Vec::new(),
            }],
            ..Default::default()
        }
    }

    fn manual_archetypes(config: &TrackerConfig) -> i64 {
        open_reader(&config.db_path)
            .expect("reader")
            .query_row(
                "SELECT count(*) FROM archetypes WHERE uuid = 'qdev0001-q'",
                [],
                |row| row.get(0),
            )
            .expect("count")
    }

    #[test]
    fn an_archetype_import_is_answered_once_committed() {
        let dir = TestDir::new("supervisor-archetype-import");
        let config = league_config(&dir);
        let (sink, receiver) = mpsc::channel();
        let handle = spawn(config.clone(), sink).expect("spawn");
        let counts = handle
            .importer()
            .import_archetypes(archetype_data())
            .expect("imported");
        assert_eq!(counts.archetypes_added, 1);
        assert_eq!(manual_archetypes(&config), 1);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut told = false;
        while !told && Instant::now() < deadline {
            if let Ok(event) = receiver.recv_timeout(Duration::from_millis(100)) {
                told = matches!(event, TrackerEvent::ArchetypesChanged);
            }
        }
        assert!(told, "the archetype pages are told");
        handle.stop();
    }

    #[test]
    fn an_archetype_import_past_its_deadline_changes_nothing() {
        let dir = TestDir::new("supervisor-archetype-import-expired");
        let config = league_config(&dir);
        let mut engine = Engine::open(
            config.clone(),
            Env::current(),
            Arc::default(),
            Vec::new(),
            now_ms(),
        )
        .expect("engine");
        let (commands, inbox) = mpsc::sync_channel(1);
        let importer = Importer { commands };
        assert!(
            importer
                .import_archetypes_within(archetype_data(), Duration::from_millis(50))
                .is_err()
        );
        let late = inbox.try_recv().expect("queued");
        handle_real(&mut engine, late).expect("handled");
        assert_eq!(manual_archetypes(&config), 0);
    }

    #[test]
    fn an_api_import_past_its_deadline_changes_nothing() {
        let dir = TestDir::new("supervisor-api-import-expired");
        let config = league_config(&dir);
        let mut engine = Engine::open(
            config.clone(),
            Env::current(),
            Arc::default(),
            Vec::new(),
            now_ms(),
        )
        .expect("engine");
        let (commands, inbox) = mpsc::sync_channel(1);
        let importer = Importer { commands };
        assert!(
            importer
                .import_api_within(api_state(), Duration::from_millis(50))
                .is_err()
        );
        let late = inbox.try_recv().expect("queued");
        handle_real(&mut engine, late).expect("handled");
        assert_eq!(report_status(&config), None);
    }
}
