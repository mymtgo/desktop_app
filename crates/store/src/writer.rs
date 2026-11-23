//! The single writer (spec 3): one thread owns the write connection and commits
//! batches in the order they were sent. Callers never wait on it, except
//! `flush` (answering a lookup), `create_log_file` (new file id), `import`,
//! `import_accounts`, `import_decks`, `import_leagues`, `import_limited`,
//! `import_overlay_labels` and `import_detail` (a 0.x import's counts),
//! `import_history` and `delete_history_imports` (Plan M), `edit_deck`,
//! `edit_league`, `edit_match`, `edit_account`, `edit_manual`, `edit_draft_note` and
//! `edit_archetypes` (a user or worker edit's outcome), `sync_leagues` and
//! `sync_tournaments` (a sync's outcome) and
//! `edit_outbox` (the API worker's queue) and `edit_sync` (its sync state),
//! all rare.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, SyncSender};

use chrono_tz::Tz;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use cards::tokens::TokenCard;
use endstep_protocol::EndstepCardKey;

use crate::accounts::{AccountEdit, AccountEditOutcome, edit_account};
use crate::archetype_edit::{ArchetypeEdit, ArchetypeEditOutcome, edit_archetypes};
use crate::arena::{NewArenaLogFile, insert_arena_log_file};
use crate::arena_game_sync::{ArenaSyncOutcome, sync_arena_games};
use crate::cards::{
    CardsUpdate, ResolveOutcome, apply_resolutions, keep_present_images, log_matches_using,
    scan_missing, store_images, take_stubs_added,
};
use crate::decks::{DeckEdit, EditOutcome, edit_deck};
use crate::endstep_captures::{CaptureImportReport, NewCapture, store_captures};
use crate::endstep_cards::endstep_card_ids;
use crate::game_sync::{GameSyncOutcome, record_game_cards_version, sync_games};
use crate::history_import::{
    HistoryImportCounts, HistoryMatch, delete_history_imports, import_history,
};
use crate::import::{
    AccountImportCounts, ImportCounts, ImportedAccount, ImportedMatch, import_accounts,
    import_matches,
};
use crate::import_api::{ApiImportCounts, ImportedApiState, import_api_state};
use crate::import_archetypes::{ArchetypeImportCounts, ImportedArchetypes, import_archetypes};
use crate::import_decks::{DeckImportCounts, ImportedDecks, import_decks};
use crate::import_leagues::{ImportedLeague, LeagueImportCounts, import_leagues};
use crate::import_limited::{ImportedLimited, LimitedImportCounts, import_limited};
use crate::import_overlay::{LegacyDeckLabel, OverlayImportCounts, import_overlay_labels};
use crate::imported_detail::{DetailImport, ImportedDetail, import_detail};
use crate::league_sync::{LeagueOwner, SyncOutcome, sync_all, sync_owner};
use crate::leagues::{LeagueEdit, LeagueEditOutcome, edit_league};
use crate::limited::{DraftNoteEdit, DraftNoteOutcome, edit_draft_note};
use crate::manual::{ManualEdit, ManualEditOutcome, edit_manual};
use crate::match_edits::{MatchEdit, MatchEditOutcome, edit_match};
use crate::outbox::{OutboxEdit, OutboxOutcome, edit_outbox};
use crate::recorded_replays::{ImportReport, NewRecording, store_recordings};
use crate::sync::{SyncEdit, edit_sync};
use crate::write::{NewLogFile, WriteBatch, commit, insert_log_file};
use crate::{Connection, StoreError, open_writer};

/// Batches waiting for the writer before `write` blocks (bounded, spec 3).
pub const QUEUE: usize = 64;

/// What one sync cost on the writer thread: the rows it wrote (inserted,
/// updated or deleted) and how long it took, in microseconds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncWork {
    pub rows: u64,
    pub micros: u64,
}

/// The game sync's answer: what changed and what was skipped, and its cost.
pub type GameSyncReply = Result<(GameSyncOutcome, SyncWork), StoreError>;

/// The Arena match-end sync's answer: what changed and what was skipped,
/// and its cost.
pub type ArenaGameSyncReply = Result<(ArenaSyncOutcome, SyncWork), StoreError>;

enum Command {
    Write(Box<WriteBatch>),
    CreateFile(NewLogFile, mpsc::Sender<Result<i64, String>>),
    CreateArenaFile(NewArenaLogFile, mpsc::Sender<Result<i64, String>>),
    Import(
        Vec<ImportedMatch>,
        i64,
        mpsc::Sender<Result<ImportCounts, StoreError>>,
    ),
    ImportAccounts(
        Vec<ImportedAccount>,
        i64,
        mpsc::Sender<Result<AccountImportCounts, StoreError>>,
    ),
    ImportDecks(
        ImportedDecks,
        i64,
        mpsc::Sender<Result<DeckImportCounts, StoreError>>,
    ),
    ImportLeagues(
        Vec<ImportedLeague>,
        i64,
        mpsc::Sender<Result<LeagueImportCounts, StoreError>>,
    ),
    ImportLimited(
        Box<ImportedLimited>,
        i64,
        mpsc::Sender<Result<LimitedImportCounts, StoreError>>,
    ),
    ImportApi(
        Box<ImportedApiState>,
        i64,
        mpsc::Sender<Result<ApiImportCounts, StoreError>>,
    ),
    ImportArchetypes(
        Box<ImportedArchetypes>,
        i64,
        mpsc::Sender<Result<ArchetypeImportCounts, StoreError>>,
    ),
    ImportOverlayLabels(
        Vec<LegacyDeckLabel>,
        mpsc::Sender<Result<OverlayImportCounts, StoreError>>,
    ),
    ImportDetail(
        String,
        Box<ImportedDetail>,
        mpsc::Sender<Result<DetailImport, StoreError>>,
    ),
    SyncGames(
        Vec<String>,
        Vec<(String, Vec<u8>)>,
        Tz,
        mpsc::Sender<GameSyncReply>,
    ),
    SyncArenaGames(Vec<String>, i64, mpsc::Sender<ArenaGameSyncReply>),
    SyncLeagues(
        Vec<LeagueOwner>,
        bool,
        Tz,
        i64,
        mpsc::Sender<Result<(SyncOutcome, SyncWork), StoreError>>,
    ),
    SyncTournaments(i64, mpsc::Sender<Result<(bool, SyncWork), StoreError>>),
    RecordGameCardsVersion(mpsc::Sender<Result<(), StoreError>>),
    ImportHistory(
        Vec<HistoryMatch>,
        i64,
        mpsc::Sender<Result<HistoryImportCounts, StoreError>>,
    ),
    DeleteHistoryImports(mpsc::Sender<Result<Vec<String>, StoreError>>),
    EditDeck(DeckEdit, mpsc::Sender<Result<EditOutcome, StoreError>>),
    EditLeague(
        LeagueEdit,
        Tz,
        mpsc::Sender<Result<LeagueEditOutcome, StoreError>>,
    ),
    EditMatch(
        MatchEdit,
        mpsc::Sender<Result<MatchEditOutcome, StoreError>>,
    ),
    EditAccount(
        AccountEdit,
        mpsc::Sender<Result<AccountEditOutcome, StoreError>>,
    ),
    EditManual(
        ManualEdit,
        Tz,
        mpsc::Sender<Result<ManualEditOutcome, StoreError>>,
    ),
    EditDraftNote(
        DraftNoteEdit,
        mpsc::Sender<Result<DraftNoteOutcome, StoreError>>,
    ),
    EditArchetypes(
        Box<ArchetypeEdit>,
        mpsc::Sender<Result<ArchetypeEditOutcome, StoreError>>,
    ),
    EditOutbox(OutboxEdit, mpsc::Sender<Result<OutboxOutcome, StoreError>>),
    EditSync(SyncEdit, mpsc::Sender<Result<u32, StoreError>>),
    ScanCardStubs(mpsc::Sender<Result<u32, StoreError>>),
    EnsureCardStubs(Vec<u32>, mpsc::Sender<Result<u32, StoreError>>),
    EnsureTokenStubs(Vec<TokenCard>, mpsc::Sender<Result<u32, StoreError>>),
    ResolveCards(
        Box<CardsUpdate>,
        mpsc::Sender<Result<(ResolveOutcome, Vec<String>), StoreError>>,
    ),
    StoreCardImages(StoredImages, mpsc::Sender<Result<u32, StoreError>>),
    KeepPresentImages(BTreeSet<String>, mpsc::Sender<Result<u32, StoreError>>),
    RegenerateCardStats(
        i64,
        mpsc::Sender<Result<crate::card_stats::Regenerated, StoreError>>,
    ),
    StoreRecordings(
        Vec<NewRecording>,
        i64,
        mpsc::Sender<Result<ImportReport, StoreError>>,
    ),
    StoreEndstepCaptures(
        Vec<NewCapture>,
        i64,
        mpsc::Sender<Result<CaptureImportReport, StoreError>>,
    ),
    EndstepCardIds(
        Vec<EndstepCardKey>,
        i64,
        mpsc::Sender<Result<Vec<u32>, StoreError>>,
    ),
    Flush(mpsc::Sender<()>),
}

/// A card's local files to record (`cards::store_images`): `None` leaves a
/// column as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredImages {
    pub catalog_id: u32,
    pub local_image: Option<String>,
    pub local_art_crop: Option<String>,
}

pub struct WriterHandle {
    sender: Option<SyncSender<Command>>,
    thread: Option<JoinHandle<()>>,
    failure: Arc<Mutex<Option<String>>>,
    /// Card stubs committed since the tracker last asked (`take_stubs_added`).
    stubs_added: Arc<AtomicU32>,
}

/// Open the database (running migrations) and start the writer thread.
pub fn spawn_writer(path: &Path) -> Result<WriterHandle, StoreError> {
    let conn = open_writer(path)?;
    let (sender, receiver) = mpsc::sync_channel(QUEUE);
    let failure = Arc::new(Mutex::new(None));
    let thread_failure = Arc::clone(&failure);
    let stubs_added = Arc::new(AtomicU32::new(0));
    let thread_stubs = Arc::clone(&stubs_added);
    let thread = std::thread::Builder::new()
        .name("store-writer".to_string())
        .spawn(move || run(conn, receiver, thread_failure, thread_stubs))?;
    Ok(WriterHandle {
        sender: Some(sender),
        thread: Some(thread),
        failure,
        stubs_added,
    })
}

/// Text reported when a panic poisoned the failure lock.
const POISONED: &str = "the writer's failure record was poisoned by a panic";

/// The first batch failure, if any. A poisoned lock counts as a failure: the
/// thread holding it panicked, so nothing after it can be trusted.
fn read_failure(failure: &Mutex<Option<String>>) -> Option<String> {
    match failure.lock() {
        Ok(slot) => slot.clone(),
        Err(poisoned) => Some(
            poisoned
                .into_inner()
                .clone()
                .unwrap_or_else(|| POISONED.to_string()),
        ),
    }
}

fn run(
    mut conn: Connection,
    receiver: Receiver<Command>,
    failure: Arc<Mutex<Option<String>>>,
    stubs_added: Arc<AtomicU32>,
) {
    for command in receiver {
        run_command(&mut conn, command, &failure);
        // Whatever command it was, the stubs it committed are reported (a
        // rolled back write counted none, `store::cards`). A count that
        // cannot be read is logged: the stubs are stored, and the next
        // population run (hourly, or the full scan) still finds them.
        if let Err(error) = report_stubs_added(&conn, &stubs_added) {
            tracing::warn!(%error, "could not count the card stubs a write added");
        }
    }
}

/// Add the stubs this connection committed since the last call to
/// `stubs_added`, and return how many that was.
fn report_stubs_added(conn: &Connection, stubs_added: &AtomicU32) -> Result<u32, StoreError> {
    let added = take_stubs_added(conn)?;
    if added > 0 {
        stubs_added.fetch_add(added, Ordering::SeqCst);
    }
    Ok(added)
}

fn run_command(conn: &mut Connection, command: Command, failure: &Mutex<Option<String>>) {
    // After a failure every later batch and new file is refused: a batch's
    // cursor would skip the events the failed batch lost, and the tracker
    // restarts anyway.
    let failed_before = read_failure(failure);
    match command {
        Command::Write(batch) => {
            if failed_before.is_some() {
                return;
            }
            if let Err(error) = commit(conn, &batch)
                && let Ok(mut slot) = failure.lock()
            {
                *slot = Some(error.to_string());
            }
        }
        Command::CreateFile(file, reply) => {
            let result = match failed_before {
                Some(message) => Err(message),
                None => insert_log_file(conn, &file).map_err(|error| error.to_string()),
            };
            let _ = reply.send(result);
        }
        Command::CreateArenaFile(file, reply) => {
            let result = match failed_before {
                Some(message) => Err(message),
                None => insert_arena_log_file(conn, &file).map_err(|error| error.to_string()),
            };
            let _ = reply.send(result);
        }
        // An import is its own transaction and holds no cursor, so its
        // failure is answered to the caller and does not stop the writer.
        Command::Import(matches, now_ms, reply) => answer(&failed_before, reply, || {
            import_matches(conn, &matches, now_ms)
        }),
        Command::ImportAccounts(accounts, now_ms, reply) => answer(&failed_before, reply, || {
            import_accounts(conn, &accounts, now_ms)
        }),
        Command::ImportDecks(data, now_ms, reply) => {
            answer(&failed_before, reply, || import_decks(conn, &data, now_ms))
        }
        Command::ImportLeagues(leagues, now_ms, reply) => answer(&failed_before, reply, || {
            import_leagues(conn, &leagues, now_ms)
        }),
        Command::ImportLimited(data, now_ms, reply) => answer(&failed_before, reply, || {
            import_limited(conn, &data, now_ms)
        }),
        Command::ImportApi(state, now_ms, reply) => answer(&failed_before, reply, || {
            import_api_state(conn, &state, now_ms)
        }),
        Command::ImportArchetypes(data, now_ms, reply) => answer(&failed_before, reply, || {
            import_archetypes(conn, &data, now_ms)
        }),
        Command::ImportOverlayLabels(labels, reply) => answer(&failed_before, reply, || {
            import_overlay_labels(conn, &labels)
        }),
        Command::ImportDetail(token, detail, reply) => answer(&failed_before, reply, || {
            import_detail(conn, &token, &detail)
        }),
        // A game sync is its own transaction and holds no cursor: its
        // failure is answered and does not stop the writer.
        Command::SyncGames(tokens, dat, tz, reply) => answer(&failed_before, reply, || {
            measured(conn, |conn| sync_games(conn, &tokens, &dat, tz))
        }),
        Command::SyncArenaGames(tokens, now_ms, reply) => answer(&failed_before, reply, || {
            measured(conn, |conn| sync_arena_games(conn, &tokens, now_ms))
        }),
        // The league sync is its own transaction too.
        Command::SyncLeagues(owners, all, tz, now_ms, reply) => {
            answer(&failed_before, reply, || {
                measured(conn, |conn| sync_leagues(conn, &owners, all, tz, now_ms))
            })
        }
        // The tournament sync is its own transaction too.
        Command::SyncTournaments(now_ms, reply) => answer(&failed_before, reply, || {
            measured(conn, |conn| crate::tournament_sync::sync_all(conn, now_ms))
        }),
        Command::RecordGameCardsVersion(reply) => {
            answer(&failed_before, reply, || record_game_cards_version(conn))
        }
        Command::ImportHistory(matches, now_ms, reply) => answer(&failed_before, reply, || {
            import_history(conn, &matches, now_ms)
        }),
        Command::DeleteHistoryImports(reply) => {
            answer(&failed_before, reply, || delete_history_imports(conn))
        }
        // A user edit is its own transaction, answered like an import.
        Command::EditDeck(edit, reply) => answer(&failed_before, reply, || edit_deck(conn, &edit)),
        // `tz` is for the league sync the edit runs.
        Command::EditLeague(edit, tz, reply) => {
            answer(&failed_before, reply, || edit_league(conn, &edit, tz))
        }
        Command::EditMatch(edit, reply) => {
            answer(&failed_before, reply, || edit_match(conn, &edit))
        }
        Command::EditAccount(edit, reply) => {
            answer(&failed_before, reply, || edit_account(conn, &edit))
        }
        Command::EditManual(edit, tz, reply) => {
            answer(&failed_before, reply, || edit_manual(conn, &edit, tz))
        }
        Command::EditDraftNote(edit, reply) => {
            answer(&failed_before, reply, || edit_draft_note(conn, &edit))
        }
        Command::EditArchetypes(edit, reply) => {
            answer(&failed_before, reply, || edit_archetypes(conn, &edit))
        }
        Command::EditOutbox(edit, reply) => {
            answer(&failed_before, reply, || edit_outbox(conn, &edit))
        }
        Command::EditSync(edit, reply) => answer(&failed_before, reply, || edit_sync(conn, &edit)),
        Command::ScanCardStubs(reply) => answer(&failed_before, reply, || scan_card_stubs(conn)),
        Command::EnsureCardStubs(catalog_ids, reply) => answer(&failed_before, reply, || {
            ensure_card_stubs(conn, &catalog_ids)
        }),
        Command::EnsureTokenStubs(tokens, reply) => answer(&failed_before, reply, || {
            in_transaction(conn, |tx| crate::cards::ensure_token_stubs(tx, &tokens))
        }),
        Command::ResolveCards(update, reply) => {
            answer(&failed_before, reply, || resolve_cards(conn, &update))
        }
        Command::StoreCardImages(stored, reply) => answer(&failed_before, reply, || {
            in_transaction(conn, |tx| {
                store_images(
                    tx,
                    stored.catalog_id,
                    stored.local_image.as_deref(),
                    stored.local_art_crop.as_deref(),
                )
            })
        }),
        Command::StoreRecordings(recordings, now_ms, reply) => {
            answer(&failed_before, reply, || {
                in_transaction(conn, |tx| store_recordings(tx, &recordings, now_ms))
            })
        }
        Command::StoreEndstepCaptures(captures, now_ms, reply) => {
            answer(&failed_before, reply, || {
                in_transaction(conn, |tx| store_captures(tx, &captures, now_ms))
            })
        }
        Command::EndstepCardIds(keys, now_ms, reply) => answer(&failed_before, reply, || {
            in_transaction(conn, |tx| endstep_card_ids(tx, &keys, now_ms))
        }),
        Command::KeepPresentImages(present, reply) => answer(&failed_before, reply, || {
            in_transaction(conn, |tx| keep_present_images(tx, &present))
        }),
        Command::RegenerateCardStats(deck_id, reply) => answer(&failed_before, reply, || {
            in_transaction(conn, |tx| crate::card_stats::regenerate(tx, deck_id))
        }),
        Command::Flush(reply) => {
            let _ = reply.send(());
        }
    }
}

/// Answer a command that writes in its own transaction: refused with
/// `Failed` after a batch failure (`failed_before`), otherwise `work` runs
/// and its result is the answer.
fn answer<T>(
    failed_before: &Option<String>,
    reply: mpsc::Sender<Result<T, StoreError>>,
    work: impl FnOnce() -> Result<T, StoreError>,
) {
    let result = match failed_before {
        Some(message) => Err(StoreError::Failed(message.clone())),
        None => work(),
    };
    let _ = reply.send(result);
}

/// One write in its own transaction.
fn in_transaction<T>(
    conn: &mut Connection,
    write: impl FnOnce(&rusqlite::Transaction) -> Result<T, StoreError>,
) -> Result<T, StoreError> {
    let tx = conn.transaction()?;
    let value = write(&tx)?;
    tx.commit()?;
    Ok(value)
}

/// The full card stub scan in its own transaction (`cards::scan_missing`).
fn scan_card_stubs(conn: &mut Connection) -> Result<u32, StoreError> {
    let tx = conn.transaction()?;
    let added = scan_missing(&tx)?;
    tx.commit()?;
    Ok(added)
}

/// Stubs for the given catalog ids in their own transaction.
fn ensure_card_stubs(conn: &mut Connection, catalog_ids: &[u32]) -> Result<u32, StoreError> {
    let tx = conn.transaction()?;
    let added = crate::cards::ensure_id_stubs(&tx, catalog_ids)?;
    tx.commit()?;
    Ok(added)
}

/// Apply a population run in its own transaction (`cards::apply_resolutions`)
/// and find, in it, the log matches whose game rows read a changed card.
fn resolve_cards(
    conn: &mut Connection,
    update: &CardsUpdate,
) -> Result<(ResolveOutcome, Vec<String>), StoreError> {
    let tx = conn.transaction()?;
    let outcome = apply_resolutions(&tx, update)?;
    let resync = log_matches_using(&tx, &outcome.changed)?;
    tx.commit()?;
    Ok((outcome, resync))
}

/// Run one sync on the writer's connection and measure it.
fn measured<T>(
    conn: &mut Connection,
    sync: impl FnOnce(&mut Connection) -> Result<T, StoreError>,
) -> Result<(T, SyncWork), StoreError> {
    let started = Instant::now();
    let before = conn.total_changes();
    let value = sync(conn)?;
    let work = SyncWork {
        rows: conn.total_changes().saturating_sub(before),
        micros: started.elapsed().as_micros() as u64,
    };
    Ok((value, work))
}

/// The league sync of these owners in one transaction, or of every owner
/// (`league_sync::sync_all`, which also records the fold version and zone).
fn sync_leagues(
    conn: &mut Connection,
    owners: &[LeagueOwner],
    all: bool,
    tz: Tz,
    now_ms: i64,
) -> Result<SyncOutcome, StoreError> {
    if all {
        return sync_all(conn, tz, now_ms);
    }
    let tx = conn.transaction()?;
    let mut out = SyncOutcome::default();
    for owner in owners {
        let one = sync_owner(&tx, *owner, tz, now_ms)?;
        out.leagues_changed |= one.leagues_changed;
        out.limited_changed |= one.limited_changed;
        out.next_abandon_at = match (out.next_abandon_at, one.next_abandon_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        out.skipped_snapshots.extend(one.skipped_snapshots);
    }
    tx.commit()?;
    Ok(out)
}

impl WriterHandle {
    /// Queue a batch. Empty batches are skipped.
    pub fn write(&self, batch: WriteBatch) -> Result<(), StoreError> {
        if batch.is_empty() {
            return Ok(());
        }
        self.send(Command::Write(Box::new(batch)))
    }

    /// Insert a `log_files` row and return its id (waits for the writer).
    /// Refused with `Failed` after a batch failure.
    pub fn create_log_file(&self, file: NewLogFile) -> Result<i64, StoreError> {
        self.queue(|reply| Command::CreateFile(file, reply))?
            .recv()
            .map_err(|_| StoreError::WriterGone)?
            .map_err(StoreError::Failed)
    }

    /// Insert an `arena_log_files` row and return its id (waits for the
    /// writer). Refused with `Failed` after a batch failure.
    pub fn create_arena_log_file(&self, file: NewArenaLogFile) -> Result<i64, StoreError> {
        self.queue(|reply| Command::CreateArenaFile(file, reply))?
            .recv()
            .map_err(|_| StoreError::WriterGone)?
            .map_err(StoreError::Failed)
    }

    /// Import 0.x matches in one transaction and return the counts (waits for
    /// the writer). Refused with `Failed` after a batch failure.
    pub fn import(
        &self,
        matches: Vec<ImportedMatch>,
        now_ms: i64,
    ) -> Result<ImportCounts, StoreError> {
        self.ask(|reply| Command::Import(matches, now_ms, reply))
    }

    /// Register the 0.x accounts and their login ids (Plan O) in one
    /// transaction and return the counts (waits for the writer). Refused
    /// with `Failed` after a batch failure.
    pub fn import_accounts(
        &self,
        accounts: Vec<ImportedAccount>,
        now_ms: i64,
    ) -> Result<AccountImportCounts, StoreError> {
        self.ask(|reply| Command::ImportAccounts(accounts, now_ms, reply))
    }

    /// Import 0.x cards, decks, versions and match links in one transaction
    /// and return the counts (waits for the writer). Send it after the
    /// matches, so the links find their rows. Refused with `Failed` after a
    /// batch failure.
    pub fn import_decks(
        &self,
        data: ImportedDecks,
        now_ms: i64,
    ) -> Result<DeckImportCounts, StoreError> {
        self.ask(|reply| Command::ImportDecks(data, now_ms, reply))
    }

    /// Import 0.x leagues in one transaction and return the counts (waits for
    /// the writer). Send it after the matches and the decks, so members and
    /// deck versions are found. Refused with `Failed` after a batch failure.
    pub fn import_leagues(
        &self,
        leagues: Vec<ImportedLeague>,
        now_ms: i64,
    ) -> Result<LeagueImportCounts, StoreError> {
        self.ask(|reply| Command::ImportLeagues(leagues, now_ms, reply))
    }

    /// Import 0.x drafts, picks, notes and limited registered decks in one
    /// transaction and return the counts (waits for the writer). Send it
    /// after the leagues, so drafts and decks find their runs. Refused with
    /// `Failed` after a batch failure.
    pub fn import_limited(
        &self,
        data: ImportedLimited,
        now_ms: i64,
    ) -> Result<LimitedImportCounts, StoreError> {
        self.ask(|reply| Command::ImportLimited(Box::new(data), now_ms, reply))
    }

    /// Import 0.x's API state (07 spec section 11) in one transaction and
    /// return the counts (waits for the writer). Send it after the matches,
    /// decks and leagues, so their rows are found. Refused with `Failed`
    /// after a batch failure.
    pub fn import_api(
        &self,
        state: ImportedApiState,
        now_ms: i64,
    ) -> Result<ApiImportCounts, StoreError> {
        self.ask(|reply| Command::ImportApi(Box::new(state), now_ms, reply))
    }

    /// Import 0.x's archetypes, match archetype rows, deck archetypes,
    /// sideboard guides and matchup notes (archetypes spec sections 15 and
    /// 21.5) in one transaction and return the counts (waits for the
    /// writer). Send it after the matches and decks, so their rows are found.
    /// Refused with `Failed` after a batch failure.
    pub fn import_archetypes(
        &self,
        data: ImportedArchetypes,
        now_ms: i64,
    ) -> Result<ArchetypeImportCounts, StoreError> {
        self.ask(|reply| Command::ImportArchetypes(Box::new(data), now_ms, reply))
    }

    /// Import 0.x's overlay deck labels (overlay spec section 12) in one
    /// transaction and return the counts (waits for the writer). Send it
    /// after the decks, so the labels find their decks. Refused with
    /// `Failed` after a batch failure.
    pub fn import_overlay_labels(
        &self,
        labels: Vec<LegacyDeckLabel>,
    ) -> Result<OverlayImportCounts, StoreError> {
        self.ask(|reply| Command::ImportOverlayLabels(labels, reply))
    }

    /// Store one imported 0.x match's detail (Plan O) in its own
    /// transaction and return what it did (waits for the writer). Send it
    /// after the matches, so it finds its row. Refused with `Failed` after a
    /// batch failure.
    pub fn import_detail(
        &self,
        token: String,
        detail: ImportedDetail,
    ) -> Result<DetailImport, StoreError> {
        self.ask(|reply| Command::ImportDetail(token, Box::new(detail), reply))
    }

    /// Queue a game sync (`game_sync::sync_games`) of the log matches with
    /// these tokens, each with its `.dat` file when one is given, in one
    /// transaction after everything queued before it. Does not wait: the
    /// returned receiver gets whether anything changed and the sync's cost,
    /// or the error.
    pub fn sync_games(
        &self,
        tokens: Vec<String>,
        dat: Vec<(String, Vec<u8>)>,
        tz: Tz,
    ) -> Result<Receiver<GameSyncReply>, StoreError> {
        self.queue(|reply| Command::SyncGames(tokens, dat, tz, reply))
    }

    /// Queue the Arena match-end sync (`arena_game_sync::sync_arena_games`)
    /// of these matches in one transaction after everything queued before
    /// it (the batch with their results). Does not wait: the returned
    /// receiver gets what changed and the sync's cost, or the error.
    pub fn sync_arena_games(
        &self,
        tokens: Vec<String>,
        now_ms: i64,
    ) -> Result<Receiver<ArenaGameSyncReply>, StoreError> {
        self.queue(|reply| Command::SyncArenaGames(tokens, now_ms, reply))
    }

    /// Run the league sync (`league_sync::sync_owner`) of these owners in one
    /// transaction after everything queued before it, or of every owner when
    /// `all` (`league_sync::sync_all`), and return what changed and the
    /// sync's cost (waits for the writer: the caller tells the pages from the
    /// outcome). `tz` is the OS timezone; `now_ms` decides which open drafts
    /// are abandoned. Refused with `Failed` after a batch failure.
    pub fn sync_leagues(
        &self,
        owners: Vec<LeagueOwner>,
        all: bool,
        tz: Tz,
        now_ms: i64,
    ) -> Result<(SyncOutcome, SyncWork), StoreError> {
        self.ask(|reply| Command::SyncLeagues(owners, all, tz, now_ms, reply))
    }

    /// Run the tournament sync (`tournament_sync::sync_all`) after everything
    /// queued before it, and return whether a stored standing changed and
    /// the sync's cost (waits for the writer). Refused with `Failed` after a
    /// batch failure.
    pub fn sync_tournaments(&self, now_ms: i64) -> Result<(bool, SyncWork), StoreError> {
        self.ask(|reply| Command::SyncTournaments(now_ms, reply))
    }

    /// Queue the record that every log match's games were synced at this
    /// build's `GAME_CARDS_VERSION` (the end of the backfill). Does not wait.
    pub fn record_game_cards_version(
        &self,
    ) -> Result<Receiver<Result<(), StoreError>>, StoreError> {
        self.queue(Command::RecordGameCardsVersion)
    }

    /// Store a batch of MTGO history matches in one transaction and return
    /// the counts (waits for the writer). Refused with `Failed` after a batch
    /// failure.
    pub fn import_history(
        &self,
        matches: Vec<HistoryMatch>,
        now_ms: i64,
    ) -> Result<HistoryImportCounts, StoreError> {
        self.ask(|reply| Command::ImportHistory(matches, now_ms, reply))
    }

    /// Remove every history import in one transaction and return the removed
    /// tokens (waits for the writer). Refused with `Failed` after a batch
    /// failure.
    pub fn delete_history_imports(&self) -> Result<Vec<String>, StoreError> {
        self.ask(Command::DeleteHistoryImports)
    }

    /// Apply a user's deck edit in its own transaction and return whether it
    /// was done or refused (waits for the writer). Refused with `Failed` after
    /// a batch failure.
    pub fn edit_deck(&self, edit: DeckEdit) -> Result<EditOutcome, StoreError> {
        self.ask(|reply| Command::EditDeck(edit, reply))
    }

    /// Apply a user's league edit in its own transaction and return whether
    /// it was done or refused (waits for the writer). `tz` is the OS
    /// timezone, for the league sync an edit of fold input runs. Refused with
    /// `Failed` after a batch failure.
    pub fn edit_league(&self, edit: LeagueEdit, tz: Tz) -> Result<LeagueEditOutcome, StoreError> {
        self.ask(|reply| Command::EditLeague(edit, tz, reply))
    }

    /// Apply a user's match edit (notes, hide) in its own transaction and
    /// return whether it was done or refused (waits for the writer). Refused
    /// with `Failed` after a batch failure.
    pub fn edit_match(&self, edit: MatchEdit) -> Result<MatchEditOutcome, StoreError> {
        self.ask(|reply| Command::EditMatch(edit, reply))
    }

    /// Apply a user's account edit (switch, tracking) in its own transaction
    /// and return whether it was done or refused (waits for the writer).
    /// Refused with `Failed` after a batch failure.
    pub fn edit_account(&self, edit: AccountEdit) -> Result<AccountEditOutcome, StoreError> {
        self.ask(|reply| Command::EditAccount(edit, reply))
    }

    /// Apply a user's manual match edit (create, hand, sideboard, reveals) in
    /// its own transaction and return whether it was done or refused (waits
    /// for the writer). `tz` is the OS timezone, for the league sync a
    /// placement runs. Refused with `Failed` after a batch failure.
    pub fn edit_manual(&self, edit: ManualEdit, tz: Tz) -> Result<ManualEditOutcome, StoreError> {
        self.ask(|reply| Command::EditManual(edit, tz, reply))
    }

    /// Apply a user's pick note edit in its own transaction and return
    /// whether it was done or refused (waits for the writer). Refused with
    /// `Failed` after a batch failure.
    pub fn edit_draft_note(&self, edit: DraftNoteEdit) -> Result<DraftNoteOutcome, StoreError> {
        self.ask(|reply| Command::EditDraftNote(edit, reply))
    }

    /// Apply an archetype edit in its own transaction and return whether it
    /// was done or refused (waits for the writer). Refused with `Failed`
    /// after a batch failure.
    pub fn edit_archetypes(&self, edit: ArchetypeEdit) -> Result<ArchetypeEditOutcome, StoreError> {
        self.ask(|reply| Command::EditArchetypes(Box::new(edit), reply))
    }

    /// Apply an API worker's outbox edit in its own transaction and return
    /// what it did (waits for the writer). Refused with `Failed` after a
    /// batch failure.
    pub fn edit_outbox(&self, edit: OutboxEdit) -> Result<OutboxOutcome, StoreError> {
        self.ask(|reply| Command::EditOutbox(edit, reply))
    }

    /// Apply an API worker's sync edit in its own transaction and return how
    /// many rows it changed (waits for the writer). Refused with `Failed`
    /// after a batch failure.
    pub fn edit_sync(&self, edit: SyncEdit) -> Result<u32, StoreError> {
        self.ask(|reply| Command::EditSync(edit, reply))
    }

    /// Write a stub for every stored catalog id without a `cards` row (the
    /// full scan) and return how many were added (waits for the writer).
    /// Refused with `Failed` after a batch failure.
    pub fn scan_card_stubs(&self) -> Result<u32, StoreError> {
        self.ask(Command::ScanCardStubs)
    }

    /// Write a stub for each given catalog id without a `cards` row and
    /// return how many were added (waits for the writer). Refused with
    /// `Failed` after a batch failure.
    pub fn ensure_card_stubs(&self, catalog_ids: Vec<u32>) -> Result<u32, StoreError> {
        self.ask(|reply| Command::EnsureCardStubs(catalog_ids, reply))
    }

    /// Write named token stubs (`cards::ensure_token_stubs`) and return how
    /// many rows were added (waits for the writer). Refused with `Failed`
    /// after a batch failure.
    pub fn ensure_token_stubs(&self, tokens: Vec<TokenCard>) -> Result<u32, StoreError> {
        self.ask(|reply| Command::EnsureTokenStubs(tokens, reply))
    }

    /// Apply a population run (token names, then the API's answers) and
    /// return what it did with the log matches to sync again (waits for the
    /// writer). Refused with `Failed` after a batch failure.
    pub fn resolve_cards(
        &self,
        update: CardsUpdate,
    ) -> Result<(ResolveOutcome, Vec<String>), StoreError> {
        self.ask(|reply| Command::ResolveCards(Box::new(update), reply))
    }

    /// Record a card's downloaded or copied files and return the rows
    /// written (waits for the writer). Refused with `Failed` after a batch
    /// failure.
    pub fn store_card_images(&self, stored: StoredImages) -> Result<u32, StoreError> {
        self.ask(|reply| Command::StoreCardImages(stored, reply))
    }

    /// Store recorder files in one transaction and return what each did
    /// (waits for the writer). Refused with `Failed` after a batch failure.
    pub fn store_recordings(
        &self,
        recordings: Vec<NewRecording>,
        now_ms: i64,
    ) -> Result<ImportReport, StoreError> {
        self.ask(|reply| Command::StoreRecordings(recordings, now_ms, reply))
    }

    /// Store Endstep capture files in one transaction and return what each
    /// did (waits for the writer). Refused with `Failed` after a batch
    /// failure.
    pub fn store_endstep_captures(
        &self,
        captures: Vec<NewCapture>,
        now_ms: i64,
    ) -> Result<CaptureImportReport, StoreError> {
        self.ask(|reply| Command::StoreEndstepCaptures(captures, now_ms, reply))
    }

    /// The catalog id of each Endstep card key, in input order, adding a
    /// row and a named `cards` stub for a key not seen before
    /// (`endstep_cards::endstep_card_ids`; waits for the writer). Refused
    /// with `Failed` after a batch failure.
    pub fn endstep_card_ids(
        &self,
        keys: Vec<EndstepCardKey>,
        now_ms: i64,
    ) -> Result<Vec<u32>, StoreError> {
        self.ask(|reply| Command::EndstepCardIds(keys, now_ms, reply))
    }

    /// Forget the local files not in `present` and return the columns
    /// nulled (waits for the writer). Refused with `Failed` after a batch
    /// failure.
    pub fn keep_present_images(&self, present: BTreeSet<String>) -> Result<u32, StoreError> {
        self.ask(|reply| Command::KeepPresentImages(present, reply))
    }

    /// "Regenerate" a deck's card stats (`card_stats::regenerate`) in one
    /// transaction and return the log matches to sync again (waits for the
    /// writer). Refused with `Failed` after a batch failure.
    pub fn regenerate_card_stats(
        &self,
        deck_id: i64,
    ) -> Result<crate::card_stats::Regenerated, StoreError> {
        self.ask(|reply| Command::RegenerateCardStats(deck_id, reply))
    }

    /// The card stubs committed since the last call, by any write (counted
    /// after each command; a `flush` first makes the count include
    /// everything sent so far).
    pub fn take_stubs_added(&self) -> u32 {
        self.stubs_added.swap(0, Ordering::SeqCst)
    }

    /// Wait until everything sent so far is committed (or has failed).
    pub fn flush(&self) -> Result<(), StoreError> {
        self.queue(Command::Flush)?
            .recv()
            .map_err(|_| StoreError::WriterGone)?;
        match self.failure() {
            Some(message) => Err(StoreError::Failed(message)),
            None => Ok(()),
        }
    }

    /// The first batch failure, if any (a poisoned failure lock counts as one).
    pub fn failure(&self) -> Option<String> {
        read_failure(&self.failure)
    }

    /// Send the command `make` builds around a new reply channel and return
    /// the channel's receiving end, without waiting for the answer.
    fn queue<R>(
        &self,
        make: impl FnOnce(mpsc::Sender<R>) -> Command,
    ) -> Result<Receiver<R>, StoreError> {
        let (reply, answer) = mpsc::channel();
        self.send(make(reply))?;
        Ok(answer)
    }

    /// Send the command `make` builds and wait for the writer's answer.
    /// `WriterGone` when the writer stopped before answering.
    fn ask<T>(
        &self,
        make: impl FnOnce(mpsc::Sender<Result<T, StoreError>>) -> Command,
    ) -> Result<T, StoreError> {
        self.queue(make)?
            .recv()
            .map_err(|_| StoreError::WriterGone)?
    }

    fn send(&self, command: Command) -> Result<(), StoreError> {
        self.sender
            .as_ref()
            .ok_or(StoreError::WriterGone)?
            .send(command)
            .map_err(|_| StoreError::WriterGone)
    }
}

impl Drop for WriterHandle {
    /// Close the queue, let the thread commit what is left, and wait for it.
    fn drop(&mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::leagues::{DeleteMatches, LeagueEdit, LeagueEditOutcome, UserState};
    use crate::match_edits::{MatchEdit, MatchEditOutcome};
    use crate::test_support::*;
    use crate::write::WriteBatch;
    use crate::{open_reader, open_writer};
    use fixture_tools::TestDir;

    #[test]
    fn endstep_card_ids_come_through_the_writer() {
        use endstep_protocol::EndstepCardKey;
        let dir = TestDir::new("writer-endstep-card-ids");
        let path = dir.join("mymtgo.sqlite");
        let writer = spawn_writer(&path).expect("writer");
        let bolt = EndstepCardKey {
            name: "Lightning Bolt".to_string(),
            set_code: "m10".to_string(),
            collector_number: "146".to_string(),
        };
        let first = writer
            .endstep_card_ids(vec![bolt.clone()], 5)
            .expect("first");
        let second = writer.endstep_card_ids(vec![bolt], 6).expect("second");
        assert_eq!(first, second);
        assert_eq!(first, vec![cards::endstep_catalog_id(1)]);
        writer.flush().expect("flush");
        let reader = open_reader(&path).expect("reader");
        let name: String = reader
            .query_row(
                "SELECT name FROM cards WHERE catalog_id = ?1",
                [first[0]],
                |row| row.get(0),
            )
            .expect("the stub");
        assert_eq!(name, "Lightning Bolt");
    }

    #[test]
    fn endstep_captures_are_stored_through_the_writer() {
        use crate::endstep_captures::{CaptureImportReport, NewCapture, capture_count};
        use endstep_capture::{CaptureHeader, Classification, UnitKind};
        let dir = TestDir::new("writer-store-endstep-captures");
        let path = dir.join("mymtgo.sqlite");
        let writer = spawn_writer(&path).expect("writer");
        let batch = vec![NewCapture {
            header: CaptureHeader {
                format: 1,
                unit_id: "0f8fad5b-d9cb-469f-a165-70867728950e".to_string(),
                kind: UnitKind::Session,
                started_at: 1,
                counter: 1,
                extension: "0.1.0".to_string(),
                browser: "chrome".to_string(),
                developer: true,
                classification: Classification::Unknown,
            },
            frame_count: 0,
            text: "a".to_string(),
            file_hash: "h1".to_string(),
        }];
        let first = writer
            .store_endstep_captures(batch.clone(), 5)
            .expect("first");
        assert_eq!(
            first,
            CaptureImportReport {
                added: 1,
                replaced: 0,
                unchanged: 0
            }
        );
        let second = writer.store_endstep_captures(batch, 6).expect("second");
        assert_eq!(
            second,
            CaptureImportReport {
                added: 0,
                replaced: 0,
                unchanged: 1
            }
        );
        writer.flush().expect("flush");
        let reader = open_reader(&path).expect("reader");
        assert_eq!(capture_count(&reader).unwrap(), 1);
    }

    #[test]
    fn importing_twice_changes_nothing() {
        use crate::recorded_replays::{ImportReport, NewRecording, recording_count};
        use mtgr::{HeaderGame, RecordingHeader};
        let dir = TestDir::new("writer-store-recordings");
        let path = dir.join("mymtgo.sqlite");
        let writer = spawn_writer(&path).expect("writer");
        let batch = vec![NewRecording {
            header: RecordingHeader {
                match_token: "6f1c2d3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f".to_string(),
                mtgo_match_id: None,
                recorder: None,
                games: vec![HeaderGame {
                    position: 0,
                    game_number: 1,
                    mtgo_game_id: None,
                }],
            },
            text: "a".to_string(),
            file_hash: "h1".to_string(),
        }];
        let first = writer.store_recordings(batch.clone(), 5).expect("first");
        assert_eq!(
            first,
            ImportReport {
                added: 1,
                replaced: 0,
                unchanged: 0
            }
        );
        let second = writer.store_recordings(batch, 6).expect("second");
        assert_eq!(
            second,
            ImportReport {
                added: 0,
                replaced: 0,
                unchanged: 1
            }
        );
        writer.flush().expect("flush");
        let reader = open_reader(&path).expect("reader");
        assert_eq!(recording_count(&reader).unwrap(), 1);
    }

    /// S1-1: a stub count that cannot be read is an error the writer logs,
    /// not a silent zero; a readable one is added and reset.
    #[test]
    fn a_stub_count_that_cannot_be_read_is_reported() {
        let counter = AtomicU32::new(0);
        let plain = Connection::open_in_memory().expect("plain");
        assert!(report_stubs_added(&plain, &counter).is_err());
        let dir = TestDir::new("writer-stub-count");
        let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("writer");
        conn.execute("UPDATE temp.card_stubs_added SET added = 3", [])
            .expect("count");
        assert_eq!(report_stubs_added(&conn, &counter).expect("read"), 3);
        assert_eq!(report_stubs_added(&conn, &counter).expect("read"), 0);
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    // Issue #326: the decklist's unknown cards get stubs through the writer,
    // counted like any other stubs so the cards worker wakes for them.
    #[test]
    fn the_writer_adds_stubs_for_given_ids_and_counts_them() {
        let dir = TestDir::new("writer-ensure-stubs");
        let path = dir.join("mymtgo.sqlite");
        let writer = spawn_writer(&path).expect("writer");
        assert_eq!(
            writer
                .ensure_card_stubs(vec![303, 202, 303])
                .expect("stubs"),
            2
        );
        writer.flush().expect("flush");
        assert_eq!(writer.take_stubs_added(), 2);
        assert_eq!(writer.ensure_card_stubs(vec![202]).expect("again"), 0);
        writer.flush().expect("flush");
        assert_eq!(writer.take_stubs_added(), 0);
    }

    // An Arena replay's tokens with no art: each gets its row, named and
    // marked as a token, counted like any other stub so the cards worker
    // wakes and asks the API for it by name. A row already there (queued
    // before, or known) keeps its name.
    #[test]
    fn the_writer_adds_named_token_stubs_and_counts_them() {
        let dir = TestDir::new("writer-token-stubs");
        let path = dir.join("mymtgo.sqlite");
        let writer = spawn_writer(&path).expect("writer");
        let token = |catalog_id: u32, name: &str| cards::tokens::TokenCard {
            catalog_id,
            name: name.to_string(),
            card_type: "Token".to_string(),
            sub_type: Some(name.to_string()),
            color_identity: "C".to_string(),
        };
        assert_eq!(
            writer
                .ensure_token_stubs(vec![
                    token(100_090_001, "Map"),
                    token(100_090_002, "Lander")
                ])
                .expect("stubs"),
            2
        );
        writer.flush().expect("flush");
        assert_eq!(writer.take_stubs_added(), 2);
        assert_eq!(
            writer
                .ensure_token_stubs(vec![token(100_090_001, "Other")])
                .expect("again"),
            0
        );
        writer.flush().expect("flush");
        assert_eq!(writer.take_stubs_added(), 0);
        let conn = crate::open_reader(&path).expect("reader");
        let row: (String, String, String) = conn
            .query_row(
                "SELECT name, rarity, type FROM cards WHERE catalog_id = 100090001",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("row");
        assert_eq!(
            row,
            ("Map".to_string(), "token".to_string(), "Token".to_string())
        );
    }

    #[test]
    fn the_writer_scans_for_card_stubs_and_reports_what_any_command_added() {
        let dir = TestDir::new("writer-card-stubs");
        let path = dir.join("mymtgo.sqlite");
        let writer = spawn_writer(&path).expect("writer");
        let signature = decks::signature(&[
            decks::DeckCard {
                catalog_id: 101,
                quantity: 4,
                sideboard: false,
            },
            decks::DeckCard {
                catalog_id: 202,
                quantity: 1,
                sideboard: true,
            },
        ]);
        open_writer(&path)
            .expect("second connection")
            .execute_batch(&format!(
                "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source)
                     VALUES (1, 'q-deck', 'Q deck', 'CMODERN', 0, 0, 'imported');
                 INSERT INTO deck_versions (deck_id, signature, modified_at)
                     VALUES (1, '{signature}', 0);"
            ))
            .expect("a version without card rows");
        assert_eq!(writer.scan_card_stubs().expect("scan"), 2);
        writer.flush().expect("flush");
        assert_eq!(writer.take_stubs_added(), 2);
        assert_eq!(writer.take_stubs_added(), 0);
        assert_eq!(writer.scan_card_stubs().expect("again"), 0);
        writer.flush().expect("flush");
        assert_eq!(writer.take_stubs_added(), 0);
    }

    #[test]
    fn the_writer_creates_arena_log_files() {
        let dir = TestDir::new("writer-arena-file");
        let path = dir.join("mymtgo.sqlite");
        let writer = spawn_writer(&path).expect("spawn");
        let file = writer
            .create_arena_log_file(new_arena_log_file("/arena/Player.log"))
            .expect("file");
        let reader = open_reader(&path).expect("reader");
        let files = crate::arena::active_arena_log_files(&reader).expect("files");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].id, file);
    }

    #[test]
    fn the_writer_thread_commits_in_order_and_flushes() {
        let dir = TestDir::new("writer-ok");
        let path = dir.join("mymtgo.sqlite");
        let writer = spawn_writer(&path).expect("spawn");
        let file = writer
            .create_log_file(new_log_file("C:/mtgo.log"))
            .expect("file");
        writer
            .write(WriteBatch {
                events: vec![token_event(file, 0, TOKEN)],
                files: vec![progress(file, 10)],
                ..WriteBatch::default()
            })
            .expect("write");
        writer.flush().expect("flush");
        let reader = open_reader(&path).expect("reader");
        assert_eq!(number(&reader, "SELECT COUNT(*) FROM log_events"), 1);
        assert_eq!(number(&reader, "SELECT cursor_offset FROM log_files"), 10);
    }

    #[test]
    fn a_failure_is_reported_and_later_batches_are_refused() {
        let dir = TestDir::new("writer-fail");
        let path = dir.join("mymtgo.sqlite");
        let writer = spawn_writer(&path).expect("spawn");
        let file = writer
            .create_log_file(new_log_file("C:/mtgo.log"))
            .expect("file");
        writer
            .write(WriteBatch {
                events: vec![token_event(999, 0, TOKEN)],
                ..WriteBatch::default()
            })
            .expect("send");
        assert!(matches!(writer.flush(), Err(StoreError::Failed(_))));
        assert!(writer.failure().is_some());

        writer
            .write(WriteBatch {
                events: vec![token_event(file, 0, TOKEN)],
                ..WriteBatch::default()
            })
            .expect("send");
        assert!(writer.flush().is_err());
        drop(writer);
        let reader = open_reader(&path).expect("reader");
        assert_eq!(number(&reader, "SELECT COUNT(*) FROM log_events"), 0);
    }

    #[test]
    fn after_a_failure_new_files_are_refused() {
        let dir = TestDir::new("writer-fail-create");
        let path = dir.join("mymtgo.sqlite");
        let writer = spawn_writer(&path).expect("spawn");
        writer
            .write(WriteBatch {
                events: vec![token_event(999, 0, TOKEN)],
                ..WriteBatch::default()
            })
            .expect("send");
        assert!(writer.flush().is_err());
        let created = writer.create_log_file(new_log_file("C:/mtgo.log"));
        assert!(matches!(created, Err(StoreError::Failed(_))), "{created:?}");
        drop(writer);
        let reader = open_reader(&path).expect("reader");
        assert_eq!(number(&reader, "SELECT COUNT(*) FROM log_files"), 0);
    }

    #[test]
    fn after_a_failure_imports_and_deck_edits_are_refused() {
        let dir = TestDir::new("writer-fail-import");
        let path = dir.join("mymtgo.sqlite");
        let writer = spawn_writer(&path).expect("spawn");
        writer
            .write(WriteBatch {
                events: vec![token_event(999, 0, TOKEN)],
                ..WriteBatch::default()
            })
            .expect("send");
        assert!(writer.flush().is_err());

        let matches = writer.import(vec![imported(TOKEN, Some(1))], 5_000);
        assert!(matches!(matches, Err(StoreError::Failed(_))), "{matches:?}");
        let decks = writer.import_decks(ImportedDecks::default(), 5_000);
        assert!(matches!(decks, Err(StoreError::Failed(_))), "{decks:?}");
        let leagues = writer.import_leagues(Vec::new(), 5_000);
        assert!(matches!(leagues, Err(StoreError::Failed(_))), "{leagues:?}");
        let detail = writer.import_detail(TOKEN.to_string(), Default::default());
        assert!(matches!(detail, Err(StoreError::Failed(_))), "{detail:?}");
        let edit = writer.edit_deck(DeckEdit::Restore { deck_id: 1 });
        assert!(matches!(edit, Err(StoreError::Failed(_))), "{edit:?}");
        let league = writer.edit_league(
            LeagueEdit::SetStatus {
                run_key: "manual:1".to_string(),
                status: Some(crate::leagues::UserState::Dropped),
                now_ms: 5_000,
            },
            chrono_tz::UTC,
        );
        assert!(matches!(league, Err(StoreError::Failed(_))), "{league:?}");
        let hidden = writer.edit_match(MatchEdit::Hide {
            token: TOKEN.to_string(),
            now_ms: 5_000,
        });
        assert!(matches!(hidden, Err(StoreError::Failed(_))), "{hidden:?}");
        let manual = writer.edit_manual(
            ManualEdit::SetReveals {
                token: TOKEN.to_string(),
                game: 1,
                cards: Vec::new(),
            },
            chrono_tz::UTC,
        );
        assert!(matches!(manual, Err(StoreError::Failed(_))), "{manual:?}");
        let history = writer.import_history(Vec::new(), 5_000);
        assert!(matches!(history, Err(StoreError::Failed(_))), "{history:?}");
        let deleted = writer.delete_history_imports();
        assert!(matches!(deleted, Err(StoreError::Failed(_))), "{deleted:?}");
        let limited = writer.import_limited(Default::default(), 5_000);
        assert!(matches!(limited, Err(StoreError::Failed(_))), "{limited:?}");
        let note = writer.edit_draft_note(crate::limited::DraftNoteEdit::Set {
            draft_token: "q-draft".to_string(),
            ordinal: 1,
            note: "Q".to_string(),
            now_ms: 5_000,
        });
        assert!(matches!(note, Err(StoreError::Failed(_))), "{note:?}");
        drop(writer);
        let reader = open_reader(&path).expect("reader");
        assert_eq!(number(&reader, "SELECT COUNT(*) FROM matches"), 0);
    }

    /// Issue #468: every other command that writes is refused after a
    /// failed batch too, so none can write past the events the batch lost.
    #[test]
    fn after_a_failure_every_other_write_command_is_refused() {
        let dir = TestDir::new("writer-fail-other");
        let path = dir.join("mymtgo.sqlite");
        let writer = spawn_writer(&path).expect("spawn");
        writer
            .write(WriteBatch {
                events: vec![token_event(999, 0, TOKEN)],
                ..WriteBatch::default()
            })
            .expect("send");
        assert!(writer.flush().is_err());

        fn refused<T: std::fmt::Debug>(what: &str, result: Result<T, StoreError>) {
            assert!(
                matches!(result, Err(StoreError::Failed(_))),
                "{what}: {result:?}"
            );
        }
        refused("accounts", writer.import_accounts(Vec::new(), 5_000));
        refused("api", writer.import_api(Default::default(), 5_000));
        refused(
            "archetypes",
            writer.import_archetypes(Default::default(), 5_000),
        );
        refused("labels", writer.import_overlay_labels(Vec::new()));
        let games = writer
            .sync_games(vec![TOKEN.to_string()], Vec::new(), chrono_tz::UTC)
            .expect("queued");
        refused("game sync", games.recv().expect("answered"));
        refused(
            "league sync",
            writer.sync_leagues(Vec::new(), true, chrono_tz::UTC, 5_000),
        );
        let version = writer.record_game_cards_version().expect("queued");
        refused("version", version.recv().expect("answered"));
        refused(
            "account",
            writer.edit_account(AccountEdit::SetTracked {
                username: "q-player".to_string(),
                tracked: false,
            }),
        );
        refused(
            "archetype",
            writer.edit_archetypes(ArchetypeEdit::ClearDetected { match_ids: vec![1] }),
        );
        refused(
            "outbox",
            writer.edit_outbox(OutboxEdit::Enqueue { rows: Vec::new() }),
        );
        refused(
            "sync",
            writer.edit_sync(SyncEdit::Carried {
                sync_type: mymtgo_api::SyncType::Deck,
                rows: Vec::new(),
            }),
        );
        refused("scan", writer.scan_card_stubs());
        refused("stubs", writer.ensure_card_stubs(vec![101]));
        refused("token stubs", writer.ensure_token_stubs(Vec::new()));
        refused("resolve", writer.resolve_cards(CardsUpdate::default()));
        refused(
            "images",
            writer.store_card_images(StoredImages {
                catalog_id: 101,
                local_image: Some("q.jpg".to_string()),
                local_art_crop: None,
            }),
        );
        refused("keep", writer.keep_present_images(BTreeSet::new()));
        refused("card stats", writer.regenerate_card_stats(1));
        drop(writer);
        let reader = open_reader(&path).expect("reader");
        assert_eq!(number(&reader, "SELECT COUNT(*) FROM cards"), 0);
    }

    #[test]
    fn the_writer_saves_pick_notes_and_imports_limited_data_and_answers() {
        use crate::import_limited::{ImportedLimited, LegacyDraft, LimitedImportCounts};
        use crate::limited::{DraftNoteEdit, DraftNoteOutcome, pick_notes};
        let dir = TestDir::new("writer-limited");
        let path = dir.join("mymtgo.sqlite");
        let writer = spawn_writer(&path).expect("spawn");
        let note = DraftNoteEdit::Set {
            draft_token: "q-draft".to_string(),
            ordinal: 1,
            note: " Q note ".to_string(),
            now_ms: 5_000,
        };
        assert_eq!(
            writer.edit_draft_note(note.clone()).expect("answered"),
            DraftNoteOutcome::NotFound
        );
        let draft = limited::Draft {
            token: "q-draft".to_string(),
            league_event_id: None,
            course_id: None,
            draft_id: None,
            pod_token: None,
            seat_index: None,
            seat_count: 8,
            booster_catalog_id: None,
            state: limited::DraftState::Finished,
            pack_size: 14,
            picks_expected: 42,
            started_at_ms: None,
            ended_at_ms: None,
            picks: vec![limited::Pick {
                ordinal: 1,
                pack_number: 1,
                pick_number: 1,
                pack_id: None,
                direction: None,
                cards_available: vec![1],
                picked_catalog_id: Some(1),
                picked_card_id: None,
                reservations: Vec::new(),
                shown_at_ms: None,
                deadline_at_ms: None,
                picked_at_ms: None,
            }],
        };
        let counts = writer
            .import_limited(
                ImportedLimited {
                    drafts: vec![LegacyDraft {
                        legacy_id: 1,
                        league_id: None,
                        draft,
                        notes: Vec::new(),
                    }],
                    decks: Vec::new(),
                    league_keys: None,
                },
                5_000,
            )
            .expect("imported");
        assert_eq!(
            counts,
            LimitedImportCounts {
                drafts_added: 1,
                drafts_without_account: 1,
                ..LimitedImportCounts::default()
            }
        );
        assert_eq!(
            writer.edit_draft_note(note).expect("answered"),
            DraftNoteOutcome::Done
        );
        drop(writer);
        let reader = open_reader(&path).expect("reader");
        assert_eq!(pick_notes(&reader, "q-draft").expect("notes")[&1], "Q note");
    }

    #[test]
    fn the_writer_edits_leagues_and_answers() {
        let dir = TestDir::new("writer-league-edit");
        let path = dir.join("mymtgo.sqlite");
        let writer = spawn_writer(&path).expect("spawn");
        let notes = |run_key: &str| LeagueEdit::SetNotes {
            run_key: run_key.to_string(),
            notes: "Q note".to_string(),
            now_ms: 5_000,
        };
        assert_eq!(
            writer
                .edit_league(notes("manual:1"), chrono_tz::UTC)
                .expect("answered"),
            LeagueEditOutcome::NotFound
        );
        {
            let conn = open_writer(&path).expect("second connection");
            insert_manual_league(&conn, 1, None, None, "Qmanual", 1, 1);
        }
        assert_eq!(
            writer
                .edit_league(notes("manual:1"), chrono_tz::UTC)
                .expect("answered"),
            LeagueEditOutcome::Done
        );
        let reader = open_reader(&path).expect("reader");
        assert_eq!(
            number(
                &reader,
                "SELECT COUNT(*) FROM leagues WHERE notes IS NOT NULL"
            ),
            1
        );
    }

    #[test]
    fn the_writer_sets_the_status_of_deletes_and_restores_leagues() {
        let dir = TestDir::new("writer-league-delete");
        let path = dir.join("mymtgo.sqlite");
        let writer = spawn_writer(&path).expect("spawn");
        {
            let conn = open_writer(&path).expect("second connection");
            insert_manual_league(&conn, 1, None, None, "Qmanual", 1, 1);
        }
        let key = || "manual:1".to_string();
        let answer = |edit: LeagueEdit| writer.edit_league(edit, chrono_tz::UTC).expect("answered");
        assert_eq!(
            answer(LeagueEdit::SetStatus {
                run_key: key(),
                status: Some(UserState::Partial),
                now_ms: 5_000,
            }),
            LeagueEditOutcome::Done
        );
        let delete = |now_ms: i64| LeagueEdit::Delete {
            run_key: key(),
            matches: DeleteMatches::KeepAsNonLeague,
            now_ms,
        };
        assert_eq!(answer(delete(6_000)), LeagueEditOutcome::Done);
        let reader = open_reader(&path).expect("reader");
        assert_eq!(number(&reader, "SELECT removed_at FROM leagues"), 6_000);
        assert_eq!(
            answer(LeagueEdit::UndoDelete {
                league_id: number(&reader, "SELECT id FROM leagues"),
                removed_at: 6_000,
                now_ms: 7_000,
            }),
            LeagueEditOutcome::Done
        );
        assert_eq!(
            number(
                &reader,
                "SELECT COUNT(*) FROM leagues WHERE removed_at IS NULL AND edited_at = 7000"
            ),
            1
        );
        assert_eq!(answer(delete(8_000)), LeagueEditOutcome::Done);
        assert_eq!(
            answer(LeagueEdit::Restore {
                run_key: key(),
                now_ms: 9_000,
            }),
            LeagueEditOutcome::Done
        );
        assert_eq!(
            number(
                &reader,
                "SELECT COUNT(*) FROM leagues WHERE removed_at IS NULL AND user_state = 'partial'"
            ),
            1
        );
    }

    #[test]
    fn the_writer_edits_manual_matches_and_answers() {
        let dir = TestDir::new("writer-manual-edit");
        let path = dir.join("mymtgo.sqlite");
        let writer = spawn_writer(&path).expect("spawn");
        let reveals = ManualEdit::SetReveals {
            token: TOKEN.to_string(),
            game: 1,
            cards: Vec::new(),
        };
        assert_eq!(
            writer
                .edit_manual(reveals.clone(), chrono_tz::UTC)
                .expect("answered"),
            ManualEditOutcome::NotFound
        );
        insert_manual(&open_writer(&path).expect("second connection"), TOKEN);
        assert_eq!(
            writer
                .edit_manual(reveals, chrono_tz::UTC)
                .expect("answered"),
            ManualEditOutcome::Done
        );
        let reader = open_reader(&path).expect("reader");
        assert_eq!(number(&reader, "SELECT revision FROM matches"), 2);
    }

    #[test]
    fn the_writer_restores_a_hidden_match() {
        let dir = TestDir::new("writer-match-restore");
        let path = dir.join("mymtgo.sqlite");
        let writer = spawn_writer(&path).expect("spawn");
        writer
            .write(WriteBatch {
                matches: vec![crate::write::MatchWrite::Upsert {
                    view: Box::new(view(TOKEN, "Qopponent", 1_000)),
                    revision: 1,
                }],
                ..WriteBatch::default()
            })
            .expect("a match");
        let hide = MatchEdit::Hide {
            token: TOKEN.to_string(),
            now_ms: 5_000,
        };
        assert_eq!(
            writer.edit_match(hide).expect("answered"),
            MatchEditOutcome::Done
        );
        let restore = MatchEdit::Restore {
            token: TOKEN.to_string(),
            now_ms: 6_000,
        };
        assert_eq!(
            writer.edit_match(restore).expect("answered"),
            MatchEditOutcome::Done
        );
        let reader = open_reader(&path).expect("reader");
        assert_eq!(
            number(
                &reader,
                "SELECT COUNT(*) FROM matches WHERE hidden_at IS NULL AND edited_at = 6000"
            ),
            1
        );
    }

    #[test]
    fn the_writer_edits_matches_and_answers() {
        let dir = TestDir::new("writer-match-edit");
        let path = dir.join("mymtgo.sqlite");
        let writer = spawn_writer(&path).expect("spawn");
        let hide = MatchEdit::Hide {
            token: TOKEN.to_string(),
            now_ms: 5_000,
        };
        assert_eq!(
            writer.edit_match(hide.clone()).expect("answered"),
            MatchEditOutcome::NotFound
        );
        writer
            .write(WriteBatch {
                matches: vec![crate::write::MatchWrite::Upsert {
                    view: Box::new(view(TOKEN, "Qopponent", 1_000)),
                    revision: 1,
                }],
                ..WriteBatch::default()
            })
            .expect("a match");
        assert_eq!(
            writer.edit_match(hide).expect("answered"),
            MatchEditOutcome::Done,
            "the edit follows the batch sent before it"
        );
        let reader = open_reader(&path).expect("reader");
        assert_eq!(
            number(
                &reader,
                "SELECT COUNT(*) FROM matches WHERE hidden_at IS NOT NULL"
            ),
            1
        );
    }

    #[test]
    fn a_poisoned_failure_lock_counts_as_a_failure() {
        let dir = TestDir::new("writer-poisoned");
        let writer = spawn_writer(&dir.join("mymtgo.sqlite")).expect("spawn");
        let failure = Arc::clone(&writer.failure);
        let poisoner = std::thread::spawn(move || {
            let _slot = failure.lock().expect("lock");
            panic!("poison the failure lock");
        });
        assert!(poisoner.join().is_err());
        assert!(writer.failure().is_some());
        assert!(matches!(writer.flush(), Err(StoreError::Failed(_))));
    }

    #[test]
    fn the_writer_imports_and_answers_with_counts() {
        use crate::import::ImportCounts;
        let dir = TestDir::new("writer-import");
        let path = dir.join("mymtgo.sqlite");
        let writer = spawn_writer(&path).expect("spawn");
        let batch = vec![imported(TOKEN, Some(1)), imported(OTHER, Some(2))];
        let first = writer.import(batch.clone(), 5_000).expect("import");
        assert_eq!(
            first,
            ImportCounts {
                added: 2,
                ..ImportCounts::default()
            }
        );
        let second = writer.import(batch, 6_000).expect("re-import");
        assert_eq!(
            second,
            ImportCounts {
                unchanged: 2,
                ..ImportCounts::default()
            }
        );
        let reader = open_reader(&path).expect("reader");
        assert_eq!(
            number(
                &reader,
                "SELECT COUNT(*) FROM matches WHERE source = 'imported'"
            ),
            2
        );
    }

    #[test]
    fn the_writer_imports_decks_after_matches_and_answers_with_counts() {
        use crate::import_decks::{
            DeckImportCounts, ImportedDeck, ImportedDecks, ImportedLink, ImportedVersion,
        };
        let dir = TestDir::new("writer-import-decks");
        let path = dir.join("mymtgo.sqlite");
        let writer = spawn_writer(&path).expect("spawn");
        writer
            .import(vec![imported(TOKEN, Some(1))], 5_000)
            .expect("matches");
        let data = ImportedDecks {
            cards: Vec::new(),
            decks: vec![ImportedDeck {
                mtgo_id: "1001".to_string(),
                account: Some("Qlocal".to_string()),
                name: "Q Burn".to_string(),
                original_name: None,
                format: "CMODERN".to_string(),
                color_identity: Some("R".to_string()),
                cover_catalog_id: None,
                archived_at_ms: None,
                created_at_ms: Some(1),
                updated_at_ms: Some(2),
                versions: vec![ImportedVersion {
                    signature: "MTI6NDpmYWxzZQ==".to_string(),
                    modified_at_ms: 3,
                }],
            }],
            links: vec![ImportedLink {
                token: TOKEN.to_string(),
                mtgo_match_id: Some(1),
                deck_mtgo_id: "1001".to_string(),
                signature: "MTI6NDpmYWxzZQ==".to_string(),
            }],
        };
        let counts = writer.import_decks(data, 5_000).expect("decks");
        assert_eq!(
            counts,
            DeckImportCounts {
                decks_added: 1,
                versions_added: 1,
                links_added: 1,
                ..DeckImportCounts::default()
            }
        );
        let reader = open_reader(&path).expect("reader");
        assert_eq!(
            number(
                &reader,
                &format!(
                    "SELECT COUNT(*) FROM matches
                     WHERE token = '{TOKEN}' AND deck_version_id IS NOT NULL"
                )
            ),
            1
        );
    }

    #[test]
    fn the_writer_imports_a_match_detail_after_its_match() {
        use crate::imported_detail::{DetailChange, DetailLogEntry};
        let dir = TestDir::new("writer-import-detail");
        let path = dir.join("mymtgo.sqlite");
        let writer = spawn_writer(&path).expect("spawn");
        writer
            .import(vec![imported(TOKEN, Some(1))], 5_000)
            .expect("matches");
        let detail = ImportedDetail {
            games: Vec::new(),
            log: vec![DetailLogEntry {
                at_ms: 1,
                message: "Q line".to_string(),
            }],
        };
        let first = writer
            .import_detail(TOKEN.to_string(), detail.clone())
            .expect("detail");
        assert_eq!(first.change, DetailChange::Added);
        let again = writer
            .import_detail(TOKEN.to_string(), detail.clone())
            .expect("again");
        assert_eq!(again.change, DetailChange::Unchanged);
        let reader = open_reader(&path).expect("reader");
        assert_eq!(number(&reader, "SELECT entry_count FROM game_logs"), 1);
    }

    #[test]
    fn the_writer_syncs_games_after_the_batches_before_it() {
        let dir = TestDir::new("writer-sync-games");
        let path = dir.join("mymtgo.sqlite");
        let writer = spawn_writer(&path).expect("spawn");
        let bytes = fixture_tools::load_game_log("clean_2_0_win");
        let entries = game_detail::parse_game_log(&bytes, chrono_tz::UTC)
            .expect("parses")
            .entries;
        let players = game_detail::detect_players(&entries);
        let mut live = view(TOKEN, &players[1], 1_000);
        live.local_username = players[0].clone();
        writer
            .write(WriteBatch {
                matches: vec![crate::write::MatchWrite::Upsert {
                    view: Box::new(live),
                    revision: 1,
                }],
                ..WriteBatch::default()
            })
            .expect("batch");
        let synced = writer
            .sync_games(
                vec![TOKEN.to_string(), "q-unknown".to_string()],
                vec![(TOKEN.to_string(), bytes)],
                chrono_tz::UTC,
            )
            .expect("queued");
        let (outcome, work) = synced.recv().expect("answered").expect("synced");
        assert!(outcome.changed);
        assert!(outcome.skipped.is_empty());
        assert!(work.rows > 0, "{work:?}");
        writer
            .record_game_cards_version()
            .expect("queued")
            .recv()
            .expect("answered")
            .expect("recorded");
        let reader = open_reader(&path).expect("reader");
        assert_eq!(number(&reader, "SELECT COUNT(*) FROM game_logs"), 1);
        assert!(number(&reader, "SELECT COUNT(*) FROM game_cards") > 0);
        assert_eq!(
            crate::game_sync::game_cards_version(&reader).expect("version"),
            Some(game_detail::GAME_CARDS_VERSION)
        );
    }

    #[test]
    fn the_writer_imports_leagues_after_matches_and_answers_with_counts() {
        use crate::import_leagues::{ImportedLeague, LeagueImportCounts, LeagueMember};
        let dir = TestDir::new("writer-import-leagues");
        let path = dir.join("mymtgo.sqlite");
        let writer = spawn_writer(&path).expect("spawn");
        writer
            .import(vec![imported(TOKEN, Some(1))], 5_000)
            .expect("matches");
        let league = ImportedLeague {
            legacy_id: 7,
            manual: false,
            kind: "constructed".to_string(),
            set_code: None,
            course_id: None,
            token: "q-league".to_string(),
            event_id: Some(70),
            name: "Q League".to_string(),
            format: "CMODERN".to_string(),
            state: "complete".to_string(),
            started_at_ms: 1,
            joined_at_ms: Some(1),
            completed_at_ms: Some(2),
            dropped_at_ms: None,
            created_at_ms: Some(1),
            deck: None,
            notes: Some("Q notes".to_string()),
            matches: vec![LeagueMember {
                token: TOKEN.to_string(),
                mtgo_match_id: Some(1),
            }],
        };
        let counts = writer
            .import_leagues(vec![league.clone()], 5_000)
            .expect("leagues");
        assert_eq!(
            counts,
            LeagueImportCounts {
                added: 1,
                ..LeagueImportCounts::default()
            }
        );
        let again = writer.import_leagues(vec![league], 6_000).expect("again");
        assert_eq!(again.unchanged, 1);
        let reader = open_reader(&path).expect("reader");
        assert_eq!(
            number(
                &reader,
                &format!(
                    "SELECT COUNT(*) FROM matches m JOIN leagues l ON l.id = m.league_id
                     WHERE m.token = '{TOKEN}' AND m.league_linked = 1
                         AND l.imported_from = 7"
                )
            ),
            1
        );
    }

    #[test]
    fn a_failed_import_is_answered_and_does_not_stop_the_writer() {
        let dir = TestDir::new("writer-import-fail");
        let path = dir.join("mymtgo.sqlite");
        let writer = spawn_writer(&path).expect("spawn");
        let mut bad = imported(OTHER, Some(2));
        bad.state = "not a state".to_string();
        let result = writer.import(vec![imported(TOKEN, Some(1)), bad], 5_000);
        assert!(matches!(result, Err(StoreError::Sqlite(_))), "{result:?}");
        writer.flush().expect("live batches still commit");
        let reader = open_reader(&path).expect("reader");
        assert_eq!(
            number(&reader, "SELECT COUNT(*) FROM matches"),
            0,
            "one transaction: the good match rolled back too"
        );
    }

    #[test]
    fn batches_queued_without_a_flush_commit_when_the_handle_is_dropped() {
        let dir = TestDir::new("writer-drop");
        let path = dir.join("mymtgo.sqlite");
        let writer = spawn_writer(&path).expect("spawn");
        let file = writer
            .create_log_file(new_log_file("C:/mtgo.log"))
            .expect("file");
        for batch in 0..10_u64 {
            writer
                .write(WriteBatch {
                    events: vec![token_event(file, batch * 10, TOKEN)],
                    files: vec![progress(file, batch * 10 + 10)],
                    ..WriteBatch::default()
                })
                .expect("queue");
        }
        drop(writer);
        let reader = open_reader(&path).expect("reader");
        assert_eq!(number(&reader, "SELECT COUNT(*) FROM log_events"), 10);
        assert_eq!(number(&reader, "SELECT cursor_offset FROM log_files"), 100);
    }
}
