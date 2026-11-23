//! Reading 0.x's API state (Plan Q task 12, 07 spec section 11): what it
//! reported (`matches.submitted_at`), its card stats queue, what it synced
//! (`synced_hash`, `synced_at` of matches, decks and leagues, the deck sync
//! flags, `sync_state`, `sync_rejections`), the tournaments it named and
//! its matches' tournament tokens. Read loosely, like the other readers: a
//! table or column an older 0.x lacks reads as nothing.

use std::path::Path;

use mymtgo_api::SyncType;
use rusqlite::Connection;
use store::import_api::{
    ImportedApiState, LegacyCardStats, LegacyRejection, LegacySubmitted, LegacySyncState,
    LegacySyncedRow, LegacyTournament,
};
use sync_bundles::{deck_client_id, league_client_id};

use crate::read::{ImportError, open, read_error};
use crate::sql::{has_columns, int, parse_id, rows, text, time};

/// Every `(table, columns)` this reader selects or joins on.
pub const READ_COLUMNS: [(&str, &[&str]); 8] = [
    (
        "matches",
        &[
            "submitted_at",
            "synced_hash",
            "synced_at",
            "tournament_token",
        ],
    ),
    ("decks", &["synced_hash", "synced_at", "cloud_sync_enabled"]),
    ("leagues", &["synced_hash", "synced_at"]),
    (
        "card_stat_ship_queue",
        &[
            "game_id",
            "match_id",
            "payload",
            "status",
            "attempts",
            "next_attempt_at",
            "last_error",
            "created_at",
            "updated_at",
        ],
    ),
    (
        "sync_state",
        &["type", "last_synced_at", "canonical_version", "full_cursor"],
    ),
    ("sync_rejections", &["type", "client_id", "reason", "hash"]),
    (
        "tournaments",
        &[
            "mtgo_event_id",
            "token",
            "name",
            "format",
            "started_at",
            "name_synthesized",
        ],
    ),
    ("games", &["id", "mtgo_id"]),
];

fn sync_type(text: Option<String>) -> Option<SyncType> {
    match text?.trim() {
        "deck" => Some(SyncType::Deck),
        "league" => Some(SyncType::League),
        "match" => Some(SyncType::Match),
        _ => None,
    }
}

/// 0.x's API state from the database at `path`, opened read-only.
pub fn read_api_state_at(path: &Path) -> Result<ImportedApiState, ImportError> {
    read_api_state(&open(path)?)
}

/// 0.x's API state, read in one transaction.
pub fn read_api_state(conn: &Connection) -> Result<ImportedApiState, ImportError> {
    let tx = conn.unchecked_transaction().map_err(read_error)?;
    let mut state = ImportedApiState::default();
    if has_columns(&tx, "matches", &["token", "mtgo_id", "submitted_at"])? {
        for row in rows(
            &tx,
            "SELECT token, mtgo_id, submitted_at FROM matches
             WHERE submitted_at IS NOT NULL ORDER BY id",
        )? {
            let (Some(token), Some(at)) = (text(&row, 0), time(&row, 2)) else {
                continue;
            };
            state.submitted.push(LegacySubmitted {
                token,
                mtgo_match_id: text(&row, 1).as_deref().and_then(parse_id),
                submitted_at_ms: at,
            });
        }
    }
    if has_columns(
        &tx,
        "card_stat_ship_queue",
        &["game_id", "match_id", "payload", "status"],
    )? {
        for row in rows(
            &tx,
            "SELECT m.token, m.mtgo_id, g.mtgo_id, q.payload, q.status, q.attempts,
                 q.next_attempt_at, q.last_error, q.created_at, q.updated_at
             FROM card_stat_ship_queue q
             LEFT JOIN matches m ON m.id = q.match_id
             LEFT JOIN games g ON g.id = q.game_id
             ORDER BY q.id",
        )? {
            state.card_stats.push(LegacyCardStats {
                match_token: text(&row, 0).unwrap_or_default(),
                mtgo_match_id: text(&row, 1).as_deref().and_then(parse_id),
                game_mtgo_id: text(&row, 2).as_deref().and_then(parse_id),
                payload: text(&row, 3).unwrap_or_default(),
                status: text(&row, 4).unwrap_or_else(|| "pending".to_string()),
                attempts: int(&row, 5).unwrap_or(0),
                next_attempt_at_ms: time(&row, 6),
                last_error: text(&row, 7),
                created_at_ms: time(&row, 8),
                updated_at_ms: time(&row, 9),
            });
        }
    }
    if has_columns(&tx, "matches", &["token", "synced_hash", "synced_at"])? {
        for row in rows(
            &tx,
            "SELECT token, synced_hash, synced_at FROM matches
             WHERE synced_hash IS NOT NULL ORDER BY id",
        )? {
            if let (Some(token), Some(hash)) = (text(&row, 0), text(&row, 1)) {
                state.synced.push(LegacySyncedRow {
                    sync_type: SyncType::Match,
                    client_id: token,
                    hash,
                    synced_at_ms: time(&row, 2),
                });
            }
        }
    }
    if has_columns(&tx, "decks", &["mtgo_id", "synced_hash", "synced_at"])? {
        for row in rows(
            &tx,
            "SELECT mtgo_id, synced_hash, synced_at FROM decks
             WHERE synced_hash IS NOT NULL ORDER BY id",
        )? {
            if let (Some(mtgo_id), Some(hash)) = (text(&row, 0), text(&row, 1)) {
                state.synced.push(LegacySyncedRow {
                    sync_type: SyncType::Deck,
                    client_id: deck_client_id(&mtgo_id),
                    hash,
                    synced_at_ms: time(&row, 2),
                });
            }
        }
    }
    if has_columns(
        &tx,
        "leagues",
        &["token", "started_at", "synced_hash", "synced_at"],
    )? {
        for row in rows(
            &tx,
            "SELECT token, started_at, synced_hash, synced_at FROM leagues
             WHERE synced_hash IS NOT NULL ORDER BY id",
        )? {
            if let (Some(token), Some(started), Some(hash)) =
                (text(&row, 0), time(&row, 1), text(&row, 2))
            {
                state.synced.push(LegacySyncedRow {
                    sync_type: SyncType::League,
                    client_id: league_client_id(&token, started),
                    hash,
                    synced_at_ms: time(&row, 3),
                });
            }
        }
    }
    if has_columns(&tx, "decks", &["mtgo_id", "cloud_sync_enabled"])? {
        for row in rows(
            &tx,
            "SELECT mtgo_id, cloud_sync_enabled FROM decks ORDER BY id",
        )? {
            if let Some(mtgo_id) = text(&row, 0) {
                state
                    .deck_flags
                    .push((mtgo_id, int(&row, 1).is_some_and(|flag| flag != 0)));
            }
        }
    }
    if has_columns(
        &tx,
        "sync_state",
        &["type", "last_synced_at", "canonical_version", "full_cursor"],
    )? {
        for row in rows(
            &tx,
            "SELECT type, last_synced_at, canonical_version, full_cursor FROM sync_state
             ORDER BY id",
        )? {
            if let Some(sync_type) = sync_type(text(&row, 0)) {
                state.sync_states.push(LegacySyncState {
                    sync_type,
                    last_synced_at_ms: time(&row, 1),
                    canonical_version: int(&row, 2).unwrap_or(0),
                    interrupted: row
                        .get(3)
                        .is_some_and(|cursor| *cursor != rusqlite::types::Value::Null),
                });
            }
        }
    }
    if has_columns(
        &tx,
        "sync_rejections",
        &["type", "client_id", "reason", "hash"],
    )? {
        for row in rows(
            &tx,
            "SELECT type, client_id, reason, hash FROM sync_rejections ORDER BY id",
        )? {
            if let (Some(sync_type), Some(client_id), Some(reason), Some(hash)) = (
                sync_type(text(&row, 0)),
                text(&row, 1),
                text(&row, 2),
                text(&row, 3),
            ) {
                state.rejections.push(LegacyRejection {
                    sync_type,
                    client_id,
                    reason,
                    hash,
                });
            }
        }
    }
    if has_columns(
        &tx,
        "tournaments",
        &["mtgo_event_id", "name", "started_at", "name_synthesized"],
    )? {
        for row in rows(
            &tx,
            "SELECT mtgo_event_id, token, name, format, started_at FROM tournaments
             WHERE name_synthesized = 0 ORDER BY id",
        )? {
            if let (Some(mtgo_event_id), Some(name), Some(started_at_ms)) =
                (int(&row, 0), text(&row, 2), time(&row, 4))
            {
                state.tournaments.push(LegacyTournament {
                    mtgo_event_id,
                    token: text(&row, 1),
                    name,
                    format: text(&row, 3),
                    started_at_ms,
                });
            }
        }
    }
    if has_columns(&tx, "matches", &["token", "tournament_token"])? {
        for row in rows(
            &tx,
            "SELECT token, tournament_token FROM matches
             WHERE tournament_token IS NOT NULL ORDER BY id",
        )? {
            if let (Some(token), Some(tournament)) = (text(&row, 0), text(&row, 1)) {
                state.tournament_tokens.push((token, tournament));
            }
        }
    }
    drop(tx);
    Ok(state)
}
