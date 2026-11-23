use super::*;
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use fixture_tools::dat::{anonymise_dat, read_dat, write_dat};
use fixture_tools::{TestDir, fixtures_dir, load_game_log};
use game_detail::{extract_log_cards, parse_game_log};
use mtgo_history::parse_history;
use mtgo_log::Platform;
use store::import::{ImportedMatch, import_matches};

const UTC: Tz = chrono_tz::UTC;
const ACCOUNT: &str = "mtgo/Q1/A1/acct";

// ---- Helpers --------------------------------------------------------------------

fn history_bytes() -> Vec<u8> {
    std::fs::read(fixtures_dir().join("history/mtgo_game_history")).expect("history fixture")
}

fn history_records() -> Vec<HistoryRecord> {
    parse_history(&history_bytes(), UTC).expect("history parses")
}

/// A fake MTGO data folder and a database beside it.
struct World {
    dir: TestDir,
    conn: Connection,
}

impl World {
    fn new(label: &str) -> Self {
        let dir = TestDir::new(label);
        let conn = store::open_writer(&dir.join("data/mymtgo.sqlite")).expect("database");
        Self { dir, conn }
    }

    fn input(&self) -> ScanInput {
        ScanInput {
            env: Env {
                platform: Platform::Windows,
                home: Some(self.dir.join("home")),
                user: None,
            },
            data_folder: Some(self.dir.join("mtgo")),
            tz: UTC,
        }
    }

    fn write_history(&self) -> PathBuf {
        self.dir
            .write(&format!("{ACCOUNT}/mtgo_game_history"), &history_bytes())
    }

    fn write_log(&self, token: &str, bytes: &[u8]) -> PathBuf {
        self.dir
            .write(&format!("{ACCOUNT}/Match_GameLog_{token}.dat"), bytes)
    }

    /// The deck version 60 of deck "Qdeck", holding `catalog_ids`.
    fn deck_version(&self, catalog_ids: &[u32]) -> ScanVersion {
        let cards: Vec<decks::DeckCard> = catalog_ids
            .iter()
            .map(|&catalog_id| decks::DeckCard {
                catalog_id,
                quantity: 4,
                sideboard: false,
            })
            .collect();
        let signature = decks::signature(&cards);
        self.conn
            .execute_batch(&format!(
                "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source)
                     VALUES (50, 'q-deck', 'Qdeck', 'CMODERN', 0, 0, 'file');
                 INSERT INTO deck_versions (id, deck_id, signature, modified_at)
                     VALUES (60, 50, '{signature}', 0);"
            ))
            .expect("a deck version");
        store::history_import::scan_version(&self.conn, 60)
            .expect("read")
            .expect("the version")
    }

    fn card(&self, catalog_id: u32, oracle_id: &str) {
        self.conn
            .execute(
                "INSERT INTO cards (catalog_id, oracle_id, name) VALUES (?1, ?2, 'Qcard')",
                store::params![catalog_id, oracle_id],
            )
            .expect("a card");
    }

    fn store_match(&mut self, token: &str, mtgo_match_id: Option<i64>) {
        let imported = ImportedMatch {
            token: token.to_string(),
            mtgo_match_id,
            local_username: "Qlocal".to_string(),
            opponent: Some("Qopponent".to_string()),
            format: Some("CMODERN".to_string()),
            match_type: Some("Constructed".to_string()),
            tournament_event_id: None,
            tournament_round: None,
            state: "complete".to_string(),
            outcome: "win".to_string(),
            started_at_ms: 10_000,
            ended_at_ms: Some(14_000),
            note: None,
            games: Vec::new(),
            manual: None,
            games_won_imported: None,
            games_lost_imported: None,
            deck_linked: false,
            replay_share: None,
            legacy_history: false,
        };
        import_matches(&mut self.conn, &[imported], 0).expect("import");
    }

    /// Run a scan to its end; returns the run and every status reported.
    fn scan(
        &self,
        scans: &HistoryScans,
        version: &ScanVersion,
    ) -> (Arc<ScanRun>, Vec<HistoryScanStatus>) {
        let run = scans.begin(version.clone());
        let mut reports = Vec::new();
        run_scan(scans, &run, &self.input(), &self.conn, &mut |status| {
            reports.push(status)
        });
        (run, reports)
    }
}

fn results(scans: &HistoryScans) -> Vec<HistoryScanMatch> {
    scans
        .page(&HistoryScanQuery {
            sort: Some(HistorySort::StartedAt),
            dir: Some(MatchSortDir::Asc),
            per_page: Some(MAX_PER_PAGE),
            ..HistoryScanQuery::default()
        })
        .rows
}

fn all_rows(scans: &HistoryScans) -> Vec<HistoryScanMatch> {
    let run = scans.current().expect("a scan");
    lock(&run.state).results.clone()
}

/// `clean_2_0_win.dat` (local `Qbxxxxxxxx`, opponent `Qaxxxxxx`) rewritten
/// so its opponent is `record`'s and its first entry is a minute after the
/// record's start: a pair the scan should find.
fn paired_log(record: &HistoryRecord) -> Vec<u8> {
    let opponent = record.opponents[0].clone();
    assert_eq!(opponent.len(), "Qaxxxxxx".len());
    let original = load_game_log("clean_2_0_win");
    let first_ms = parse_game_log(&original, UTC).expect("parses").entries[0].timestamp_ms;
    let delta_ticks = (record.start_time_ms.expect("a start") + 60_000 - first_ms) * 10_000;
    let file = read_dat(&original).expect("dat");
    let mut renamed = anonymise_dat(&file, &BTreeMap::from([("Qaxxxxxx".to_string(), opponent)]));
    for entry in &mut renamed.records {
        entry.ticks = (entry.ticks as i64 + delta_ticks) as u64;
    }
    write_dat(&renamed)
}

/// The first history record whose opponent fits `clean_2_0_win`'s.
fn pairable_record() -> HistoryRecord {
    history_records()
        .into_iter()
        .find(|record| {
            record.start_time_ms.is_some()
                && record
                    .opponents
                    .first()
                    .is_some_and(|name| name.len() == "Qaxxxxxx".len())
        })
        .expect("a record with an 8-byte opponent")
}

fn set_mtime(path: &Path, at: SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .expect("open")
        .set_modified(at)
        .expect("set mtime");
}

fn row(history_id: u64, opponent: &str, confidence: Option<f64>) -> HistoryScanMatch {
    HistoryScanMatch {
        history_id,
        started_at_ms: Some(history_id as i64 * 1000),
        opponent: opponent.to_string(),
        format: "CMODERN".to_string(),
        format_display: "Modern".to_string(),
        games_won: 2,
        games_lost: 1,
        outcome: ScanOutcome::Win,
        game_log_token: None,
        game_log_path: None,
        confidence,
        round: 0,
        description: String::new(),
        game_ids: Vec::new(),
        local_player: None,
    }
}

fn ids(page: &HistoryScanPage) -> Vec<u64> {
    page.rows.iter().map(|row| row.history_id).collect()
}

fn query(sort: HistorySort, dir: MatchSortDir) -> HistoryScanQuery {
    HistoryScanQuery {
        sort: Some(sort),
        dir: Some(dir),
        ..HistoryScanQuery::default()
    }
}

// ---- The scan --------------------------------------------------------------------

#[test]
fn a_scan_lists_every_new_history_match_and_drops_stored_ids() {
    let mut world = World::new("history-scan-fixture");
    world.write_history();
    world.write_log("q-one", &load_game_log("clean_2_1_win"));
    world.write_log("q-two", &load_game_log("instant_concede"));
    let version = world.deck_version(&[1, 2]);
    // The first record's match is already stored (any source drops it).
    world.store_match("q-stored", Some(278_248_231));

    let scans = HistoryScans::new();
    let (run, reports) = world.scan(&scans, &version);

    let status = run.status();
    assert_eq!(status.status, ScanState::Complete);
    assert_eq!(status.stage, ScanStage::Scoring);
    assert_eq!((status.progress, status.total), (670, 670));
    assert_eq!(status.match_count, 670);
    assert_eq!(status.formats, vec!["Modern", "Pauper", "Pioneer"]);
    assert_eq!(
        status.confidence_stats,
        Some(ConfidenceStats {
            high: 0,
            low: 0,
            none: 670
        })
    );
    assert_eq!(status.deck_name, "Qdeck");
    assert_eq!(status.deck_version_id, 60);

    let rows = all_rows(&scans);
    assert_eq!(rows.len(), 670);
    assert!(rows.iter().all(|row| row.history_id != 278_248_231));
    assert!(rows.iter().all(|row| row.game_log_token.is_none()));

    // Rows follow the history's order and carry each record's facts.
    let records = history_records();
    let second = &records[1];
    let first_row = &rows[0];
    assert_eq!(first_row.history_id, second.id);
    assert_eq!(first_row.started_at_ms, second.start_time_ms);
    assert_eq!(first_row.opponent, second.opponents[0]);
    assert_eq!(first_row.format, second.format.clone().expect("a format"));
    assert_eq!(
        first_row.format_display,
        decks::display_format(&first_row.format)
    );
    assert_eq!(
        (first_row.games_won, first_row.games_lost),
        (second.game_wins, second.game_losses)
    );
    assert_eq!(
        first_row.outcome,
        ScanOutcome::from_games(second.game_wins, second.game_losses)
    );
    assert_eq!(first_row.game_ids, second.game_ids);
    assert_eq!(first_row.round, second.round);
    assert_eq!(first_row.description, second.description);
    assert_eq!(first_row.confidence, None);
    assert_eq!(first_row.local_player, None);

    // A record without an opponent is listed as "Unknown".
    let empty = records
        .iter()
        .find(|record| record.opponents.is_empty())
        .expect("a record without opponents");
    let listed = rows
        .iter()
        .find(|row| row.history_id == empty.id)
        .expect("listed");
    assert_eq!(listed.opponent, "Unknown");

    // Every stage was reported, in order, and the last report is the end.
    let mut stages: Vec<ScanStage> = reports.iter().map(|status| status.stage).collect();
    stages.dedup();
    assert_eq!(
        stages,
        vec![
            ScanStage::Discovering,
            ScanStage::Decoding,
            ScanStage::Parsing,
            ScanStage::Scoring
        ]
    );
    assert_eq!(reports.last(), Some(&status));
    assert!(
        reports.len() < 30,
        "progress is reported in steps, not per record"
    );
}

#[test]
fn a_paired_log_gives_its_token_local_player_and_confidence() {
    let world = World::new("history-scan-pair");
    world.write_history();
    let record = pairable_record();
    let bytes = paired_log(&record);
    let path = world.write_log("q-paired", &bytes);
    world.write_log("q-other", &load_game_log("concede_2_0"));

    // The local player's cards: four with oracle ids, three in the deck.
    let entries = parse_game_log(&bytes, UTC).expect("parses").entries;
    let local_ids: Vec<u32> = extract_log_cards(&entries)
        .match_cards("Qbxxxxxxxx")
        .iter()
        .map(|card| card.mtgo_id)
        .collect();
    assert!(local_ids.len() >= 4);
    for (index, catalog_id) in local_ids.iter().take(4).enumerate() {
        world.card(*catalog_id, &format!("oracle-{index}"));
    }
    let version = world.deck_version(&local_ids[..3]);

    let scans = HistoryScans::new();
    world.scan(&scans, &version);
    let rows = all_rows(&scans);
    let paired = rows
        .iter()
        .find(|row| row.history_id == record.id)
        .expect("listed");
    assert_eq!(paired.game_log_token.as_deref(), Some("q-paired"));
    assert_eq!(
        paired.game_log_path.as_deref(),
        Some(path.to_string_lossy().as_ref())
    );
    assert_eq!(paired.local_player.as_deref(), Some("Qbxxxxxxxx"));
    assert_eq!(paired.confidence, Some(0.75));
    let others = rows
        .iter()
        .filter(|row| row.game_log_token.is_some())
        .count();
    assert_eq!(others, 1, "no other record pairs");
    let stats = scans
        .status()
        .and_then(|status| status.confidence_stats)
        .expect("stats");
    assert_eq!((stats.high, stats.low, stats.none), (1, 0, 670));
}

#[test]
fn a_log_whose_match_is_stored_is_not_paired() {
    let mut world = World::new("history-scan-linked");
    world.write_history();
    let record = pairable_record();
    world.write_log("q-linked", &paired_log(&record));
    let version = world.deck_version(&[1]);
    world.store_match("q-linked", None);

    let scans = HistoryScans::new();
    world.scan(&scans, &version);
    let rows = all_rows(&scans);
    let row = rows
        .iter()
        .find(|row| row.history_id == record.id)
        .expect("listed");
    assert_eq!(row.game_log_token, None);
    assert_eq!(row.confidence, None);
}

#[test]
fn a_rescan_reads_only_new_or_changed_game_logs() {
    let world = World::new("history-scan-cache");
    world.write_history();
    let record = pairable_record();
    world.write_log("q-paired", &paired_log(&record));
    let changing = world.write_log("q-two", &load_game_log("concede_2_0"));
    let gone = world.write_log("q-three", &load_game_log("instant_concede"));
    world.write_log("q-garbage", b"not a game log");
    let version = world.deck_version(&[1]);
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    set_mtime(&changing, at);

    let scans = HistoryScans::new();
    world.scan(&scans, &version);
    // Four files indexed, and the paired one read again for its cards.
    assert_eq!(scans.index_reads(), 5);

    world.scan(&scans, &version);
    assert_eq!(scans.index_reads(), 5, "nothing changed, nothing read");

    set_mtime(&changing, at + Duration::from_secs(60));
    std::fs::remove_file(&gone).expect("delete");
    let (_, reports) = world.scan(&scans, &version);
    assert_eq!(scans.index_reads(), 6, "only the changed file");
    let decoding = reports
        .iter()
        .rfind(|status| status.stage == ScanStage::Decoding)
        .expect("a decoding report");
    assert_eq!((decoding.progress, decoding.total), (1, 1));
    assert_eq!(lock(&scans.index).len(), 3, "the deleted file is forgotten");
}

#[test]
fn a_cancelled_scan_stops_and_keeps_no_results() {
    let world = World::new("history-scan-cancel");
    world.write_history();
    for index in 0..5 {
        world.write_log(&format!("q-{index}"), &load_game_log("concede_2_0"));
    }
    let version = world.deck_version(&[1]);

    let scans = HistoryScans::new();
    let run = scans.begin(version);
    let mut reports = Vec::new();
    run_scan(&scans, &run, &world.input(), &world.conn, &mut |status| {
        if status.stage == ScanStage::Decoding {
            scans.cancel();
        }
        reports.push(status);
    });

    let status = run.status();
    assert_eq!(status.status, ScanState::Cancelled);
    assert_eq!(status.stage, ScanStage::Decoding);
    assert!(lock(&run.state).results.is_empty());
    assert!(scans.index_reads() < 5, "it stopped reading");
    assert!(
        reports
            .iter()
            .all(|status| status.stage != ScanStage::Scoring),
        "it never reached scoring"
    );
}

#[test]
fn cancelling_an_ended_scan_forgets_it() {
    let world = World::new("history-scan-forget");
    world.write_history();
    let version = world.deck_version(&[1]);
    let scans = HistoryScans::new();
    world.scan(&scans, &version);
    assert_eq!(
        scans.status().map(|status| status.status),
        Some(ScanState::Complete)
    );

    assert_eq!(scans.cancel(), None);
    assert_eq!(scans.status(), None);
    assert!(scans.page(&HistoryScanQuery::default()).rows.is_empty());
}

#[test]
fn a_new_scan_cancels_and_replaces_the_running_one() {
    let world = World::new("history-scan-replace");
    let version = world.deck_version(&[1]);
    let scans = HistoryScans::new();
    let first = scans.begin(version.clone());
    let second = scans.begin(version);

    assert!(first.is_cancelled());
    assert_eq!(first.status().status, ScanState::Cancelled);
    assert!(!second.is_cancelled());
    assert_ne!(first.id, second.id);
    assert_eq!(scans.current().map(|run| run.id), Some(second.id));
}

#[test]
fn without_history_files_a_scan_completes_with_nothing() {
    let world = World::new("history-scan-empty");
    world.write_log("q-one", &load_game_log("concede_2_0"));
    let version = world.deck_version(&[1]);
    let scans = HistoryScans::new();
    let (run, _) = world.scan(&scans, &version);
    let status = run.status();
    assert_eq!(status.status, ScanState::Complete);
    assert_eq!((status.total, status.match_count), (0, 0));
    assert_eq!(status.confidence_stats, Some(ConfidenceStats::default()));
}

#[test]
fn a_failed_run_says_why() {
    let world = World::new("history-scan-fail");
    let version = world.deck_version(&[1]);
    let scans = HistoryScans::new();
    let run = scans.begin(version);
    assert!(run.fail("the database is gone"));
    let status = run.status();
    assert_eq!(status.status, ScanState::Failed);
    assert_eq!(status.error.as_deref(), Some("the database is gone"));
    // A cancelled run stays cancelled.
    let again = scans.begin(run.version.clone());
    scans.cancel();
    assert!(!again.fail("late"));
    assert_eq!(again.status().status, ScanState::Cancelled);
}

#[test]
fn results_are_kept_only_for_the_latest_scan() {
    let world = World::new("history-scan-latest");
    world.write_history();
    let version = world.deck_version(&[1]);
    let scans = HistoryScans::new();
    world.scan(&scans, &version);
    assert_eq!(results(&scans).len(), MAX_PER_PAGE as usize);
    let run = scans.begin(version);
    assert!(results(&scans).is_empty(), "the new scan has none yet");
    assert_eq!(run.status().match_count, 0);
}

// ---- The game log index ------------------------------------------------------------

#[test]
fn the_index_reads_a_file_once_while_its_size_and_mtime_hold() {
    let dir = TestDir::new("history-index");
    let bytes = load_game_log("concede_2_0");
    let path = dir.write("Match_GameLog_q-a.dat", &bytes);
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    set_mtime(&path, at);
    let paths = vec![path.clone()];

    let mut index = GameLogIndex::new();
    assert_eq!(index.stale(&paths, UTC), paths);
    index.read(&path, UTC);
    assert!(index.stale(&paths, UTC).is_empty());
    assert_eq!(index.reads(), 1);

    let log = parse_game_log(&bytes, UTC).expect("parses");
    let candidates = index.candidates();
    assert_eq!(
        candidates,
        vec![CandidateLog {
            path: path.clone(),
            token: "q-a".to_string(),
            first_entry_ms: log.entries[0].timestamp_ms,
            players: game_detail::detect_players(&log.entries),
        }]
    );

    // Another timezone reads the ticks differently: read again.
    assert_eq!(index.stale(&paths, chrono_tz::Europe::London), paths);
    // So does a new mtime, even at the same size.
    set_mtime(&path, at + Duration::from_secs(1));
    assert_eq!(index.stale(&paths, UTC), paths);
    // A file no longer listed is forgotten.
    assert!(index.stale(&[], UTC).is_empty());
    assert!(index.is_empty());
}

#[test]
fn an_unreadable_or_empty_file_is_indexed_without_entries() {
    let dir = TestDir::new("history-index-garbage");
    let garbage = dir.write("Match_GameLog_q-bad.dat", b"nope");
    let mut index = GameLogIndex::new();
    index.read(&garbage, UTC);
    assert!(index.candidates().is_empty());
    assert!(index.stale(std::slice::from_ref(&garbage), UTC).is_empty());

    let missing = dir.join("Match_GameLog_q-missing.dat");
    index.read(&missing, UTC);
    assert!(index.candidates().is_empty());
}

#[test]
fn player_cards_are_read_once() {
    let dir = TestDir::new("history-index-cards");
    let bytes = load_game_log("clean_2_0_win");
    let path = dir.write("Match_GameLog_q-a.dat", &bytes);
    let mut index = GameLogIndex::new();
    index.read(&path, UTC);

    let entries = parse_game_log(&bytes, UTC).expect("parses").entries;
    let expected: Vec<u32> = extract_log_cards(&entries)
        .match_cards("Qbxxxxxxxx")
        .iter()
        .map(|card| card.mtgo_id)
        .collect();
    assert!(!expected.is_empty());
    assert_eq!(index.player_catalog_ids(&path, UTC, "Qbxxxxxxxx"), expected);
    assert_eq!(index.player_catalog_ids(&path, UTC, "Qbxxxxxxxx"), expected);
    assert!(index.player_catalog_ids(&path, UTC, "Qnobody").is_empty());
    assert_eq!(index.reads(), 2);
}

// ---- Results pages -------------------------------------------------------------------

#[test]
fn confidence_sorts_scored_rows_first_in_either_direction() {
    let rows = vec![
        row(1, "Qa", None),
        row(2, "Qb", Some(0.5)),
        row(3, "Qc", Some(0.9)),
        row(4, "Qd", None),
        row(5, "Qe", Some(0.5)),
    ];
    let desc = page_of(&rows, &HistoryScanQuery::default());
    assert_eq!(ids(&desc), vec![3, 2, 5, 1, 4]);
    let asc = page_of(&rows, &query(HistorySort::Confidence, MatchSortDir::Asc));
    assert_eq!(ids(&asc), vec![2, 5, 3, 1, 4]);
}

#[test]
fn rows_sort_by_time_opponent_format_and_outcome_keeping_ties_in_scan_order() {
    let mut rows = vec![row(3, "Qb", None), row(1, "Qa", None), row(2, "Qb", None)];
    rows[0].format_display = "Pauper".to_string();
    rows[1].outcome = ScanOutcome::Loss;
    rows[2].outcome = ScanOutcome::Draw;
    rows[2].started_at_ms = None;

    let started = page_of(&rows, &query(HistorySort::StartedAt, MatchSortDir::Desc));
    assert_eq!(ids(&started), vec![3, 1, 2]);
    let started = page_of(&rows, &query(HistorySort::StartedAt, MatchSortDir::Asc));
    assert_eq!(ids(&started), vec![2, 1, 3]);
    let opponent = page_of(&rows, &query(HistorySort::Opponent, MatchSortDir::Asc));
    assert_eq!(ids(&opponent), vec![1, 3, 2]);
    let opponent = page_of(&rows, &query(HistorySort::Opponent, MatchSortDir::Desc));
    assert_eq!(ids(&opponent), vec![3, 2, 1]);
    let format = page_of(&rows, &query(HistorySort::Format, MatchSortDir::Asc));
    assert_eq!(ids(&format), vec![1, 2, 3]);
    let outcome = page_of(&rows, &query(HistorySort::Outcome, MatchSortDir::Asc));
    assert_eq!(ids(&outcome), vec![2, 1, 3]);
}

#[test]
fn filters_keep_a_format_and_scored_rows_above_a_minimum() {
    let mut rows = vec![
        row(1, "Qa", None),
        row(2, "Qb", Some(0.3)),
        row(3, "Qc", Some(0.29)),
        row(4, "Qd", Some(0.8)),
    ];
    rows[3].format_display = "Pauper".to_string();

    let pauper = page_of(
        &rows,
        &HistoryScanQuery {
            format: Some("Pauper".to_string()),
            ..HistoryScanQuery::default()
        },
    );
    assert_eq!(ids(&pauper), vec![4]);
    let blank = page_of(
        &rows,
        &HistoryScanQuery {
            format: Some(String::new()),
            ..HistoryScanQuery::default()
        },
    );
    assert_eq!(blank.total, 4);

    // A slider step of 0.1 three times is 0.30000000000000004 in floats.
    let min = page_of(
        &rows,
        &HistoryScanQuery {
            min_confidence: Some(0.1 + 0.1 + 0.1),
            ..HistoryScanQuery::default()
        },
    );
    assert_eq!(ids(&min), vec![4, 2]);
    // Zero is "off": unscored rows stay.
    let off = page_of(
        &rows,
        &HistoryScanQuery {
            min_confidence: Some(0.0),
            ..HistoryScanQuery::default()
        },
    );
    assert_eq!(off.total, 4);
    assert_eq!(filtered(&rows, Some("Modern"), Some(0.5)).len(), 0);
    assert_eq!(filtered(&rows, None, Some(0.01)).len(), 3);
}

#[test]
fn pages_hold_fifty_by_default_and_at_most_a_hundred() {
    let rows: Vec<HistoryScanMatch> = (1..=230).map(|id| row(id, "Qa", None)).collect();
    let first = page_of(&rows, &query(HistorySort::StartedAt, MatchSortDir::Asc));
    assert_eq!((first.page, first.per_page, first.total), (1, 50, 230));
    assert_eq!(first.last_page, 5);
    assert_eq!(first.rows.len(), 50);
    assert_eq!(first.rows[0].history_id, 1);

    let big = page_of(
        &rows,
        &HistoryScanQuery {
            per_page: Some(500),
            page: Some(3),
            ..query(HistorySort::StartedAt, MatchSortDir::Asc)
        },
    );
    assert_eq!((big.per_page, big.last_page), (100, 3));
    assert_eq!(ids(&big), (201..=230).collect::<Vec<u64>>());

    let past = page_of(
        &rows,
        &HistoryScanQuery {
            page: Some(999),
            per_page: Some(0),
            ..HistoryScanQuery::default()
        },
    );
    assert_eq!(past.per_page, 1);
    assert!(past.rows.is_empty());

    let none = page_of(&[], &HistoryScanQuery::default());
    assert_eq!((none.total, none.last_page, none.page), (0, 1, 1));
}

#[test]
fn a_record_s_outcome_is_mtgo_s_verdict_then_its_game_counts() {
    let mut record = pairable_record();
    let opponent = record.opponents[0].clone();
    record.game_wins = 1;
    record.game_losses = 1;
    // The opponent left at 1-1 (#454): MTGO names them the loser.
    record.match_winners = vec!["Qlocal".to_string()];
    record.match_losers = vec![opponent.clone()];
    assert_eq!(ScanOutcome::from_record(&record), ScanOutcome::Win);
    record.match_winners = vec![opponent];
    record.match_losers = vec!["Qlocal".to_string()];
    assert_eq!(ScanOutcome::from_record(&record), ScanOutcome::Loss);
    // No winner named: the counts, as 0.x read them.
    record.match_winners.clear();
    record.match_losers.clear();
    assert_eq!(ScanOutcome::from_record(&record), ScanOutcome::Draw);
}
