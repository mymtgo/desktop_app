//! Per-match compaction (spec 4.2): once a match has finished and been quiet for
//! the grace period, its `log_events` rows are rewritten into one zstd archive row.

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::read::{StoredEvent, live_events_for_match, sort_and_dedup};
use crate::{Connection, StoreError};

/// Grace period after a match finishes before its events are archived.
pub const COMPACT_AFTER_MS: i64 = 10 * 60 * 1000;

#[derive(Serialize, Deserialize)]
struct ArchivedEvent {
    file: i64,
    offset: u64,
    at: i64,
    raw: String,
}

/// The archived events of a match (empty when it has no archive).
pub fn archived_events(conn: &Connection, token: &str) -> Result<Vec<StoredEvent>, StoreError> {
    let blob: Option<Vec<u8>> = conn
        .query_row(
            "SELECT raw FROM match_event_archives WHERE match_token = ?1",
            [token],
            |row| row.get(0),
        )
        .optional()?;
    let Some(blob) = blob else {
        return Ok(Vec::new());
    };
    let json = zstd::decode_all(blob.as_slice())?;
    let archived: Vec<ArchivedEvent> = serde_json::from_slice(&json)?;
    Ok(archived
        .into_iter()
        .map(|e| StoredEvent {
            log_file_id: e.file,
            byte_offset: e.offset,
            logged_at_ms: e.at,
            raw: e.raw,
        })
        .collect())
}

/// When this match's archive was last written (`None` without one). A
/// recompaction rewrites it, so callers can cache what they read from an
/// archive under this stamp.
pub fn compacted_at(conn: &Connection, token: &str) -> Result<Option<i64>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT compacted_at FROM match_event_archives WHERE match_token = ?1",
            [token],
            |row| row.get(0),
        )
        .optional()?)
}

/// Move a match's remaining `log_events` rows into its archive (merging with an
/// existing one). Returns how many rows moved.
pub fn compact_match(conn: &Connection, token: &str, now_ms: i64) -> Result<usize, StoreError> {
    let live = live_events_for_match(conn, token)?;
    if live.is_empty() {
        return Ok(0);
    }
    let mut all = archived_events(conn, token)?;
    all.extend(live.iter().cloned());
    sort_and_dedup(&mut all);
    let archived: Vec<ArchivedEvent> = all
        .iter()
        .map(|e| ArchivedEvent {
            file: e.log_file_id,
            offset: e.byte_offset,
            at: e.logged_at_ms,
            raw: e.raw.clone(),
        })
        .collect();
    let blob = zstd::encode_all(serde_json::to_vec(&archived)?.as_slice(), 3)?;
    conn.execute(
        "INSERT INTO match_event_archives (match_token, raw, event_count, compacted_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (match_token) DO UPDATE SET raw = excluded.raw,
             event_count = excluded.event_count, compacted_at = excluded.compacted_at",
        params![token, blob, all.len() as i64, now_ms],
    )?;
    let mut delete =
        conn.prepare_cached("DELETE FROM log_events WHERE log_file_id = ?1 AND byte_offset = ?2")?;
    for event in &live {
        delete.execute(params![event.log_file_id, event.byte_offset as i64])?;
    }
    Ok(live.len())
}
