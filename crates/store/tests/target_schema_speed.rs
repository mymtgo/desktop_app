//! The target schema's speed budgets (foundation spec section 8, Plan R
//! Task 8) over owner-sized data, and a guard that no code still names a
//! table the target schema removed.
//!
//! The timing test is ignored by default because debug builds are much
//! slower. Run it in release:
//! `cargo test --release -p store --test target_schema_speed -- --ignored --nocapture`.
//!
//! The data set is built by row inserts (the store's own write, import and
//! SQL paths) and synthetic log lines, never from real logs:
//! - 2 accounts, each with 20 log files and a Login line in each;
//! - 2,000 constructed league matches in 400 runs (a join line per run);
//! - 200 drafts of 42 picks (0.x imports), each in a run with 3 matches and
//!   their registered decks;
//! - 1,000 other matches with 3 games, each game 100 state frames growing to
//!   75 cards (the game sync's rows), all but the newest 50 compacted;
//! - 20 decks with 2 versions each, and the cards they name;
//! - the archetype seed (archetypes spec section 17): 500 held downloaded
//!   archetypes with a definition and a 60-card list 1, 10 manual
//!   archetypes with 5 variants each (50 lists), both seats'
//!   `match_archetypes` on the constructed matches (a tenth without the
//!   player seat), and every deck's `archetype_id`.
//!
//! The archetype reads have their own timing test,
//! `the_archetype_reads_meet_the_speed_budgets` (Plan T ruling C11): later
//! Plan T tasks append their measures there, never to the Plan R test. The
//! overlay reads (Plan U, overlay spec section 14) have theirs too,
//! `league_card_compute_under_5_ms` (with the event card's tournaments and
//! the tournament sync over the Challenge fixture); the game view's own budget is
//! `game_view_compute_under_10_ms` in `app`. The replay reads (Plan V,
//! replays spec section 12) have `the_replay_reads_meet_the_speed_budgets`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use archetypes::metagame::Timeframe;
use chrono::NaiveDate;
use chrono_tz::Tz;
use decks::stats::Range;
use fixture_tools::TestDir;
use leagues::LeagueKind;
use limited::{Draft, DraftState, Pick, RegisteredCard};
use mtgo_match::{EndReason, GameResult, GameView, MatchState, MatchView, Outcome, Resolution};
use mtgo_protocol::prepare_at;
use store::accounts::current_account_id;
use store::api_reads::{
    CARD_STATS_CANDIDATES_SQL, ENQUEUE_LIMIT, card_stats_candidates, pending_report_count,
    report_candidates,
};
use store::archetypes::archetype_options;
use store::card_stats::opponent_archetypes;
use store::cards::{CardsPageQuery, card_archetypes, cards_page};
use store::dashboard::{
    FormatFilter, earlier_matches, format_options, last_session,
    matchup_spread as dashboard_matchup_spread, overview, recent_decks, recent_matches,
    recent_sessions, window_matches,
};
use store::decks::card_infos;
use store::game_stats::{game_stats, game_stats_opponents};
use store::game_sync::{game_log, log_match_tokens, match_game_cards, sync_games};
use store::import::{ImportedGame, ImportedMatch, import_matches};
use store::import_leagues::{ImportedLeague, LeagueMember, import_leagues};
use store::import_limited::{ImportedLimited, LegacyDraft, LegacyLimitedDeck, import_limited};
use store::league_sync::{LeagueOwner, sync_all, sync_owner};
use store::leagues::{
    RunFilter, StoredRun, account_ids, league_decks, notes_by_match, stored_limited_runs,
    stored_runs, version_dates,
};
use store::match_detail::{match_games, match_header, opponent_color_identities};
use store::match_list::{ArchetypeFilter, MatchListFilter, Scope, match_list};
use store::matchups::{deck_version_ids, matchup_detail, matchup_spread};
use store::metagame::{archetype_detail, metagame_data, metagame_formats, metagame_window};
use store::read::events_for_match;
use store::sync::{deck_bundle_parts, match_bundle_input};
use store::write::{MatchWrite, NewAccount, NewEvent, NewLogFile, WriteBatch, commit};
use store::{Connection, open_reader, open_writer, params};

// ---- The removed tables ------------------------------------------------------

/// The tables the target schema removed (spec section 16).
const REMOVED_TABLES: [&str; 16] = [
    "match_notes",
    "hidden_matches",
    "match_decks",
    "history_imports",
    "manual_game_details",
    "imported_match_details",
    "manual_leagues",
    "manual_league_matches",
    "imported_league_runs",
    "imported_league_matches",
    "league_notes",
    "league_drops",
    "imported_drafts",
    "imported_draft_picks",
    "draft_pick_notes",
    "imported_limited_decks",
];

/// The workspace root (this crate is `crates/store`).
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every source file under `dir` (Rust and SQL), leaving out build output.
fn source_files(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if name != "target" && name != "node_modules" {
                source_files(&path, found);
            }
        } else if name.ends_with(".rs") || name.ends_with(".sql") {
            found.push(path);
        }
    }
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Whether `line` holds `name` as a whole word (an identifier or a table
/// name, not part of a longer one).
fn names(line: &str, name: &str) -> bool {
    let mut start = 0;
    while let Some(found) = line[start..].find(name) {
        let at = start + found;
        let end = at + name.len();
        let before = line[..at].chars().next_back();
        let after = line[end..].chars().next();
        if !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char) {
            return true;
        }
        start = end;
    }
    false
}

#[test]
fn no_code_names_a_removed_table() {
    let root = workspace_root();
    let this_file = Path::new(file!())
        .file_name()
        .expect("this file's name")
        .to_owned();
    let mut files = Vec::new();
    source_files(&root.join("crates"), &mut files);
    source_files(&root.join("app/src"), &mut files);
    assert!(files.len() > 50, "the scan found the sources");

    let mut found = Vec::new();
    for path in files {
        if path.file_name() == Some(this_file.as_os_str()) {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("a source file reads");
        for (index, line) in text.lines().enumerate() {
            for table in REMOVED_TABLES {
                if names(line, table) {
                    let shown = path.strip_prefix(&root).unwrap_or(&path).display();
                    found.push(format!("{shown}:{}: {table}", index + 1));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "code still names a removed table:\n{}",
        found.join("\n")
    );
}

// ---- The data set ------------------------------------------------------------

const UTC: Tz = chrono_tz::UTC;
/// A now long after every generated match's `SEND_HOLD_MS`.
const AFTER_THE_HOLD: i64 = i64::MAX;
const ACCOUNTS: [&str; 2] = ["Qspeedone", "Qspeedtwo"];
const FILES_PER_ACCOUNT: usize = 20;
const CONSTRUCTED_RUNS: usize = 400;
const MATCHES_PER_RUN: usize = 5;
const DRAFTS: usize = 200;
const DRAFT_MATCHES: usize = 3;
const CARD_MATCHES: usize = 1_000;
/// The newest card matches are left uncompacted (live).
const LIVE_MATCHES: usize = 50;
const GAMES: usize = 3;
const FRAMES_PER_GAME: usize = 100;
const LOCAL_CARDS: usize = 60;
const OPPONENT_CARDS: usize = 15;
const DECKS_PER_ACCOUNT: usize = 10;
/// The first match starts here (2025-01-01); matches are an hour apart.
const START_MS: i64 = 1_735_689_600_000;
const HOUR_MS: i64 = 3_600_000;
/// A draft league's MTGO event id.
const DRAFT_EVENT_ID: i64 = 11039;
/// The game backfill's slice: `tracker::GAME_SYNCS_PER_TICK` matches per
/// writer transaction.
const BACKFILL_SLICE: usize = 10;
const LOCAL_CATALOG: u32 = 10_000;
/// Held downloaded archetypes (spec section 17).
const DOWNLOADED_ARCHETYPES: usize = 500;
/// Manual archetypes, each with `MANUAL_VARIANTS` lists (50 variants).
const MANUAL_ARCHETYPES: usize = 10;
const MANUAL_VARIANTS: usize = 5;
const OPPONENT_CATALOG: u32 = 20_000;
const DRAFT_CATALOG: u32 = 30_000;

/// A UUID-shaped token, unique per `(kind, n)`.
fn token(kind: u32, n: usize) -> String {
    format!("{kind:08x}-0000-4000-8000-{n:012x}")
}

/// The data set under construction.
struct Builder {
    conn: Connection,
    /// Each account's log files.
    files: Vec<Vec<i64>>,
    /// The next byte offset of each file.
    offsets: HashMap<i64, u64>,
    next_match_number: usize,
    next_mtgo_match_id: u64,
    next_mtgo_game_id: u64,
    /// Each account's deck versions.
    versions: Vec<Vec<i64>>,
}

impl Builder {
    fn new(path: &Path) -> Builder {
        let conn = open_writer(path).expect("open the database");
        Builder {
            conn,
            files: Vec::new(),
            offsets: HashMap::new(),
            next_match_number: 0,
            next_mtgo_match_id: 300_000_000,
            next_mtgo_game_id: 900_000_000,
            versions: Vec::new(),
        }
    }

    /// The account index's file for match number `n`.
    fn file_of(&self, account: usize, n: usize) -> i64 {
        let files = &self.files[account];
        files[n % files.len()]
    }

    /// A classified event at the file's next offset.
    fn event(&mut self, file: i64, at_ms: i64, raw: &str) -> NewEvent {
        let offset = self.offsets.entry(file).or_insert(0);
        let byte_offset = *offset;
        *offset += raw.len() as u64 + 2;
        let prepared = prepare_at(file as u64, byte_offset, at_ms, raw, None)
            .unwrap_or_else(|| panic!("a synthetic line classifies: {raw:.120}"));
        NewEvent {
            log_file_id: file,
            byte_offset,
            kind: Some(prepared.kind),
            subjects: prepared.subjects,
            logged_at_ms: at_ms,
            raw: raw.to_string(),
            quarantined: false,
        }
    }

    fn accounts_and_files(&mut self) {
        let mut batch = WriteBatch {
            now_ms: START_MS,
            ..WriteBatch::default()
        };
        for (index, name) in ACCOUNTS.iter().enumerate() {
            batch.accounts.push(NewAccount {
                username: name.to_string(),
                login_id: Some(3_000_000 + index as u64),
                seen_at_ms: START_MS,
                logged_at_ms: Some(START_MS),
            });
        }
        commit(&mut self.conn, &batch).expect("accounts");

        let mut events = Vec::new();
        for (index, name) in ACCOUNTS.iter().enumerate() {
            let mut files = Vec::new();
            for number in 0..FILES_PER_ACCOUNT {
                let file = store::write::insert_log_file(
                    &self.conn,
                    &NewLogFile {
                        path: format!("Q/{name}/mtgo-{number}.log"),
                        identity_hash: (index * 100 + number) as u64,
                        head_hash: 2,
                        head_len: 10,
                        created_at_fs: Some(START_MS),
                        day_anchor: NaiveDate::from_ymd_opt(2025, 1, 1).expect("date"),
                        timezone: "UTC".to_string(),
                        inherited_username: Some(name.to_string()),
                        seen_at_ms: START_MS,
                    },
                )
                .expect("log file");
                let login = format!(
                    "12:00:00 [INF] (Login|MtGO Login Success) Username: {name} ({})",
                    3_000_000 + index
                );
                events.push(self.event(file, START_MS, &login));
                files.push(file);
            }
            self.files.push(files);
        }
        commit(
            &mut self.conn,
            &WriteBatch {
                events,
                now_ms: START_MS,
                ..WriteBatch::default()
            },
        )
        .expect("logins");
    }

    fn cards_and_decks(&mut self) {
        let colors = ["W", "U", "B", "R", "G", "U,B", "R,G", ""];
        let tx = self.conn.transaction().expect("tx");
        let mut ids: Vec<u32> = Vec::new();
        ids.extend((0..LOCAL_CARDS as u32).map(|i| LOCAL_CATALOG + i));
        ids.extend((0..OPPONENT_CARDS as u32).map(|i| OPPONENT_CATALOG + i));
        ids.extend((0..60).map(|i| DRAFT_CATALOG + i));
        for id in ids {
            tx.execute(
                "INSERT INTO cards (catalog_id, oracle_id, name, type, color_identity, cmc)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    id,
                    format!("oracle-{id}"),
                    format!("Qcard {id}"),
                    if id % 3 == 0 { "Land" } else { "Creature" },
                    colors[id as usize % colors.len()],
                    f64::from(id % 6),
                ],
            )
            .expect("card");
        }
        let mut versions = Vec::new();
        for (index, name) in ACCOUNTS.iter().enumerate() {
            let account: i64 = tx
                .query_row(
                    "SELECT id FROM accounts WHERE username = ?1",
                    [name],
                    |row| row.get(0),
                )
                .expect("account");
            let mut own = Vec::new();
            for deck in 0..DECKS_PER_ACCOUNT {
                tx.execute(
                    "INSERT INTO decks (mtgo_id, account_id, name, format, color_identity,
                         created_at, updated_at, source)
                     VALUES (?1, ?2, ?3, 'CMODERN', 'U,B', ?4, ?4, 'file')",
                    params![
                        format!("q-deck-{index}-{deck}"),
                        account,
                        format!("Qdeck {deck}"),
                        START_MS
                    ],
                )
                .expect("deck");
                let deck_id = tx.last_insert_rowid();
                for version in 0..2 {
                    tx.execute(
                        "INSERT INTO deck_versions (deck_id, signature, modified_at)
                         VALUES (?1, ?2, ?3)",
                        params![
                            deck_id,
                            format!("q-signature-{index}-{deck}-{version}"),
                            START_MS + version * 30 * 24 * HOUR_MS
                        ],
                    )
                    .expect("version");
                    own.push(tx.last_insert_rowid());
                }
            }
            versions.push(own);
        }
        tx.commit().expect("commit");
        self.versions = versions;
    }

    /// A complete three-game match view; `n` picks the result and times.
    fn view(&mut self, account: usize, format: &str, league_token: Option<&str>) -> MatchView {
        let n = self.next_match_number;
        self.next_match_number += 1;
        let started_at_ms = START_MS + n as i64 * HOUR_MS;
        let mtgo_match_id = self.next_mtgo_match_id;
        self.next_mtgo_match_id += 1;
        let mut games = Vec::new();
        let mut wins = 0;
        let mut losses = 0;
        for number in 0..GAMES {
            let won = !(n + number).is_multiple_of(3);
            if won {
                wins += 1;
            } else {
                losses += 1;
            }
            let game_start = started_at_ms + number as i64 * 15 * 60_000;
            games.push(GameView {
                mtgo_game_id: Some(self.next_mtgo_game_id),
                game_number: number as u32 + 1,
                result: if won {
                    GameResult::Win
                } else {
                    GameResult::Loss
                },
                end_reason: EndReason::Win,
                on_play: Some(number % 2 == 0),
                local_mulligans: Some(0),
                opp_mulligans: Some((n % 2) as u32),
                local_die_roll: Some(3),
                opp_die_roll: Some(5),
                turn_count: Some(9),
                started_at_ms: Some(game_start),
                ended_at_ms: Some(game_start + 14 * 60_000),
            });
            self.next_mtgo_game_id += 1;
        }
        MatchView {
            token: token(1, n),
            mtgo_match_id: Some(mtgo_match_id),
            local_username: ACCOUNTS[account].to_string(),
            opponent: Some(format!("Qopp{:05}", n % 700)),
            format: Some(format.to_string()),
            match_type: Some("Modern".to_string()),
            tournament_event_id: None,
            tournament_round: None,
            league_token: league_token.map(str::to_string),
            game_structure: Some("Modern".to_string()),
            state: MatchState::Complete,
            resolution: Some(Resolution::Signal),
            outcome: if wins > losses {
                Outcome::Win
            } else {
                Outcome::Loss
            },
            tentative_outcome: None,
            wins,
            losses,
            started_at_ms,
            ended_at_ms: Some(started_at_ms + 50 * 60_000),
            last_activity_ms: started_at_ms + 50 * 60_000,
            games,
        }
    }

    /// Link the matches to a deck version of their account.
    fn link(&mut self, tokens: &[(String, i64)]) {
        let tx = self.conn.transaction().expect("tx");
        for (token, version) in tokens {
            tx.execute(
                "UPDATE matches SET deck_version_id = ?2 WHERE token = ?1",
                params![token, version],
            )
            .expect("link");
        }
        tx.commit().expect("commit");
    }

    /// 400 constructed runs of 5 matches, a join line each, half per account.
    fn constructed_runs(&mut self) {
        for run in 0..CONSTRUCTED_RUNS {
            let account = run % ACCOUNTS.len();
            let league_token = token(2, run);
            let version = self.versions[account][run % self.versions[account].len()];
            let file = self.file_of(account, run);
            let join_at = START_MS + self.next_match_number as i64 * HOUR_MS - 60_000;
            let join = format!(
                "12:00:00 [INF] (UI|Creating GameDetailsView) League EventToken={league_token} EventId={}",
                12_000 + run % 7
            );
            let mut batch = WriteBatch {
                now_ms: join_at,
                ..WriteBatch::default()
            };
            batch.events.push(self.event(file, join_at, &join));
            let mut links = Vec::new();
            for _ in 0..MATCHES_PER_RUN {
                let view = self.view(account, "CMODERN", Some(&league_token));
                links.push((view.token.clone(), version));
                batch.matches.push(MatchWrite::Upsert {
                    view: Box::new(view),
                    revision: 1,
                });
            }
            commit(&mut self.conn, &batch).expect("league matches");
            self.link(&links);
        }
    }

    /// One state frame of a game: the first `visible` cards of the game.
    fn frame(
        match_id: u64,
        game_id: u64,
        account: usize,
        opponent: &str,
        visible: usize,
    ) -> String {
        let mut cards = Vec::new();
        let mut local = 0;
        let mut opponents = 0;
        for index in 0..visible {
            let take_opponent =
                index >= 7 && index % 4 == 3 && opponents < OPPONENT_CARDS || local >= LOCAL_CARDS;
            let (id, catalog_id, owner, zone) = if take_opponent {
                opponents += 1;
                (
                    100 + LOCAL_CARDS + opponents,
                    OPPONENT_CATALOG + opponents as u32 - 1,
                    2,
                    "Battlefield",
                )
            } else {
                local += 1;
                let zone = if local <= 7 {
                    "Hand"
                } else if local % 3 == 0 {
                    "Graveyard"
                } else {
                    "Battlefield"
                };
                (100 + local, LOCAL_CATALOG + local as u32 - 1, 1, zone)
            };
            cards.push(format!(
                "{{\"Id\":{id},\"CatalogID\":{catalog_id},\"Zone\":\"{zone}\",\"ActualZone\":\"{zone}\",\"Owner\":{owner},\"Controller\":{owner}}}"
            ));
        }
        format!(
            "12:00:00 [INF] (Twitch Info|Game Play Status Update for Game ID: {game_id}, Match ID: {match_id}, Event ID: {match_id}) {{\"Players\":[{{\"Id\":1,\"Name\":\"{}\",\"LibraryCount\":53,\"HandCount\":7,\"Life\":20}},{{\"Id\":2,\"Name\":\"{opponent}\",\"LibraryCount\":53,\"HandCount\":7,\"Life\":20}}],\"Cards\":[{}]}}",
            ACCOUNTS[account],
            cards.join(",")
        )
    }

    /// 1,000 matches with 3 games of 100 frames each, ten per batch; all but
    /// the newest `LIVE_MATCHES` compacted.
    fn card_matches(&mut self) -> Vec<String> {
        let mut tokens = Vec::new();
        for chunk in 0..CARD_MATCHES / 10 {
            let mut batch = WriteBatch::default();
            let mut links = Vec::new();
            for slot in 0..10 {
                let number = chunk * 10 + slot;
                let account = number % ACCOUNTS.len();
                let view = self.view(account, "CMODERN", None);
                let file = self.file_of(account, number);
                let match_id = view.mtgo_match_id.expect("match id");
                let opponent = view.opponent.clone().expect("opponent");
                for game in &view.games {
                    let game_id = game.mtgo_game_id.expect("game id");
                    let start = game.started_at_ms.expect("start");
                    for frame in 0..FRAMES_PER_GAME {
                        let visible = (LOCAL_CARDS + OPPONENT_CARDS).min(
                            7 + frame * (LOCAL_CARDS + OPPONENT_CARDS - 7) / (FRAMES_PER_GAME - 1),
                        );
                        let raw = Self::frame(match_id, game_id, account, &opponent, visible);
                        let event = self.event(file, start + frame as i64 * 5_000, &raw);
                        batch.events.push(event);
                    }
                }
                if number < CARD_MATCHES - LIVE_MATCHES {
                    batch.compact.push(view.token.clone());
                }
                batch.now_ms = view.ended_at_ms.expect("ended") + HOUR_MS;
                let version = self.versions[account][number % self.versions[account].len()];
                links.push((view.token.clone(), version));
                tokens.push(view.token.clone());
                batch.matches.push(MatchWrite::Upsert {
                    view: Box::new(view),
                    revision: 1,
                });
            }
            commit(&mut self.conn, &batch).expect("card matches");
            self.link(&links);
        }
        tokens
    }

    /// 200 drafts of 42 picks imported from 0.x, each in a run with three
    /// imported matches and their registered decks.
    fn drafts(&mut self) {
        let mut matches = Vec::new();
        let mut leagues = Vec::new();
        let mut limited = ImportedLimited::default();
        for number in 0..DRAFTS {
            let account = number % ACCOUNTS.len();
            let legacy_league = 5_000 + number as i64;
            let started = START_MS + self.next_match_number as i64 * HOUR_MS;
            let mut members = Vec::new();
            for _ in 0..DRAFT_MATCHES {
                let view = self.view(account, "DHOB", None);
                let mut games = Vec::new();
                for game in &view.games {
                    games.push(ImportedGame {
                        mtgo_game_id: game.mtgo_game_id.map(|id| id as i64),
                        game_number: game.game_number,
                        result: if game.result == GameResult::Win {
                            "win".to_string()
                        } else {
                            "loss".to_string()
                        },
                        on_play: game.on_play,
                        local_mulligans: Some(0),
                        opp_mulligans: Some(0),
                        local_die_roll: None,
                        opp_die_roll: None,
                        turn_count: Some(8),
                        started_at_ms: game.started_at_ms,
                        ended_at_ms: game.ended_at_ms,
                    });
                }
                let mtgo_match_id = view.mtgo_match_id.map(|id| id as i64);
                members.push(LeagueMember {
                    token: view.token.clone(),
                    mtgo_match_id,
                });
                let mut cards = Vec::new();
                for card in 0..23u32 {
                    cards.push(RegisteredCard {
                        catalog_id: DRAFT_CATALOG + card,
                        quantity: 1,
                        sideboard: false,
                    });
                }
                cards.push(RegisteredCard {
                    catalog_id: DRAFT_CATALOG + 59,
                    quantity: 17,
                    sideboard: false,
                });
                for card in 23..42u32 {
                    cards.push(RegisteredCard {
                        catalog_id: DRAFT_CATALOG + card,
                        quantity: 1,
                        sideboard: true,
                    });
                }
                limited.decks.push(LegacyLimitedDeck {
                    league_id: legacy_league,
                    match_token: view.token.clone(),
                    mtgo_match_id,
                    cards,
                    captured_at_ms: view.started_at_ms,
                });
                matches.push(ImportedMatch {
                    token: view.token.clone(),
                    mtgo_match_id,
                    local_username: view.local_username.clone(),
                    opponent: view.opponent.clone(),
                    format: view.format.clone(),
                    match_type: Some("Draft".to_string()),
                    tournament_event_id: None,
                    tournament_round: None,
                    state: "complete".to_string(),
                    outcome: if view.outcome == Outcome::Win {
                        "win".to_string()
                    } else {
                        "loss".to_string()
                    },
                    started_at_ms: view.started_at_ms,
                    ended_at_ms: view.ended_at_ms,
                    note: None,
                    games,
                    manual: None,
                    games_won_imported: None,
                    games_lost_imported: None,
                    deck_linked: false,
                    replay_share: None,
                    legacy_history: false,
                });
            }
            let ended = START_MS + self.next_match_number as i64 * HOUR_MS;
            leagues.push(ImportedLeague {
                legacy_id: legacy_league,
                manual: false,
                kind: "draft".to_string(),
                set_code: Some("HOB".to_string()),
                course_id: Some(35_000_000 + number as i64),
                token: token(3, number),
                event_id: Some(DRAFT_EVENT_ID),
                name: "HOB Draft League".to_string(),
                format: "DHOB".to_string(),
                state: "complete".to_string(),
                started_at_ms: started,
                joined_at_ms: Some(started),
                completed_at_ms: Some(ended),
                dropped_at_ms: None,
                created_at_ms: Some(started),
                deck: None,
                notes: None,
                matches: members,
            });
            let mut picks = Vec::new();
            for ordinal in 1..=42u32 {
                let pack_number = (ordinal - 1) / 14 + 1;
                let pick_number = (ordinal - 1) % 14 + 1;
                let shown = started - HOUR_MS + i64::from(ordinal) * 30_000;
                picks.push(Pick {
                    ordinal,
                    pack_number,
                    pick_number,
                    pack_id: Some(u64::from(ordinal)),
                    direction: Some(pack_number % 2),
                    cards_available: (0..(15 - pick_number)).map(|i| DRAFT_CATALOG + i).collect(),
                    picked_catalog_id: Some(DRAFT_CATALOG + ordinal % 59),
                    picked_card_id: Some(u64::from(ordinal) + 1_000),
                    reservations: Vec::new(),
                    shown_at_ms: Some(shown),
                    deadline_at_ms: None,
                    picked_at_ms: Some(shown + 10_000),
                });
            }
            limited.drafts.push(LegacyDraft {
                legacy_id: 7_000 + number as i64,
                league_id: Some(legacy_league),
                draft: Draft {
                    token: token(4, number),
                    league_event_id: Some(DRAFT_EVENT_ID as u64),
                    course_id: Some(35_000_000 + number as u64),
                    draft_id: Some(80_000 + number as u64),
                    pod_token: None,
                    seat_index: Some(0),
                    seat_count: 8,
                    booster_catalog_id: None,
                    state: DraftState::Finished,
                    pack_size: 14,
                    picks_expected: 42,
                    started_at_ms: Some(started - HOUR_MS),
                    ended_at_ms: Some(started - 1_000),
                    picks,
                },
                notes: vec![(1, "Qnote first pick".to_string())],
            });
        }
        let now = START_MS + self.next_match_number as i64 * HOUR_MS;
        import_matches(&mut self.conn, &matches, now).expect("draft matches");
        import_leagues(&mut self.conn, &leagues, now).expect("draft leagues");
        import_limited(&mut self.conn, &limited, now).expect("drafts");
    }
}

impl Builder {
    /// One archetype list: 15 main cards of 4 copies (60) from the local
    /// catalog, offset by `seed`, and 15 sideboard cards of 1 from the
    /// opponent catalog.
    fn archetype_list(seed: usize) -> Vec<(u32, u32, bool)> {
        let mut cards = Vec::new();
        for index in 0..15 {
            let catalog_id = LOCAL_CATALOG + ((seed + index * 4) % LOCAL_CARDS) as u32;
            cards.push((catalog_id, 4, false));
        }
        for index in 0..OPPONENT_CARDS {
            cards.push((OPPONENT_CATALOG + index as u32, 1, true));
        }
        cards
    }

    /// The archetype seed (spec section 17): 500 downloaded archetypes with
    /// a definition and list 1, 10 manual archetypes with 5 variants, both
    /// seats of every constructed match (a tenth without the player seat;
    /// some opponents Homebrew or manual), and every deck's archetype.
    fn archetypes(&mut self) {
        let colors = ["W", "U", "B", "R", "G", "U,B", "R,G", "W,U,B"];
        let tx = self.conn.transaction().expect("tx");
        let mut downloaded = Vec::new();
        for number in 0..DOWNLOADED_ARCHETYPES {
            let uuid = token(7, number);
            let name = format!("Qarchetype {number}");
            let list = Self::archetype_list(number);
            let definition = archetypes::Definition {
                uuid: uuid.clone(),
                name: name.clone(),
                format: Some("CMODERN".to_string()),
                color_identity: Some(colors[number % colors.len()].to_string()),
                version: Some("1".to_string()),
                cards: list
                    .iter()
                    .map(
                        |(catalog_id, quantity, sideboard)| archetypes::DefinitionCard {
                            mtgo_id: i64::from(*catalog_id),
                            arena_id: None,
                            oracle_id: Some(format!("oracle-{catalog_id}")),
                            name: Some(format!("Qcard {catalog_id}")),
                            quantity: *quantity,
                            sideboard: *sideboard,
                        },
                    )
                    .collect(),
                ..archetypes::Definition::default()
            };
            let id: i64 = tx
                .query_row(
                    "INSERT INTO archetypes (uuid, name, format, color_identity, source,
                         definition, definition_version, created_at, updated_at)
                     VALUES (?1, ?2, 'CMODERN', ?3, 'downloaded', ?4, '1', ?5, ?5)
                     RETURNING id",
                    params![
                        uuid,
                        name,
                        colors[number % colors.len()],
                        serde_json::to_string(&definition).expect("definition json"),
                        START_MS
                    ],
                    |row| row.get(0),
                )
                .expect("downloaded archetype");
            for (catalog_id, quantity, sideboard) in list {
                tx.execute(
                    "INSERT INTO archetype_cards (archetype_id, list, catalog_id, oracle_id,
                         quantity, sideboard)
                     VALUES (?1, 1, ?2, ?3, ?4, ?5)",
                    params![
                        id,
                        catalog_id,
                        format!("oracle-{catalog_id}"),
                        quantity,
                        sideboard
                    ],
                )
                .expect("list card");
            }
            downloaded.push(id);
        }
        let mut manual = Vec::new();
        for number in 0..MANUAL_ARCHETYPES {
            let id: i64 = tx
                .query_row(
                    "INSERT INTO archetypes (uuid, name, format, color_identity, source,
                         created_at, updated_at)
                     VALUES (?1, ?2, 'CMODERN', 'U,B', 'manual', ?3, ?3)
                     RETURNING id",
                    params![
                        format!("q1b2c3d4-{}", token(8, number)),
                        format!("Qmanual {number}"),
                        START_MS
                    ],
                    |row| row.get(0),
                )
                .expect("manual archetype");
            for variant in 0..MANUAL_VARIANTS {
                for (catalog_id, quantity, sideboard) in
                    Self::archetype_list(number * MANUAL_VARIANTS + variant)
                {
                    tx.execute(
                        "INSERT INTO archetype_cards (archetype_id, list, catalog_id,
                             oracle_id, quantity, sideboard)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![
                            id,
                            variant as i64 + 1,
                            catalog_id,
                            format!("oracle-{catalog_id}"),
                            quantity,
                            sideboard
                        ],
                    )
                    .expect("variant card");
                }
            }
            manual.push(id);
        }
        let homebrew: i64 = tx
            .query_row(
                "SELECT id FROM archetypes WHERE uuid = '00000000-0000-0000-0000-000000000001'",
                [],
                |row| row.get(0),
            )
            .expect("homebrew");
        tx.execute(
            "UPDATE decks SET archetype_id = ?1 + (id % ?2) WHERE format = 'CMODERN'",
            params![downloaded[0], DOWNLOADED_ARCHETYPES as i64],
        )
        .expect("deck archetypes");
        let matches: Vec<(i64, i64)> = {
            let mut statement = tx
                .prepare(
                    "SELECT m.id, d.archetype_id FROM matches m
                     JOIN deck_versions v ON v.id = m.deck_version_id
                     JOIN decks d ON d.id = v.deck_id
                     WHERE m.format = 'CMODERN' ORDER BY m.id",
                )
                .expect("matches");
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .expect("query")
                .map(|row| row.expect("row"))
                .collect()
        };
        for (index, (match_id, deck_archetype)) in matches.iter().enumerate() {
            let opponent = if index % 20 == 0 {
                homebrew
            } else if index % 25 == 0 {
                manual[index % manual.len()]
            } else {
                downloaded[(index * 37) % downloaded.len()]
            };
            tx.execute(
                "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
                 VALUES (?1, 1, ?2, 0.85)",
                params![match_id, opponent],
            )
            .expect("opponent seat");
            if index % 10 != 9 {
                tx.execute(
                    "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id,
                         confidence)
                     VALUES (?1, 0, ?2, 1.0)",
                    params![match_id, deck_archetype],
                )
                .expect("player seat");
            }
            tx.execute(
                "UPDATE matches SET archetypes_detected_at = ended_at WHERE id = ?1",
                [match_id],
            )
            .expect("detected");
        }
        tx.commit().expect("commit");
    }
}

/// Every log match and league of the data set, stored (not yet synced).
/// Returns the live card match tokens (newest last).
fn build(path: &Path) -> (Connection, Vec<String>) {
    let mut builder = Builder::new(path);
    builder.accounts_and_files();
    builder.cards_and_decks();
    builder.constructed_runs();
    builder.drafts();
    let card_tokens = builder.card_matches();
    builder.archetypes();
    let live = card_tokens[CARD_MATCHES - LIVE_MATCHES..].to_vec();
    (builder.conn, live)
}

// ---- Timing ------------------------------------------------------------------

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// p50 and p95 of the samples, in milliseconds.
fn percentiles(samples: &[Duration]) -> (f64, f64) {
    let mut sorted: Vec<f64> = samples.iter().map(|sample| ms(*sample)).collect();
    sorted.sort_by(f64::total_cmp);
    let at = |share: usize| sorted[(sorted.len() * share / 100).min(sorted.len() - 1)];
    (at(50), at(95))
}

/// Time `run` `times` times.
fn sample(times: usize, mut run: impl FnMut()) -> Vec<Duration> {
    let mut samples = Vec::new();
    for _ in 0..times {
        let started = Instant::now();
        run();
        samples.push(started.elapsed());
    }
    samples
}

/// One measured row of the report.
struct Measured {
    name: &'static str,
    budget: &'static str,
    p50: f64,
    p95: f64,
    within: bool,
}

fn row(name: &'static str, budget: &'static str, samples: &[Duration], limit_ms: f64) -> Measured {
    let (p50, p95) = percentiles(samples);
    Measured {
        name,
        budget,
        p50,
        p95,
        within: p95 < limit_ms,
    }
}

fn number(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |row| row.get(0)).expect(sql)
}

// ---- The page reads, as the app makes them ------------------------------------

/// The match page's store reads (`app::matches::detail`): the header, the
/// games and their card rows, the events (each game's last frame, for the
/// turn estimate), the stored game log and the card lookups.
fn match_page(conn: &Connection, token: &str) -> usize {
    let header = match_header(conn, token).expect("header").expect("listed");
    let games = match_games(conn, header.id).expect("games");
    let cards = match_game_cards(conn, header.id).expect("game cards");
    let mut frames: HashMap<u64, Vec<String>> = HashMap::new();
    for stored in events_for_match(conn, token).expect("events") {
        let Some(event) = prepare_at(
            stored.log_file_id as u64,
            stored.byte_offset,
            stored.logged_at_ms,
            &stored.raw,
            None,
        ) else {
            continue;
        };
        if event.kind == mtgo_protocol::EventKind::GameStateUpdate
            && let Some(game_id) = event.subjects.game_id
        {
            frames.entry(game_id).or_default().push(event.text);
        }
    }
    let _log = game_log(conn, header.id).expect("game log");
    let mut ids: Vec<u32> = Vec::new();
    let mut board = 0;
    for game in &games {
        if let Some(texts) = game.mtgo_game_id.and_then(|id| frames.get(&id)) {
            let last = texts.iter().rev().find_map(|text| {
                game_detail::parse_frames(&[text.as_str()])
                    .into_iter()
                    .next()
            });
            if let Some(last) = last {
                board += last.cards.len();
                ids.extend(last.cards.iter().map(|card| card.catalog_id));
            }
        }
        if let Some(game_cards) = cards.get(&game.id) {
            ids.extend(game_cards.cards.iter().map(|row| row.catalog_id));
        }
    }
    ids.sort_unstable();
    ids.dedup();
    let lookup = card_infos(conn, &ids).expect("cards");
    board + lookup.len()
}

/// The account the pages read, and every owner they take.
fn owners(conn: &Connection) -> (Option<i64>, Vec<LeagueOwner>, Vec<i64>) {
    let account = current_account_id(conn, store::Platform::Mtgo).expect("account");
    let accounts = match account {
        Some(id) => vec![id],
        None => account_ids(conn, store::Platform::Mtgo).expect("accounts"),
    };
    let mut owners: Vec<LeagueOwner> = accounts
        .iter()
        .map(|id| LeagueOwner::Account(*id))
        .collect();
    owners.push(LeagueOwner::Nobody);
    (account, owners, accounts)
}

/// The Leagues page's store reads (`app::leagues::gather`): decks, version
/// dates, the constructed runs and the match notes.
fn constructed_runs(conn: &Connection) -> Vec<StoredRun> {
    let (_, owners, _) = owners(conn);
    let _decks = league_decks(conn).expect("decks");
    let _versions = version_dates(conn).expect("versions");
    let runs = stored_runs(
        conn,
        &RunFilter {
            owners,
            kinds: vec![LeagueKind::Constructed],
            deck_id: None,
            run_key: None,
        },
    )
    .expect("runs");
    let _notes = notes_by_match(conn).expect("notes");
    runs
}

/// The Limited index's store read (`app::limited::gather`).
fn limited_index(conn: &Connection) -> usize {
    let (_, owners, _) = owners(conn);
    let found = stored_limited_runs(
        conn,
        &RunFilter {
            owners,
            ..RunFilter::default()
        },
    )
    .expect("limited runs");
    found.runs.len() + found.unlinked_drafts.len()
}

/// The home dashboard's store reads (`app::dashboard::home_dashboard_view`,
/// All, all time): both gathers once, then each card's read.
fn dashboard(conn: &Connection) -> usize {
    let account = current_account_id(conn, store::Platform::Mtgo).expect("account");
    let range = Range::all_time();
    let all = FormatFilter::All;
    let runs = constructed_runs(conn);
    let limited = limited_index(conn);
    let _options = format_options(conn, account, store::Platform::Mtgo).expect("format options");
    let window =
        window_matches(conn, account, store::Platform::Mtgo, range, &all).expect("window matches");
    let kpis = overview(conn, account, store::Platform::Mtgo, range, None, &all).expect("overview");
    let now_ms: i64 = 1_791_374_400_000;
    let earlier =
        earlier_matches(conn, account, store::Platform::Mtgo, range, &all).expect("earlier");
    let sessions = recent_sessions(conn, account, store::Platform::Mtgo, &all, now_ms, UTC)
        .expect("recent sessions");
    let session = last_session(conn, account, &all, UTC).expect("last session");
    let spread = dashboard_matchup_spread(conn, account, range, &all).expect("spread");
    let recent =
        recent_matches(conn, account, store::Platform::Mtgo, range, &all).expect("recent matches");
    let decks = recent_decks(conn, account, store::Platform::Mtgo, &all).expect("recent decks");
    usize::from(kpis.record.wins > 0)
        + runs.len().min(1)
        + limited.min(1)
        + window.len().min(1)
        + earlier.len().min(1)
        + sessions.len()
        + usize::from(session.is_some())
        + spread.rows.len()
        + recent.len()
        + decks.len()
}

/// The Matches page (`app::matches` list): 50 rows, newest first, and each
/// row's opponent colours.
fn matches_list(conn: &Connection) -> usize {
    let page = match_list(
        conn,
        Scope::All(store::Platform::Mtgo),
        &MatchListFilter::newest_first(Range::all_time()),
        1,
        50,
    )
    .expect("list");
    let mut colored = 0;
    for row in &page.rows {
        if !opponent_color_identities(conn, &row.token)
            .expect("colors")
            .is_empty()
        {
            colored += 1;
        }
    }
    page.rows.len() + colored
}

// ---- The test ----------------------------------------------------------------

#[test]
#[ignore = "timing check, run in release"]
fn the_target_schema_meets_the_speed_budgets() {
    let dir = TestDir::new("target-schema-speed");
    let path = dir.join("mymtgo.sqlite");
    let started = Instant::now();
    let (mut conn, live) = build(&path);
    println!(
        "data set built in {:.1} s: {} matches, {} games, {} events, {} archives",
        started.elapsed().as_secs_f64(),
        number(&conn, "SELECT COUNT(*) FROM matches"),
        number(&conn, "SELECT COUNT(*) FROM games"),
        number(&conn, "SELECT COUNT(*) FROM log_events"),
        number(&conn, "SELECT COUNT(*) FROM match_event_archives"),
    );

    let mut measured = Vec::new();

    // The backfill of a fresh database: every log match's games in the
    // tracker's slices (one writer transaction each), then every league.
    let tokens = log_match_tokens(&conn).expect("tokens");
    let mut slices = Vec::new();
    let backfill = Instant::now();
    for slice in tokens.chunks(BACKFILL_SLICE) {
        let one = Instant::now();
        sync_games(&mut conn, slice, &[], UTC).expect("game backfill");
        slices.push(one.elapsed());
    }
    let games_done = backfill.elapsed();
    let league_started = Instant::now();
    sync_all(&mut conn, UTC, START_MS + 10_000 * HOUR_MS).expect("league backfill");
    let league_done = league_started.elapsed();
    let backfill_total = backfill.elapsed();
    let slowest_slice = slices.iter().max().copied().unwrap_or_default();
    println!(
        "backfill: games {:.0} ms in {} slices (slowest {:.1} ms), leagues {:.0} ms (one transaction), total {:.2} s",
        ms(games_done),
        slices.len(),
        ms(slowest_slice),
        ms(league_done),
        backfill_total.as_secs_f64()
    );
    println!(
        "stored: {} game card rows, {} leagues, {} drafts, {} picks, {} snapshots",
        number(&conn, "SELECT COUNT(*) FROM game_cards"),
        number(&conn, "SELECT COUNT(*) FROM leagues"),
        number(&conn, "SELECT COUNT(*) FROM drafts"),
        number(&conn, "SELECT COUNT(*) FROM draft_picks"),
        number(&conn, "SELECT COUNT(*) FROM limited_deck_snapshots"),
    );
    assert!(
        number(&conn, "SELECT COUNT(*) FROM game_cards")
            >= (CARD_MATCHES * GAMES * (LOCAL_CARDS + OPPONENT_CARDS)) as i64,
        "every card match has its rows"
    );
    assert_eq!(
        number(
            &conn,
            "SELECT COUNT(*) FROM leagues WHERE kind = 'constructed'"
        ),
        CONSTRUCTED_RUNS as i64
    );
    assert_eq!(
        number(&conn, "SELECT COUNT(*) FROM leagues WHERE kind = 'draft'"),
        DRAFTS as i64
    );
    measured.push(Measured {
        name: "Backfill, every game and league",
        budget: "< 10 s",
        p50: ms(backfill_total),
        p95: ms(backfill_total),
        within: backfill_total < Duration::from_secs(10),
    });
    measured.push(Measured {
        name: "Backfill, slowest writer transaction (game slice)",
        budget: "< 100 ms",
        p50: ms(slowest_slice),
        p95: ms(slowest_slice),
        within: slowest_slice < Duration::from_millis(100),
    });
    measured.push(Measured {
        name: "Backfill, league sync_all (one transaction)",
        budget: "< 100 ms",
        p50: ms(league_done),
        p95: ms(league_done),
        within: league_done < Duration::from_millis(100),
    });

    // Game sync, one settled game: the live match's game 3 rows are gone
    // (as before it settled), then its match is synced.
    let mut game_samples = Vec::new();
    for token in &live {
        conn.execute(
            "DELETE FROM game_cards WHERE game_id IN (SELECT g.id FROM games g
                 JOIN matches m ON m.id = g.match_id WHERE m.token = ?1 AND g.game_number = 3)",
            [token],
        )
        .expect("clear game 3");
        let one = Instant::now();
        let changed = sync_games(&mut conn, std::slice::from_ref(token), &[], UTC)
            .expect("sync")
            .changed;
        game_samples.push(one.elapsed());
        assert!(changed, "the settled game's rows are written");
    }
    measured.push(row(
        "Game sync, one settled game (live match)",
        "p95 < 20 ms",
        &game_samples,
        20.0,
    ));

    // League sync, one owner after a settled match: a new league match of
    // the first account (a new run, with its join line), then the owner's
    // sync in its own transaction.
    let account: i64 = conn
        .query_row(
            "SELECT id FROM accounts WHERE username = ?1",
            [ACCOUNTS[0]],
            |row| row.get(0),
        )
        .expect("account");
    let file: i64 = conn
        .query_row(
            "SELECT MAX(id) FROM log_files WHERE inherited_username = ?1",
            [ACCOUNTS[0]],
            |row| row.get(0),
        )
        .expect("file");
    let mut league_samples = Vec::new();
    for extra in 0..30 {
        let league_token = token(5, extra);
        let started_at = START_MS + (5_000 + extra as i64) * HOUR_MS;
        let mtgo_match_id = 400_000_000 + extra as u64;
        let games = (0..2)
            .map(|number| GameView {
                mtgo_game_id: Some(950_000_000 + extra as u64 * 10 + number),
                game_number: number as u32 + 1,
                result: GameResult::Win,
                end_reason: EndReason::Win,
                on_play: Some(number == 0),
                local_mulligans: Some(0),
                opp_mulligans: Some(0),
                local_die_roll: None,
                opp_die_roll: None,
                turn_count: Some(7),
                started_at_ms: Some(started_at + number as i64 * 60_000),
                ended_at_ms: Some(started_at + number as i64 * 60_000 + 50_000),
            })
            .collect();
        let view = MatchView {
            token: token(6, extra),
            mtgo_match_id: Some(mtgo_match_id),
            local_username: ACCOUNTS[0].to_string(),
            opponent: Some("Qoppnew".to_string()),
            format: Some("CMODERN".to_string()),
            match_type: Some("Modern".to_string()),
            tournament_event_id: None,
            tournament_round: None,
            league_token: Some(league_token.clone()),
            game_structure: Some("Modern".to_string()),
            state: MatchState::Complete,
            resolution: Some(Resolution::Signal),
            outcome: Outcome::Win,
            tentative_outcome: None,
            wins: 2,
            losses: 0,
            started_at_ms: started_at,
            ended_at_ms: Some(started_at + 120_000),
            last_activity_ms: started_at + 120_000,
            games,
        };
        let join = format!(
            "12:00:00 [INF] (UI|Creating GameDetailsView) League EventToken={league_token} EventId=12000"
        );
        let offset: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(byte_offset), 0) + 1000 FROM log_events WHERE log_file_id = ?1",
                [file],
                |row| row.get(0),
            )
            .expect("offset");
        let prepared =
            prepare_at(file as u64, offset as u64, started_at - 1_000, &join, None).expect("join");
        commit(
            &mut conn,
            &WriteBatch {
                events: vec![NewEvent {
                    log_file_id: file,
                    byte_offset: offset as u64,
                    kind: Some(prepared.kind),
                    subjects: prepared.subjects,
                    logged_at_ms: started_at - 1_000,
                    raw: join,
                    quarantined: false,
                }],
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(view),
                    revision: 1,
                }],
                now_ms: started_at + 120_000,
                ..WriteBatch::default()
            },
        )
        .expect("a settled league match");
        let one = Instant::now();
        let tx = conn.transaction().expect("tx");
        let outcome = sync_owner(
            &tx,
            LeagueOwner::Account(account),
            UTC,
            started_at + 130_000,
        )
        .expect("owner sync");
        tx.commit().expect("commit");
        league_samples.push(one.elapsed());
        assert!(outcome.leagues_changed, "the new run is stored");
    }
    measured.push(row(
        "League sync, one owner after a settled match",
        "p95 < 50 ms",
        &league_samples,
        50.0,
    ));
    drop(conn);

    // The page reads, on a read-only connection as the app opens it.
    let reader = open_reader(&path).expect("reader");
    let compacted: String = reader
        .query_row(
            "SELECT match_token FROM match_event_archives ORDER BY match_token LIMIT 1 OFFSET 400",
            [],
            |row| row.get(0),
        )
        .expect("a compacted match");
    let detail = sample(30, || {
        assert!(match_page(&reader, &compacted) > 0);
    });
    measured.push(row(
        "Match page data (match_detail, compacted match)",
        "< 20 ms",
        &detail,
        20.0,
    ));
    let leagues_page = sample(30, || {
        assert!(!constructed_runs(&reader).is_empty());
    });
    measured.push(row("Leagues page data", "< 20 ms", &leagues_page, 20.0));
    let limited_page = sample(30, || {
        assert!(limited_index(&reader) > 0);
    });
    measured.push(row("Limited index data", "< 20 ms", &limited_page, 20.0));
    let dashboard_page = sample(30, || {
        assert!(dashboard(&reader) > 0);
    });
    measured.push(row(
        "Home dashboard data (All, all time)",
        "< 20 ms",
        &dashboard_page,
        20.0,
    ));
    let list = sample(30, || {
        assert!(matches_list(&reader) >= 50);
    });
    measured.push(row(
        "Matches list, 50 rows with opponent colours",
        "< 20 ms",
        &list,
        20.0,
    ));

    println!();
    println!("| Measure | Budget | p50 (ms) | p95 (ms) | Within |");
    println!("|---|---|---|---|---|");
    for one in &measured {
        println!(
            "| {} | {} | {:.2} | {:.2} | {} |",
            one.name,
            one.budget,
            one.p50,
            one.p95,
            if one.within { "yes" } else { "no" }
        );
    }
    let missed: Vec<&str> = measured
        .iter()
        .filter(|one| !one.within)
        .map(|one| one.name)
        .collect();
    assert!(missed.is_empty(), "missed budgets: {missed:?}");
}

/// Print the measured table and fail on a missed budget.
fn report(measured: &[Measured]) {
    println!();
    println!("| Measure | Budget | p50 (ms) | p95 (ms) | Within |");
    println!("|---|---|---|---|---|");
    for one in measured {
        println!(
            "| {} | {} | {:.2} | {:.2} | {} |",
            one.name,
            one.budget,
            one.p50,
            one.p95,
            if one.within { "yes" } else { "no" }
        );
    }
    let missed: Vec<&str> = measured
        .iter()
        .filter(|one| !one.within)
        .map(|one| one.name)
        .collect();
    assert!(missed.is_empty(), "missed budgets: {missed:?}");
}

// The archetype reads (archetypes spec section 17) over the same data set
// with the archetype seed, its games synced. Plan T ruling C11: later Plan T
// tasks append their measures here.
#[test]
#[ignore = "timing check, run in release"]
fn the_archetype_reads_meet_the_speed_budgets() {
    let dir = TestDir::new("archetype-speed");
    let path = dir.join("mymtgo.sqlite");
    let started = Instant::now();
    let (mut conn, _live) = build(&path);
    let tokens = log_match_tokens(&conn).expect("tokens");
    for slice in tokens.chunks(BACKFILL_SLICE) {
        sync_games(&mut conn, slice, &[], UTC).expect("game backfill");
    }
    sync_all(&mut conn, UTC, START_MS + 10_000 * HOUR_MS).expect("league backfill");
    println!(
        "data set built and synced in {:.1} s: {} matches, {} archetypes, {} archetype cards, {} match archetype rows ({} player seats), {} game card rows",
        started.elapsed().as_secs_f64(),
        number(&conn, "SELECT COUNT(*) FROM matches"),
        number(&conn, "SELECT COUNT(*) FROM archetypes"),
        number(&conn, "SELECT COUNT(*) FROM archetype_cards"),
        number(&conn, "SELECT COUNT(*) FROM match_archetypes"),
        number(
            &conn,
            "SELECT COUNT(*) FROM match_archetypes WHERE is_opponent = 0"
        ),
        number(&conn, "SELECT COUNT(*) FROM game_cards"),
    );
    assert_eq!(
        number(
            &conn,
            "SELECT COUNT(*) FROM archetypes WHERE source = 'downloaded'"
        ),
        DOWNLOADED_ARCHETYPES as i64
    );
    assert_eq!(
        number(
            &conn,
            "SELECT COUNT(DISTINCT archetype_id || ':' || list) FROM archetype_cards
             WHERE archetype_id IN (SELECT id FROM archetypes WHERE source = 'manual')"
        ),
        (MANUAL_ARCHETYPES * MANUAL_VARIANTS) as i64
    );

    let mut measured = Vec::new();

    // Report and card stat candidates (spec section 13): the first run,
    // nothing queued yet, reads `ENQUEUE_LIMIT` of each in full.
    let reader = open_reader(&path).expect("reader");
    let first_reports = sample(10, || {
        let inputs =
            report_candidates(&reader, ENQUEUE_LIMIT, "1.0.0", AFTER_THE_HOLD).expect("reports");
        assert_eq!(inputs.len(), ENQUEUE_LIMIT as usize);
    });
    measured.push(row(
        "Report candidates, first run (500 read in full)",
        "background",
        &first_reports,
        f64::INFINITY,
    ));
    let first_stats = sample(10, || {
        let games =
            card_stats_candidates(&reader, ENQUEUE_LIMIT, AFTER_THE_HOLD).expect("card stats");
        assert_eq!(games.len(), ENQUEUE_LIMIT as usize);
    });
    measured.push(row(
        "Card stat candidates, first run (500 read in full)",
        "background",
        &first_stats,
        f64::INFINITY,
    ));
    // The candidate query alone, without the per-card reads (T6 review M1):
    // the first run's 500 games against the 50 ms budget.
    let first_query = sample(10, || {
        let mut statement = reader
            .prepare_cached(CARD_STATS_CANDIDATES_SQL)
            .expect("prepare");
        let mut rows = statement
            .query(rusqlite::params![ENQUEUE_LIMIT, AFTER_THE_HOLD])
            .expect("query");
        let mut read = 0;
        while rows.next().expect("row").is_some() {
            read += 1;
        }
        assert_eq!(read, ENQUEUE_LIMIT as usize);
    });
    measured.push(row(
        "Card stat candidate query alone, first run (500 games)",
        "< 50 ms",
        &first_query,
        50.0,
    ));
    let to_report = number(
        &reader,
        "SELECT COUNT(*) FROM matches m JOIN match_archetypes x
             ON x.match_id = m.id AND x.is_opponent = 0
         WHERE m.format = 'CMODERN' AND m.deck_version_id IS NOT NULL",
    ) as u32;
    let count = sample(30, || {
        assert_eq!(
            pending_report_count(&reader, AFTER_THE_HOLD).expect("count"),
            to_report
        );
    });
    measured.push(row(
        "Pending report count (to queue, 3,600 matches)",
        "< 50 ms",
        &count,
        50.0,
    ));

    // Sideboard guides and notes (spec section 21, T11): the deck and
    // opponent archetype with the most matches; the deck's latest version
    // becomes a real 75-card list of cards it played, and it gets a guide
    // with 15 planned cards and 5 notes against 30 archetypes.
    let (guide_deck, faced): (i64, i64) = conn
        .query_row(
            "SELECT v.deck_id, x.archetype_id FROM matches m
             JOIN deck_versions v ON v.id = m.deck_version_id
             JOIN match_archetypes x ON x.match_id = m.id AND x.is_opponent = 1
             GROUP BY v.deck_id, x.archetype_id ORDER BY COUNT(*) DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("most faced");
    let played: Vec<u32> = {
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT gc.catalog_id FROM game_cards gc
                 JOIN games g ON g.id = gc.game_id JOIN matches m ON m.id = g.match_id
                 JOIN deck_versions v ON v.id = m.deck_version_id
                 WHERE v.deck_id = ?1 AND gc.side = 'local' ORDER BY gc.catalog_id LIMIT 30",
            )
            .expect("prepare");
        stmt.query_map([guide_deck], |row| row.get(0))
            .expect("played")
            .map(|row| row.expect("row"))
            .collect()
    };
    let list: Vec<decks::DeckCard> = played
        .iter()
        .enumerate()
        .map(|(index, catalog_id)| decks::DeckCard {
            catalog_id: *catalog_id,
            quantity: if index < 15 { 4 } else { 1 },
            sideboard: index >= 15,
        })
        .collect();
    conn.execute(
        "UPDATE deck_versions SET signature = ?2 WHERE id =
             (SELECT id FROM deck_versions WHERE deck_id = ?1
              ORDER BY modified_at DESC, id DESC LIMIT 1)",
        params![guide_deck, decks::signature(&list)],
    )
    .expect("real list");
    let mut guided: Vec<i64> = vec![faced];
    {
        let mut stmt = conn
            .prepare("SELECT id FROM archetypes WHERE id <> ?1 ORDER BY id LIMIT 29")
            .expect("prepare");
        let rows = stmt.query_map([faced], |row| row.get(0)).expect("others");
        for row in rows {
            guided.push(row.expect("row"));
        }
    }
    // Four more decks have guides against the same archetypes too (#300
    // F13), so the per-deck and per-archetype reads skip other decks' rows.
    let mut guided_decks: Vec<i64> = vec![guide_deck];
    {
        let mut stmt = conn
            .prepare("SELECT id FROM decks WHERE id <> ?1 ORDER BY id LIMIT 4")
            .expect("prepare");
        let rows = stmt
            .query_map([guide_deck], |row| row.get(0))
            .expect("decks");
        for row in rows {
            guided_decks.push(row.expect("row"));
        }
    }
    assert_eq!(guided_decks.len(), 5, "guides on several decks");
    let mut faced_guide = 0;
    for (deck_id, archetype_id) in guided_decks.iter().flat_map(|deck_id| {
        guided
            .iter()
            .map(move |archetype_id| (*deck_id, archetype_id))
    }) {
        let guide_id: i64 = conn
            .query_row(
                "INSERT INTO sideboard_guides (deck_id, archetype_id, created_at, updated_at)
                 VALUES (?1, ?2, 1, 1) RETURNING id",
                [deck_id, *archetype_id],
                |row| row.get(0),
            )
            .expect("guide");
        if deck_id == guide_deck && *archetype_id == faced {
            faced_guide = guide_id;
        }
        for (index, card) in list.iter().enumerate().take(15) {
            let oracle: Option<String> = conn
                .query_row(
                    "SELECT oracle_id FROM cards WHERE catalog_id = ?1",
                    [card.catalog_id],
                    |row| row.get(0),
                )
                .unwrap_or(None);
            let direction = if index % 2 == 0 { "in" } else { "out" };
            conn.execute(
                "INSERT OR IGNORE INTO sideboard_guide_cards (guide_id, oracle_id, direction, quantity)
                 VALUES (?1, ?2, ?3, 1)",
                params![
                    guide_id,
                    oracle.unwrap_or_else(|| format!("q-oracle-{}", card.catalog_id)),
                    direction
                ],
            )
            .expect("plan card");
        }
        for n in 0..5 {
            conn.execute(
                "INSERT INTO deck_archetype_notes (deck_id, archetype_id, body, created_at)
                 VALUES (?1, ?2, 'Qnote ' || ?3, ?3)",
                params![deck_id, archetype_id, n],
            )
            .expect("note");
        }
    }
    let guide_list = sample(30, || {
        let guides = store::guides::deck_guides(&reader, guide_deck).expect("guides");
        assert_eq!(guides.len(), 30);
    });
    measured.push(row(
        "Deck guide list (30 guides, all-time matchups)",
        "< 50 ms",
        &guide_list,
        50.0,
    ));
    let editor = sample(30, || {
        let page = store::guides::guide_editor(&reader, faced_guide)
            .expect("editor")
            .expect("guide");
        assert!(
            page.editor
                .is_some_and(|editor| !editor.take_out.is_empty())
        );
    });
    measured.push(row(
        "Guide editor (75-card list, every version's games)",
        "< 50 ms",
        &editor,
        50.0,
    ));
    let block = sample(30, || {
        let rows =
            store::guides::archetype_guides(&reader, store::Platform::Mtgo, faced).expect("block");
        assert!(!rows.is_empty());
    });
    measured.push(row(
        "Archetype page guides and notes block",
        "< 50 ms",
        &block,
        50.0,
    ));

    // The steady state: everything but the newest ten matches and games
    // queued, so each query walks every match to find the few left.
    conn.execute_batch(
        "INSERT INTO outbox (kind, subject, payload, status, sent_at, created_at)
             SELECT 'match_report', token, x'', 'sent', 1, 1 FROM matches
             WHERE id NOT IN (SELECT id FROM matches ORDER BY started_at DESC LIMIT 10);
         INSERT INTO outbox (kind, subject, payload, status, sent_at, created_at)
             SELECT 'card_stats', m.token || ':' || g.mtgo_game_id, x'', 'sent', 1, 1
             FROM games g JOIN matches m ON m.id = g.match_id
             WHERE g.mtgo_game_id IS NOT NULL
               AND g.id NOT IN (SELECT id FROM games ORDER BY id DESC LIMIT 10);",
    )
    .expect("queued");
    drop(conn);
    let steady_reports = sample(30, || {
        let inputs =
            report_candidates(&reader, ENQUEUE_LIMIT, "1.0.0", AFTER_THE_HOLD).expect("reports");
        assert!(inputs.len() <= 10, "{}", inputs.len());
    });
    measured.push(row(
        "Report candidates, steady state (all but 10 queued)",
        "< 50 ms",
        &steady_reports,
        50.0,
    ));
    let steady_stats = sample(30, || {
        let games =
            card_stats_candidates(&reader, ENQUEUE_LIMIT, AFTER_THE_HOLD).expect("card stats");
        assert!(!games.is_empty() && games.len() <= 10, "{}", games.len());
    });
    measured.push(row(
        "Card stat candidates, steady state (all but 10 queued)",
        "< 50 ms",
        &steady_stats,
        50.0,
    ));
    let steady_count = sample(30, || {
        pending_report_count(&reader, AFTER_THE_HOLD).expect("count");
    });
    measured.push(row(
        "Pending report count, steady state",
        "< 50 ms",
        &steady_count,
        50.0,
    ));

    // Deck Matchups and Game Stats (archetypes spec section 17): the deck
    // with the most matches, all time, every version.
    let busiest = number(
        &reader,
        "SELECT v.deck_id FROM matches m JOIN deck_versions v ON v.id = m.deck_version_id
         GROUP BY v.deck_id ORDER BY COUNT(*) DESC LIMIT 1",
    );
    let versions = deck_version_ids(&reader, busiest, None).expect("versions");
    let spread_rows = matchup_spread(&reader, &versions, Range::all_time(), None)
        .expect("spread")
        .len();
    let matchups = sample(30, || {
        let spread = matchup_spread(&reader, &versions, Range::all_time(), None).expect("spread");
        assert_eq!(spread.len(), spread_rows);
    });
    measured.push(row(
        "Deck Matchups data (busiest deck, all time)",
        "< 20 ms",
        &matchups,
        20.0,
    ));
    let faced = number(
        &reader,
        &format!(
            "SELECT ma.archetype_id FROM matches m
             JOIN deck_versions v ON v.id = m.deck_version_id
             JOIN match_archetypes ma ON ma.match_id = m.id AND ma.is_opponent = 1
             WHERE v.deck_id = {busiest} GROUP BY ma.archetype_id ORDER BY COUNT(*) DESC LIMIT 1"
        ),
    );
    let detail = sample(30, || {
        matchup_detail(&reader, &versions, faced, Range::all_time(), UTC, None).expect("detail");
    });
    measured.push(row(
        "Deck Matchups drawer (most faced archetype)",
        "< 20 ms",
        &detail,
        20.0,
    ));
    let game_rows = sample(30, || {
        let rows = game_stats(&reader, busiest, Range::all_time(), None).expect("game stats");
        assert_eq!(rows.len(), 12);
        game_stats_opponents(&reader, busiest).expect("opponents");
    });
    measured.push(row(
        "Deck Game Stats data (busiest deck, all time)",
        "< 20 ms",
        &game_rows,
        20.0,
    ));

    // The Metagame page (spec section 9): one format, the 30 days up to
    // the newest match, without the network.
    let newest = number(&reader, "SELECT MAX(started_at) FROM matches");
    let window = metagame_window(Timeframe::Last30, newest, UTC);
    let account = current_account_id(&reader, store::Platform::Mtgo).expect("account");
    let metagame = sample(30, || {
        metagame_formats(&reader, account, store::Platform::Mtgo).expect("formats");
        let data = metagame_data(
            &reader,
            account,
            store::Platform::Mtgo,
            "modern",
            window,
            None,
        )
        .expect("metagame");
        let total = data.total;
        let _ = archetypes::metagame::tiles(data.faced, total, None);
    });
    measured.push(row(
        "Metagame page data (modern, 30 days)",
        "< 20 ms",
        &metagame,
        20.0,
    ));
    // The archetype detail (spec section 10): the most faced archetype, all
    // time, every deck against it.
    let most_faced = number(
        &reader,
        "SELECT archetype_id FROM match_archetypes WHERE is_opponent = 1
         GROUP BY archetype_id ORDER BY COUNT(*) DESC LIMIT 1",
    );
    let archetype_page = sample(30, || {
        let found = archetype_detail(
            &reader,
            account,
            store::Platform::Mtgo,
            most_faced,
            Range::all_time(),
            UTC,
        )
        .expect("detail");
        assert!(found.is_some());
    });
    measured.push(row(
        "Archetype detail data (most faced, all time)",
        "< 50 ms",
        &archetype_page,
        50.0,
    ));

    // Task 13: the Cards page with popularity (spec section 17: inside Plan
    // S's 100 ms), its archetype sheet, the card stats filter's options and
    // the two dashboard cards.
    let cards = sample(30, || {
        let page = cards_page(&reader, &CardsPageQuery::new()).expect("cards page");
        assert!(page.cards[0].popularity > 0, "popularity is counted");
    });
    measured.push(row(
        "Cards page with popularity (grouped, page 1)",
        "< 100 ms",
        &cards,
        100.0,
    ));
    let modern = CardsPageQuery {
        format: Some("modern".to_string()),
        ..CardsPageQuery::new()
    };
    let cards_modern = sample(30, || {
        cards_page(&reader, &modern).expect("cards page");
    });
    measured.push(row(
        "Cards page with popularity (modern only)",
        "< 100 ms",
        &cards_modern,
        100.0,
    ));
    let top_group: String = reader
        .query_row(
            "SELECT coalesce(c.oracle_id, 'id:' || ac.catalog_id) FROM archetype_cards ac
             LEFT JOIN cards c ON c.catalog_id = ac.catalog_id
             GROUP BY 1 ORDER BY COUNT(DISTINCT ac.archetype_id) DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("most held card");
    let sheet = sample(30, || {
        assert!(
            !card_archetypes(&reader, &top_group, None)
                .expect("sheet")
                .is_empty()
        );
    });
    measured.push(row(
        "Cards page archetype sheet (most held card)",
        "< 100 ms",
        &sheet,
        100.0,
    ));
    let options = sample(30, || {
        assert!(
            !opponent_archetypes(&reader, busiest, None, None)
                .expect("options")
                .is_empty()
        );
    });
    measured.push(row(
        "Card stats opponent archetype options (busiest deck)",
        "< 100 ms",
        &options,
        100.0,
    ));
    let spread = sample(30, || {
        assert!(
            !dashboard_matchup_spread(&reader, account, Range::all_time(), &FormatFilter::All)
                .expect("spread")
                .rows
                .is_empty()
        );
    });
    measured.push(row(
        "Dashboard Top matchups (all time)",
        "< 20 ms",
        &spread,
        20.0,
    ));

    // T15: the Matches list filtered to the most faced opponent archetype
    // (spec section 17, inside Plan K's 20 ms), the filter's options (T8
    // review M7), and the sync bundle reads that now take the archetype,
    // guide and note rows (spec sections 14 and 21.5; the sync runs in the
    // background on the api worker).
    let mut filtered = MatchListFilter::newest_first(Range::all_time());
    filtered.archetype = Some(ArchetypeFilter::Is(faced));
    let list = sample(30, || {
        let page =
            match_list(&reader, Scope::All(store::Platform::Mtgo), &filtered, 1, 50).expect("list");
        assert!(!page.rows.is_empty());
    });
    measured.push(row(
        "Matches list, 50 rows filtered to an opponent archetype",
        "< 20 ms",
        &list,
        20.0,
    ));
    let options = sample(30, || {
        assert!(
            !archetype_options(&reader, Some("modern"))
                .expect("options")
                .is_empty()
        );
    });
    measured.push(row(
        "Matches list archetype filter options (modern)",
        "< 20 ms",
        &options,
        20.0,
    ));
    let parts = sample(30, || {
        let parts = deck_bundle_parts(&reader, guide_deck).expect("parts");
        assert!(parts.guides.len() >= 30, "{}", parts.guides.len());
    });
    measured.push(row(
        "Deck bundle rows (30 guides of 15 planned cards, 5 notes each)",
        "background",
        &parts,
        f64::INFINITY,
    ));
    let seated = number(
        &reader,
        "SELECT x.match_id FROM match_archetypes x JOIN matches m ON m.id = x.match_id
         WHERE m.source = 'log' GROUP BY x.match_id HAVING COUNT(*) = 2 LIMIT 1",
    );
    let bundle = sample(10, || {
        let input = match_bundle_input(&reader, seated, UTC)
            .expect("input")
            .expect("a match");
        assert_eq!(input.archetypes.len(), 2);
    });
    measured.push(row(
        "Match bundle input with both archetype seats (a log match)",
        "background",
        &bundle,
        f64::INFINITY,
    ));

    report(&measured);
}

// The overlay reads (overlay spec section 14) over the same data set, its
// games and leagues synced; its own test (Plan U, as Plan T ruling C11).
// One of account 1's constructed runs is made active and one of the live
// (uncompacted) card matches joins it in progress, so the league card reads
// a run with a live match, its frames and its sideboarding changes.
#[test]
#[ignore = "timing check, run in release"]
fn league_card_compute_under_5_ms() {
    use overlay::{CardLook, GRACE_MS, LeagueStatus, league_state};
    use store::overlay::{
        head_to_head, last_faced_archetype, latest_frame, league_card_inputs, live_game_inputs,
    };

    let dir = TestDir::new("overlay-speed");
    let path = dir.join("mymtgo.sqlite");
    let (mut conn, live) = build(&path);
    let tokens = log_match_tokens(&conn).expect("tokens");
    for slice in tokens.chunks(BACKFILL_SLICE) {
        sync_games(&mut conn, slice, &[], UTC).expect("game backfill");
    }
    sync_all(&mut conn, UTC, START_MS + 10_000 * HOUR_MS).expect("league backfill");
    assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), 3_600);

    let account = number(
        &conn,
        &format!("SELECT id FROM accounts WHERE username = '{}'", ACCOUNTS[0]),
    );
    let run = number(
        &conn,
        &format!(
            "SELECT id FROM leagues WHERE account_id = {account} AND kind = 'constructed'
             ORDER BY started_at DESC LIMIT 1"
        ),
    );
    // The newest live card match of account 1 (card match numbers alternate
    // accounts, so the last token is account 2's).
    let live_token = live[live.len() - 2].clone();
    conn.execute(
        "UPDATE leagues SET state = 'active', completed_at = NULL WHERE id = ?1",
        [run],
    )
    .expect("active run");
    conn.execute(
        "UPDATE matches SET league_id = ?1, state = 'in_progress', outcome = 'unknown'
         WHERE token = ?2",
        params![run, live_token],
    )
    .expect("live member");
    let now_ms: i64 = number(
        &conn,
        &format!("SELECT started_at FROM matches WHERE token = '{live_token}'"),
    ) + 30 * 60_000;
    drop(conn);
    let reader = open_reader(&path).expect("reader");
    let look = CardLook {
        card_file_base: "card://localhost/".to_string(),
        ..CardLook::default()
    };

    let mut measured = Vec::new();
    let card = sample(50, || {
        let input = league_card_inputs(&reader, account, now_ms, GRACE_MS).expect("card inputs");
        let state = league_state(input, now_ms, GRACE_MS, &look);
        assert!(
            matches!(
                state.status,
                LeagueStatus::InGame | LeagueStatus::Sideboarding
            ),
            "{:?}",
            state.status
        );
    });
    measured.push(row(
        "League card compute (active run, live match)",
        "< 5 ms",
        &card,
        5.0,
    ));
    let other = number(
        &reader,
        &format!("SELECT id FROM accounts WHERE username = '{}'", ACCOUNTS[1]),
    );
    // A minute after account 2's last run ended (every kind): still on the
    // card.
    let ended = number(
        &reader,
        &format!(
            "SELECT MAX(COALESCE(completed_at, dropped_at)) FROM leagues
             WHERE account_id = {other}"
        ),
    ) + 60_000;
    let grace = sample(50, || {
        let input = league_card_inputs(&reader, other, ended, GRACE_MS).expect("card inputs");
        let status = league_state(input, ended, GRACE_MS, &look).status;
        assert!(
            matches!(
                status,
                LeagueStatus::Complete | LeagueStatus::Trophied | LeagueStatus::Dropped
            ),
            "{status:?}"
        );
    });
    measured.push(row(
        "League card compute (a run ended a minute ago)",
        "< 5 ms",
        &grace,
        5.0,
    ));
    // Every run ended long before: the idle card and the last played deck.
    let idle_other = sample(50, || {
        let later = START_MS + 20_000 * HOUR_MS;
        let input = league_card_inputs(&reader, other, later, GRACE_MS).expect("card inputs");
        assert_eq!(
            league_state(input, later, GRACE_MS, &look).status,
            LeagueStatus::Idle
        );
    });
    measured.push(row(
        "League card compute (idle, last played deck)",
        "< 5 ms",
        &idle_other,
        5.0,
    ));

    // The game view's reads of the live match (inside its 10 ms, measured
    // with the view in `app`), head-to-head over every match (no index on
    // `matches.opponent_id`, U4 concern 4).
    let inputs = live_game_inputs(&reader, &live_token)
        .expect("inputs")
        .expect("live match");
    let opponent = inputs.opponent_id.expect("opponent");
    let live_game = inputs
        .games
        .last()
        .and_then(|game| game.mtgo_game_id)
        .expect("game id");
    let newest = sample(200, || {
        assert!(latest_frame(&reader, live_game).expect("frame").is_some());
    });
    measured.push(row(
        "Newest frame read (log_events_frames)",
        "< 1 ms",
        &newest,
        1.0,
    ));
    let reads = sample(50, || {
        let found = live_game_inputs(&reader, &live_token)
            .expect("inputs")
            .expect("live match");
        assert_eq!(found.games.len(), GAMES);
    });
    measured.push(row(
        "Live game inputs (3 games, newest frames decoded)",
        "< 10 ms (game view)",
        &reads,
        10.0,
    ));
    let record = sample(200, || {
        let (wins, losses) =
            head_to_head(&reader, opponent, inputs.format.as_deref(), inputs.match_id)
                .expect("head to head");
        assert!(wins + losses > 0);
    });
    measured.push(row(
        "Head-to-head (3,600 matches, opponent_id unindexed)",
        "< 10 ms (game view)",
        &record,
        10.0,
    ));
    let faced = sample(200, || {
        last_faced_archetype(&reader, opponent, inputs.format.as_deref(), inputs.match_id)
            .expect("last faced");
    });
    measured.push(row(
        "Last faced archetype (3,600 matches)",
        "< 10 ms (game view)",
        &faced,
        10.0,
    ));

    // The event card's tournaments (event overlay plan B6): the Challenge
    // fixture's two Challenges as account 1's (the lobby one's state changes
    // made joined ones of the same length), the tournament sync over them,
    // then the card with a live league match and both tournaments.
    let mut writer = open_writer(&path).expect("writer");
    let mut fixture = fixture_tools::load_fixture("../tournaments/challenge_showcase");
    let text = String::from_utf8(fixture.bytes.clone()).expect("utf-8");
    let lobby = "Tournament State Changed for 63c90ead-8c2c-407d-9f10-67378e0b96d0";
    fixture.bytes = text
        .split_inclusive('\n')
        .map(|line| {
            if line.contains(lobby) {
                line.replace("NotJoined", "XxxJoined")
            } else {
                line.to_string()
            }
        })
        .collect::<String>()
        .into_bytes();
    let file = store::write::insert_log_file(
        &writer,
        &NewLogFile {
            path: "Q/challenge/mtgo.log".to_string(),
            identity_hash: 9_999,
            head_hash: 2,
            head_len: 10,
            created_at_fs: Some(START_MS),
            day_anchor: fixture.meta.day_anchor(),
            timezone: fixture.meta.timezone.clone(),
            inherited_username: Some(ACCOUNTS[0].to_string()),
            seen_at_ms: START_MS,
        },
    )
    .expect("fixture file");
    writer
        .execute(
            "UPDATE log_files SET local_login_id = 900001 WHERE id = ?1",
            [file],
        )
        .expect("login id");
    let events = fixture_tools::prepared_events(&fixture)
        .into_iter()
        .map(|event| NewEvent {
            log_file_id: file,
            byte_offset: event.offset,
            kind: Some(event.kind),
            subjects: event.subjects,
            logged_at_ms: event.at_ms,
            raw: event.text,
            quarantined: false,
        })
        .collect();
    commit(
        &mut writer,
        &WriteBatch {
            events,
            ..WriteBatch::default()
        },
    )
    .expect("fixture events");
    let fixture_now = fixture.meta.now_ms();
    store::tournament_sync::sync_all(&mut writer, fixture_now).expect("first sync");
    assert_eq!(
        number(&writer, "SELECT COUNT(*) FROM tournament_standings"),
        2
    );
    let synced = sample(50, || {
        store::tournament_sync::sync_all(&mut writer, fixture_now).expect("sync");
    });
    measured.push(row(
        "Tournament sync (two joined Challenges)",
        "< 10 ms",
        &synced,
        10.0,
    ));
    // Cold: every round info line decoded again (a first sync, or after a
    // version raise).
    let cold = sample(20, || {
        writer
            .execute_batch("DELETE FROM tournament_rounds; DELETE FROM tournament_standings;")
            .expect("cold");
        store::tournament_sync::sync_all(&mut writer, fixture_now).expect("sync");
    });
    // Once at start or after a fold version raise, off the live path: its
    // own budget (proposed), not the live sync's.
    measured.push(row(
        "Tournament sync, first (every round info line decoded)",
        "< 50 ms (once)",
        &cold,
        50.0,
    ));
    writer
        .execute(
            "UPDATE tournament_standings SET status = 'waiting_for_round', finish = NULL,
                 place = NULL, last_event_at = ?1, last_transition_at = ?1",
            [now_ms - 60_000],
        )
        .expect("both running at the league card's time");
    drop(writer);
    let strip_look = CardLook {
        strip: true,
        ..look.clone()
    };
    let event_card = sample(50, || {
        let input = league_card_inputs(&reader, account, now_ms, GRACE_MS).expect("card inputs");
        assert_eq!(input.tournaments.len(), 2);
        let state = league_state(input, now_ms, GRACE_MS, &strip_look);
        assert!(
            matches!(
                state.status,
                LeagueStatus::InGame | LeagueStatus::Sideboarding
            ),
            "{:?}",
            state.status
        );
        assert!(state.other.is_some(), "a tournament on the strip");
    });
    measured.push(row(
        "Event card compute (live league match, two tournaments)",
        "< 5 ms",
        &event_card,
        5.0,
    ));

    // 30,000 more matches of the account in the last 90 days, in a format
    // neither tournament plays: the format deck walk's worst case.
    let writer = open_writer(&path).expect("writer");
    writer
        .execute(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 30000)
             INSERT INTO matches (token, account_id, source, format, match_type, state,
                 outcome, started_at, last_activity_at, revision, derived_version,
                 deck_version_id)
             SELECT 'q-many-' || i, ?1, 'log', 'CPAUPER', 'Constructed', 'complete', 'win',
                 ?2 - i * 250000, ?2 - i * 250000, 1, 1,
                 (SELECT MIN(id) FROM deck_versions)
             FROM n",
            params![account, now_ms],
        )
        .expect("many matches");
    let waiting_many = sample(50, || {
        let input = league_card_inputs(&reader, account, now_ms, GRACE_MS).expect("card inputs");
        assert_eq!(input.tournaments.len(), 2);
        league_state(input, now_ms, GRACE_MS, &strip_look);
    });
    measured.push(row(
        "Event card compute (as above, 33,600 matches)",
        "< 5 ms",
        &waiting_many,
        5.0,
    ));
    writer
        .execute(
            "UPDATE tournament_standings SET status = 'registered', round = NULL,
                 matches_left = NULL, starts_at = ?1",
            [now_ms + 600_000],
        )
        .expect("both registered");
    drop(writer);
    let registered = sample(50, || {
        let input = league_card_inputs(&reader, account, now_ms, GRACE_MS).expect("card inputs");
        assert_eq!(input.tournaments.len(), 2);
        league_state(input, now_ms, GRACE_MS, &strip_look);
    });
    measured.push(row(
        "Event card compute (two registered tournaments, format deck walk, 33,600 matches)",
        "< 5 ms",
        &registered,
        5.0,
    ));

    report(&measured);
}

// The replay reads (replays spec section 12) over the same data set, its
// games synced: the replay window's data for one game of a compacted and of
// a live match (the whole match read, one game decorated, as `replay_view`
// does), and the match page with the Replay button flags it adds
// (`games_with_frames`, one read per page).
#[test]
#[ignore = "timing check, run in release"]
fn the_replay_reads_meet_the_speed_budgets() {
    use replay::{ImageMode, decorate_frames};
    use store::replay::{card_infos as replay_card_infos, games_with_frames, replay_inputs};

    let dir = TestDir::new("replay-reads-speed");
    let path = dir.join("mymtgo.sqlite");
    let (mut conn, live) = build(&path);
    let tokens = log_match_tokens(&conn).expect("tokens");
    for slice in tokens.chunks(BACKFILL_SLICE) {
        sync_games(&mut conn, slice, &[], UTC).expect("game backfill");
    }
    drop(conn);

    let reader = open_reader(&path).expect("reader");
    let compacted: String = reader
        .query_row(
            "SELECT match_token FROM match_event_archives ORDER BY match_token LIMIT 1 OFFSET 400",
            [],
            |row| row.get(0),
        )
        .expect("a compacted match");
    let id_of = |token: &str| -> i64 {
        reader
            .query_row("SELECT id FROM matches WHERE token = ?1", [token], |row| {
                row.get(0)
            })
            .expect("match id")
    };
    let view = |match_id: i64| -> usize {
        let inputs = replay_inputs(&reader, match_id, UTC, &[])
            .expect("read")
            .expect("a match");
        let game = inputs.games[0].clone();
        let mut ids: Vec<u32> = game
            .frames
            .iter()
            .filter_map(|frame| frame.content["Cards"].as_array())
            .flatten()
            .filter_map(|card| card["CatalogID"].as_u64())
            .map(|id| id as u32)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        let cards = replay_card_infos(&reader, &ids).expect("cards");
        decorate_frames(game.frames, &inputs.local_username, &cards, ImageMode::App).len()
    };

    let mut measured = Vec::new();
    let compacted_id = id_of(&compacted);
    let samples = sample(30, || {
        assert!(view(compacted_id) > 0);
    });
    measured.push(row(
        "Replay view data, one game (compacted match)",
        "p95 < 300 ms",
        &samples,
        300.0,
    ));
    let live_id = id_of(&live[0]);
    let samples = sample(30, || {
        assert!(view(live_id) > 0);
    });
    measured.push(row(
        "Replay view data, one game (live match)",
        "p95 < 300 ms",
        &samples,
        300.0,
    ));
    let samples = sample(30, || {
        assert!(match_page(&reader, &compacted) > 0);
        assert_eq!(
            games_with_frames(&reader, compacted_id)
                .expect("flags")
                .len(),
            3
        );
    });
    measured.push(row(
        "Match page data with the Replay flags (compacted match)",
        "< 20 ms",
        &samples,
        20.0,
    ));
    report(&measured);
}
