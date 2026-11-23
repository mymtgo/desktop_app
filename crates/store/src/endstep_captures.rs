//! Imported Endstep capture files (endstep support spec): each file the
//! browser extension's helper wrote, kept whole (zstd of its bytes), one row
//! per capture unit. A capture is a fact about a file; plan 2 folds the rows
//! in (started_at, counter) order.

use endstep_capture::{CaptureHeader, Classification, UnitKind};
use endstep_protocol::{Unit, decode_unit};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::StoreError;

/// The zstd level for capture bodies, as for the Arena game archives.
const ZSTD_LEVEL: i32 = 3;

/// A capture file read from the captures folder, ready to store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCapture {
    pub header: CaptureHeader,
    pub frame_count: u32,
    pub text: String,
    /// Lowercase hex sha256 of the file's bytes.
    pub file_hash: String,
}

/// What storing a batch of captures did, file by file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CaptureImportReport {
    pub added: u32,
    pub replaced: u32,
    pub unchanged: u32,
}

/// What storing one capture did.
enum Outcome {
    Added,
    Replaced,
    Unchanged,
}

fn kind_text(kind: UnitKind) -> &'static str {
    match kind {
        UnitKind::Match => "match",
        UnitKind::Session => "session",
    }
}

fn classification_text(classification: Classification) -> &'static str {
    match classification {
        Classification::Human => "human",
        Classification::Bot => "bot",
        Classification::Unknown => "unknown",
    }
}

/// Stores a capture under its unit id. The same hash again changes nothing; a
/// new hash replaces the row's columns and marks it unsynced.
fn store_capture(
    tx: &Transaction,
    capture: &NewCapture,
    now_ms: i64,
) -> Result<Outcome, StoreError> {
    let header = &capture.header;
    let known: Option<String> = tx
        .query_row(
            "SELECT file_hash FROM endstep_captures WHERE unit_id = ?1",
            [&header.unit_id],
            |row| row.get(0),
        )
        .optional()?;
    let outcome = match known {
        Some(hash) if hash == capture.file_hash => return Ok(Outcome::Unchanged),
        Some(_) => Outcome::Replaced,
        None => Outcome::Added,
    };
    let body = zstd::encode_all(capture.text.as_bytes(), ZSTD_LEVEL)?;
    // A replaced unit is unsynced again; its refs stay until the Endstep
    // sync reads it again (`endstep_sync`).
    tx.execute(
        "INSERT INTO endstep_captures
             (unit_id, kind, started_at, counter, format, extension, browser, developer,
              classification, frame_count, file_hash, body, imported_at, synced_version)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 0)
         ON CONFLICT (unit_id) DO UPDATE SET kind = excluded.kind,
             started_at = excluded.started_at, counter = excluded.counter,
             format = excluded.format, extension = excluded.extension,
             browser = excluded.browser, developer = excluded.developer,
             classification = excluded.classification, frame_count = excluded.frame_count,
             file_hash = excluded.file_hash, body = excluded.body,
             imported_at = excluded.imported_at, synced_version = 0",
        params![
            header.unit_id,
            kind_text(header.kind),
            header.started_at,
            header.counter,
            header.format,
            header.extension,
            header.browser,
            header.developer,
            classification_text(header.classification),
            capture.frame_count,
            capture.file_hash,
            body,
            now_ms
        ],
    )?;
    Ok(outcome)
}

/// Stores a batch of captures and counts the outcomes. Added and replaced
/// units are unsynced (`endstep_sync::unsynced_units`).
pub fn store_captures(
    tx: &Transaction,
    captures: &[NewCapture],
    now_ms: i64,
) -> Result<CaptureImportReport, StoreError> {
    let mut report = CaptureImportReport::default();
    for capture in captures {
        match store_capture(tx, capture, now_ms)? {
            Outcome::Added => report.added += 1,
            Outcome::Replaced => report.replaced += 1,
            Outcome::Unchanged => report.unchanged += 1,
        }
    }
    Ok(report)
}

/// A stored capture's file text, decompressed, or `None` when no capture has
/// this unit id.
pub fn capture_text(conn: &Connection, unit_id: &str) -> Result<Option<String>, StoreError> {
    let body: Option<Vec<u8>> = conn
        .query_row(
            "SELECT body FROM endstep_captures WHERE unit_id = ?1",
            [unit_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(body) = body else {
        return Ok(None);
    };
    let bytes = zstd::decode_all(body.as_slice())?;
    let text = String::from_utf8(bytes)
        .map_err(|_| StoreError::Corrupt(format!("capture {unit_id} is not UTF-8")))?;
    Ok(Some(text))
}

/// The stored unit `unit_id`, decoded. A unit that is gone, or that does
/// not decompress, is not UTF-8 or does not decode, is `None` (logged: it
/// stays stored, so a parser fix and a version bump fold it). Only a SQLite
/// error is an error.
pub fn decoded_unit(conn: &Connection, unit_id: &str) -> Result<Option<Unit>, StoreError> {
    let text = match capture_text(conn, unit_id) {
        Ok(Some(text)) => text,
        Ok(None) => return Ok(None),
        Err(StoreError::Sqlite(error)) => return Err(StoreError::Sqlite(error)),
        Err(error) => {
            tracing::warn!(unit_id, %error, "an Endstep capture unit could not be read; skipped");
            return Ok(None);
        }
    };
    match decode_unit(&text) {
        Ok(unit) => Ok(Some(unit)),
        Err(error) => {
            tracing::warn!(unit_id, %error, "an Endstep capture unit did not decode; skipped");
            Ok(None)
        }
    }
}

/// A stored capture's file hash, or `None` when no capture has this unit id.
pub fn capture_hash(conn: &Connection, unit_id: &str) -> Result<Option<String>, StoreError> {
    Ok(conn
        .prepare_cached("SELECT file_hash FROM endstep_captures WHERE unit_id = ?1")?
        .query_row([unit_id], |row| row.get(0))
        .optional()?)
}

/// How many captures are stored.
pub fn capture_count(conn: &Connection) -> Result<i64, StoreError> {
    Ok(
        conn.query_row("SELECT COUNT(*) FROM endstep_captures", [], |row| {
            row.get(0)
        })?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::db;

    const UNIT: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

    fn header() -> CaptureHeader {
        CaptureHeader {
            format: 1,
            unit_id: UNIT.to_string(),
            kind: UnitKind::Match,
            started_at: 1_000,
            counter: 1,
            extension: "0.1.0".to_string(),
            browser: "chrome".to_string(),
            developer: false,
            classification: Classification::Human,
        }
    }

    fn capture(text: &str, hash: &str, frame_count: u32) -> NewCapture {
        NewCapture {
            header: header(),
            frame_count,
            text: text.to_string(),
            file_hash: hash.to_string(),
        }
    }

    fn store(conn: &mut Connection, captures: &[NewCapture]) -> CaptureImportReport {
        let tx = conn.transaction().expect("tx");
        let report = store_captures(&tx, captures, 99).expect("store");
        tx.commit().expect("commit");
        report
    }

    #[test]
    fn a_capture_is_stored_and_read_back() {
        let (_dir, mut conn) = db("endstep-captures-read-back");
        let text = "{\"header\":1}\n{\"at\":1}\n";
        let report = store(&mut conn, &[capture(text, "h1", 1)]);
        assert_eq!(
            report,
            CaptureImportReport {
                added: 1,
                replaced: 0,
                unchanged: 0
            }
        );
        assert_eq!(capture_text(&conn, UNIT).unwrap().as_deref(), Some(text));
        assert_eq!(
            capture_text(&conn, "00000000-0000-4000-8000-000000000000").unwrap(),
            None
        );
        assert_eq!(capture_count(&conn).unwrap(), 1);
    }

    #[test]
    fn the_same_unit_again_is_unchanged() {
        let (_dir, mut conn) = db("endstep-captures-unchanged");
        store(&mut conn, &[capture("a\n", "h1", 0)]);
        let report = store(&mut conn, &[capture("a\n", "h1", 0)]);
        assert_eq!(
            report,
            CaptureImportReport {
                added: 0,
                replaced: 0,
                unchanged: 1
            }
        );
        assert_eq!(capture_count(&conn).unwrap(), 1);
    }

    #[test]
    fn a_resent_unit_with_more_frames_replaces_it() {
        let (_dir, mut conn) = db("endstep-captures-replaced");
        store(&mut conn, &[capture("a\nb\n", "h1", 1)]);
        let report = store(&mut conn, &[capture("a\nb\nc\n", "h2", 2)]);
        assert_eq!(
            report,
            CaptureImportReport {
                added: 0,
                replaced: 1,
                unchanged: 0
            }
        );
        assert_eq!(
            capture_text(&conn, UNIT).unwrap().as_deref(),
            Some("a\nb\nc\n")
        );
        assert_eq!(capture_count(&conn).unwrap(), 1);
        let frames: i64 = conn
            .query_row(
                "SELECT frame_count FROM endstep_captures WHERE unit_id = ?1",
                [UNIT],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(frames, 2);
    }

    #[test]
    fn a_stored_unit_is_decoded_and_a_broken_one_is_none() {
        let (_dir, mut conn) = db("endstep-captures-decoded");
        let header = format!(
            r#"{{"mymtgo_endstep_capture":1,"unit_id":"{UNIT}","kind":"match","started_at":1000,"counter":1,"extension":"0.1.0","browser":"chrome","developer":false,"classification":"human"}}"#
        );
        let frame = r#"{"at":1001,"src":"rest","dir":"in","url":"/api/other","body":{}}"#;
        let text = format!("{header}\n{frame}\n");
        store(&mut conn, &[capture(&text, "h1", 1)]);
        let unit = decoded_unit(&conn, UNIT).unwrap().expect("decodes");
        assert_eq!(unit.unit_id, UNIT);
        assert_eq!((unit.started_at, unit.counter), (1_000, 1));
        assert_eq!(unit.frames.len(), 1);
        assert_eq!(unit.frames[0].at, 1_001);

        // Gone, not a unit, not zstd: none, and no error.
        assert!(
            decoded_unit(&conn, "00000000-0000-4000-8000-000000000000")
                .unwrap()
                .is_none()
        );
        store(&mut conn, &[capture("not a unit\n", "h2", 1)]);
        assert!(decoded_unit(&conn, UNIT).unwrap().is_none());
        conn.execute(
            "UPDATE endstep_captures SET body = x'00010203' WHERE unit_id = ?1",
            [UNIT],
        )
        .unwrap();
        assert!(decoded_unit(&conn, UNIT).unwrap().is_none());
    }

    #[test]
    fn an_unknown_kind_is_refused() {
        let (_dir, conn) = db("endstep-captures-unknown-kind");
        let insert = conn.execute(
            "INSERT INTO endstep_captures (unit_id, kind, started_at, counter, format, extension,
                 browser, developer, classification, frame_count, file_hash, body, imported_at)
             VALUES (?1, 'replay', 1, 1, 1, '0.1.0', 'chrome', 0, 'human', 0, 'h', x'00', 1)",
            [UNIT],
        );
        assert!(insert.is_err(), "the kind CHECK refuses it");
        assert_eq!(capture_count(&conn).unwrap(), 0);
    }
}
