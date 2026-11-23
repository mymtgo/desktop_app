//! Reading 0.x's API state (Plan Q task 12, 07 spec section 11) from a 0.x
//! database built from `fixtures/0x/schema.sql` and fake rows. Fake names
//! start with Q.

use importer::api_read::read_api_state;
use mymtgo_api::SyncType;
use rusqlite::Connection;
use store::import_api::{
    LegacyCardStats, LegacyRejection, LegacySubmitted, LegacySyncState, LegacySyncedRow,
    LegacyTournament,
};

const SCHEMA: &str = include_str!("../../../fixtures/0x/schema.sql");

fn legacy() -> Connection {
    let conn = Connection::open_in_memory().expect("memory");
    conn.execute_batch(SCHEMA).expect("0.x schema");
    conn.execute_batch(
        "INSERT INTO matches (id, token, mtgo_id, format, match_type, started_at, state,
             submitted_at, synced_hash, synced_at, tournament_token)
         VALUES (1, 'q-sent', '285746550', 'CMODERN', 'Constructed', '2026-08-19 10:00:00',
             'complete', '2026-08-19 10:30:00', 'h-match', '2026-08-19 11:00:00', 'q-tourney'),
             (2, 'q-open', '285746551', 'CMODERN', 'Constructed', '2026-08-19 12:00:00',
             'complete', NULL, NULL, NULL, NULL);
         INSERT INTO games (id, match_id, mtgo_id, started_at) VALUES
             (10, 1, '945500844', '2026-08-19 10:00:00');
         INSERT INTO card_stat_ship_queue (id, game_id, match_id, payload, status, attempts,
             next_attempt_at, last_error, created_at, updated_at)
         VALUES (1, 10, 1, '{\"oracle_id\":\"q\"}', 'sending', 2, '2026-08-19 10:40:00',
             'HTTP 503', '2026-08-19 10:31:00', '2026-08-19 10:35:00');
         INSERT INTO decks (id, mtgo_id, name, format, synced_hash, synced_at,
             cloud_sync_enabled)
         VALUES (1, 'limited:q 1', 'Qdeck', 'CMODERN', 'h-deck', '2026-08-19 11:01:00', 1),
             (2, '777', 'Qother', 'CPAUPER', NULL, NULL, 0);
         INSERT INTO leagues (id, token, name, format, started_at, synced_hash, synced_at)
         VALUES (1, 'q-run', 'Qleague', 'CMODERN', '2026-08-19 10:00:05', 'h-league',
             '2026-08-19 11:02:00');
         INSERT INTO sync_state (id, type, last_synced_at, canonical_version, full_cursor)
         VALUES (1, 'deck', '2026-08-19 11:03:00', 4, NULL),
             (2, 'match', '2026-08-19 11:04:00', 4, 17);
         INSERT INTO sync_rejections (id, type, client_id, reason, hash)
         VALUES (1, 'league', 'q-run_20260819T100005Z', 'deck_not_synced', 'h-run');
         INSERT INTO tournaments (id, mtgo_event_id, token, name, format, started_at,
             name_synthesized)
         VALUES (1, 77, 'q-tourney', 'Qlegacy Challenge 32', 'CLEGACY', '2026-08-19 09:00:00',
             0),
             (2, 78, NULL, 'Q synthesized', 'CLEGACY', '2026-08-19 09:00:00', 1);",
    )
    .expect("rows");
    conn
}

fn ms(text: &str) -> i64 {
    chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S")
        .expect("time")
        .and_utc()
        .timestamp_millis()
}

#[test]
fn reads_every_part_of_the_0x_api_state() {
    let state = read_api_state(&legacy()).expect("read");
    assert_eq!(
        state.submitted,
        vec![LegacySubmitted {
            token: "q-sent".to_string(),
            mtgo_match_id: Some(285_746_550),
            submitted_at_ms: ms("2026-08-19 10:30:00"),
        }]
    );
    assert_eq!(
        state.card_stats,
        vec![LegacyCardStats {
            match_token: "q-sent".to_string(),
            mtgo_match_id: Some(285_746_550),
            game_mtgo_id: Some(945_500_844),
            payload: r#"{"oracle_id":"q"}"#.to_string(),
            status: "sending".to_string(),
            attempts: 2,
            next_attempt_at_ms: Some(ms("2026-08-19 10:40:00")),
            last_error: Some("HTTP 503".to_string()),
            created_at_ms: Some(ms("2026-08-19 10:31:00")),
            updated_at_ms: Some(ms("2026-08-19 10:35:00")),
        }]
    );
    assert_eq!(
        state.synced,
        vec![
            LegacySyncedRow {
                sync_type: SyncType::Match,
                client_id: "q-sent".to_string(),
                hash: "h-match".to_string(),
                synced_at_ms: Some(ms("2026-08-19 11:00:00")),
            },
            LegacySyncedRow {
                sync_type: SyncType::Deck,
                client_id: "limited_q_1".to_string(),
                hash: "h-deck".to_string(),
                synced_at_ms: Some(ms("2026-08-19 11:01:00")),
            },
            LegacySyncedRow {
                sync_type: SyncType::League,
                client_id: "q-run_20260819T100005Z".to_string(),
                hash: "h-league".to_string(),
                synced_at_ms: Some(ms("2026-08-19 11:02:00")),
            },
        ]
    );
    assert_eq!(
        state.deck_flags,
        vec![
            ("limited:q 1".to_string(), true),
            ("777".to_string(), false)
        ]
    );
    assert_eq!(
        state.sync_states,
        vec![
            LegacySyncState {
                sync_type: SyncType::Deck,
                last_synced_at_ms: Some(ms("2026-08-19 11:03:00")),
                canonical_version: 4,
                interrupted: false,
            },
            LegacySyncState {
                sync_type: SyncType::Match,
                last_synced_at_ms: Some(ms("2026-08-19 11:04:00")),
                canonical_version: 4,
                interrupted: true,
            },
        ]
    );
    assert_eq!(
        state.rejections,
        vec![LegacyRejection {
            sync_type: SyncType::League,
            client_id: "q-run_20260819T100005Z".to_string(),
            reason: "deck_not_synced".to_string(),
            hash: "h-run".to_string(),
        }]
    );
    assert_eq!(
        state.tournaments,
        vec![LegacyTournament {
            mtgo_event_id: 77,
            token: Some("q-tourney".to_string()),
            name: "Qlegacy Challenge 32".to_string(),
            format: Some("CLEGACY".to_string()),
            started_at_ms: ms("2026-08-19 09:00:00"),
        }],
        "only real names"
    );
    assert_eq!(
        state.tournament_tokens,
        vec![("q-sent".to_string(), "q-tourney".to_string())]
    );
}

#[test]
fn an_old_0x_without_the_api_tables_reads_as_nothing() {
    let conn = Connection::open_in_memory().expect("memory");
    conn.execute_batch(
        "CREATE TABLE matches (id integer primary key, token varchar, mtgo_id varchar);
         CREATE TABLE decks (id integer primary key, mtgo_id varchar);",
    )
    .expect("old tables");
    let state = read_api_state(&conn).expect("read");
    assert_eq!(state, store::import_api::ImportedApiState::default());
}
