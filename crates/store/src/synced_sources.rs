//! The raw source of a match pulled from another device (cross-device sync
//! spec section 7, "Facts storage"): the bundle's `sources` section, kept
//! in canonical form as zstd, one row per match. Pulled sources never enter
//! the local fact tables, so no local fold sees them; the readers of an
//! `imported` Arena or Endstep match read them from here.

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::Value;
use sync_bundles::{BundlePlatform, MatchSources, canonical_json, read_sources, sources_section};

use crate::StoreError;

/// The zstd level, as for the Endstep captures and the Arena game archives.
const ZSTD_LEVEL: i32 = 3;

/// The `platform` column of a source.
fn platform_text(sources: &MatchSources) -> &'static str {
    match sources {
        MatchSources::Arena(_) => "arena",
        MatchSources::Endstep(_) => "endstep",
    }
}

/// The `platform` column as a bundle platform.
fn bundle_platform(text: &str) -> Option<BundlePlatform> {
    match text {
        "arena" => Some(BundlePlatform::Arena),
        "endstep" => Some(BundlePlatform::Endstep),
        _ => None,
    }
}

/// Stores (or replaces) the source of match `match_id`.
pub fn write(tx: &Transaction, match_id: i64, sources: &MatchSources) -> Result<(), StoreError> {
    let canonical = canonical_json(&sources_section(sources));
    let payload = zstd::encode_all(canonical.as_bytes(), ZSTD_LEVEL)?;
    tx.prepare_cached(
        "INSERT INTO synced_sources (match_id, platform, payload) VALUES (?1, ?2, ?3)
         ON CONFLICT (match_id) DO UPDATE SET platform = excluded.platform,
             payload = excluded.payload",
    )?
    .execute(params![match_id, platform_text(sources), payload])?;
    Ok(())
}

/// The stored source of match `match_id`, or `None` when it has none. A row
/// that does not read back is `Corrupt`.
pub fn read(conn: &Connection, match_id: i64) -> Result<Option<MatchSources>, StoreError> {
    let row: Option<(String, Vec<u8>)> = conn
        .prepare_cached("SELECT platform, payload FROM synced_sources WHERE match_id = ?1")?
        .query_row([match_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let Some((platform, payload)) = row else {
        return Ok(None);
    };
    let corrupt =
        |what: &str| StoreError::Corrupt(format!("synced sources of match {match_id}: {what}"));
    let platform = bundle_platform(&platform).ok_or_else(|| corrupt("unknown platform"))?;
    let section: Value = serde_json::from_slice(&zstd::decode_all(payload.as_slice())?)?;
    let bundle = serde_json::json!({ "sources": section });
    match read_sources(&bundle, platform) {
        Ok(Some(sources)) => Ok(Some(sources)),
        Ok(None) => Err(corrupt("empty")),
        Err(error) => Err(corrupt(&error.to_string())),
    }
}

/// Deletes the source of match `match_id` (a live recording took the match
/// over); no row is fine.
pub fn delete(tx: &Transaction, match_id: i64) -> Result<(), StoreError> {
    tx.prepare_cached("DELETE FROM synced_sources WHERE match_id = ?1")?
        .execute([match_id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::db;
    use sync_bundles::{
        ArenaGameSource, ArenaSourceMessage, ArenaSources, EndstepSourceUnit, EndstepSources,
    };

    fn arena() -> MatchSources {
        MatchSources::Arena(ArenaSources {
            event_id: Some("PremierDraft_FRA_20260929".to_string()),
            games: vec![ArenaGameSource {
                game_number: 1,
                messages: vec![ArenaSourceMessage {
                    source: "gre".to_string(),
                    logged_at_ms: 1_700_000_000_123,
                    raw: "{\"a\":1}".to_string(),
                }],
            }],
        })
    }

    fn endstep() -> MatchSources {
        MatchSources::Endstep(EndstepSources {
            forge_game_id: "g-1".to_string(),
            units: vec![EndstepSourceUnit {
                unit_id: "u1".to_string(),
                lines: vec!["l1".to_string(), "l2".to_string()],
            }],
        })
    }

    /// A match row for the sources to belong to.
    fn a_match(conn: &Connection, token: &str) -> i64 {
        conn.execute_batch(
            "INSERT OR IGNORE INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0)",
        )
        .expect("account");
        conn.query_row(
            "INSERT INTO matches (token, account_id, source, state, outcome, started_at,
                 last_activity_at, revision, derived_version)
             VALUES (?1, 1, 'imported', 'complete', 'win', 1, 2, 1, 0) RETURNING id",
            [token],
            |row| row.get(0),
        )
        .expect("match")
    }

    #[test]
    fn sources_round_trip_and_replace() {
        let (_dir, mut conn) = db("synced-sources-round-trip");
        let arena_id = a_match(&conn, "q-arena");
        let endstep_id = a_match(&conn, "q-endstep");
        let tx = conn.transaction().expect("tx");
        write(&tx, arena_id, &endstep()).expect("write");
        write(&tx, arena_id, &arena()).expect("replace");
        write(&tx, endstep_id, &endstep()).expect("write");
        tx.commit().expect("commit");

        assert_eq!(read(&conn, arena_id).expect("read"), Some(arena()));
        assert_eq!(read(&conn, endstep_id).expect("read"), Some(endstep()));
        let platforms: Vec<String> = conn
            .prepare("SELECT platform FROM synced_sources ORDER BY match_id")
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        assert_eq!(platforms, vec!["arena", "endstep"]);
        let payload: Vec<u8> = conn
            .query_row(
                "SELECT payload FROM synced_sources WHERE match_id = ?1",
                [endstep_id],
                |row| row.get(0),
            )
            .expect("payload");
        assert_eq!(
            String::from_utf8(zstd::decode_all(payload.as_slice()).expect("zstd")).expect("text"),
            r#"{"forge_game_id":"g-1","units":[{"lines":["l1","l2"],"unit_id":"u1"}]}"#,
            "the canonical section, compressed"
        );
    }

    #[test]
    fn a_match_without_sources_reads_none_and_delete_removes_them() {
        let (_dir, mut conn) = db("synced-sources-delete");
        let id = a_match(&conn, "q-arena");
        assert_eq!(read(&conn, id).expect("read"), None);
        let tx = conn.transaction().expect("tx");
        write(&tx, id, &arena()).expect("write");
        delete(&tx, id).expect("delete");
        delete(&tx, id).expect("deleting nothing is fine");
        tx.commit().expect("commit");
        assert_eq!(read(&conn, id).expect("read"), None);
    }

    #[test]
    fn deleting_the_match_deletes_its_sources() {
        let (_dir, mut conn) = db("synced-sources-cascade");
        let id = a_match(&conn, "q-arena");
        let tx = conn.transaction().expect("tx");
        write(&tx, id, &arena()).expect("write");
        tx.commit().expect("commit");
        conn.execute("DELETE FROM matches WHERE id = ?1", [id])
            .expect("delete the match");
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM synced_sources", [], |row| row.get(0))
            .expect("count");
        assert_eq!(rows, 0);
    }
}
