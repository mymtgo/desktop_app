//! The history scan (0.x `DiscoverGameLogsJob`, `DecodeGameLogsJob`,
//! `ParseAndFilterHistoryJob` and `MatchAndScoreJob`): find MTGO's game log
//! files and read each one's first entry time and players once (kept by
//! path, size and modified time, so a rescan skips unchanged files), read
//! and merge the `mtgo_game_history` files, drop the matches already
//! stored, then pair each record with its game log and score it against the
//! chosen deck version. The result lives in app memory: one scan at a time,
//! a new scan replaces the old one.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use chrono_tz::Tz;
use game_detail::{GameLogEntry, detect_players, extract_log_cards, parse_game_log};
use mtgo_history::{HistoryRecord, Verdict, local_verdict, merge_records, parse_history};
use mtgo_log::{Env, find_history_files, game_log_token, list_game_logs};
use serde::{Deserialize, Serialize};
use store::history_import::{ScanVersion, stored_match_tokens, stored_mtgo_match_ids};
use store::{Connection, StoreError};

use crate::bridge::lock;
use crate::history_import::pairing::{
    CandidateLog, DeckOracles, confidence, local_player, pair_records, record_opponent,
};
use crate::matches::MatchSortDir;
use crate::matches::log_cache::{FileStamp, stamp_of};

/// Progress is reported at least this often (in files or records), and at
/// every stage's end.
pub const PROGRESS_EVERY: u32 = 100;

/// A results page holds this many rows unless asked otherwise.
pub const DEFAULT_PER_PAGE: u32 = 50;

/// And at most this many.
pub const MAX_PER_PAGE: u32 = 100;

/// 0.x counted a confidence of at least this as high.
pub const HIGH_CONFIDENCE: f64 = 0.6;

/// What a scan is doing (0.x `import_scans.stage`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum ScanStage {
    Discovering,
    Decoding,
    Parsing,
    Scoring,
}

/// Where a scan is (0.x `import_scans.status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum ScanState {
    Processing,
    Complete,
    Cancelled,
    Failed,
}

/// A history match's result from its game counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum ScanOutcome {
    Win,
    Loss,
    Draw,
}

impl ScanOutcome {
    /// More games won is a win, fewer a loss, equal a draw.
    pub fn from_games(won: u32, lost: u32) -> Self {
        if won > lost {
            ScanOutcome::Win
        } else if won < lost {
            ScanOutcome::Loss
        } else {
            ScanOutcome::Draw
        }
    }

    /// MTGO's verdict where the record names its opponent among the match
    /// winners or losers (#454: a match left at a level score has a winner),
    /// else the game counts (0.x read only the counts).
    pub fn from_record(record: &HistoryRecord) -> Self {
        let verdict = record_opponent(record).and_then(|opponent| local_verdict(record, opponent));
        match verdict {
            Some(Verdict::Win) => ScanOutcome::Loss,
            Some(Verdict::Loss) => ScanOutcome::Win,
            None => ScanOutcome::from_games(record.game_wins, record.game_losses),
        }
    }

    /// The text 0.x stored and sorted by.
    pub fn as_str(self) -> &'static str {
        match self {
            ScanOutcome::Win => "win",
            ScanOutcome::Loss => "loss",
            ScanOutcome::Draw => "draw",
        }
    }
}

/// One scanned history match (0.x `import_scan_matches` row).
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryScanMatch {
    /// MTGO's match id.
    #[specta(type = specta_typescript::Number)]
    pub history_id: u64,
    /// UTC ms; `None` when MTGO's time was out of range.
    #[specta(type = Option<specta_typescript::Number>)]
    pub started_at_ms: Option<i64>,
    /// `Opponents[0]`, or "Unknown".
    pub opponent: String,
    /// The format code (`CMODERN`), empty when there is none.
    pub format: String,
    /// The format for display (`Modern`).
    pub format_display: String,
    pub games_won: u32,
    pub games_lost: u32,
    pub outcome: ScanOutcome,
    /// The paired game log's match token.
    pub game_log_token: Option<String>,
    /// The paired game log file. Kept server side (the cards dialog and the
    /// import read it), never sent to the page.
    #[serde(skip)]
    pub game_log_path: Option<String>,
    /// 0 to 1 in hundredths; `None` without a log or anything to score.
    pub confidence: Option<f64>,
    pub round: u32,
    pub description: String,
    #[specta(type = Vec<specta_typescript::Number>)]
    pub game_ids: Vec<u64>,
    /// The paired log's local player.
    pub local_player: Option<String>,
}

/// 0.x `confidence_stats`: at least `HIGH_CONFIDENCE`, below it, and none.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ConfidenceStats {
    pub high: u32,
    pub low: u32,
    pub none: u32,
}

/// A scan's state for the import page (0.x `ScanStatusController`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryScanStatus {
    pub scan_id: u32,
    pub deck_version_id: u32,
    pub deck_name: String,
    pub status: ScanState,
    pub stage: ScanStage,
    pub progress: u32,
    pub total: u32,
    pub error: Option<String>,
    /// The rest is filled once the scan is complete.
    pub match_count: u32,
    /// Distinct display formats, sorted.
    pub formats: Vec<String>,
    pub confidence_stats: Option<ConfidenceStats>,
}

/// How the results are sorted (0.x `sort`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum HistorySort {
    /// Scored rows first, then by confidence.
    #[default]
    Confidence,
    StartedAt,
    Opponent,
    /// By display format.
    Format,
    Outcome,
}

/// A results page request (0.x `ScanMatchesController` query).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryScanQuery {
    /// A display format; blank for all.
    pub format: Option<String>,
    /// Above 0, only scored rows at least this confident.
    pub min_confidence: Option<f64>,
    /// Confidence when absent.
    pub sort: Option<HistorySort>,
    /// Descending when absent.
    pub dir: Option<MatchSortDir>,
    /// 1-based; 1 when absent.
    pub page: Option<u32>,
    /// `DEFAULT_PER_PAGE` when absent, at most `MAX_PER_PAGE`.
    pub per_page: Option<u32>,
}

/// One page of results.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryScanPage {
    pub rows: Vec<HistoryScanMatch>,
    /// Rows through the filters.
    pub total: u32,
    pub page: u32,
    pub per_page: u32,
    /// At least 1.
    pub last_page: u32,
}

/// Where the scan looks and how it reads times.
#[derive(Debug, Clone)]
pub struct ScanInput {
    pub env: Env,
    /// The chosen MTGO data folder, else the detected ones are searched.
    pub data_folder: Option<PathBuf>,
    /// MTGO writes local wall-clock times: the OS timezone.
    pub tz: Tz,
}

// ---- Game log index ------------------------------------------------------------

/// The cards one player was seen with in a log, as catalog ids.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PlayerCatalogIds {
    player: String,
    catalog_ids: Vec<u32>,
}

/// What a game log file holds, as far as the scan needs it.
#[derive(Debug, Clone)]
struct IndexedFile {
    stamp: FileStamp,
    tz: Tz,
    token: Option<String>,
    /// `None` when the file has no entries (or could not be read).
    first_entry_ms: Option<i64>,
    players: Vec<String>,
    /// Read when the file is first paired.
    cards: Option<Vec<PlayerCatalogIds>>,
}

/// Every game log file's first entry time and players (0.x kept them in
/// its `game_logs` table), by path, so a rescan reads only new or changed
/// files.
#[derive(Debug, Default)]
pub struct GameLogIndex {
    files: HashMap<PathBuf, IndexedFile>,
    /// How many times a file was read, for tests and logs.
    reads: u64,
}

impl GameLogIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many file reads the index has made.
    pub fn reads(&self) -> u64 {
        self.reads
    }

    /// How many files are indexed.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// The listed files that are new or changed since they were read (size,
    /// modified time or timezone). Files no longer listed are forgotten.
    pub fn stale(&mut self, paths: &[PathBuf], tz: Tz) -> Vec<PathBuf> {
        let listed: HashSet<&Path> = paths.iter().map(PathBuf::as_path).collect();
        self.files.retain(|path, _| listed.contains(path.as_path()));
        let mut stale = Vec::new();
        for path in paths {
            let unchanged = match (self.files.get(path), stamp_of(path)) {
                (Some(file), Some(stamp)) => file.stamp == stamp && file.tz == tz,
                _ => false,
            };
            if !unchanged {
                stale.push(path.clone());
            }
        }
        stale
    }

    /// Read one file and index it. An unreadable or empty file is indexed
    /// with no entries, so it is not read again while unchanged.
    pub fn read(&mut self, path: &Path, tz: Tz) {
        let Some(stamp) = stamp_of(path) else {
            self.files.remove(path);
            return;
        };
        let entries = self.read_entries(path, tz);
        self.files.insert(
            path.to_path_buf(),
            IndexedFile {
                stamp,
                tz,
                token: game_log_token(path),
                first_entry_ms: entries.first().map(|entry| entry.timestamp_ms),
                players: detect_players(&entries),
                cards: None,
            },
        );
    }

    /// The indexed files that can be paired: those with entries and a token.
    pub fn candidates(&self) -> Vec<CandidateLog> {
        let mut candidates = Vec::new();
        for (path, file) in &self.files {
            if let (Some(token), Some(first_entry_ms)) = (&file.token, file.first_entry_ms) {
                candidates.push(CandidateLog {
                    path: path.clone(),
                    token: token.clone(),
                    first_entry_ms,
                    players: file.players.clone(),
                });
            }
        }
        candidates
    }

    /// The catalog ids of `player`'s cards in the log at `path` (0.x
    /// `ExtractCardsFromGameLog`'s `cards_by_player`). The file is read the
    /// first time and its cards kept.
    pub fn player_catalog_ids(&mut self, path: &Path, tz: Tz, player: &str) -> Vec<u32> {
        let needs_read = match self.files.get(path) {
            Some(file) => file.cards.is_none(),
            None => return Vec::new(),
        };
        if needs_read {
            let entries = self.read_entries(path, tz);
            let cards = extract_log_cards(&entries)
                .cards_by_player
                .into_iter()
                .map(|list| PlayerCatalogIds {
                    player: list.player,
                    catalog_ids: list.cards.iter().map(|card| card.mtgo_id).collect(),
                })
                .collect();
            if let Some(file) = self.files.get_mut(path) {
                file.cards = Some(cards);
            }
        }
        self.files
            .get(path)
            .and_then(|file| file.cards.as_ref())
            .and_then(|cards| cards.iter().find(|list| list.player == player))
            .map(|list| list.catalog_ids.clone())
            .unwrap_or_default()
    }

    /// The file's entries, none when it cannot be read or parsed.
    fn read_entries(&mut self, path: &Path, tz: Tz) -> Vec<GameLogEntry> {
        self.reads += 1;
        match std::fs::read(path) {
            Ok(bytes) => parse_game_log(&bytes, tz)
                .map(|log| log.entries)
                .unwrap_or_default(),
            Err(error) => {
                tracing::warn!(%error, "could not read a game log file");
                Vec::new()
            }
        }
    }
}

// ---- Scans ---------------------------------------------------------------------

/// What a run has reached.
#[derive(Debug, Clone)]
struct RunState {
    status: ScanState,
    stage: ScanStage,
    progress: u32,
    total: u32,
    error: Option<String>,
    results: Vec<HistoryScanMatch>,
}

/// One scan.
#[derive(Debug)]
pub struct ScanRun {
    pub id: u32,
    pub version: ScanVersion,
    cancelled: AtomicBool,
    state: Mutex<RunState>,
}

impl ScanRun {
    fn new(id: u32, version: ScanVersion) -> Self {
        Self {
            id,
            version,
            cancelled: AtomicBool::new(false),
            state: Mutex::new(RunState {
                status: ScanState::Processing,
                stage: ScanStage::Discovering,
                progress: 0,
                total: 0,
                error: None,
                results: Vec::new(),
            }),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// The run's status for the page.
    pub fn status(&self) -> HistoryScanStatus {
        let state = lock(&self.state);
        let complete = state.status == ScanState::Complete;
        let mut formats = Vec::new();
        let mut stats = ConfidenceStats::default();
        if complete {
            for row in &state.results {
                if !formats.contains(&row.format_display) {
                    formats.push(row.format_display.clone());
                }
                match row.confidence {
                    Some(confidence) if hundredths(confidence) >= hundredths(HIGH_CONFIDENCE) => {
                        stats.high += 1
                    }
                    Some(_) => stats.low += 1,
                    None => stats.none += 1,
                }
            }
            formats.sort();
        }
        HistoryScanStatus {
            scan_id: self.id,
            deck_version_id: u32::try_from(self.version.id).unwrap_or(0),
            deck_name: self.version.deck_name.clone(),
            status: state.status,
            stage: state.stage,
            progress: state.progress,
            total: state.total,
            error: state.error.clone(),
            match_count: if complete {
                count(state.results.len())
            } else {
                0
            },
            formats,
            confidence_stats: complete.then_some(stats),
        }
    }

    /// End the run as failed, unless it already ended or was cancelled.
    /// `false` when nothing changed (so there is nothing to report).
    pub fn fail(&self, message: &str) -> bool {
        self.update(|state| {
            state.status = ScanState::Failed;
            state.error = Some(message.to_string());
        })
    }

    /// Stop the run: it ends as cancelled at its next file or record.
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let mut state = lock(&self.state);
        if state.status == ScanState::Processing {
            state.status = ScanState::Cancelled;
        }
    }

    fn is_processing(&self) -> bool {
        lock(&self.state).status == ScanState::Processing
    }

    /// Change a run that is still processing; `false` when it has ended.
    fn update(&self, change: impl FnOnce(&mut RunState)) -> bool {
        let mut state = lock(&self.state);
        if state.status != ScanState::Processing || self.is_cancelled() {
            return false;
        }
        change(&mut state);
        true
    }

    fn enter(&self, stage: ScanStage, total: usize) -> bool {
        self.update(|state| {
            state.stage = stage;
            state.progress = 0;
            state.total = count(total);
        })
    }

    pub(crate) fn complete(&self, results: Vec<HistoryScanMatch>) -> bool {
        self.update(|state| {
            state.status = ScanState::Complete;
            state.results = results;
        })
    }

    /// The results with these history ids, in scan order.
    pub fn rows_with_ids(&self, ids: &[u64]) -> Vec<HistoryScanMatch> {
        let wanted: HashSet<u64> = ids.iter().copied().collect();
        lock(&self.state)
            .results
            .iter()
            .filter(|row| wanted.contains(&row.history_id))
            .cloned()
            .collect()
    }

    /// The results through the page's format and confidence filters, in scan
    /// order (0.x `ImportAllController`).
    pub fn rows_filtered(
        &self,
        format: Option<&str>,
        min_confidence: Option<f64>,
    ) -> Vec<HistoryScanMatch> {
        filtered(&lock(&self.state).results, format, min_confidence)
            .into_iter()
            .cloned()
            .collect()
    }

    /// One result, `None` when the scan has no such row.
    pub fn row(&self, history_id: u64) -> Option<HistoryScanMatch> {
        lock(&self.state)
            .results
            .iter()
            .find(|row| row.history_id == history_id)
            .cloned()
    }

    /// Drop these rows from the results: they were imported (0.x
    /// `ImportMatchesJob` deletes them from the scan).
    pub fn forget_rows(&self, ids: &HashSet<u64>) {
        lock(&self.state)
            .results
            .retain(|row| !ids.contains(&row.history_id));
    }
}

/// Holds the one import (or delete) slot while it lives.
pub struct ImportSlot<'a>(&'a AtomicBool);

impl Drop for ImportSlot<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// The app's scans: the latest one and the game log index they share.
#[derive(Debug, Default)]
pub struct HistoryScans {
    /// Held for the whole of a run, so two runs never overlap.
    index: Mutex<GameLogIndex>,
    current: Mutex<Option<Arc<ScanRun>>>,
    next_id: AtomicU32,
    /// An import or a delete is running (one at a time).
    importing: AtomicBool,
}

impl HistoryScans {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a scan of `version`: a running scan is cancelled and replaced
    /// (0.x `ScanController`). The caller runs it with `run_scan`.
    pub fn begin(&self, version: ScanVersion) -> Arc<ScanRun> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let run = Arc::new(ScanRun::new(id, version));
        let mut current = lock(&self.current);
        if let Some(old) = current.take() {
            old.cancel();
        }
        *current = Some(Arc::clone(&run));
        run
    }

    /// The latest scan.
    pub fn current(&self) -> Option<Arc<ScanRun>> {
        lock(&self.current).clone()
    }

    /// The latest scan's status, `None` when there is none.
    pub fn status(&self) -> Option<HistoryScanStatus> {
        self.current().map(|run| run.status())
    }

    /// 0.x `CancelScanController`: a running scan is cancelled (it stops at
    /// its next file or record); an ended one is forgotten. Returns the
    /// status afterwards.
    pub fn cancel(&self) -> Option<HistoryScanStatus> {
        let mut current = lock(&self.current);
        let run = current.clone()?;
        if run.is_processing() {
            run.cancel();
            Some(run.status())
        } else {
            *current = None;
            None
        }
    }

    /// One page of the latest scan's results; empty when there are none.
    pub fn page(&self, query: &HistoryScanQuery) -> HistoryScanPage {
        match self.current() {
            Some(run) => page_of(&lock(&run.state).results, query),
            None => page_of(&[], query),
        }
    }

    /// Claim the one import slot; `None` while an import or delete runs.
    pub fn claim_import(&self) -> Option<ImportSlot<'_>> {
        self.importing
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| ImportSlot(&self.importing))
    }

    /// How many file reads the game log index has made.
    pub fn index_reads(&self) -> u64 {
        lock(&self.index).reads()
    }
}

/// Run `run` to its end on this thread: discover and index the game logs,
/// read the history, drop stored matches, pair and score. `conn` reads the
/// database. Every stage change and every `PROGRESS_EVERY` steps go to
/// `report`. A cancelled run stops at its next file or record.
pub fn run_scan(
    scans: &HistoryScans,
    run: &ScanRun,
    input: &ScanInput,
    conn: &Connection,
    report: &mut dyn FnMut(HistoryScanStatus),
) {
    let mut index = lock(&scans.index);
    if let Err(error) = scan_steps(&mut index, run, input, conn, report) {
        tracing::warn!(%error, "the history scan could not read the database");
        // The page gets a plain message: an error can name the file.
        if run.fail(crate::history_import::SCAN_DATABASE_FAILED) {
            report(run.status());
        }
    }
}

fn scan_steps(
    index: &mut GameLogIndex,
    run: &ScanRun,
    input: &ScanInput,
    conn: &Connection,
    report: &mut dyn FnMut(HistoryScanStatus),
) -> Result<(), StoreError> {
    let data_folder = input.data_folder.as_deref();

    // Discovering: every game log file.
    if !run.enter(ScanStage::Discovering, 0) {
        return Ok(());
    }
    report(run.status());
    let paths = list_game_logs(&input.env, data_folder);
    if !run.enter(ScanStage::Discovering, paths.len()) {
        return Ok(());
    }
    step(run, paths.len(), paths.len(), report);

    // Decoding: the new and changed ones.
    let stale = index.stale(&paths, input.tz);
    if !run.enter(ScanStage::Decoding, stale.len()) {
        return Ok(());
    }
    report(run.status());
    for (done, path) in stale.iter().enumerate() {
        if run.is_cancelled() {
            return Ok(());
        }
        index.read(path, input.tz);
        step(run, done + 1, stale.len(), report);
    }

    // Parsing: the history, without the matches already stored.
    if !run.enter(ScanStage::Parsing, 0) {
        return Ok(());
    }
    report(run.status());
    let mut files = Vec::new();
    for path in find_history_files(&input.env, data_folder) {
        if run.is_cancelled() {
            return Ok(());
        }
        match std::fs::read(&path) {
            Ok(bytes) => match parse_history(&bytes, input.tz) {
                Ok(records) => files.push(records),
                Err(error) => tracing::warn!(%error, "could not read a game history file"),
            },
            Err(error) => tracing::warn!(%error, "could not read a game history file"),
        }
    }
    let stored = stored_mtgo_match_ids(conn)?;
    let records: Vec<HistoryRecord> = merge_records(files)
        .into_iter()
        .filter(|record| !stored.contains(&record.id))
        .collect();
    if records.is_empty() {
        if run.complete(Vec::new()) {
            report(run.status());
        }
        return Ok(());
    }

    // Scoring: pair each record with its game log and score it.
    if !run.enter(ScanStage::Scoring, records.len()) {
        return Ok(());
    }
    report(run.status());
    let linked = stored_match_tokens(conn)?;
    let candidates = index.candidates();
    let pairs = pair_records(&records, &candidates, &linked);
    let deck = deck_oracles(conn, &run.version.signature)?;
    let mut rows = Vec::with_capacity(records.len());
    for (done, (record, pair)) in records.iter().zip(&pairs).enumerate() {
        if run.is_cancelled() {
            return Ok(());
        }
        let log = pair.map(|position| &candidates[position]);
        rows.push(scored_row(index, conn, input.tz, record, log, &deck)?);
        step(run, done + 1, records.len(), report);
    }
    if run.complete(rows) {
        report(run.status());
    }
    Ok(())
}

/// Record progress, reporting every `PROGRESS_EVERY` steps and the last.
fn step(run: &ScanRun, done: usize, total: usize, report: &mut dyn FnMut(HistoryScanStatus)) {
    let done = count(done);
    if !run.update(|state| state.progress = done) {
        return;
    }
    if done.is_multiple_of(PROGRESS_EVERY) || done == count(total) {
        report(run.status());
    }
}

/// The deck version's cards as oracle ids.
fn deck_oracles(conn: &Connection, signature: &str) -> Result<DeckOracles, StoreError> {
    let catalog_ids: Vec<u32> = decks::decode_signature(signature)
        .iter()
        .map(|card| card.catalog_id)
        .collect();
    let oracle_ids = store::decks::oracle_ids(conn, &catalog_ids)?;
    Ok(DeckOracles {
        has_cards: !catalog_ids.is_empty(),
        oracle_ids: oracle_ids.into_values().collect(),
    })
}

/// A record's row: with a paired log, its local player and the confidence
/// that the local player's cards are the deck's (0.x `MatchAndScoreJob`).
fn scored_row(
    index: &mut GameLogIndex,
    conn: &Connection,
    tz: Tz,
    record: &HistoryRecord,
    log: Option<&CandidateLog>,
    deck: &DeckOracles,
) -> Result<HistoryScanMatch, StoreError> {
    let mut local = None;
    let mut score = None;
    if let (Some(log), Some(opponent)) = (log, record_opponent(record)) {
        local = local_player(&log.players, opponent);
        if let Some(player) = &local {
            let catalog_ids = index.player_catalog_ids(&log.path, tz, player);
            if !catalog_ids.is_empty() {
                let oracle_ids: HashSet<String> = store::decks::oracle_ids(conn, &catalog_ids)?
                    .into_values()
                    .collect();
                score = confidence(&oracle_ids, deck);
            }
        }
    }
    let format = record.format.clone().unwrap_or_default();
    Ok(HistoryScanMatch {
        history_id: record.id,
        started_at_ms: record.start_time_ms,
        opponent: record
            .opponents
            .first()
            .cloned()
            .unwrap_or_else(|| "Unknown".to_string()),
        format_display: decks::display_format(&format),
        format,
        games_won: record.game_wins,
        games_lost: record.game_losses,
        outcome: ScanOutcome::from_record(record),
        game_log_token: log.map(|log| log.token.clone()),
        game_log_path: log.map(|log| log.path.to_string_lossy().into_owned()),
        confidence: score,
        round: record.round,
        description: record.description.clone(),
        game_ids: record.game_ids.clone(),
        local_player: local,
    })
}

/// A confidence in whole hundredths, so filters compare what the page
/// shows (a slider at 0.1 + 0.1 + 0.1 is 0.30000000000000004).
fn hundredths(value: f64) -> i64 {
    (value * 100.0).round() as i64
}

fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// The rows the import page's format and confidence filters keep (0.x
/// `ScanMatchesController` and `ImportAllController`): a blank format keeps
/// every format; a minimum above 0 keeps scored rows at least that
/// confident.
pub fn filtered<'a>(
    rows: &'a [HistoryScanMatch],
    format: Option<&str>,
    min_confidence: Option<f64>,
) -> Vec<&'a HistoryScanMatch> {
    let format = format.filter(|format| !format.is_empty());
    let minimum = min_confidence
        .filter(|minimum| *minimum > 0.0)
        .map(hundredths);
    rows.iter()
        .filter(|row| format.is_none_or(|format| row.format_display == format))
        .filter(|row| match minimum {
            None => true,
            Some(minimum) => row
                .confidence
                .is_some_and(|confidence| hundredths(confidence) >= minimum),
        })
        .collect()
}

/// How two rows compare under `sort` and `dir`. Unscored rows come last in
/// a confidence sort, whatever the direction.
fn compare(
    a: &HistoryScanMatch,
    b: &HistoryScanMatch,
    sort: HistorySort,
    dir: MatchSortDir,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let directed = |ordering: Ordering| match dir {
        MatchSortDir::Asc => ordering,
        MatchSortDir::Desc => ordering.reverse(),
    };
    match sort {
        HistorySort::Confidence => match (a.confidence, b.confidence) {
            (Some(x), Some(y)) => directed(hundredths(x).cmp(&hundredths(y))),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        },
        HistorySort::StartedAt => directed(a.started_at_ms.cmp(&b.started_at_ms)),
        HistorySort::Opponent => directed(a.opponent.cmp(&b.opponent)),
        HistorySort::Format => directed(a.format_display.cmp(&b.format_display)),
        HistorySort::Outcome => directed(a.outcome.as_str().cmp(b.outcome.as_str())),
    }
}

/// One page of `rows` through the query's filters and sort. Rows that
/// compare equal keep the scan's order.
pub fn page_of(rows: &[HistoryScanMatch], query: &HistoryScanQuery) -> HistoryScanPage {
    let mut kept = filtered(rows, query.format.as_deref(), query.min_confidence);
    let sort = query.sort.unwrap_or_default();
    let dir = query.dir.unwrap_or(MatchSortDir::Desc);
    // `sort_by` is stable.
    kept.sort_by(|a, b| compare(a, b, sort, dir));
    let per_page = query
        .per_page
        .unwrap_or(DEFAULT_PER_PAGE)
        .clamp(1, MAX_PER_PAGE);
    let page = query.page.unwrap_or(1).max(1);
    let total = count(kept.len());
    let last_page = total.div_ceil(per_page).max(1);
    let skip = (page as usize - 1).saturating_mul(per_page as usize);
    HistoryScanPage {
        rows: kept
            .into_iter()
            .skip(skip)
            .take(per_page as usize)
            .cloned()
            .collect(),
        total,
        page,
        per_page,
        last_page,
    }
}

#[cfg(test)]
mod tests;
