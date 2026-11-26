//! 0.x's API state imported into v1 (07 spec section 11), so nothing 0.x
//! sent is sent again and the sync picks up where 0.x left off:
//! - `matches.submitted_at` becomes a `sent` `match_report` outbox row (a v1
//!   row in any other state becomes `sent`; a `sent` row never changes);
//! - `card_stat_ship_queue` rows keep their status and payload under the
//!   subject `"{match token}:{mtgo game id}"` (0.x `sending` becomes
//!   `pending`); a row whose match v1 does not have is skipped;
//! - `synced_hash` and `synced_at` of matches, decks and leagues become
//!   `synced_rows` (no carry: the first rebuild fetches it); a manual match
//!   or league is marked the server's copy (`sync::mark_server_copy`), so it
//!   is pushed only once edited here;
//! - `decks.cloud_sync_enabled` is copied (the next manifest corrects it);
//! - `sync_state` and `sync_rejections` carry over; a type 0.x left in the
//!   middle of a full run (a `full_cursor`, a 0.x row id) gets version 0, so
//!   v1 runs it in full;
//! - a real tournament name (`name_synthesized = 0`) names v1's row (or a new
//!   one with 0.x's format and start); `matches.tournament_token` is copied.
//!
//! Every step only adds or fills, so a rerun adds and changes nothing. 0.x's
//! `tournament_observation_queue` is not imported (spec section 11).

use mymtgo_api::SyncType;
use rusqlite::{OptionalExtension, Transaction, params};

use crate::outbox::{OutboxKind, record_sent};
use crate::{Connection, StoreError};

/// A match 0.x reported (`matches.submitted_at`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacySubmitted {
    pub token: String,
    pub mtgo_match_id: Option<i64>,
    pub submitted_at_ms: i64,
}

/// One 0.x `card_stat_ship_queue` row, with its game's match token and MTGO
/// game id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyCardStats {
    pub match_token: String,
    pub mtgo_match_id: Option<i64>,
    pub game_mtgo_id: Option<i64>,
    /// The game's JSON as 0.x queued it.
    pub payload: String,
    /// `pending`, `sending`, `sent`, `failed` or `skipped`.
    pub status: String,
    pub attempts: i64,
    pub next_attempt_at_ms: Option<i64>,
    pub last_error: Option<String>,
    pub created_at_ms: Option<i64>,
    pub updated_at_ms: Option<i64>,
}

/// One synced row (0.x `synced_hash`, `synced_at`), under its v1 client id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacySyncedRow {
    pub sync_type: SyncType,
    pub client_id: String,
    pub hash: String,
    pub synced_at_ms: Option<i64>,
}

/// One 0.x `sync_state` row. `interrupted`: 0.x left a `full_cursor`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacySyncState {
    pub sync_type: SyncType,
    pub last_synced_at_ms: Option<i64>,
    pub canonical_version: i64,
    pub interrupted: bool,
}

/// One 0.x `sync_rejections` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyRejection {
    pub sync_type: SyncType,
    pub client_id: String,
    pub reason: String,
    pub hash: String,
}

/// A 0.x tournament with its real name (`name_synthesized = 0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyTournament {
    pub mtgo_event_id: i64,
    pub token: Option<String>,
    pub name: String,
    pub format: Option<String>,
    pub started_at_ms: i64,
}

/// Everything 0.x knew about the API.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportedApiState {
    pub submitted: Vec<LegacySubmitted>,
    pub card_stats: Vec<LegacyCardStats>,
    pub synced: Vec<LegacySyncedRow>,
    /// `(deck mtgo id, cloud_sync_enabled)`.
    pub deck_flags: Vec<(String, bool)>,
    pub sync_states: Vec<LegacySyncState>,
    pub rejections: Vec<LegacyRejection>,
    pub tournaments: Vec<LegacyTournament>,
    /// `(match token, tournament token)`.
    pub tournament_tokens: Vec<(String, String)>,
}

/// What one API state import changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ApiImportCounts {
    /// Match reports recorded as sent.
    pub reports_sent: u32,
    /// Card stats rows added.
    pub card_stats_added: u32,
    /// Card stats rows of a game v1 has no match (or game id) for.
    pub card_stats_skipped: u32,
    /// Synced rows recorded.
    pub synced_rows: u32,
    /// Deck sync flags set.
    pub deck_flags: u32,
    pub sync_states: u32,
    pub rejections: u32,
    /// Tournaments given their real name.
    pub tournaments_named: u32,
    /// Matches given their tournament token.
    pub tournament_tokens: u32,
}

/// Import 0.x's API state in one transaction (07 spec section 11).
pub fn import_api_state(
    conn: &mut Connection,
    state: &ImportedApiState,
    now_ms: i64,
) -> Result<ApiImportCounts, StoreError> {
    let tx = conn.transaction()?;
    let mut counts = ApiImportCounts::default();
    for submitted in &state.submitted {
        for token in tokens_of(&tx, &submitted.token, submitted.mtgo_match_id)? {
            if record_sent(
                &tx,
                OutboxKind::MatchReport,
                &token,
                submitted.submitted_at_ms,
            )? {
                counts.reports_sent += 1;
            }
        }
    }
    for row in &state.card_stats {
        import_card_stats(&tx, row, now_ms, &mut counts)?;
    }
    for row in &state.synced {
        let filled = tx
            .prepare_cached(
                "INSERT INTO synced_rows (type, client_id, hash, synced_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (type, client_id) DO UPDATE SET hash = excluded.hash,
                     synced_at = excluded.synced_at
                 WHERE synced_rows.hash IS NULL",
            )?
            .execute(params![
                row.sync_type.as_str(),
                row.client_id,
                row.hash,
                row.synced_at_ms
            ])? as u32;
        if filled > 0 {
            // A manual match or league 0.x synced is the server's copy until
            // it is edited here (0.x built it with parts v1 does not keep).
            crate::sync::mark_server_copy(&tx, row.sync_type, &row.client_id)?;
        }
        counts.synced_rows += filled;
    }
    // Once v1 has a deck sync state (from this import or its own deck
    // manifest), the manifest owns the flags: a rerun leaves them (#264).
    let deck_state: bool = tx
        .prepare_cached("SELECT EXISTS (SELECT 1 FROM sync_state WHERE type = ?1)")?
        .query_row([SyncType::Deck.as_str()], |row| row.get(0))?;
    let deck_flags: &[(String, bool)] = if deck_state { &[] } else { &state.deck_flags };
    for (mtgo_id, enabled) in deck_flags {
        counts.deck_flags += tx
            .prepare_cached(
                "UPDATE decks SET cloud_sync_enabled = ?2
                 WHERE mtgo_id = ?1 AND cloud_sync_enabled <> ?2",
            )?
            .execute(params![mtgo_id, enabled])? as u32;
    }
    for row in &state.sync_states {
        // A 0.x cursor is a 0.x row id v1 cannot resume from: run in full.
        let version = if row.interrupted {
            0
        } else {
            row.canonical_version
        };
        counts.sync_states += tx
            .prepare_cached(
                "INSERT INTO sync_state (type, last_synced_at, canonical_version, full_cursor)
                 VALUES (?1, ?2, ?3, NULL)
                 ON CONFLICT (type) DO NOTHING",
            )?
            .execute(params![
                row.sync_type.as_str(),
                row.last_synced_at_ms,
                version
            ])? as u32;
    }
    for row in &state.rejections {
        counts.rejections += tx
            .prepare_cached(
                "INSERT INTO sync_rejections (type, client_id, reason, hash) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (type, client_id) DO NOTHING",
            )?
            .execute(params![
                row.sync_type.as_str(),
                row.client_id,
                row.reason,
                row.hash
            ])? as u32;
    }
    for tournament in &state.tournaments {
        counts.tournaments_named += name_tournament(&tx, tournament, now_ms)?;
    }
    for (match_token, tournament_token) in &state.tournament_tokens {
        counts.tournament_tokens +=
            tx.prepare_cached(
                "UPDATE matches SET tournament_token = ?2
                 WHERE token = ?1 AND source <> 'log' AND tournament_token IS NULL",
            )?
            .execute(params![match_token, tournament_token])? as u32;
    }
    tx.commit()?;
    Ok(counts)
}

/// The v1 tokens a 0.x match goes by: its own, and the live row's when v1
/// tracked it live under another token (by MTGO match id).
fn tokens_of(
    tx: &Transaction,
    token: &str,
    mtgo_match_id: Option<i64>,
) -> Result<Vec<String>, StoreError> {
    let mut tokens = vec![token.to_string()];
    if let Some(live) = crate::import::live_row_token(tx, token, mtgo_match_id)?
        && live != token
    {
        tokens.push(live);
    }
    Ok(tokens)
}

/// One card stats row: kept with its status and payload when v1 has its
/// match (by token, else the live row's); a row 0.x sent is recorded as
/// sent on a row v1 queued meanwhile.
fn import_card_stats(
    tx: &Transaction,
    row: &LegacyCardStats,
    now_ms: i64,
    counts: &mut ApiImportCounts,
) -> Result<(), StoreError> {
    let token = match crate::import::live_row_token(tx, &row.match_token, row.mtgo_match_id)? {
        Some(live) => Some(live),
        None => tx
            .prepare_cached("SELECT token FROM matches WHERE token = ?1")?
            .query_row([&row.match_token], |found| found.get::<_, String>(0))
            .optional()?,
    };
    let (Some(token), Some(game)) = (token, row.game_mtgo_id) else {
        counts.card_stats_skipped += 1;
        return Ok(());
    };
    let subject = format!("{token}:{game}");
    let created_at = row.created_at_ms.unwrap_or(now_ms);
    let status = match row.status.as_str() {
        "sent" | "failed" | "skipped" => row.status.as_str(),
        _ => "pending",
    };
    if status == "sent" {
        let sent_at = row.updated_at_ms.unwrap_or(created_at);
        let exists: bool = tx
            .prepare_cached(
                "SELECT EXISTS (SELECT 1 FROM outbox WHERE kind = 'card_stats' AND subject = ?1)",
            )?
            .query_row([&subject], |found| found.get(0))?;
        if record_sent(tx, OutboxKind::CardStats, &subject, sent_at)? && !exists {
            counts.card_stats_added += 1;
        }
        return Ok(());
    }
    let payload = if status == "skipped" {
        Vec::new()
    } else {
        zstd::encode_all(row.payload.as_bytes(), 0)?
    };
    counts.card_stats_added += tx
        .prepare_cached(
            "INSERT INTO outbox (kind, subject, payload, status, attempts, next_attempt_at,
                 last_error, created_at)
             VALUES ('card_stats', ?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (kind, subject) DO NOTHING",
        )?
        .execute(params![
            subject,
            payload,
            status,
            row.attempts,
            row.next_attempt_at_ms,
            row.last_error,
            created_at
        ])? as u32;
    Ok(())
}

/// A real 0.x tournament name: v1's row by MTGO event id takes it while its
/// name is synthesized; without a row, one is made with 0.x's format and
/// start (the league sync then owns those). The API need not be asked.
fn name_tournament(
    tx: &Transaction,
    tournament: &LegacyTournament,
    now_ms: i64,
) -> Result<u32, StoreError> {
    let named = tx
        .prepare_cached(
            "UPDATE tournaments SET name = ?2, name_synthesized = 0, name_checked_at = ?3
             WHERE mtgo_event_id = ?1 AND name_synthesized = 1",
        )?
        .execute(params![tournament.mtgo_event_id, tournament.name, now_ms])?;
    if named > 0 {
        return Ok(named as u32);
    }
    let added = tx
        .prepare_cached(
            "INSERT INTO tournaments (mtgo_event_id, token, name, format, started_at,
                 name_synthesized, name_checked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)
             ON CONFLICT (mtgo_event_id) DO NOTHING",
        )?
        .execute(params![
            tournament.mtgo_event_id,
            tournament.token,
            tournament.name,
            tournament.format,
            tournament.started_at_ms,
            now_ms
        ])?;
    Ok(added as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outbox::{NewOutboxRow, OutboxEdit, OutboxKind, edit_outbox};
    use crate::test_support::{db, imported, insert_manual};
    use fixture_tools::TestDir;

    const NOW: i64 = 1_800_000_000_000;

    fn report(conn: &Connection, subject: &str) -> Option<(String, Option<i64>, Vec<u8>)> {
        conn.query_row(
            "SELECT status, sent_at, payload FROM outbox
             WHERE kind = 'match_report' AND subject = ?1",
            [subject],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .expect("outbox")
    }

    fn enqueue_report(conn: &mut Connection, subject: &str) {
        edit_outbox(
            conn,
            &OutboxEdit::Enqueue {
                rows: vec![NewOutboxRow {
                    kind: OutboxKind::MatchReport,
                    subject: subject.to_string(),
                    payload: Some(b"q-payload".to_vec()),
                    now_ms: 1,
                }],
            },
        )
        .expect("queued");
    }

    fn submitted(token: &str, at: i64) -> ImportedApiState {
        ImportedApiState {
            submitted: vec![LegacySubmitted {
                token: token.to_string(),
                mtgo_match_id: None,
                submitted_at_ms: at,
            }],
            ..ImportedApiState::default()
        }
    }

    fn full_state() -> ImportedApiState {
        ImportedApiState {
            submitted: vec![LegacySubmitted {
                token: "q-sent".to_string(),
                mtgo_match_id: Some(9),
                submitted_at_ms: 1_000,
            }],
            card_stats: vec![
                LegacyCardStats {
                    match_token: "q-sent".to_string(),
                    mtgo_match_id: Some(9),
                    game_mtgo_id: Some(7),
                    payload: r#"{"oracle_id":"q"}"#.to_string(),
                    status: "sending".to_string(),
                    attempts: 2,
                    next_attempt_at_ms: Some(5),
                    last_error: Some("HTTP 503".to_string()),
                    created_at_ms: Some(10),
                    updated_at_ms: Some(20),
                },
                LegacyCardStats {
                    match_token: "q-sent".to_string(),
                    mtgo_match_id: Some(9),
                    game_mtgo_id: Some(8),
                    payload: r#"{"oracle_id":"r"}"#.to_string(),
                    status: "sent".to_string(),
                    attempts: 1,
                    next_attempt_at_ms: None,
                    last_error: None,
                    created_at_ms: Some(10),
                    updated_at_ms: Some(30),
                },
                LegacyCardStats {
                    match_token: "q-gone".to_string(),
                    mtgo_match_id: None,
                    game_mtgo_id: Some(1),
                    payload: "{}".to_string(),
                    status: "pending".to_string(),
                    attempts: 0,
                    next_attempt_at_ms: None,
                    last_error: None,
                    created_at_ms: None,
                    updated_at_ms: None,
                },
            ],
            synced: vec![
                LegacySyncedRow {
                    sync_type: SyncType::Match,
                    client_id: "q-sent".to_string(),
                    hash: "h-match".to_string(),
                    synced_at_ms: Some(40),
                },
                LegacySyncedRow {
                    sync_type: SyncType::Deck,
                    client_id: "limited_q".to_string(),
                    hash: "h-deck".to_string(),
                    synced_at_ms: Some(41),
                },
            ],
            deck_flags: vec![("111".to_string(), true)],
            sync_states: vec![
                LegacySyncState {
                    sync_type: SyncType::Deck,
                    last_synced_at_ms: Some(50),
                    canonical_version: 4,
                    interrupted: false,
                },
                LegacySyncState {
                    sync_type: SyncType::Match,
                    last_synced_at_ms: Some(51),
                    canonical_version: 4,
                    interrupted: true,
                },
            ],
            rejections: vec![LegacyRejection {
                sync_type: SyncType::League,
                client_id: "q-run_20260819T100005Z".to_string(),
                reason: "deck_not_synced".to_string(),
                hash: "h-run".to_string(),
            }],
            tournaments: vec![LegacyTournament {
                mtgo_event_id: 77,
                token: Some("q-tournament".to_string()),
                name: "Qlegacy Challenge 32".to_string(),
                format: Some("CLEGACY".to_string()),
                started_at_ms: 60,
            }],
            tournament_tokens: vec![("q-sent".to_string(), "q-tournament".to_string())],
        }
    }

    fn seeded(label: &str) -> (TestDir, Connection) {
        let (dir, mut conn) = db(label);
        crate::import::import_matches(&mut conn, &[imported("q-sent", Some(9))], 1)
            .expect("the 0.x match");
        conn.execute(
            "INSERT INTO decks (mtgo_id, name, format, created_at, updated_at, source)
             VALUES ('111', 'Qdeck', 'CMODERN', 1, 1, 'file')",
            [],
        )
        .expect("deck");
        (dir, conn)
    }

    fn table(conn: &Connection, sql: &str) -> Vec<String> {
        let mut stmt = conn.prepare(sql).expect("prepare");
        let columns = stmt.column_count();
        stmt.query_map([], |row| {
            let mut values = Vec::new();
            for index in 0..columns {
                let value: rusqlite::types::Value = row.get(index)?;
                values.push(format!("{value:?}"));
            }
            Ok(values.join(" | "))
        })
        .expect("query")
        .map(|row| row.expect("row"))
        .collect()
    }

    // Review Focus 3: a match 0.x already reported is never reported by v1,
    // whether v1 had queued it before the import or not.
    #[test]
    fn a_0x_submitted_match_ends_sent_and_never_ships() {
        let (_dir, mut conn) = db("api-import-submitted");
        // Imported before v1 queued it.
        import_api_state(&mut conn, &submitted("q-before", 1_000), NOW).expect("import");
        assert_eq!(
            report(&conn, "q-before"),
            Some(("sent".to_string(), Some(1_000), Vec::new()))
        );
        enqueue_report(&mut conn, "q-before");
        assert_eq!(report(&conn, "q-before").expect("row").0, "sent");
        // Imported after v1 queued it.
        enqueue_report(&mut conn, "q-after");
        import_api_state(&mut conn, &submitted("q-after", 2_000), NOW).expect("import");
        assert_eq!(
            report(&conn, "q-after"),
            Some(("sent".to_string(), Some(2_000), Vec::new()))
        );
        // Nothing is left to claim.
        let claimed = edit_outbox(
            &mut conn,
            &OutboxEdit::Claim {
                kind: OutboxKind::MatchReport,
                now_ms: NOW,
                limit: 200,
            },
        )
        .expect("claim");
        assert_eq!(claimed, crate::outbox::OutboxOutcome::Claimed(Vec::new()));
    }

    /// Issue #264 (Q12 M1): once v1 has a deck `sync_state` row, the deck
    /// manifest owns `cloud_sync_enabled`, so a rerun leaves the flag alone.
    #[test]
    fn a_rerun_keeps_the_cloud_sync_flag_v1_set() {
        let (_dir, mut conn) = seeded("api-import-deck-flag-rerun");
        let state = full_state();
        let first = import_api_state(&mut conn, &state, NOW).expect("import");
        assert_eq!(first.deck_flags, 1, "the first import takes 0.x's flag");
        conn.execute("UPDATE decks SET cloud_sync_enabled = 0", [])
            .expect("turned off in v1");
        let again = import_api_state(&mut conn, &state, NOW).expect("rerun");
        assert_eq!(again.deck_flags, 0);
        assert_eq!(
            table(&conn, "SELECT cloud_sync_enabled FROM decks"),
            vec!["Integer(0)"]
        );
    }

    #[test]
    fn a_match_v1_sent_is_untouched() {
        let (_dir, mut conn) = db("api-import-v1-sent");
        enqueue_report(&mut conn, "q-v1");
        edit_outbox(
            &mut conn,
            &OutboxEdit::Claim {
                kind: OutboxKind::MatchReport,
                now_ms: 5,
                limit: 1,
            },
        )
        .expect("claim");
        edit_outbox(
            &mut conn,
            &OutboxEdit::Sent {
                kind: OutboxKind::MatchReport,
                subjects: vec!["q-v1".to_string()],
                now_ms: 7,
                linked: false,
            },
        )
        .expect("sent");
        import_api_state(&mut conn, &submitted("q-v1", 1_000), NOW).expect("import");
        assert_eq!(
            report(&conn, "q-v1"),
            Some(("sent".to_string(), Some(7), Vec::new()))
        );
    }

    #[test]
    fn the_api_import_is_rerun_safe() {
        let (_dir, mut conn) = seeded("api-import-rerun");
        let first = import_api_state(&mut conn, &full_state(), NOW).expect("first");
        assert!(first.reports_sent > 0);
        let tables = |conn: &Connection| {
            [
                table(conn, "SELECT * FROM outbox ORDER BY id"),
                table(conn, "SELECT * FROM synced_rows ORDER BY type, client_id"),
                table(conn, "SELECT * FROM sync_state ORDER BY type"),
                table(
                    conn,
                    "SELECT * FROM sync_rejections ORDER BY type, client_id",
                ),
                table(conn, "SELECT * FROM tournaments ORDER BY id"),
                table(
                    conn,
                    "SELECT token, tournament_token FROM matches ORDER BY id",
                ),
                table(
                    conn,
                    "SELECT mtgo_id, cloud_sync_enabled, updated_at FROM decks",
                ),
            ]
        };
        let before = tables(&conn);
        let second = import_api_state(&mut conn, &full_state(), NOW + 1).expect("second");
        assert_eq!(
            second,
            ApiImportCounts {
                card_stats_skipped: 1,
                ..ApiImportCounts::default()
            }
        );
        assert_eq!(
            tables(&conn),
            before,
            "a second run adds and changes nothing"
        );
    }

    #[test]
    fn card_stats_keep_their_status_and_payload_and_a_missing_match_is_skipped() {
        // subject, status, attempts, next_attempt_at, last_error, sent_at, payload
        type OutboxRow = (
            String,
            String,
            i64,
            Option<i64>,
            Option<String>,
            Option<i64>,
            Vec<u8>,
        );
        let (_dir, mut conn) = seeded("api-import-card-stats");
        let counts = import_api_state(&mut conn, &full_state(), NOW).expect("import");
        assert_eq!((counts.card_stats_added, counts.card_stats_skipped), (2, 1));
        let rows: Vec<OutboxRow> = conn
            .prepare(
                "SELECT subject, status, attempts, next_attempt_at, last_error, sent_at, payload
                 FROM outbox WHERE kind = 'card_stats' ORDER BY subject",
            )
            .expect("prepare")
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            })
            .expect("query")
            .map(|row| row.expect("row"))
            .collect();
        assert_eq!(rows.len(), 2);
        let (subject, status, attempts, next, error, sent_at, payload) = rows[0].clone();
        assert_eq!(subject, "q-sent:7");
        assert_eq!(status, "pending", "0.x sending becomes pending");
        assert_eq!(
            (attempts, next, error),
            (2, Some(5), Some("HTTP 503".to_string()))
        );
        assert_eq!(sent_at, None);
        let json = zstd::decode_all(payload.as_slice()).expect("zstd");
        assert_eq!(json, br#"{"oracle_id":"q"}"#.to_vec());
        let (subject, status, _, _, _, sent_at, payload) = rows[1].clone();
        assert_eq!(subject, "q-sent:8");
        assert_eq!((status.as_str(), sent_at), ("sent", Some(30)));
        assert!(payload.is_empty());
    }

    // Review fix of Plan Q task 11: a manual match or league 0.x synced is
    // the server's copy (0.x built it, with parts v1 does not keep), so v1
    // pushes it only once it is edited here.
    #[test]
    fn a_manual_match_and_league_0x_synced_are_the_servers_copy() {
        let (_dir, mut conn) = seeded("api-import-manual-copy");
        conn.execute("UPDATE decks SET cloud_sync_enabled = 1", [])
            .expect("enabled");
        conn.execute(
            "INSERT INTO deck_versions (deck_id, signature, modified_at)
             VALUES ((SELECT id FROM decks), 'Qsig', 1)",
            [],
        )
        .expect("version");
        insert_manual(&conn, "q-hand");
        crate::test_support::insert_manual_league(&conn, 3, None, Some(1), "Qmine", NOW, NOW);
        conn.execute(
            "UPDATE matches SET deck_version_id = (SELECT id FROM deck_versions)
             WHERE token IN ('q-hand', 'q-sent')",
            [],
        )
        .expect("played with");
        let token: String = conn
            .query_row("SELECT token FROM leagues", [], |row| row.get(0))
            .expect("token");
        let league_id = sync_bundles::league_client_id(&token, NOW);
        let row = |sync_type: SyncType, client_id: &str| LegacySyncedRow {
            sync_type,
            client_id: client_id.to_string(),
            hash: "h-0x".to_string(),
            synced_at_ms: Some(40),
        };
        let state = ImportedApiState {
            synced: vec![
                row(SyncType::Match, "q-hand"),
                row(SyncType::Match, "q-sent"),
                row(SyncType::League, &league_id),
            ],
            ..ImportedApiState::default()
        };
        import_api_state(&mut conn, &state, NOW).expect("import");
        let owned = |conn: &Connection, sync_type: SyncType, client_id: &str| {
            let candidates = if sync_type == SyncType::Match {
                crate::sync::match_candidates(conn, mymtgo_api::Tier::Supporter, i64::MAX)
            } else {
                crate::sync::league_candidates(conn, mymtgo_api::Tier::Supporter)
            };
            candidates
                .expect("candidates")
                .into_iter()
                .find(|candidate| candidate.client_id == client_id)
                .expect("a candidate")
                .owned
        };
        assert!(!owned(&conn, SyncType::Match, "q-hand"));
        assert!(!owned(&conn, SyncType::League, &league_id));
        conn.execute("UPDATE leagues SET notes = 'Qedited here'", [])
            .expect("edit");
        conn.execute(
            "UPDATE matches SET notes = 'Qedited here' WHERE token = 'q-hand'",
            [],
        )
        .expect("edit");
        // A rerun never takes an edit back.
        import_api_state(&mut conn, &state, NOW).expect("rerun");
        assert!(owned(&conn, SyncType::Match, "q-hand"));
        assert!(owned(&conn, SyncType::League, &league_id));
    }

    #[test]
    fn synced_rows_and_state_round_trip_and_an_interrupted_full_run_forces_full_mode() {
        let (_dir, mut conn) = seeded("api-import-sync-state");
        import_api_state(&mut conn, &full_state(), NOW).expect("import");
        let rows = crate::sync::synced_rows(&conn, SyncType::Match).expect("rows");
        let row = &rows["q-sent"];
        assert_eq!(row.hash.as_deref(), Some("h-match"));
        assert_eq!(row.synced_at, Some(40));
        assert_eq!(row.carry, None, "fetched on the first rebuild");
        let deck = crate::sync::synced_rows(&conn, SyncType::Deck).expect("rows");
        assert_eq!(deck["limited_q"].hash.as_deref(), Some("h-deck"));
        let deck_state = crate::sync::sync_state(&conn, SyncType::Deck)
            .expect("state")
            .expect("deck");
        assert_eq!(
            (
                deck_state.last_synced_at,
                deck_state.canonical_version,
                deck_state.full_cursor
            ),
            (Some(50), 4, None)
        );
        let match_state = crate::sync::sync_state(&conn, SyncType::Match)
            .expect("state")
            .expect("match");
        assert_eq!(match_state.canonical_version, 0, "runs in full");
        assert_eq!(
            match_state.full_cursor, None,
            "0.x's cursor is a 0.x row id"
        );
        assert_eq!(
            crate::sync::rejections(&conn, SyncType::League).expect("rejections")["q-run_20260819T100005Z"],
            ("deck_not_synced".to_string(), "h-run".to_string())
        );
        let (flag, updated_at): (bool, i64) = conn
            .query_row(
                "SELECT cloud_sync_enabled, updated_at FROM decks WHERE mtgo_id = '111'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("deck");
        assert_eq!((flag, updated_at), (true, 1), "the flag, no updated_at");
        let token: Option<String> = conn
            .query_row(
                "SELECT tournament_token FROM matches WHERE token = 'q-sent'",
                [],
                |row| row.get(0),
            )
            .expect("match");
        assert_eq!(token.as_deref(), Some("q-tournament"));
    }

    #[test]
    fn a_real_0x_tournament_name_survives_the_next_league_sync() {
        let (_dir, mut conn) = db("api-import-tournament");
        insert_manual(&conn, "q-t");
        conn.execute_batch(
            "UPDATE matches SET source = 'log', tournament_event_id = 77, tournament_round = 1,
                 format = 'CLEGACY', derived_version = 1, mtgo_match_id = 5",
        )
        .expect("a tournament match");
        crate::test_support::sync_leagues(&conn);
        let synthesized: (String, bool) = conn
            .query_row(
                "SELECT name, name_synthesized FROM tournaments WHERE mtgo_event_id = 77",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("synthesized");
        assert!(synthesized.1);
        let counts = import_api_state(&mut conn, &full_state(), NOW).expect("import");
        assert_eq!(counts.tournaments_named, 1);
        crate::test_support::sync_leagues(&conn);
        let row: (String, bool, Option<i64>) = conn
            .query_row(
                "SELECT name, name_synthesized, name_checked_at FROM tournaments
                 WHERE mtgo_event_id = 77",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("named");
        assert_eq!(row, ("Qlegacy Challenge 32".to_string(), false, Some(NOW)));
        // One the sync has not made yet is inserted with 0.x's format and start.
        let (_dir2, mut fresh) = db("api-import-tournament-new");
        import_api_state(&mut fresh, &full_state(), NOW).expect("import");
        let inserted: (String, Option<String>, i64) = fresh
            .query_row(
                "SELECT name, format, started_at FROM tournaments WHERE mtgo_event_id = 77",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("inserted");
        assert_eq!(
            inserted,
            (
                "Qlegacy Challenge 32".to_string(),
                Some("CLEGACY".to_string()),
                60
            )
        );
    }
}
