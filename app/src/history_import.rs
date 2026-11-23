//! Importing MTGO's own match history (Plan M; 0.x "Import Match History"):
//! the scan that pairs each past match with its game log and scores it
//! against a chosen deck, then the import of the chosen matches. The scan
//! runs on a background thread and keeps its result in memory; progress
//! reaches the page as `HistoryScanChanged`. Imports and "Delete all
//! imported" go through the tracker (the only writer) in batches, waiting
//! outside the tracker lock, and end with `HistoryImportsChanged`.

pub mod import;
pub mod pairing;
pub mod scan;

use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use mtgo_log::Env;
use mtgo_match::panic_message;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_specta::Event;

use crate::bridge::lock;
use crate::commands::{AppState, CommandError, Startup, read_settings, ready, storage};
use crate::events::{HistoryImportsChanged, HistoryScanChanged};
use crate::logging::Timer;
use import::{HistoryImportFilters, HistoryImportResult};
use scan::{HistoryScanPage, HistoryScanQuery, HistoryScanStatus, ScanInput, ScanRun, run_scan};

/// 0.x's message for a deck version that does not exist.
pub const UNKNOWN_VERSION: &str = "The selected deck version id is invalid.";

/// Start a scan of deck version `deck_version_id` (0.x `ScanController`):
/// a running scan is cancelled and replaced. The caller runs it with
/// `run_in_background`.
pub fn begin_scan(
    state: &AppState,
    deck_version_id: i64,
) -> Result<(Arc<ScanRun>, ScanInput), CommandError> {
    let version = store::history_import::scan_version(&lock(&state.reader), deck_version_id)
        .map_err(storage)?
        .ok_or_else(|| CommandError::Refused(UNKNOWN_VERSION.to_string()))?;
    let input = ScanInput {
        env: Env::current(),
        data_folder: read_settings(state)?.mtgo_data_folder_path(),
        tz: state.timezone,
    };
    Ok((state.history.begin(version), input))
}

/// Run `run` on this thread with its own read-only connection, ending as
/// failed if the database cannot be read or the scan panics.
pub fn run_in_background(
    state: &AppState,
    run: &ScanRun,
    input: &ScanInput,
    report: &mut dyn FnMut(HistoryScanStatus),
) {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let conn = store::open_reader(&state.db_path).map_err(|error| {
            // The detail names the database file: the log has it, the page
            // gets a plain message.
            tracing::warn!(%error, "the history scan could not open the database");
            SCAN_DATABASE_FAILED.to_string()
        })?;
        run_scan(&state.history, run, input, &conn, &mut *report);
        Ok::<(), String>(())
    }));
    let failure = match outcome {
        Ok(Ok(())) => None,
        Ok(Err(message)) => Some(message),
        Err(panic) => Some(scan_panic_message(&*panic)),
    };
    if let Some(message) = failure {
        tracing::warn!(%message, "the history scan failed");
        // A replaced (cancelled) run reports nothing more.
        if run.fail(&message) {
            report(run.status());
        }
    }
}

/// The message for an import with no scan to import from.
pub const NO_SCAN: &str = "There is no scan to import from.";

/// 0.x's message when the import names a scan that is no longer the
/// current one.
pub const SCAN_CHANGED: &str = "The scan changed. Review the results again.";

/// The page's message when the scan cannot open the database (the detail,
/// which names the file, goes to the log).
pub const SCAN_DATABASE_FAILED: &str = "The scan could not read the MyMTGO database.";

/// The page's message when the scan panics (the panic text goes to the log).
pub const SCAN_PANICKED: &str = "The scan stopped unexpectedly. See the log for details.";

/// The plain message for a scan panic; the detail is only logged.
fn scan_panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    tracing::warn!(detail = %panic_message(panic), "the history scan panicked");
    SCAN_PANICKED.to_string()
}

/// The message while another import or delete runs.
pub const BUSY: &str = "An import is already running.";

/// Which scanned matches to import.
#[derive(Debug, Clone, PartialEq)]
pub enum ImportChoice {
    /// These history ids (0.x `StoreController`).
    Ids(Vec<u64>),
    /// Every row through the page's filters (0.x `ImportAllController`).
    Filtered(HistoryImportFilters),
}

/// Import the chosen matches of scan `scan_id`, which must be the latest
/// one, through the tracker, in
/// batches of `import::BATCH_SIZE`. The rows of a batch that stored a match
/// leave the scan's results (0.x `ImportMatchesJob`).
pub fn import_chosen(
    state: &AppState,
    scan_id: u32,
    choice: &ImportChoice,
) -> Result<HistoryImportResult, CommandError> {
    let run = state
        .history
        .current()
        .ok_or_else(|| CommandError::Refused(NO_SCAN.to_string()))?;
    // The ids and filters were chosen on that scan's results (0.x passed
    // `scanId`): another scan's rows and deck version are never used.
    if run.id != scan_id {
        return Err(CommandError::Refused(SCAN_CHANGED.to_string()));
    }
    let _slot = state
        .history
        .claim_import()
        .ok_or_else(|| CommandError::Refused(BUSY.to_string()))?;
    let rows = match choice {
        ImportChoice::Ids(ids) => run.rows_with_ids(ids),
        ImportChoice::Filtered(filters) => {
            run.rows_filtered(filters.format.as_deref(), filters.min_confidence)
        }
    };
    let fallback = store::history_import::fallback_account(&lock(&state.reader), run.version.id)
        .map_err(storage)?;
    crate::scrub::remember(player_names(&rows));

    let mut result = HistoryImportResult::default();
    for chunk in rows.chunks(import::BATCH_SIZE) {
        let mut batch = Vec::new();
        for row in chunk {
            match import::history_match(row, run.version.id, fallback.as_deref(), state.timezone) {
                Some(found) => batch.push(found),
                None => result.skipped += 1,
            }
        }
        if batch.is_empty() {
            continue;
        }
        let counts = import_batch(state, batch).map_err(CommandError::Tracker)?;
        let imported = u32::try_from(counts.imported.len()).unwrap_or(u32::MAX);
        result.imported += imported;
        result.skipped += counts.skipped_existing + counts.skipped_token;
        // 0.x dropped every requested row once its job stored a match.
        if imported > 0 {
            let done: HashSet<u64> = chunk.iter().map(|row| row.history_id).collect();
            run.forget_rows(&done);
        }
    }
    tracing::info!(
        imported = result.imported,
        skipped = result.skipped,
        "the history import finished"
    );
    Ok(result)
}

/// Every local and opponent name of the rows, for the report scrubber.
fn player_names(rows: &[scan::HistoryScanMatch]) -> Vec<String> {
    let mut names = Vec::new();
    for row in rows {
        names.push(row.opponent.clone());
        if let Some(local) = &row.local_player {
            names.push(local.clone());
        }
    }
    names
}

/// Send one batch to the tracker and wait for its counts. The tracker lock
/// is held only to take an `Importer`, never while waiting (as the 0.x
/// import does).
fn import_batch(
    state: &AppState,
    batch: Vec<store::history_import::HistoryMatch>,
) -> Result<store::history_import::HistoryImportCounts, String> {
    importer(state)?.import_history(batch)
}

fn importer(state: &AppState) -> Result<tracker::Importer, String> {
    state
        .tracker_part(tracker::TrackerHandle::importer)
        .map_err(|error| error.message().to_string())
}

/// "Delete all imported" (0.x `DestroyController`, narrowed to history
/// imports): returns how many matches were removed.
pub fn delete_imports(state: &AppState) -> Result<u32, CommandError> {
    let _slot = state
        .history
        .claim_import()
        .ok_or_else(|| CommandError::Refused(BUSY.to_string()))?;
    let removed = importer(state)
        .and_then(|importer| importer.delete_history_imports())
        .map_err(CommandError::Tracker)?;
    Ok(u32::try_from(removed.len()).unwrap_or(u32::MAX))
}

/// How many stored matches the history importer created.
pub fn imported_count(state: &AppState) -> Result<u32, CommandError> {
    store::history_import::history_imported_count(&lock(&state.reader)).map_err(storage)
}

/// What "Delete all imported" did (0.x `deleted`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryDeleteResult {
    pub deleted: u32,
}

/// A card seen in a scanned match's game log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryScanCard {
    pub mtgo_id: u32,
    pub name: String,
}

/// The cards dialog (0.x `ScanMatchCardsController`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryScanCards {
    pub local_cards: Vec<HistoryScanCard>,
    pub opponent_cards: Vec<HistoryScanCard>,
}

/// The local player's and the opponent's cards in a scanned match's paired
/// game log, named from the cards table, else the log, else "Unknown", by
/// name. Empty without a log or such a row.
pub fn scan_cards(state: &AppState, history_id: u64) -> Result<HistoryScanCards, CommandError> {
    let Some(row) = state.history.current().and_then(|run| run.row(history_id)) else {
        return Ok(HistoryScanCards::default());
    };
    let (Some(path), Some(local)) = (&row.game_log_path, &row.local_player) else {
        return Ok(HistoryScanCards::default());
    };
    let entries = import::read_log(std::path::Path::new(path), state.timezone);
    let read = game_detail::extract_log_cards(&entries);
    // The scan matched the opponent in any ASCII case.
    let opponent = read
        .players
        .iter()
        .find(|player| pairing::same_player(player, &row.opponent))
        .cloned();
    let local_cards = read.match_cards(local);
    let opponent_cards = match &opponent {
        Some(player) => read.match_cards(player),
        None => &[],
    };
    let mut catalog_ids: Vec<u32> = Vec::new();
    for card in local_cards.iter().chain(opponent_cards) {
        catalog_ids.push(card.mtgo_id);
    }
    let known = store::decks::card_infos(&lock(&state.reader), &catalog_ids).map_err(storage)?;
    let named = |cards: &[game_detail::GameLogCard]| -> Vec<HistoryScanCard> {
        let mut named: Vec<HistoryScanCard> = cards
            .iter()
            .map(|card| {
                let stored = known
                    .get(&card.mtgo_id)
                    .and_then(|row| row.name.clone())
                    .filter(|name| !name.is_empty());
                let name = stored
                    .or_else(|| (!card.name.is_empty()).then(|| card.name.clone()))
                    .unwrap_or_else(|| "Unknown".to_string());
                HistoryScanCard {
                    mtgo_id: card.mtgo_id,
                    name,
                }
            })
            .collect();
        // Stable: equal names keep the log's order.
        named.sort_by(|a, b| a.name.cmp(&b.name));
        named
    };
    Ok(HistoryScanCards {
        local_cards: named(local_cards),
        opponent_cards: named(opponent_cards),
    })
}

/// A deck version the import page offers (0.x `Import\IndexController`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryDeckVersionView {
    pub id: u32,
    pub deck_name: String,
    /// 0.x `deck_deleted`: the deck is archived.
    pub archived: bool,
    /// `d/m/Y` in the local timezone.
    pub modified_at: String,
    pub format: String,
    /// The current account's deck, or one no account has adopted yet (every
    /// deck while no account is current). "Set deck" offers only these
    /// (issue #332).
    pub current_account: bool,
    /// A Limited deck, which "Set deck" never offers (issue #332).
    pub limited: bool,
}

/// Every deck version, archived decks too, newest first.
pub fn deck_versions(state: &AppState) -> Result<Vec<HistoryDeckVersionView>, CommandError> {
    let (versions, current) = {
        let reader = lock(&state.reader);
        (
            store::history_import::history_deck_versions(&reader).map_err(storage)?,
            store::accounts::current_account_id(&reader, store::Platform::Mtgo).map_err(storage)?,
        )
    };
    let mut views = Vec::new();
    for version in versions {
        let modified_at = chrono::DateTime::from_timestamp_millis(version.modified_at)
            .map(|at| {
                at.with_timezone(&state.timezone)
                    .format("%d/%m/%Y")
                    .to_string()
            })
            .unwrap_or_default();
        views.push(HistoryDeckVersionView {
            id: u32::try_from(version.id).unwrap_or(0),
            deck_name: version.deck_name,
            archived: version.archived,
            modified_at,
            format: version.format,
            current_account: version.account_id.is_none()
                || current.is_none()
                || version.account_id == current,
            limited: version.limited,
        });
    }
    Ok(views)
}

fn push(app: &AppHandle, status: Option<HistoryScanStatus>) {
    if let Err(error) = (HistoryScanChanged { status }).emit(app) {
        tracing::warn!(%error, "could not push the history scan progress");
    }
}

/// Start scanning MTGO's match history against a deck version, on a
/// background thread. Progress arrives as `HistoryScanChanged` events.
#[tauri::command]
#[specta::specta]
pub fn start_history_scan(
    app: AppHandle,
    deck_version_id: u32,
    startup: State<'_, Startup>,
) -> Result<HistoryScanStatus, CommandError> {
    let _timer = Timer::start("start_history_scan");
    let state = ready(&startup)?;
    let (run, input) = begin_scan(state, i64::from(deck_version_id))?;
    let status = run.status();
    push(&app, Some(status.clone()));
    let thread_run = Arc::clone(&run);
    let thread_app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("history-scan".to_string())
        .spawn(move || {
            let startup = thread_app.state::<Startup>();
            // Startup was ready when the scan began, and never changes.
            if let Ok(state) = ready(&startup) {
                run_in_background(state, &thread_run, &input, &mut |status| {
                    push(&thread_app, Some(status));
                });
            }
        });
    if let Err(error) = spawned {
        let message = format!("could not start the scan: {error}");
        if run.fail(&message) {
            push(&app, Some(run.status()));
        }
        return Err(CommandError::Tracker(message));
    }
    Ok(status)
}

/// The latest scan's status (0.x `ScanStatusController`), `None` when there
/// is none.
#[tauri::command]
#[specta::specta]
pub fn history_scan_status(
    startup: State<'_, Startup>,
) -> Result<Option<HistoryScanStatus>, CommandError> {
    let _timer = Timer::start("history_scan_status");
    Ok(ready(&startup)?.history.status())
}

/// Cancel the running scan, or forget an ended one (0.x
/// `CancelScanController`).
#[tauri::command]
#[specta::specta]
pub fn cancel_history_scan(
    app: AppHandle,
    startup: State<'_, Startup>,
) -> Result<Option<HistoryScanStatus>, CommandError> {
    let _timer = Timer::start("cancel_history_scan");
    let status = ready(&startup)?.history.cancel();
    push(&app, status.clone());
    Ok(status)
}

/// One page of the latest scan's matches (0.x `ScanMatchesController`).
#[tauri::command]
#[specta::specta]
pub fn history_scan_matches(
    query: HistoryScanQuery,
    startup: State<'_, Startup>,
) -> Result<HistoryScanPage, CommandError> {
    let _timer = Timer::start("history_scan_matches");
    Ok(ready(&startup)?.history.page(&query))
}

/// Tell every window the stored history imports changed (the tracker tells
/// the deck and league pages itself), and push the scan's status: imported
/// rows left its results.
fn announce(app: &AppHandle, state: &AppState) {
    match imported_count(state) {
        Ok(imported_count) => {
            if let Err(error) = (HistoryImportsChanged { imported_count }).emit(app) {
                tracing::warn!(%error, "could not push the history import change");
            }
        }
        Err(error) => tracing::warn!(?error, "could not count the history imports"),
    }
    push(app, state.history.status());
}

/// Import the chosen matches of scan `scan_id` (0.x `StoreController`),
/// refused when another scan has replaced it.
/// Async: the batches wait for the tracker on a blocking thread. Ends with
/// `HistoryImportsChanged` and `HistoryScanChanged`, even after a failure
/// (earlier batches may have committed).
#[tauri::command]
#[specta::specta]
pub async fn import_history(
    app: AppHandle,
    scan_id: u32,
    history_ids: Vec<u32>,
) -> Result<HistoryImportResult, CommandError> {
    let _timer = Timer::start("import_history");
    crate::commands::off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let ids = history_ids.into_iter().map(u64::from).collect();
        let result = import_chosen(state, scan_id, &ImportChoice::Ids(ids));
        announce(&app, state);
        result
    })
    .await
}

/// Import every row of scan `scan_id` through the page's filters (0.x
/// `ImportAllController`). Async and refused like `import_history`.
#[tauri::command]
#[specta::specta]
pub async fn import_history_all(
    app: AppHandle,
    scan_id: u32,
    filters: HistoryImportFilters,
) -> Result<HistoryImportResult, CommandError> {
    let _timer = Timer::start("import_history_all");
    crate::commands::off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let result = import_chosen(state, scan_id, &ImportChoice::Filtered(filters));
        announce(&app, state);
        result
    })
    .await
}

/// "Delete all imported": remove every match the history importer stored,
/// and nothing else. Async, like `import_history`; the tracker reports each
/// removed match.
#[tauri::command]
#[specta::specta]
pub async fn delete_history_imports(app: AppHandle) -> Result<HistoryDeleteResult, CommandError> {
    let _timer = Timer::start("delete_history_imports");
    crate::commands::off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let deleted = delete_imports(state);
        announce(&app, state);
        Ok(HistoryDeleteResult { deleted: deleted? })
    })
    .await
}

/// How many stored matches the history importer created (0.x
/// `importedCount`).
#[tauri::command]
#[specta::specta]
pub fn history_imported_count(startup: State<'_, Startup>) -> Result<u32, CommandError> {
    let _timer = Timer::start("history_imported_count");
    imported_count(ready(&startup)?)
}

/// The cards of a scanned match's game log (0.x
/// `ScanMatchCardsController`).
#[tauri::command]
#[specta::specta]
pub fn history_scan_cards(
    history_id: u32,
    startup: State<'_, Startup>,
) -> Result<HistoryScanCards, CommandError> {
    let _timer = Timer::start("history_scan_cards");
    scan_cards(ready(&startup)?, u64::from(history_id))
}

/// The deck versions the import page offers (0.x `Import\IndexController`).
#[tauri::command]
#[specta::specta]
pub fn history_deck_versions(
    startup: State<'_, Startup>,
) -> Result<Vec<HistoryDeckVersionView>, CommandError> {
    let _timer = Timer::start("history_deck_versions");
    deck_versions(ready(&startup)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::tests::ready_state;
    use crate::history_import::scan::{HistoryScanMatch, ScanOutcome, ScanState};
    use fixture_tools::{TestDir, load_game_log};
    use game_detail::{detect_players, extract_log_cards, parse_game_log};
    use store::history_import::ScanVersion;

    /// The local player of the clean 2-1 win fixture.
    const LOCAL: &str = "Qbxxxxxxxx";

    fn number(state: &AppState, sql: &str) -> i64 {
        lock(&state.reader)
            .query_row(sql, [], |row| row.get(0))
            .expect(sql)
    }

    fn text(state: &AppState, sql: &str) -> String {
        lock(&state.reader)
            .query_row(sql, [], |row| row.get(0))
            .expect(sql)
    }

    /// Deck 5 (`Qdeck`, account Qlocal) with version 7.
    fn deck_version(state: &AppState) -> ScanVersion {
        store::open_writer(&state.db_path)
            .expect("writer")
            .execute_batch(
                "INSERT INTO accounts (username, first_seen_at) VALUES ('Qlocal', 0)
                     ON CONFLICT (username) DO NOTHING;
                 INSERT INTO decks (id, mtgo_id, account_id, name, format, created_at,
                     updated_at, source)
                 VALUES (5, 'q-deck', (SELECT id FROM accounts WHERE username = 'Qlocal'),
                     'Qdeck', 'CMODERN', 0, 0, 'file');
                 INSERT INTO deck_versions (id, deck_id, signature, modified_at)
                     VALUES (7, 5, '', 1790897400000);",
            )
            .expect("a deck version");
        ScanVersion {
            id: 7,
            deck_name: "Qdeck".to_string(),
            signature: String::new(),
        }
    }

    /// A scanned, unpaired 1-2 loss against Qopponent in a Modern queue.
    fn row(history_id: u64, confidence: Option<f64>) -> HistoryScanMatch {
        HistoryScanMatch {
            history_id,
            started_at_ms: Some(1_767_000_000_000 + history_id as i64),
            opponent: "Qopponent".to_string(),
            format: "CMODERN".to_string(),
            format_display: "Modern".to_string(),
            games_won: 1,
            games_lost: 2,
            outcome: ScanOutcome::Loss,
            game_log_token: None,
            game_log_path: None,
            confidence,
            round: 0,
            description: String::new(),
            game_ids: Vec::new(),
            local_player: None,
        }
    }

    /// The latest scan's id, 0 without one.
    fn scan_id(state: &AppState) -> u32 {
        state.history.current().map(|run| run.id).unwrap_or(0)
    }

    /// A complete scan of `version` holding `rows`.
    fn scanned(
        state: &AppState,
        version: ScanVersion,
        rows: Vec<HistoryScanMatch>,
    ) -> Arc<ScanRun> {
        let run = state.history.begin(version);
        assert!(run.complete(rows));
        run
    }

    /// The clean 2-1 win fixture written as a game log, and a row paired
    /// with it: its token, path, local player and opponent.
    fn paired_row(dir: &TestDir, history_id: u64) -> HistoryScanMatch {
        let bytes = load_game_log("clean_2_1_win");
        let log = parse_game_log(&bytes, chrono_tz::UTC).expect("parses");
        let opponent = detect_players(&log.entries)
            .into_iter()
            .find(|player| player != LOCAL)
            .expect("an opponent");
        let path = dir.write("mtgo/Match_GameLog_q.dat", &bytes);
        let mut paired = row(history_id, Some(0.9));
        paired.opponent = opponent;
        paired.games_won = 2;
        paired.games_lost = 1;
        paired.outcome = ScanOutcome::Win;
        paired.round = 2;
        paired.game_ids = vec![11, 12, 13];
        paired.game_log_token = Some(log.match_uuid);
        paired.game_log_path = Some(path.to_string_lossy().into_owned());
        paired.local_player = Some(LOCAL.to_string());
        paired
    }

    #[test]
    fn a_paired_match_is_imported_with_its_log_games_under_the_log_token() {
        let dir = TestDir::new("history-import-paired");
        let (state, _events) = ready_state(&dir);
        let version = deck_version(&state);
        let paired = paired_row(&dir, 501);
        let token = paired.game_log_token.clone().expect("token");
        let run = scanned(&state, version, vec![paired.clone(), row(502, None)]);

        let result =
            import_chosen(&state, scan_id(&state), &ImportChoice::Ids(vec![501])).expect("import");
        assert_eq!(
            result,
            HistoryImportResult {
                imported: 1,
                skipped: 0
            }
        );
        assert_eq!(
            text(
                &state,
                "SELECT concat_ws(',', m.token, m.mtgo_match_id, a.username, o.username,
                     m.source, m.format, m.match_type, m.state, m.outcome,
                     m.games_won_imported, m.games_lost_imported)
                 FROM matches m JOIN accounts a ON a.id = m.account_id
                 JOIN opponents o ON o.id = m.opponent_id"
            ),
            format!(
                "{token},501,{LOCAL},{},history,CMODERN,League,complete,win,2,1",
                paired.opponent
            )
        );
        assert_eq!(
            text(
                &state,
                "SELECT group_concat(row, ';') FROM (SELECT game_number || ':' ||
                     mtgo_game_id || ':' || result AS row FROM games ORDER BY game_number)"
            ),
            "1:11:win;2:12:loss;3:13:win"
        );
        assert_eq!(
            number(
                &state,
                "SELECT COUNT(*) FROM matches m WHERE m.ended_at =
                     (SELECT MAX(ended_at) FROM games)"
            ),
            1,
            "the match ends with its last game"
        );
        assert_eq!(
            text(
                &state,
                "SELECT token || ',' || deck_version_id FROM matches"
            ),
            format!("{token},7")
        );
        assert_eq!(
            text(&state, "SELECT token FROM matches WHERE source = 'history'"),
            token
        );
        assert!(run.row(501).is_none(), "the imported row leaves the scan");
        assert!(run.row(502).is_some(), "the others stay");
        assert_eq!(
            store::history_import::history_imported_count(&lock(&state.reader)).expect("count"),
            1
        );
        crate::commands::stop_tracker(&state);
    }

    #[test]
    fn an_unpaired_match_keeps_the_history_counts_under_a_new_token() {
        let dir = TestDir::new("history-import-unpaired");
        let (state, _events) = ready_state(&dir);
        let version = deck_version(&state);
        scanned(&state, version, vec![row(601, None)]);

        let result = import_chosen(&state, scan_id(&state), &ImportChoice::Ids(vec![601, 999]))
            .expect("import");
        assert_eq!(result.imported, 1);
        assert_eq!(result.skipped, 0, "an id the scan does not have is ignored");
        assert_eq!(number(&state, "SELECT COUNT(*) FROM games"), 0);
        assert_eq!(
            text(
                &state,
                "SELECT concat_ws(',', length(m.token), a.username, m.match_type, m.outcome,
                     m.games_won_imported, m.games_lost_imported, m.ended_at IS NULL)
                 FROM matches m JOIN accounts a ON a.id = m.account_id"
            ),
            "36,Qlocal,Constructed,loss,1,2,1",
            "a token from the match id; the deck's account"
        );
        crate::commands::stop_tracker(&state);
    }

    #[test]
    fn a_stored_match_id_or_token_is_skipped_and_its_row_stays_listed() {
        let dir = TestDir::new("history-import-duplicates");
        let (state, _events) = ready_state(&dir);
        let version = deck_version(&state);
        let paired = paired_row(&dir, 701);
        scanned(&state, version.clone(), vec![paired.clone()]);
        assert_eq!(
            import_chosen(&state, scan_id(&state), &ImportChoice::Ids(vec![701]))
                .expect("first")
                .imported,
            1
        );

        // A rescan offering the same match again, and another match whose
        // log token is already stored.
        let mut same_token = row(702, None);
        same_token.game_log_token = paired.game_log_token.clone();
        let run = scanned(&state, version, vec![paired, same_token]);
        let result = import_chosen(&state, scan_id(&state), &ImportChoice::Ids(vec![701, 702]))
            .expect("again");
        assert_eq!(
            result,
            HistoryImportResult {
                imported: 0,
                skipped: 2
            }
        );
        assert_eq!(number(&state, "SELECT COUNT(*) FROM matches"), 1);
        assert!(
            run.row(701).is_some() && run.row(702).is_some(),
            "nothing imported: the rows stay, as in 0.x"
        );
        crate::commands::stop_tracker(&state);
    }

    #[test]
    fn import_all_takes_only_the_rows_through_the_filters() {
        let dir = TestDir::new("history-import-all");
        let (state, _events) = ready_state(&dir);
        let version = deck_version(&state);
        let mut pauper = row(803, Some(0.9));
        pauper.format = "CPAUPER".to_string();
        pauper.format_display = "Pauper".to_string();
        let run = scanned(
            &state,
            version,
            vec![
                row(801, Some(0.8)),
                row(802, Some(0.3)),
                pauper,
                row(804, None),
            ],
        );
        let filters = HistoryImportFilters {
            format: Some("Modern".to_string()),
            min_confidence: Some(0.5),
        };
        let result = import_chosen(&state, scan_id(&state), &ImportChoice::Filtered(filters))
            .expect("import");
        assert_eq!(result.imported, 1);
        assert_eq!(number(&state, "SELECT mtgo_match_id FROM matches"), 801);
        let left: Vec<u64> = run
            .rows_filtered(None, None)
            .iter()
            .map(|kept| kept.history_id)
            .collect();
        assert_eq!(left, vec![802, 803, 804]);

        // No filters: every row left.
        let all = import_chosen(
            &state,
            scan_id(&state),
            &ImportChoice::Filtered(HistoryImportFilters::default()),
        )
        .expect("import all");
        assert_eq!(all.imported, 3);
        assert!(run.rows_filtered(None, None).is_empty());
        crate::commands::stop_tracker(&state);
    }

    #[test]
    fn deleting_removes_only_the_history_imports() {
        let dir = TestDir::new("history-import-delete");
        let (state, _events) = ready_state(&dir);
        let version = deck_version(&state);
        scanned(&state, version, vec![row(901, None), row(902, None)]);
        import_chosen(&state, scan_id(&state), &ImportChoice::Ids(vec![901, 902])).expect("import");
        // A 0.x import: not the history importer's.
        store::open_writer(&state.db_path)
            .expect("writer")
            .execute_batch(
                "INSERT INTO matches (token, mtgo_match_id, account_id, source, state, outcome,
                     started_at, last_activity_at, revision, derived_version)
                 VALUES ('q-0x', 41, (SELECT id FROM accounts WHERE username = 'Qlocal'),
                     'imported', 'complete', 'win', 1, 1, 1, 0);",
            )
            .expect("a 0.x import");

        assert_eq!(delete_imports(&state).expect("delete"), 2);
        assert_eq!(
            text(&state, "SELECT group_concat(token) FROM matches"),
            "q-0x"
        );
        assert_eq!(delete_imports(&state).expect("again"), 0);
        crate::commands::stop_tracker(&state);
    }

    #[test]
    fn one_import_or_delete_runs_at_a_time_and_an_import_needs_a_scan() {
        let dir = TestDir::new("history-import-busy");
        let (state, _events) = ready_state(&dir);
        assert_eq!(
            import_chosen(&state, scan_id(&state), &ImportChoice::Ids(vec![1])),
            Err(CommandError::Refused(NO_SCAN.to_string()))
        );
        let version = deck_version(&state);
        scanned(&state, version, vec![row(1, None)]);
        let slot = state.history.claim_import().expect("free");
        assert_eq!(
            import_chosen(&state, scan_id(&state), &ImportChoice::Ids(vec![1])),
            Err(CommandError::Refused(BUSY.to_string()))
        );
        assert_eq!(
            delete_imports(&state),
            Err(CommandError::Refused(BUSY.to_string()))
        );
        drop(slot);
        assert_eq!(
            import_chosen(&state, scan_id(&state), &ImportChoice::Ids(vec![1]))
                .expect("free again")
                .imported,
            1
        );
        crate::commands::stop_tracker(&state);
    }

    #[test]
    fn an_import_for_a_scan_that_was_replaced_is_refused() {
        let dir = TestDir::new("history-import-scan-changed");
        let (state, _events) = ready_state(&dir);
        let version = deck_version(&state);
        let viewed = scanned(&state, version.clone(), vec![row(1, None)]);
        let current = scanned(&state, version, vec![row(1, None)]);
        assert_ne!(viewed.id, current.id);
        for choice in [
            ImportChoice::Ids(vec![1]),
            ImportChoice::Filtered(HistoryImportFilters::default()),
        ] {
            assert_eq!(
                import_chosen(&state, viewed.id, &choice),
                Err(CommandError::Refused(SCAN_CHANGED.to_string()))
            );
        }
        assert_eq!(number(&state, "SELECT COUNT(*) FROM matches"), 0);
        assert!(current.row(1).is_some(), "the current scan is untouched");
        assert_eq!(
            import_chosen(&state, current.id, &ImportChoice::Ids(vec![1]))
                .expect("the current scan")
                .imported,
            1
        );
        crate::commands::stop_tracker(&state);
    }

    #[test]
    fn a_scan_panic_shows_a_plain_message_without_its_text() {
        let payload: Box<dyn std::any::Any + Send> = Box::new("Q boom at src/scan.rs:9");
        let message = scan_panic_message(&*payload);
        assert_eq!(message, SCAN_PANICKED);
        assert!(!message.contains("Q boom"), "no panic text: {message}");
    }

    #[test]
    fn a_scan_that_cannot_open_the_database_shows_no_path() {
        let dir = TestDir::new("history-scan-no-database");
        let (mut state, _events) = ready_state(&dir);
        deck_version(&state);
        let (run, input) = begin_scan(&state, 7).expect("begun");
        let real_path = state.db_path.clone();
        state.db_path = dir.join("q-missing-folder").join("mymtgo.sqlite");
        let mut reports = Vec::new();
        run_in_background(&state, &run, &input, &mut |status| reports.push(status));
        let error = run.status().error.expect("failed");
        assert_eq!(error, SCAN_DATABASE_FAILED);
        assert!(!error.contains("q-missing-folder"), "no path: {error}");
        assert_eq!(
            reports.last().map(|status| status.error.clone()),
            Some(Some(SCAN_DATABASE_FAILED.to_string()))
        );
        state.db_path = real_path;
        crate::commands::stop_tracker(&state);
    }

    #[test]
    fn a_scanned_row_reaches_the_page_without_its_log_path() {
        let dir = TestDir::new("history-row-no-path");
        let paired = paired_row(&dir, 501);
        assert!(paired.game_log_path.is_some(), "kept server side");
        let sent = serde_json::to_value(&paired).expect("serialises");
        let keys: Vec<&String> = sent.as_object().expect("an object").keys().collect();
        assert!(
            !keys.iter().any(|key| key.as_str() == "gameLogPath"),
            "keys: {keys:?}"
        );
        assert!(!sent.to_string().contains("Match_GameLog"), "{sent}");
    }

    #[test]
    fn the_cards_dialog_names_cards_from_the_cards_table_then_the_log() {
        let dir = TestDir::new("history-scan-cards");
        let (state, _events) = ready_state(&dir);
        let version = deck_version(&state);
        let paired = paired_row(&dir, 501);
        let log = parse_game_log(&load_game_log("clean_2_1_win"), chrono_tz::UTC).expect("log");
        let read = extract_log_cards(&log.entries);
        let local_ids: Vec<u32> = read
            .match_cards(LOCAL)
            .iter()
            .map(|card| card.mtgo_id)
            .collect();
        let opponent_count = read.match_cards(&paired.opponent).len();
        assert!(!local_ids.is_empty() && opponent_count > 0);
        store::open_writer(&state.db_path)
            .expect("writer")
            .execute(
                "INSERT INTO cards (catalog_id, name) VALUES (?1, 'AAA Q named card')",
                [local_ids[0]],
            )
            .expect("a card row");
        // The opponent named in another case still finds its cards.
        let mut shouted = paired.clone();
        shouted.opponent = shouted.opponent.to_ascii_uppercase();
        scanned(&state, version, vec![shouted, row(502, None)]);

        let cards = scan_cards(&state, 501).expect("cards");
        assert_eq!(cards.local_cards.len(), local_ids.len());
        assert_eq!(cards.opponent_cards.len(), opponent_count);
        assert_eq!(
            cards.local_cards[0],
            HistoryScanCard {
                mtgo_id: local_ids[0],
                name: "AAA Q named card".to_string()
            },
            "the cards table's name, sorted first"
        );
        for list in [&cards.local_cards, &cards.opponent_cards] {
            let names: Vec<&str> = list.iter().map(|card| card.name.as_str()).collect();
            let mut sorted = names.clone();
            sorted.sort();
            assert_eq!(names, sorted, "by name");
            assert!(names.iter().all(|name| !name.is_empty()));
        }
        assert_eq!(
            scan_cards(&state, 502).expect("unpaired"),
            HistoryScanCards::default()
        );
        assert_eq!(
            scan_cards(&state, 999).expect("missing"),
            HistoryScanCards::default()
        );
        crate::commands::stop_tracker(&state);
    }

    #[test]
    fn deck_versions_list_archived_decks_with_a_local_date() {
        let dir = TestDir::new("history-deck-versions");
        let (state, _events) = ready_state(&dir);
        deck_version(&state);
        store::open_writer(&state.db_path)
            .expect("writer")
            .execute("UPDATE decks SET archived_at = 1", [])
            .expect("archive");
        assert_eq!(
            deck_versions(&state).expect("versions"),
            vec![HistoryDeckVersionView {
                id: 7,
                deck_name: "Qdeck".to_string(),
                archived: true,
                // 2026-10-01T23:30:00Z in UTC.
                modified_at: "01/10/2026".to_string(),
                format: "CMODERN".to_string(),
                current_account: true,
                limited: false,
            }]
        );
        crate::commands::stop_tracker(&state);
    }

    // Issue #332: "Set deck" offers the current account's decks (and decks
    // no account has adopted), never a Limited one.
    #[test]
    fn deck_versions_say_whose_deck_and_whether_it_is_limited() {
        let dir = TestDir::new("history-deck-versions-owner");
        let (state, _events) = ready_state(&dir);
        deck_version(&state);
        store::open_writer(&state.db_path)
            .expect("writer")
            .execute_batch(
                "INSERT INTO accounts (username, first_seen_at) VALUES ('Qother', 0);
                 INSERT INTO settings (key, value)
                     SELECT 'active_account',
                         '{\"account_id\":' || id || ',\"switched_at_ms\":1}'
                     FROM accounts WHERE username = 'Qlocal'
                     ON CONFLICT (key) DO UPDATE SET value = excluded.value;
                 INSERT INTO decks (id, mtgo_id, account_id, name, format, created_at,
                     updated_at, source)
                 VALUES (6, 'q-other', (SELECT id FROM accounts WHERE username = 'Qother'),
                     'Qtheirs', 'CMODERN', 0, 0, 'file'),
                     (8, 'limited:q-pool', NULL, 'Qpool', 'Limited', 0, 0, 'file');
                 INSERT INTO deck_versions (id, deck_id, signature, modified_at)
                     VALUES (9, 6, '', 1), (10, 8, '', 2);",
            )
            .expect("decks");
        let flags: Vec<(u32, bool, bool)> = deck_versions(&state)
            .expect("versions")
            .into_iter()
            .map(|view| (view.id, view.current_account, view.limited))
            .collect();
        assert_eq!(
            flags,
            vec![(7, true, false), (10, true, true), (9, false, false)]
        );
        crate::commands::stop_tracker(&state);
    }

    #[test]
    fn an_unknown_deck_version_is_refused() {
        let dir = TestDir::new("history-begin-unknown");
        let (state, _events) = ready_state(&dir);
        let refused = begin_scan(&state, 99).err();
        assert_eq!(
            refused,
            Some(CommandError::Refused(UNKNOWN_VERSION.to_string()))
        );
        assert!(state.history.status().is_none());
        crate::commands::stop_tracker(&state);
    }

    #[test]
    fn a_scan_runs_with_the_app_state_and_the_chosen_data_folder() {
        let dir = TestDir::new("history-begin-run");
        let (state, _events) = ready_state(&dir);
        let mut settings = read_settings(&state).expect("settings");
        settings.log_folder = Some(dir.join("logs-a").to_string_lossy().into_owned());
        settings.mtgo_data_folder = Some(dir.join("mtgo").to_string_lossy().into_owned());
        crate::commands::apply_settings(&state, settings).expect("saved");
        {
            let writer = store::open_writer(&state.db_path).expect("writer");
            writer
                .execute_batch(
                    "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source)
                         VALUES (5, 'q-deck', 'Qdeck', 'CMODERN', 0, 0, 'file');
                     INSERT INTO deck_versions (id, deck_id, signature, modified_at)
                         VALUES (7, 5, '', 0);",
                )
                .expect("a deck version");
        }
        let (run, input) = begin_scan(&state, 7).expect("begun");
        assert_eq!(input.tz, state.timezone);
        assert_eq!(input.data_folder, Some(dir.join("mtgo")));
        let mut reports = Vec::new();
        run_in_background(&state, &run, &input, &mut |status| reports.push(status));
        assert_eq!(
            state.history.status().map(|status| status.status),
            Some(ScanState::Complete)
        );
        assert_eq!(
            reports.last().map(|status| status.status),
            Some(ScanState::Complete)
        );
        crate::commands::stop_tracker(&state);
    }
}
