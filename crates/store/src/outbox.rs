//! The outbox (07 spec sections 6 and 7): every shipment to the MyMTGO API,
//! one row per `(kind, subject)`. The API worker enqueues, claims and marks
//! rows through tracker commands (`OutboxEdit`, applied on the writer
//! thread); the rules (claim size, backoff, attempts) are the `mymtgo-api`
//! crate's.

pub use mymtgo_api::outbox::{ClaimedRow, NewOutboxRow};
pub use mymtgo_api::{OutboxKind, REAP_AFTER_MS};
use mymtgo_api::{after_failure, last_error};
use rusqlite::{OptionalExtension, Transaction, params};

use crate::{Connection, StoreError};

/// A change to the outbox, applied in its own transaction on the writer
/// thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboxEdit {
    /// Queue rows; a `(kind, subject)` already queued is left as it is.
    Enqueue { rows: Vec<NewOutboxRow> },
    /// Take up to `limit` due `pending` rows of `kind`, oldest first, and
    /// mark them `sending`.
    Claim {
        kind: OutboxKind,
        now_ms: i64,
        limit: u32,
    },
    /// The API took these claimed rows; `linked` when the request carried
    /// the account's bearer.
    Sent {
        kind: OutboxKind,
        subjects: Vec<String>,
        now_ms: i64,
        linked: bool,
    },
    /// Sending these claimed rows failed: an attempt is charged, and the row
    /// waits out its backoff or, at `MAX_ATTEMPTS`, fails for good (a match
    /// report never does: `OutboxKind::gives_up`).
    Failed {
        kind: OutboxKind,
        subjects: Vec<String>,
        now_ms: i64,
        error: String,
        /// The answer's status; `None` when no answer came.
        status: Option<u16>,
    },
    /// These claimed rows were never sent (offline mode came on): back to
    /// `pending`, no attempt charged.
    Release {
        kind: OutboxKind,
        subjects: Vec<String>,
    },
    /// These claimed rows' payloads cannot be read (no request can be
    /// built): they fail for good with `PAYLOAD_UNREADABLE`, an attempt
    /// charged (#264). A match report is deleted instead: its body is built
    /// from the match, so the next enqueue queues a fresh one and the match
    /// stays reportable, as in 0.x (Q8 ruling).
    Unreadable {
        kind: OutboxKind,
        subjects: Vec<String>,
    },
    /// Claims older than `REAP_AFTER_MS` belong to a worker that died: back
    /// to `pending`, no attempt charged.
    Reap { now_ms: i64 },
    /// What the API said about synthesized tournament names (07 spec section
    /// 8). Only rows whose name is still synthesized change.
    NameTournaments { rows: Vec<TournamentName> },
    /// Signed in: drop the match reports this device sent in device mode, so
    /// the next enqueue sends them again with the bearer and the API ties
    /// them to the account. A hidden, detached or incomplete match is not
    /// reported again. At most `REQUEUE_LIMIT` a run.
    RequeueDeviceReports,
}

/// One tournament lookup's result: the real name (a 200), or `None` (a 404:
/// only the check is recorded, the synthesized name stays).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TournamentName {
    pub mtgo_event_id: i64,
    pub name: Option<String>,
    pub checked_at_ms: i64,
}

/// `last_error` of a row whose stored payload cannot be read.
pub const PAYLOAD_UNREADABLE: &str = "payload unreadable";

/// What an `OutboxEdit` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboxOutcome {
    /// Rows queued (repeats not counted).
    Enqueued(u32),
    Claimed(Vec<ClaimedRow>),
    /// Rows changed.
    Updated(u32),
}

/// The claim's query: due `pending` rows of a kind through `outbox_due`.
pub const CLAIM_SQL: &str = "SELECT id, subject, payload FROM outbox
     WHERE kind = ?1 AND status = 'pending'
       AND (next_attempt_at IS NULL OR next_attempt_at <= ?2)
     ORDER BY created_at, id
     LIMIT ?3";

/// Apply an outbox edit in its own transaction.
pub fn edit_outbox(conn: &mut Connection, edit: &OutboxEdit) -> Result<OutboxOutcome, StoreError> {
    let tx = conn.transaction()?;
    let outcome = match edit {
        OutboxEdit::Enqueue { rows } => OutboxOutcome::Enqueued(enqueue(&tx, rows)?),
        OutboxEdit::Claim {
            kind,
            now_ms,
            limit,
        } => OutboxOutcome::Claimed(claim(&tx, *kind, *now_ms, *limit)?),
        OutboxEdit::Sent {
            kind,
            subjects,
            now_ms,
            linked,
        } => OutboxOutcome::Updated(mark_sent(&tx, *kind, subjects, *now_ms, *linked)?),
        OutboxEdit::Failed {
            kind,
            subjects,
            now_ms,
            error,
            status,
        } => OutboxOutcome::Updated(mark_failed(&tx, *kind, subjects, *now_ms, error, *status)?),
        OutboxEdit::Release { kind, subjects } => {
            OutboxOutcome::Updated(release(&tx, *kind, subjects)?)
        }
        OutboxEdit::Unreadable { kind, subjects } => {
            OutboxOutcome::Updated(mark_unreadable(&tx, *kind, subjects)?)
        }
        OutboxEdit::Reap { now_ms } => OutboxOutcome::Updated(reap(&tx, *now_ms)?),
        OutboxEdit::NameTournaments { rows } => {
            OutboxOutcome::Updated(name_tournaments(&tx, rows)?)
        }
        OutboxEdit::RequeueDeviceReports => OutboxOutcome::Updated(requeue_device_reports(&tx)?),
    };
    tx.commit()?;
    Ok(outcome)
}

fn enqueue(tx: &Transaction, rows: &[NewOutboxRow]) -> Result<u32, StoreError> {
    let mut statement = tx.prepare_cached(
        "INSERT OR IGNORE INTO outbox (kind, subject, payload, status, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    let mut added = 0;
    for row in rows {
        let (payload, status) = match &row.payload {
            Some(payload) => (payload.as_slice(), "pending"),
            None => (&[][..], "skipped"),
        };
        added += statement.execute(params![
            row.kind.as_str(),
            row.subject,
            payload,
            status,
            row.now_ms
        ])? as u32;
    }
    Ok(added)
}

fn claim(
    tx: &Transaction,
    kind: OutboxKind,
    now_ms: i64,
    limit: u32,
) -> Result<Vec<ClaimedRow>, StoreError> {
    let mut ids = Vec::new();
    let mut claimed = Vec::new();
    {
        let mut statement = tx.prepare_cached(CLAIM_SQL)?;
        let mut rows = statement.query(params![kind.as_str(), now_ms, limit])?;
        while let Some(row) = rows.next()? {
            ids.push(row.get::<_, i64>(0)?);
            claimed.push(ClaimedRow {
                subject: row.get(1)?,
                payload: row.get(2)?,
            });
        }
    }
    let mut mark =
        tx.prepare_cached("UPDATE outbox SET status = 'sending', claimed_at = ?2 WHERE id = ?1")?;
    for id in ids {
        mark.execute(params![id, now_ms])?;
    }
    Ok(claimed)
}

fn mark_sent(
    tx: &Transaction,
    kind: OutboxKind,
    subjects: &[String],
    now_ms: i64,
    linked: bool,
) -> Result<u32, StoreError> {
    let mut statement = tx.prepare_cached(
        "UPDATE outbox SET status = 'sent', attempts = attempts + 1, last_error = NULL,
             sent_at = ?3, claimed_at = NULL, payload = X'', sent_linked = ?4
         WHERE kind = ?1 AND subject = ?2 AND status = 'sending'",
    )?;
    let mut changed = 0;
    for subject in subjects {
        changed += statement.execute(params![kind.as_str(), subject, now_ms, linked])? as u32;
    }
    Ok(changed)
}

/// Match reports dropped for sending again in one requeue, so a signed-out
/// history drains a run at a time (`ENQUEUE_LIMIT` per enqueue).
pub const REQUEUE_LIMIT: u32 = 500;

fn requeue_device_reports(tx: &Transaction) -> Result<u32, StoreError> {
    let changed = tx.execute(
        "DELETE FROM outbox WHERE id IN (
             SELECT o.id FROM outbox o JOIN listed_matches m ON m.token = o.subject
             WHERE o.kind = 'match_report' AND o.status = 'sent' AND o.sent_linked = 0
               AND m.state = 'complete'
             ORDER BY o.id
             LIMIT ?1)",
        [REQUEUE_LIMIT],
    )?;
    Ok(changed as u32)
}

fn mark_failed(
    tx: &Transaction,
    kind: OutboxKind,
    subjects: &[String],
    now_ms: i64,
    error: &str,
    status: Option<u16>,
) -> Result<u32, StoreError> {
    let error = last_error(error);
    let mut read = tx.prepare_cached(
        "SELECT attempts FROM outbox WHERE kind = ?1 AND subject = ?2 AND status = 'sending'",
    )?;
    let mut write = tx.prepare_cached(
        "UPDATE outbox SET status = ?3, attempts = ?4, next_attempt_at = ?5,
             last_error = ?6, claimed_at = NULL
         WHERE kind = ?1 AND subject = ?2",
    )?;
    let mut changed = 0;
    for subject in subjects {
        let attempts: Option<i64> = read
            .query_row(params![kind.as_str(), subject], |row| row.get(0))
            .optional()?;
        let Some(attempts) = attempts else {
            continue;
        };
        let attempts_before = u32::try_from(attempts).unwrap_or(u32::MAX);
        let failure = after_failure(kind, attempts_before, now_ms, status);
        let status = if failure.failed { "failed" } else { "pending" };
        changed += write.execute(params![
            kind.as_str(),
            subject,
            status,
            failure.attempts,
            failure.next_attempt_at,
            error
        ])? as u32;
    }
    Ok(changed)
}

fn release(tx: &Transaction, kind: OutboxKind, subjects: &[String]) -> Result<u32, StoreError> {
    let mut statement = tx.prepare_cached(
        "UPDATE outbox SET status = 'pending', claimed_at = NULL
         WHERE kind = ?1 AND subject = ?2 AND status = 'sending'",
    )?;
    let mut changed = 0;
    for subject in subjects {
        changed += statement.execute(params![kind.as_str(), subject])? as u32;
    }
    Ok(changed)
}

fn mark_unreadable(
    tx: &Transaction,
    kind: OutboxKind,
    subjects: &[String],
) -> Result<u32, StoreError> {
    if kind == OutboxKind::MatchReport {
        let mut drop = tx.prepare_cached(
            "DELETE FROM outbox WHERE kind = ?1 AND subject = ?2 AND status = 'sending'",
        )?;
        let mut changed = 0;
        for subject in subjects {
            changed += drop.execute(params![kind.as_str(), subject])? as u32;
        }
        return Ok(changed);
    }
    let mut statement = tx.prepare_cached(
        "UPDATE outbox SET status = 'failed', attempts = attempts + 1, next_attempt_at = NULL,
             last_error = ?3, claimed_at = NULL
         WHERE kind = ?1 AND subject = ?2 AND status = 'sending'",
    )?;
    let mut changed = 0;
    for subject in subjects {
        changed += statement.execute(params![kind.as_str(), subject, PAYLOAD_UNREADABLE])? as u32;
    }
    Ok(changed)
}

fn reap(tx: &Transaction, now_ms: i64) -> Result<u32, StoreError> {
    let changed = tx.execute(
        "UPDATE outbox SET status = 'pending', claimed_at = NULL
         WHERE status = 'sending' AND claimed_at <= ?1",
        [now_ms - REAP_AFTER_MS],
    )?;
    Ok(changed as u32)
}

fn name_tournaments(tx: &Transaction, rows: &[TournamentName]) -> Result<u32, StoreError> {
    let mut named = tx.prepare_cached(
        "UPDATE tournaments SET name = ?2, name_synthesized = 0, name_checked_at = ?3
         WHERE mtgo_event_id = ?1 AND name_synthesized = 1",
    )?;
    let mut checked = tx.prepare_cached(
        "UPDATE tournaments SET name_checked_at = ?2
         WHERE mtgo_event_id = ?1 AND name_synthesized = 1",
    )?;
    let mut changed = 0;
    for row in rows {
        changed += match &row.name {
            Some(name) => named.execute(params![row.mtgo_event_id, name, row.checked_at_ms])?,
            None => checked.execute(params![row.mtgo_event_id, row.checked_at_ms])?,
        } as u32;
    }
    Ok(changed)
}

/// Record that `subject` of `kind` was delivered at `sent_at` by someone
/// else (a pulled match another device reported, a 0.x submission): a new
/// `sent` row with an empty payload, or an existing row in any other state
/// made `sent`. A `sent` row is left as it is. Returns whether a row changed.
pub(crate) fn record_sent(
    tx: &Transaction,
    kind: OutboxKind,
    subject: &str,
    sent_at: i64,
) -> Result<bool, StoreError> {
    let changed = tx
        .prepare_cached(
            "INSERT INTO outbox (kind, subject, payload, status, sent_at, created_at)
             VALUES (?1, ?2, X'', 'sent', ?3, ?3)
             ON CONFLICT (kind, subject) DO UPDATE SET status = 'sent',
                 sent_at = excluded.sent_at, claimed_at = NULL, next_attempt_at = NULL,
                 payload = X''
             WHERE outbox.status <> 'sent'",
        )?
        .execute(params![kind.as_str(), subject, sent_at])?;
    Ok(changed > 0)
}

/// Rows of a kind not yet delivered or given up: `pending`, or claimed and
/// being sent.
pub fn pending_count(conn: &Connection, kind: OutboxKind) -> Result<u32, StoreError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM outbox WHERE kind = ?1 AND status IN ('pending', 'sending')",
        [kind.as_str()],
        |row| row.get(0),
    )?;
    Ok(count as u32)
}

/// One outbox row as stored, for tests (here and in the app).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxRowState {
    pub kind: String,
    pub subject: String,
    pub payload: Vec<u8>,
    pub status: String,
    pub attempts: i64,
    pub next_attempt_at: Option<i64>,
    pub claimed_at: Option<i64>,
    pub last_error: Option<String>,
    pub sent_at: Option<i64>,
    pub created_at: i64,
    pub sent_linked: Option<bool>,
}

/// Every outbox row, in id order. For tests.
pub fn rows_for_test(conn: &Connection) -> Result<Vec<OutboxRowState>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT kind, subject, payload, status, attempts, next_attempt_at, claimed_at,
             last_error, sent_at, created_at, sent_linked
         FROM outbox ORDER BY id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(OutboxRowState {
            kind: row.get(0)?,
            subject: row.get(1)?,
            payload: row.get(2)?,
            status: row.get(3)?,
            attempts: row.get(4)?,
            next_attempt_at: row.get(5)?,
            claimed_at: row.get(6)?,
            last_error: row.get(7)?,
            sent_at: row.get(8)?,
            created_at: row.get(9)?,
            sent_linked: row.get(10)?,
        })
    })?;
    let mut all = Vec::new();
    for row in rows {
        all.push(row?);
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::db;

    const T: i64 = 1_800_000_000_000;
    const OBS: OutboxKind = OutboxKind::TournamentObservation;

    fn row(subject: &str, created_at: i64) -> NewOutboxRow {
        NewOutboxRow {
            kind: OBS,
            subject: subject.to_string(),
            payload: Some(format!("payload {subject}").into_bytes()),
            now_ms: created_at,
        }
    }

    fn enqueue(conn: &mut Connection, rows: Vec<NewOutboxRow>) -> u32 {
        match edit_outbox(conn, &OutboxEdit::Enqueue { rows }).expect("enqueue") {
            OutboxOutcome::Enqueued(count) => count,
            other => panic!("{other:?}"),
        }
    }

    fn claim(conn: &mut Connection, now_ms: i64, limit: u32) -> Vec<ClaimedRow> {
        let edit = OutboxEdit::Claim {
            kind: OBS,
            now_ms,
            limit,
        };
        match edit_outbox(conn, &edit).expect("claim") {
            OutboxOutcome::Claimed(rows) => rows,
            other => panic!("{other:?}"),
        }
    }

    fn updated(conn: &mut Connection, edit: OutboxEdit) -> u32 {
        match edit_outbox(conn, &edit).expect("edit") {
            OutboxOutcome::Updated(count) => count,
            other => panic!("{other:?}"),
        }
    }

    fn subjects(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn only(conn: &Connection, subject: &str) -> OutboxRowState {
        rows_for_test(conn)
            .expect("rows")
            .into_iter()
            .find(|row| row.subject == subject)
            .expect("the row")
    }

    #[test]
    fn enqueue_twice_adds_nothing() {
        let (_dir, mut conn) = db("outbox-enqueue-twice");
        assert_eq!(enqueue(&mut conn, vec![row("1:10", T)]), 1);
        let mut again = row("1:10", T + 5);
        again.payload = Some(b"other".to_vec());
        assert_eq!(enqueue(&mut conn, vec![again, row("1:20", T + 5)]), 1);
        let rows = rows_for_test(&conn).expect("rows");
        assert_eq!(rows.len(), 2);
        let first = only(&conn, "1:10");
        assert_eq!(first.payload, b"payload 1:10".to_vec(), "kept as it was");
        assert_eq!(first.created_at, T);
        assert_eq!(first.status, "pending");
        assert_eq!(first.kind, "tournament_observation");
        assert_eq!(first.attempts, 0);
        // The same subject under another kind is another row.
        let mut stats = row("1:10", T);
        stats.kind = OutboxKind::CardStats;
        assert_eq!(enqueue(&mut conn, vec![stats]), 1);
        assert_eq!(pending_count(&conn, OBS).expect("count"), 2);
        assert_eq!(
            pending_count(&conn, OutboxKind::CardStats).expect("count"),
            1
        );
    }

    // 0.x EnqueueTournamentObservations: an empty payload is a terminal
    // `skipped` row, never selected again.
    #[test]
    fn an_empty_payload_is_skipped_and_never_claimed() {
        let (_dir, mut conn) = db("outbox-skipped");
        let mut empty = row("1:10", T);
        empty.payload = None;
        assert_eq!(enqueue(&mut conn, vec![empty]), 1);
        let skipped = only(&conn, "1:10");
        assert_eq!(skipped.status, "skipped");
        assert!(skipped.payload.is_empty());
        assert!(claim(&mut conn, T + 1, 200).is_empty());
        assert_eq!(pending_count(&conn, OBS).expect("count"), 0);
    }

    #[test]
    fn a_claim_takes_due_rows_oldest_first_and_marks_them_sending() {
        let (_dir, mut conn) = db("outbox-claim");
        enqueue(
            &mut conn,
            vec![row("c", T + 3), row("a", T + 1), row("b", T + 2)],
        );
        let claimed = claim(&mut conn, T + 10, 2);
        assert_eq!(
            claimed,
            vec![
                ClaimedRow {
                    subject: "a".to_string(),
                    payload: b"payload a".to_vec(),
                },
                ClaimedRow {
                    subject: "b".to_string(),
                    payload: b"payload b".to_vec(),
                },
            ]
        );
        let a = only(&conn, "a");
        assert_eq!(a.status, "sending");
        assert_eq!(a.claimed_at, Some(T + 10));
        assert_eq!(a.attempts, 0, "a claim charges nothing");
        assert_eq!(only(&conn, "c").status, "pending");
        let rest = claim(&mut conn, T + 11, 200);
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].subject, "c");
        assert!(claim(&mut conn, T + 12, 200).is_empty(), "claimed once");
        assert_eq!(pending_count(&conn, OBS).expect("count"), 3, "unsent");
    }

    // 0.x "skips rows whose next_attempt_at is in the future".
    #[test]
    fn skips_rows_whose_next_attempt_at_is_in_the_future() {
        let (_dir, mut conn) = db("outbox-future");
        enqueue(&mut conn, vec![row("a", T)]);
        conn.execute("UPDATE outbox SET next_attempt_at = ?1", [T + 5 * 60_000])
            .expect("set");
        assert!(claim(&mut conn, T, 200).is_empty());
        let a = only(&conn, "a");
        assert_eq!((a.status.as_str(), a.attempts), ("pending", 0));
        assert_eq!(claim(&mut conn, T + 5 * 60_000, 200).len(), 1, "due then");
    }

    // 0.x "marks queue rows sent on 2xx response".
    #[test]
    fn marks_rows_sent_and_empties_their_payload() {
        let (_dir, mut conn) = db("outbox-sent");
        enqueue(&mut conn, vec![row("a", T), row("b", T)]);
        claim(&mut conn, T, 200);
        let count = updated(
            &mut conn,
            OutboxEdit::Sent {
                kind: OBS,
                subjects: subjects(&["a"]),
                now_ms: T + 50,
                linked: false,
            },
        );
        assert_eq!(count, 1);
        let a = only(&conn, "a");
        assert_eq!(a.status, "sent");
        assert_eq!(a.attempts, 1);
        assert_eq!(a.sent_at, Some(T + 50));
        assert_eq!(a.claimed_at, None);
        assert_eq!(a.last_error, None);
        assert!(a.payload.is_empty());
        assert_eq!(only(&conn, "b").status, "sending", "untouched");
    }

    // 0.x "flips failed rows back to pending with exponential backoff".
    #[test]
    fn flips_failed_rows_back_to_pending_with_exponential_backoff() {
        let (_dir, mut conn) = db("outbox-failed");
        enqueue(&mut conn, vec![row("a", T)]);
        claim(&mut conn, T, 200);
        let count = updated(
            &mut conn,
            OutboxEdit::Failed {
                kind: OBS,
                subjects: subjects(&["a"]),
                now_ms: T + 100,
                error: "HTTP 500".to_string(),
                status: Some(500),
            },
        );
        assert_eq!(count, 1);
        let a = only(&conn, "a");
        assert_eq!(a.status, "pending");
        assert_eq!(a.attempts, 1);
        assert_eq!(a.next_attempt_at, Some(T + 100 + 10_000));
        assert_eq!(a.claimed_at, None);
        assert_eq!(a.last_error.as_deref(), Some("HTTP 500"));
        assert_eq!(a.payload, b"payload a".to_vec(), "kept for the retry");
        // A second failure doubles the wait.
        claim(&mut conn, T + 100 + 10_000, 200);
        updated(
            &mut conn,
            OutboxEdit::Failed {
                kind: OBS,
                subjects: subjects(&["a"]),
                now_ms: T + 20_000,
                error: "é".repeat(600),
                status: None,
            },
        );
        let a = only(&conn, "a");
        assert_eq!(a.attempts, 2);
        assert_eq!(a.next_attempt_at, Some(T + 20_000 + 20_000));
        assert_eq!(a.last_error.expect("error").chars().count(), 500);
    }

    // 0.x "marks rows failed after MAX_ATTEMPTS".
    #[test]
    fn marks_rows_failed_after_max_attempts() {
        let (_dir, mut conn) = db("outbox-max-attempts");
        enqueue(&mut conn, vec![row("a", T)]);
        conn.execute(
            "UPDATE outbox SET attempts = 19, next_attempt_at = ?1",
            [T - 60_000],
        )
        .expect("set");
        assert_eq!(claim(&mut conn, T, 200).len(), 1);
        updated(
            &mut conn,
            OutboxEdit::Failed {
                kind: OBS,
                subjects: subjects(&["a"]),
                now_ms: T,
                error: "HTTP 500".to_string(),
                status: Some(500),
            },
        );
        let a = only(&conn, "a");
        assert_eq!(a.status, "failed");
        assert_eq!(a.attempts, 20);
        assert!(
            claim(&mut conn, T + 3_600_000, 200).is_empty(),
            "never again"
        );
        assert_eq!(pending_count(&conn, OBS).expect("count"), 0);
    }

    // Q8 review ruling: a match report refused well past MAX_ATTEMPTS stays
    // pending, its backoff capped at 300 s; other kinds still fail at 20.
    #[test]
    fn a_refused_match_report_stays_pending_forever() {
        let (_dir, mut conn) = db("outbox-report-forever");
        let report = NewOutboxRow {
            kind: OutboxKind::MatchReport,
            subject: "q-match".to_string(),
            payload: Some(b"{}".to_vec()),
            now_ms: T,
        };
        enqueue(&mut conn, vec![report]);
        conn.execute("UPDATE outbox SET attempts = 57", [])
            .expect("set");
        let edit = OutboxEdit::Claim {
            kind: OutboxKind::MatchReport,
            now_ms: T,
            limit: 200,
        };
        assert!(matches!(
            edit_outbox(&mut conn, &edit).expect("claim"),
            OutboxOutcome::Claimed(rows) if rows.len() == 1
        ));
        updated(
            &mut conn,
            OutboxEdit::Failed {
                kind: OutboxKind::MatchReport,
                subjects: subjects(&["q-match"]),
                now_ms: T,
                error: "HTTP 422".to_string(),
                status: Some(422),
            },
        );
        let rows = rows_for_test(&conn).expect("rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, "pending");
        assert_eq!(rows[0].attempts, 58);
        // A refused report waits a day (#500).
        assert_eq!(
            rows[0].next_attempt_at,
            Some(T + mymtgo_api::REFUSED_REPORT_BACKOFF_MS)
        );
        assert_eq!(rows[0].last_error.as_deref(), Some("HTTP 422"));
        assert_eq!(
            pending_count(&conn, OutboxKind::MatchReport).expect("count"),
            1
        );
    }

    // 0.x "Offline mid-send: unsent claimed rows back to pending, no attempt
    // charged".
    #[test]
    fn a_release_returns_claims_without_charging() {
        let (_dir, mut conn) = db("outbox-release");
        enqueue(&mut conn, vec![row("a", T), row("b", T)]);
        claim(&mut conn, T, 200);
        updated(
            &mut conn,
            OutboxEdit::Sent {
                kind: OBS,
                subjects: subjects(&["a"]),
                now_ms: T,
                linked: false,
            },
        );
        let count = updated(
            &mut conn,
            OutboxEdit::Release {
                kind: OBS,
                subjects: subjects(&["a", "b"]),
            },
        );
        assert_eq!(count, 1, "only rows still claimed");
        assert_eq!(only(&conn, "a").status, "sent", "a sent row stays sent");
        let b = only(&conn, "b");
        assert_eq!((b.status.as_str(), b.attempts), ("pending", 0));
        assert_eq!(b.claimed_at, None);
        assert_eq!(b.next_attempt_at, None);
    }

    // #264 (Q7 M1, Q8 M3): a claimed row whose payload cannot be read fails
    // for good with the reason; never pending again. A match report is
    // deleted instead: its body is rebuilt from the match at the next
    // enqueue, so it stays reportable like 0.x's (Q8 ruling).
    #[test]
    fn an_unreadable_payload_fails_for_good_and_a_report_is_dropped_to_be_rebuilt() {
        let (_dir, mut conn) = db("outbox-unreadable");
        let report = NewOutboxRow {
            kind: OutboxKind::MatchReport,
            subject: "q-match".to_string(),
            payload: Some(b"not zstd".to_vec()),
            now_ms: T,
        };
        enqueue(&mut conn, vec![report, row("a", T)]);
        for kind in [OutboxKind::MatchReport, OBS] {
            let edit = OutboxEdit::Claim {
                kind,
                now_ms: T,
                limit: 200,
            };
            edit_outbox(&mut conn, &edit).expect("claim");
        }
        let count = updated(
            &mut conn,
            OutboxEdit::Unreadable {
                kind: OutboxKind::MatchReport,
                subjects: subjects(&["q-match", "missing"]),
            },
        );
        assert_eq!(count, 1);
        let count = updated(
            &mut conn,
            OutboxEdit::Unreadable {
                kind: OBS,
                subjects: subjects(&["a"]),
            },
        );
        assert_eq!(count, 1);
        let rows = rows_for_test(&conn).expect("rows");
        assert_eq!(rows.len(), 1, "the report row is gone");
        let a = &rows[0];
        assert_eq!(a.subject, "a");
        assert_eq!(a.status, "failed");
        assert_eq!(a.attempts, 1);
        assert_eq!(a.next_attempt_at, None);
        assert_eq!(a.claimed_at, None);
        assert_eq!(a.last_error.as_deref(), Some(PAYLOAD_UNREADABLE));
        assert_eq!(a.last_error.as_deref(), Some("payload unreadable"));
        assert_eq!(pending_count(&conn, OBS).expect("count"), 0);
    }

    #[test]
    fn marks_touch_only_claimed_rows() {
        let (_dir, mut conn) = db("outbox-only-claimed");
        enqueue(&mut conn, vec![row("a", T)]);
        let sent = OutboxEdit::Sent {
            kind: OBS,
            subjects: subjects(&["a"]),
            now_ms: T,
            linked: false,
        };
        assert_eq!(updated(&mut conn, sent), 0, "a pending row is not sent");
        let failed = OutboxEdit::Failed {
            kind: OBS,
            subjects: subjects(&["a", "missing"]),
            now_ms: T,
            error: "HTTP 500".to_string(),
            status: Some(500),
        };
        assert_eq!(updated(&mut conn, failed), 0);
        assert_eq!(only(&conn, "a").attempts, 0);
    }

    #[test]
    fn the_reaper_returns_claims_older_than_ten_minutes() {
        let (_dir, mut conn) = db("outbox-reap");
        enqueue(&mut conn, vec![row("old", T), row("young", T)]);
        claim(&mut conn, T, 1);
        claim(&mut conn, T + 60_000, 1);
        let count = updated(
            &mut conn,
            OutboxEdit::Reap {
                now_ms: T + REAP_AFTER_MS,
            },
        );
        assert_eq!(count, 1);
        let old = only(&conn, "old");
        assert_eq!((old.status.as_str(), old.attempts), ("pending", 0));
        assert_eq!(old.claimed_at, None);
        assert_eq!(only(&conn, "young").status, "sending");
    }

    #[test]
    fn the_checks_refuse_a_claim_without_a_time_and_a_send_without_one() {
        let (_dir, mut conn) = db("outbox-checks");
        enqueue(&mut conn, vec![row("a", T)]);
        assert!(
            conn.execute("UPDATE outbox SET status = 'sending'", [])
                .is_err(),
            "sending without claimed_at"
        );
        assert!(
            conn.execute("UPDATE outbox SET status = 'sent'", [])
                .is_err(),
            "sent without sent_at"
        );
        assert!(
            conn.execute("UPDATE outbox SET claimed_at = 1", [])
                .is_err(),
            "claimed_at on a pending row"
        );
        assert!(
            conn.execute("UPDATE outbox SET sent_at = 1", []).is_err(),
            "sent_at on a pending row"
        );
        assert!(
            conn.execute("UPDATE outbox SET kind = 'match'", [])
                .is_err(),
            "unknown kind"
        );
        assert_eq!(only(&conn, "a").status, "pending");
    }

    #[test]
    fn the_claim_walks_the_due_index() {
        let (_dir, conn) = db("outbox-plan");
        let mut statement = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {CLAIM_SQL}"))
            .expect("plan");
        let details: Vec<String> = statement
            .query_map(rusqlite::params![OBS.as_str(), T, 200], |row| {
                row.get::<_, String>(3)
            })
            .expect("query")
            .map(|detail| detail.expect("detail"))
            .collect();
        let plan = details.join("\n");
        assert!(plan.contains("USING INDEX outbox_due"), "{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "{plan}");
    }

    fn tournament(conn: &Connection, event: i64, synthesized: bool, checked: Option<i64>) {
        conn.execute(
            "INSERT INTO tournaments (mtgo_event_id, name, started_at, name_synthesized,
                 name_checked_at)
             VALUES (?1, 'Qsynth', 1, ?2, ?3)",
            rusqlite::params![event, synthesized, checked],
        )
        .expect("tournament");
    }

    fn named(conn: &Connection, event: i64) -> (String, bool, Option<i64>) {
        conn.query_row(
            "SELECT name, name_synthesized, name_checked_at FROM tournaments
             WHERE mtgo_event_id = ?1",
            [event],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("tournament")
    }

    // 07 spec section 8: a 200 writes the name, `name_synthesized = 0` and
    // the check; a 404 writes the check only; a real name is never replaced.
    #[test]
    fn naming_tournaments_writes_the_name_or_only_the_check() {
        let (_dir, mut conn) = db("outbox-names");
        tournament(&conn, 1, true, None);
        tournament(&conn, 2, true, None);
        tournament(&conn, 3, false, Some(7));
        let edit = OutboxEdit::NameTournaments {
            rows: vec![
                TournamentName {
                    mtgo_event_id: 1,
                    name: Some("Qlegacy Challenge 32".to_string()),
                    checked_at_ms: T,
                },
                TournamentName {
                    mtgo_event_id: 2,
                    name: None,
                    checked_at_ms: T,
                },
                TournamentName {
                    mtgo_event_id: 3,
                    name: Some("Qother".to_string()),
                    checked_at_ms: T,
                },
                TournamentName {
                    mtgo_event_id: 99,
                    name: Some("Qgone".to_string()),
                    checked_at_ms: T,
                },
            ],
        };
        assert_eq!(
            edit_outbox(&mut conn, &edit).expect("named"),
            OutboxOutcome::Updated(2)
        );
        assert_eq!(
            named(&conn, 1),
            ("Qlegacy Challenge 32".to_string(), false, Some(T))
        );
        assert_eq!(named(&conn, 2), ("Qsynth".to_string(), true, Some(T)));
        assert_eq!(named(&conn, 3), ("Qsynth".to_string(), false, Some(7)));
    }
}
