//! Builders for store tests. Names are fakes starting with Q.

use chrono::{NaiveDate, NaiveTime};
use fixture_tools::TestDir;
use mtgo_match::{EndReason, GameResult, GameView, MatchState, MatchView, Outcome, Resolution};
use mtgo_protocol::{ClockState, EventKind, Subjects};
use rusqlite::Connection;

use crate::arena::{NewArenaLogFile, insert_arena_log_file};
use crate::import::{ImportedGame, ImportedMatch};
use crate::open_writer;
use crate::write::{FileProgress, NewEvent, NewLogFile, WriteBatch, commit};

pub const TOKEN: &str = "f5e33a1f-c2e7-4678-b30d-309b63f17a40";
pub const OTHER: &str = "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d";

/// A fresh database in a temporary folder named after `label`.
pub fn db(label: &str) -> (TestDir, Connection) {
    let dir = TestDir::new(label);
    let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
    (dir, conn)
}

/// Commits `events` on their own.
pub fn store_events(conn: &mut Connection, events: Vec<NewEvent>) {
    commit(
        conn,
        &WriteBatch {
            events,
            ..WriteBatch::default()
        },
    )
    .expect("events");
}

/// A new account named `name`.
pub fn account(conn: &Connection, name: &str) -> i64 {
    conn.query_row(
        "INSERT INTO accounts (username, first_seen_at) VALUES (?1, 0) RETURNING id",
        [name],
        |row| row.get(0),
    )
    .expect("account")
}

/// The row id of the match with `token`.
pub fn match_id(conn: &Connection, token: &str) -> i64 {
    conn.query_row("SELECT id FROM matches WHERE token = ?1", [token], |row| {
        row.get(0)
    })
    .expect("match id")
}

pub fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).expect("date")
}

pub fn new_log_file(path: &str) -> NewLogFile {
    NewLogFile {
        path: path.to_string(),
        identity_hash: 1,
        head_hash: 2,
        head_len: 10,
        created_at_fs: Some(5),
        day_anchor: date(2026, 8, 22),
        timezone: "UTC".to_string(),
        inherited_username: Some("Qlocal".to_string()),
        seen_at_ms: 100,
    }
}

pub fn event_with(
    file: i64,
    offset: u64,
    kind: EventKind,
    subjects: Subjects,
    raw: &str,
) -> NewEvent {
    NewEvent {
        log_file_id: file,
        byte_offset: offset,
        kind: Some(kind),
        subjects,
        logged_at_ms: 1_000 + offset as i64,
        raw: raw.to_string(),
        quarantined: false,
    }
}

pub fn token_event(file: i64, offset: u64, token: &str) -> NewEvent {
    let subjects = Subjects {
        match_token: Some(token.to_string()),
        ..Subjects::default()
    };
    event_with(
        file,
        offset,
        EventKind::MatchStateChanged,
        subjects,
        "state change",
    )
}

pub fn progress(file: i64, cursor: u64) -> FileProgress {
    FileProgress {
        log_file_id: file,
        cursor_offset: cursor,
        clock: ClockState {
            date: date(2026, 8, 23),
            last: NaiveTime::from_hms_opt(23, 59, 58),
        },
        last_observed_size: cursor + 10,
        head_hash: 3,
        head_len: 4096,
        anchor: Some((16_384, 77)),
        local_username: Some("Qlocal".to_string()),
        local_login_id: Some(5),
        seen_at_ms: 200,
    }
}

pub fn game(number: u32, id: Option<u64>, result: GameResult) -> GameView {
    GameView {
        mtgo_game_id: id,
        game_number: number,
        result,
        end_reason: EndReason::Win,
        on_play: Some(number % 2 == 1),
        local_mulligans: Some(0),
        opp_mulligans: Some(1),
        local_die_roll: Some(3),
        opp_die_roll: Some(5),
        turn_count: Some(9),
        started_at_ms: Some(1_000),
        ended_at_ms: Some(2_000),
    }
}

/// A complete 2-1 win against `opponent`, Match ID 99, Game IDs 7, 8, 9.
pub fn view(token: &str, opponent: &str, started_at_ms: i64) -> MatchView {
    MatchView {
        token: token.to_string(),
        mtgo_match_id: Some(99),
        local_username: "Qlocal".to_string(),
        opponent: Some(opponent.to_string()),
        format: Some("CMODERN".to_string()),
        match_type: Some("Modern".to_string()),
        tournament_event_id: None,
        tournament_round: None,
        league_token: None,
        game_structure: None,
        state: MatchState::Complete,
        resolution: Some(Resolution::Signal),
        outcome: Outcome::Win,
        tentative_outcome: None,
        wins: 2,
        losses: 1,
        started_at_ms,
        ended_at_ms: Some(started_at_ms + 4_000),
        last_activity_ms: started_at_ms + 4_000,
        games: vec![
            game(1, Some(7), GameResult::Win),
            game(2, Some(8), GameResult::Loss),
            game(3, Some(9), GameResult::Win),
        ],
    }
}

pub fn number(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |row| row.get(0)).expect(sql)
}

/// The league match `token` was in when it was deleted
/// (`deleted_from_league_id`, league self-management spec section 13).
pub fn deleted_from(conn: &Connection, token: &str) -> Option<i64> {
    conn.query_row(
        "SELECT deleted_from_league_id FROM matches WHERE token = ?1",
        [token],
        |row| row.get(0),
    )
    .expect("deleted_from_league_id")
}

/// An imported 0.x game: on the play in odd games, ended at 2_000.
pub fn imported_game(number: u32, id: Option<i64>, result: &str) -> ImportedGame {
    ImportedGame {
        mtgo_game_id: id,
        game_number: number,
        result: result.to_string(),
        on_play: Some(number % 2 == 1),
        local_mulligans: Some(0),
        opp_mulligans: Some(1),
        local_die_roll: Some(3),
        opp_die_roll: Some(5),
        turn_count: Some(9),
        started_at_ms: Some(1_000),
        ended_at_ms: Some(2_000),
    }
}

/// A complete 2-1 win imported from 0.x by Qlocal against Qopponent, no note,
/// Game IDs 7, 8, 9.
pub fn imported(token: &str, mtgo_match_id: Option<i64>) -> ImportedMatch {
    ImportedMatch {
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
        games: vec![
            imported_game(1, Some(7), "win"),
            imported_game(2, Some(8), "loss"),
            imported_game(3, Some(9), "win"),
        ],
        manual: None,
        games_won_imported: None,
        games_lost_imported: None,
        deck_linked: false,
        replay_share: None,
        legacy_history: false,
    }
}

/// Hide a match as "Remove from stats" does (Plan K): every read that lists
/// or counts matches leaves it out.
pub fn hide(conn: &Connection, token: &str) {
    conn.execute(
        "UPDATE matches SET hidden_at = 1 WHERE token = ?1 AND hidden_at IS NULL",
        [token],
    )
    .expect("hide");
}

/// The Arena account's id, made on first use.
pub fn arena_account(conn: &Connection) -> i64 {
    conn.execute(
        "INSERT INTO accounts (username, first_seen_at, platform, arena_user_id)
         SELECT 'Qarena#12345', 0, 'arena', 'Qarena-id'
         WHERE NOT EXISTS (SELECT 1 FROM accounts WHERE platform = 'arena')",
        [],
    )
    .expect("arena account");
    conn.query_row(
        "SELECT id FROM accounts WHERE platform = 'arena'",
        [],
        |row| row.get(0),
    )
    .expect("arena account id")
}

/// A new Arena log file at `path` with fixed fake head values.
pub fn new_arena_log_file(path: &str) -> NewArenaLogFile {
    NewArenaLogFile {
        path: path.to_string(),
        identity_hash: 1,
        head_hash: 2,
        head_len: 3,
        created_at_fs: None,
        seen_at_ms: 4,
    }
}

/// The id of the Arena log file `/arena/Player.log`, made on first use.
pub fn arena_log_file(conn: &Connection) -> i64 {
    let known: Option<i64> = conn
        .query_row("SELECT id FROM arena_log_files", [], |row| row.get(0))
        .ok();
    if let Some(id) = known {
        return id;
    }
    insert_arena_log_file(conn, &new_arena_log_file("/arena/Player.log")).expect("arena log file")
}

/// Moves the match `token` to the Arena account.
pub fn move_match_to_arena(conn: &Connection, token: &str) {
    let arena = arena_account(conn);
    conn.execute(
        "UPDATE matches SET account_id = ?2 WHERE token = ?1",
        rusqlite::params![token, arena],
    )
    .expect("move match");
}

/// Sets the match `token`'s `game_structure` (Arena `bo1` or `bo3`; MTGO's
/// structure text such as `Modern`).
pub fn set_game_structure(conn: &Connection, token: &str, value: &str) {
    conn.execute(
        "UPDATE matches SET game_structure = ?2 WHERE token = ?1",
        rusqlite::params![token, value],
    )
    .expect("game structure");
}

/// Makes the opponent `username` an Arena opponent shown as `display_name`
/// (the tracker stores Arena opponents as `arena:<user id>`).
pub fn name_arena_opponent(conn: &Connection, username: &str, display_name: &str) {
    conn.execute(
        "UPDATE opponents SET platform = 'arena', display_name = ?2 WHERE username = ?1",
        rusqlite::params![username, display_name],
    )
    .expect("name opponent");
}

/// Moves the deck `deck_id` to the Arena account.
pub fn move_deck_to_arena(conn: &Connection, deck_id: i64) {
    let arena = arena_account(conn);
    conn.execute(
        "UPDATE decks SET account_id = ?2 WHERE id = ?1",
        rusqlite::params![deck_id, arena],
    )
    .expect("move deck");
}

/// What `seed_endstep` stored.
pub struct EndstepSeed {
    pub account_id: i64,
    pub deck_id: i64,
}

/// An Endstep account, opponent, complete match `token` (three games) and a
/// deck, stored through the real write path (`commit`), with the match linked
/// to the deck's version. The deck is `endstep:d1` with cloud sync switched
/// on, so a path that does not filter by platform would pick it up.
pub fn seed_endstep(conn: &mut Connection, token: &str) -> EndstepSeed {
    let mut played = view(token, "endstep:u2", 1_787_133_600_000);
    played.local_username = "endstep:u1".to_string();
    played.mtgo_match_id = None;
    for (index, game) in played.games.iter_mut().enumerate() {
        game.mtgo_game_id = Some(7_001 + index as u64);
    }
    commit(
        conn,
        &WriteBatch {
            platform_accounts: vec![crate::arena::PlatformAccount {
                platform: crate::Platform::Endstep,
                username: "endstep:u1".to_string(),
                user_id: None,
                display_name: Some("Player".to_string()),
                seen_at_ms: 1,
            }],
            platform_opponents: vec![crate::arena::PlatformOpponent {
                platform: crate::Platform::Endstep,
                username: "endstep:u2".to_string(),
                display_name: "Opponent".to_string(),
            }],
            matches: vec![crate::write::MatchWrite::Upsert {
                view: Box::new(played),
                revision: 1,
            }],
            decks: vec![crate::decks::DeckWrite::SyncFile {
                file: decks::DeckFile {
                    name: "Qendstep deck".to_string(),
                    net_deck_id: "endstep:d1".to_string(),
                    format: "CMODERN".to_string(),
                    timestamp_ms: 50_000,
                    cards: vec![decks::DeckCard {
                        catalog_id: 1001,
                        quantity: 4,
                        sideboard: false,
                    }],
                },
                signature: "Q-endstep".to_string(),
                account: Some("endstep:u1".to_string()),
                platform: crate::Platform::Endstep,
                synced_at_ms: 60_000,
                identity: crate::decks::IdentityUpdate::Keep,
            }],
            now_ms: 60_000,
            ..WriteBatch::default()
        },
    )
    .expect("endstep rows");
    let (deck_id, version_id): (i64, i64) = conn
        .query_row(
            "SELECT d.id, v.id FROM decks d JOIN deck_versions v ON v.deck_id = d.id
             WHERE d.mtgo_id = 'endstep:d1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("endstep deck");
    commit(
        conn,
        &WriteBatch {
            decks: vec![crate::decks::DeckWrite::Link {
                match_token: token.to_string(),
                deck_version_id: version_id,
            }],
            now_ms: 60_001,
            ..WriteBatch::default()
        },
    )
    .expect("link");
    conn.execute(
        "UPDATE decks SET cloud_sync_enabled = 1 WHERE id = ?1",
        [deck_id],
    )
    .expect("enable");
    let account_id = conn
        .query_row(
            "SELECT id FROM accounts WHERE platform = 'endstep'",
            [],
            |row| row.get(0),
        )
        .expect("endstep account");
    EndstepSeed {
        account_id,
        deck_id,
    }
}

/// Every row of `table`, each as its columns' debug text, in the order of its
/// first column.
pub fn dump(conn: &Connection, table: &str) -> Vec<Vec<String>> {
    let mut stmt = conn
        .prepare(&format!("SELECT * FROM {table} ORDER BY 1"))
        .expect("prepare");
    let columns = stmt.column_count();
    let rows = stmt
        .query_map([], |row| {
            let mut values = Vec::new();
            for index in 0..columns {
                let value: rusqlite::types::Value = row.get(index)?;
                values.push(format!("{value:?}"));
            }
            Ok(values)
        })
        .expect("query");
    rows.map(|row| row.expect("row")).collect()
}

/// A manual match (Plan L) as the player entered it: Qlocal against
/// Qopponent, a 2-0 win in two games, no MTGO match id, never derived.
pub fn insert_manual(conn: &Connection, token: &str) {
    conn.execute_batch(
        "INSERT INTO accounts (username, first_seen_at) VALUES ('Qlocal', 0)
             ON CONFLICT (username) DO NOTHING;
         INSERT INTO opponents (username, first_seen_at) VALUES ('Qopponent', 0)
             ON CONFLICT (username) DO NOTHING;",
    )
    .expect("registries");
    conn.execute(
        "INSERT INTO matches (token, account_id, opponent_id, source, format, match_type, state,
             outcome, started_at, ended_at, last_activity_at, revision, derived_version)
         VALUES (?1, (SELECT id FROM accounts WHERE username = 'Qlocal'),
             (SELECT id FROM opponents WHERE username = 'Qopponent'), 'manual', 'CMODERN',
             'Constructed', 'complete', 'win', 1000, 5000, 5000, 1, 0)",
        [token],
    )
    .expect("a manual match");
    conn.execute(
        "INSERT INTO games (match_id, game_number, result, end_reason, on_play, local_mulligans,
             opp_mulligans, started_at, ended_at)
         SELECT id, 1, 'win', 'unknown', 1, 0, 0, 1000, 3000 FROM matches WHERE token = ?1
         UNION ALL
         SELECT id, 2, 'win', 'unknown', 0, 0, 0, 3000, 5000 FROM matches WHERE token = ?1",
        [token],
    )
    .expect("its games");
}

/// A manual league row `manual:{id}` (target schema section 8), as the
/// league edit makes one: the deck's format and the version it had at
/// `created_at` (its latest saved by then, else its first).
pub fn insert_manual_league(
    conn: &Connection,
    id: i64,
    account_id: Option<i64>,
    deck_id: Option<i64>,
    name: &str,
    started_at: i64,
    created_at: i64,
) {
    conn.execute(
        "INSERT INTO leagues (id, run_key, account_id, source, kind, state, name, format,
             deck_version_id, deck_id, started_at, created_at, token)
         VALUES (?1, 'manual:' || ?1, ?2, 'manual', 'constructed', 'complete', ?3,
             (SELECT format FROM decks WHERE id = ?4),
             COALESCE(
                 (SELECT id FROM deck_versions WHERE deck_id = ?4 AND modified_at <= ?6
                  ORDER BY modified_at DESC, id DESC LIMIT 1),
                 (SELECT id FROM deck_versions WHERE deck_id = ?4
                  ORDER BY modified_at, id LIMIT 1)),
             ?4, ?5, ?6, 'manual_Q' || printf('%023d', ?1))",
        rusqlite::params![id, account_id, name, deck_id, started_at, created_at],
    )
    .expect("a manual league");
}

/// Link a match to league row `league_id` by hand (a manual league's member,
/// or an imported run's 0.x member).
pub fn link_league_match(conn: &Connection, league_id: i64, token: &str) {
    conn.execute(
        "UPDATE matches SET league_id = ?2, league_linked = 1 WHERE token = ?1",
        rusqlite::params![token, league_id],
    )
    .expect("a linked match");
}

/// A 0.x run as the import stores it (`0x:{legacy_id}`, its seed and the
/// seed's values); returns its row id (`1000 + legacy_id`, clear of the
/// manual leagues' ids).
pub fn insert_imported_run(
    conn: &Connection,
    legacy_id: i64,
    account_id: Option<i64>,
    seed: &crate::league_sync::ImportedSeed,
) -> i64 {
    let id = 1000 + legacy_id;
    conn.execute(
        "INSERT INTO leagues (id, run_key, account_id, source, kind, state, token, event_id,
             mtgo_course_id, name, format, set_code, deck_version_id, deck_id, started_at,
             joined_at, completed_at, dropped_at, created_at, imported_from, imported_seed)
         VALUES (?1, '0x:' || ?2, ?3, 'imported', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
             (SELECT deck_id FROM deck_versions WHERE id = ?12), ?13, ?14, ?15, ?16, 7, ?2, ?17)",
        rusqlite::params![
            id,
            legacy_id,
            account_id,
            seed.kind,
            seed.state,
            seed.token,
            seed.event_id,
            seed.mtgo_course_id,
            seed.name,
            seed.format,
            seed.set_code,
            seed.deck_version_id,
            seed.started_at,
            seed.joined_at,
            seed.completed_at,
            seed.dropped_at,
            serde_json::to_string(seed).expect("seed json")
        ],
    )
    .expect("an imported run");
    id
}

/// Link a match to the imported run of 0.x league `legacy_id`.
pub fn link_imported_match(conn: &Connection, legacy_id: i64, token: &str) {
    link_league_match(conn, 1000 + legacy_id, token);
}

/// Change one field of an imported run's seed (`$.state`, say).
pub fn set_seed(conn: &Connection, legacy_id: i64, path: &str, value: rusqlite::types::Value) {
    conn.execute(
        "UPDATE leagues SET imported_seed = json_set(imported_seed, ?2, ?3)
         WHERE imported_from = ?1",
        rusqlite::params![legacy_id, path, value],
    )
    .expect("seed field");
}

/// Run the league sync for every owner (UTC, `now_ms` 0: no draft is
/// abandoned), as the tracker does after a change.
pub fn sync_leagues(conn: &Connection) {
    let tx = conn.unchecked_transaction().expect("transaction");
    crate::league_sync::sync_every_owner(&tx, chrono_tz::UTC, 0).expect("league sync");
    tx.commit().expect("commit");
}

/// The match `token` of `conn` as another device pulls it: its bundle
/// input read as the bundle reader reads a bundle (cross-device sync spec
/// section 7), without the player's fields.
pub fn pulled_from(conn: &Connection, token: &str) -> crate::import::PulledMatch {
    let input = crate::sync::match_bundle_input(conn, match_id(conn, token), chrono_tz::UTC)
        .expect("input")
        .expect("a match");
    let games = input
        .games
        .iter()
        .map(|game| ImportedGame {
            mtgo_game_id: game.mtgo_game_id,
            game_number: game.game_number,
            result: game.result.clone(),
            on_play: game.on_play,
            local_mulligans: game.local_mulligans,
            opp_mulligans: game.opp_mulligans,
            local_die_roll: game.local_die_roll,
            opp_die_roll: game.opp_die_roll,
            turn_count: game.turn_count,
            started_at_ms: game.started_at_ms,
            ended_at_ms: game.ended_at_ms,
        })
        .collect();
    let deck = match (&input.deck_mtgo_id, &input.deck_version_signature) {
        (Some(deck), Some(signature)) => Some((deck.clone(), signature.clone())),
        _ => None,
    };
    crate::import::PulledMatch {
        platform: input.platform,
        sources: input.sources.clone(),
        local_player_fields: input.local_player_fields.clone(),
        opponent_player_fields: input.opponent_player_fields.clone(),
        endstep_ranked: input.endstep_ranked.clone(),
        endstep_manual_cards: Vec::new(),
        game_structure: input.game_structure.clone(),
        imported: ImportedMatch {
            token: input.token.clone(),
            mtgo_match_id: input.mtgo_match_id,
            local_username: input.local_username.clone(),
            opponent: input.opponent_username.clone(),
            format: input.format.clone(),
            match_type: input.match_type.clone(),
            tournament_event_id: input.tournament_event_id,
            tournament_round: input.tournament_round,
            state: input.state.clone(),
            outcome: input.outcome.clone(),
            started_at_ms: input.started_at_ms,
            ended_at_ms: input.ended_at_ms,
            note: input.notes.clone(),
            games,
            manual: None,
            games_won_imported: input.games_won_imported,
            games_lost_imported: input.games_lost_imported,
            deck_linked: false,
            replay_share: None,
            legacy_history: false,
        },
        detail: crate::imported_detail::ImportedDetail::default(),
        deck,
        league_client_id: input.league_client_id.clone(),
        history: false,
        archetypes: input.archetypes.clone(),
        player: sync_bundles::PulledPlayerFields::default(),
    }
}

/// Apply a pulled match as the sync runner does.
pub fn pull_into(conn: &mut Connection, pulled: crate::import::PulledMatch, now_ms: i64) {
    let edit = crate::sync::SyncEdit::PulledMatch {
        client_id: pulled.imported.token.clone(),
        pulled: Box::new(pulled),
        hash: "h-pulled".to_string(),
        carry: "{}".to_string(),
        timezone: chrono_tz::UTC,
        now_ms,
    };
    crate::sync::edit_sync(conn, &edit).expect("pulled match");
}

/// The match `token`'s kind (`game_structure`), every `game_cards` and
/// `game_mulligan_cards` row of it and its games' columns, by game number, each card named by its Endstep
/// ref when it has one (Endstep catalog ids differ between devices).
pub fn pulled_rows(conn: &Connection, token: &str) -> Vec<String> {
    let card_columns: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info('game_cards') ORDER BY cid")
            .expect("columns");
        let names = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query");
        names
            .map(|name| name.expect("name"))
            .filter(|name| name != "game_id" && name != "catalog_id" && name != "id")
            .collect()
    };
    let card_name = "COALESCE(e.name || '|' || e.set_code || '|' || e.collector_number,
                              CAST(c.catalog_id AS TEXT))";
    let mut queries = vec![
        "SELECT 'match ' || quote(m.game_structure) FROM matches m WHERE m.token = ?1".to_string(),
        "SELECT 'game ' || g.game_number || ':' || quote(g.on_play) || ':'
             || quote(g.local_mulligans) || ':' || quote(g.opp_mulligans) || ':'
             || quote(g.turn_count) || ':' || quote(g.local_opening_hand_size) || ':'
             || quote(g.local_starting_hand_size) || ':' || quote(g.opp_opening_hand_size)
             || ':' || quote(g.opp_starting_hand_size) || ':' || quote(g.result)
         FROM games g JOIN matches m ON m.id = g.match_id WHERE m.token = ?1"
            .to_string(),
    ];
    let columns: Vec<String> = card_columns
        .iter()
        .map(|name| format!("quote(c.{name})"))
        .collect();
    queries.push(format!(
        "SELECT 'card ' || g.game_number || ':' || {card_name} || ':' || {}
         FROM game_cards c JOIN games g ON g.id = c.game_id
         JOIN matches m ON m.id = g.match_id
         LEFT JOIN endstep_cards e ON {} + e.id = c.catalog_id
         WHERE m.token = ?1",
        columns.join(" || ':' || "),
        cards::ENDSTEP_ID_OFFSET
    ));
    queries.push(format!(
        "SELECT 'mulligan ' || g.game_number || ':' || c.hand || ':' || {card_name} || ':'
             || c.quantity
         FROM game_mulligan_cards c JOIN games g ON g.id = c.game_id
         JOIN matches m ON m.id = g.match_id
         LEFT JOIN endstep_cards e ON {} + e.id = c.catalog_id
         WHERE m.token = ?1",
        cards::ENDSTEP_ID_OFFSET
    ));
    queries.push(
        "SELECT 'commander ' || c.seat || ':' || c.position || ':' || c.catalog_id
         FROM match_commanders c JOIN matches m ON m.id = c.match_id WHERE m.token = ?1"
            .to_string(),
    );
    let mut rows = Vec::new();
    for sql in queries {
        let mut stmt = conn.prepare(&sql).expect("prepare");
        let found = stmt
            .query_map([token], |row| row.get::<_, String>(0))
            .expect("query");
        for row in found {
            rows.push(row.expect("row"));
        }
    }
    rows.sort();
    rows
}
