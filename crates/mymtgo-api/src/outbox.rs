//! The outbox's rules (07 spec section 7, survey sections 10 and 11): one
//! table for every shipment, claimed in batches, retried with backoff and
//! given up after `MAX_ATTEMPTS` (match reports never are). The store applies them; the worker sends.

use serde::Serialize;
use serde_json::Value;

/// What a shipment is: the `outbox.kind` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OutboxKind {
    /// Subject: the match token.
    MatchReport,
    /// Subject: `"{log_file_id}:{byte_offset}"` of the event.
    TournamentObservation,
    /// Subject: `"{match token}:{mtgo game id}"`.
    CardStats,
}

impl OutboxKind {
    /// The name stored in `outbox.kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            OutboxKind::MatchReport => "match_report",
            OutboxKind::TournamentObservation => "tournament_observation",
            OutboxKind::CardStats => "card_stats",
        }
    }

    /// Whether a row of this kind is given up at `MAX_ATTEMPTS`. A match
    /// report never is (Q8 review ruling, 0.x left a refused report
    /// unsubmitted and tried again every run): a match with an outbox row is
    /// never queued again, so giving up would lose its report for good.
    pub fn gives_up(self) -> bool {
        self != OutboxKind::MatchReport
    }
}

/// A shipment to queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewOutboxRow {
    pub kind: OutboxKind,
    pub subject: String,
    /// What to send; `None` stores a terminal `skipped` row with an empty
    /// payload (0.x: an observation with nothing in it).
    pub payload: Option<Vec<u8>>,
    pub now_ms: i64,
}

/// One claimed row: the worker sends its payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedRow {
    pub subject: String,
    pub payload: Vec<u8>,
}

/// A stored payload: `value` as JSON, compressed with zstd. `None` when
/// either step fails.
pub fn pack_payload<T: Serialize + ?Sized>(value: &T) -> Option<Vec<u8>> {
    let json = serde_json::to_vec(value).ok()?;
    zstd::encode_all(json.as_slice(), 0).ok()
}

/// The claimed rows whose payloads unpack, as `(subject, JSON)` in claim
/// order. A row whose payload cannot be read is left out (the worker fails
/// it as unreadable).
pub fn readable_rows(rows: &[ClaimedRow]) -> Vec<(String, Value)> {
    let mut readable = Vec::new();
    for row in rows {
        let Ok(json) = zstd::decode_all(row.payload.as_slice()) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<Value>(&json) else {
            continue;
        };
        readable.push((row.subject.clone(), value));
    }
    readable
}

/// Rows one claim takes at most (0.x `CLAIM_LIMIT`, and the API's
/// `MAX_BATCH_SIZE` for observations).
pub const CLAIM_LIMIT: u32 = 200;

/// A row fails for good on its 20th failed attempt (0.x `MAX_ATTEMPTS`).
pub const MAX_ATTEMPTS: u32 = 20;

/// A claim older than this is a worker that died: the reaper returns its rows
/// to `pending` without charging an attempt (v1 addition, 0.x had none).
pub const REAP_AFTER_MS: i64 = 600_000;

/// How much of an error `last_error` keeps (0.x `substr($error, 0, 500)`).
pub const LAST_ERROR_CHARS: usize = 500;

/// `last_error` when the request never got an answer.
pub const REQUEST_THREW: &str = "request threw exception";

/// The longest wait between attempts, in seconds.
const MAX_BACKOFF_SECONDS: u64 = 300;

/// The wait before the next attempt, in milliseconds, given the attempts
/// counted so far, this failure included: `min(300, 5 * 2^attempts)`
/// seconds (0.x `markFailure`).
pub fn backoff_ms(attempts: u32) -> i64 {
    let doubling = 2u64.saturating_pow(attempts);
    let seconds = 5u64.saturating_mul(doubling).min(MAX_BACKOFF_SECONDS);
    (seconds * 1_000) as i64
}

/// What a failed attempt does to a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failure {
    /// The attempts counted, this one included.
    pub attempts: u32,
    /// The row is given up (`failed`), never claimed again.
    pub failed: bool,
    /// When the row may be claimed again.
    pub next_attempt_at: i64,
}

/// How long a match report the API refused waits before it is sent again
/// (#500): it never gives up, but asking every five minutes for an answer
/// that will not change cost about 288 requests a day.
pub const REFUSED_REPORT_BACKOFF_MS: i64 = 24 * 3_600_000;

/// Whether an answer refuses the request as it is, so sending it again soon
/// gets the same answer: a 4xx but 401 (credentials, recovered), 408
/// (timeout) and 429 (rate limited).
pub fn refused(status: u16) -> bool {
    (400..500).contains(&status) && !matches!(status, 401 | 408 | 429)
}

/// The row's state after a failed attempt, given its kind, its attempts
/// before it and the answer's status (`None` when none came): `failed` at
/// `MAX_ATTEMPTS` for a kind that gives up, else `pending` again after the
/// backoff, or after `REFUSED_REPORT_BACKOFF_MS` for a match report the API
/// refused.
pub fn after_failure(
    kind: OutboxKind,
    attempts_before: u32,
    now_ms: i64,
    status: Option<u16>,
) -> Failure {
    let attempts = attempts_before.saturating_add(1);
    let wait = if kind == OutboxKind::MatchReport && status.is_some_and(refused) {
        REFUSED_REPORT_BACKOFF_MS
    } else {
        backoff_ms(attempts)
    };
    Failure {
        attempts,
        failed: kind.gives_up() && attempts >= MAX_ATTEMPTS,
        next_attempt_at: now_ms + wait,
    }
}

/// `last_error` for an answer that was not 2xx.
pub fn status_error(status: u16) -> String {
    format!("HTTP {status}")
}

/// An error cut to what `last_error` keeps.
pub fn last_error(text: &str) -> String {
    text.chars().take(LAST_ERROR_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    #[test]
    fn the_kinds_are_the_schema_names() {
        assert_eq!(OutboxKind::MatchReport.as_str(), "match_report");
        assert_eq!(
            OutboxKind::TournamentObservation.as_str(),
            "tournament_observation"
        );
        assert_eq!(OutboxKind::CardStats.as_str(), "card_stats");
    }

    #[test]
    fn the_queue_numbers_are_0x_numbers() {
        assert_eq!(CLAIM_LIMIT, 200);
        assert_eq!(MAX_ATTEMPTS, 20);
        assert_eq!(REAP_AFTER_MS, 600_000);
    }

    // 0.x ShipCardStats::markFailure: min(300, 5 * 2^attempts) seconds,
    // with the attempts counted after this failure.
    #[test]
    fn backoff_doubles_from_ten_seconds_up_to_five_minutes() {
        assert_eq!(backoff_ms(1), 10_000);
        assert_eq!(backoff_ms(2), 20_000);
        assert_eq!(backoff_ms(5), 160_000);
        assert_eq!(backoff_ms(6), 300_000);
        assert_eq!(backoff_ms(19), 300_000);
        assert_eq!(backoff_ms(u32::MAX), 300_000);
    }

    #[test]
    fn a_failure_counts_an_attempt_and_waits_out_the_backoff() {
        assert_eq!(
            after_failure(OutboxKind::CardStats, 0, NOW, None),
            Failure {
                attempts: 1,
                failed: false,
                next_attempt_at: NOW + 10_000,
            }
        );
        assert_eq!(
            after_failure(OutboxKind::CardStats, 18, NOW, None),
            Failure {
                attempts: 19,
                failed: false,
                next_attempt_at: NOW + 300_000,
            }
        );
    }

    // 0.x "marks rows failed after MAX_ATTEMPTS".
    #[test]
    fn the_twentieth_failure_gives_up() {
        for kind in [OutboxKind::TournamentObservation, OutboxKind::CardStats] {
            let failure = after_failure(kind, 19, NOW, None);
            assert_eq!(failure.attempts, 20);
            assert!(failure.failed, "{kind:?}");
        }
    }

    // Q8 review ruling: a refused match report is retried forever (0.x left
    // it unsubmitted for the next run), at most every 300 s.
    #[test]
    fn a_match_report_never_gives_up() {
        for attempts_before in [19, 20, 100, u32::MAX - 1] {
            let failure = after_failure(OutboxKind::MatchReport, attempts_before, NOW, None);
            assert!(!failure.failed, "{attempts_before}");
            assert_eq!(failure.attempts, attempts_before + 1);
            assert_eq!(failure.next_attempt_at, NOW + 300_000);
        }
        assert!(!OutboxKind::MatchReport.gives_up());
        assert!(OutboxKind::CardStats.gives_up());
    }

    // #500: a report the API refuses (a 4xx but 401, 408 or 429) waits a
    // day, not five minutes, and still never gives up.
    #[test]
    fn a_refused_match_report_waits_a_day() {
        for status in [400, 403, 404, 409, 410, 422] {
            let failure = after_failure(OutboxKind::MatchReport, 3, NOW, Some(status));
            assert!(!failure.failed, "{status}");
            assert_eq!(
                failure.next_attempt_at,
                NOW + REFUSED_REPORT_BACKOFF_MS,
                "{status}"
            );
        }
        assert_eq!(REFUSED_REPORT_BACKOFF_MS, 24 * 3_600_000);
    }

    // A network error, a 5xx or a 4xx worth retrying soon keep the backoff:
    // a report must survive a long outage.
    #[test]
    fn a_report_that_may_go_soon_keeps_the_backoff() {
        for status in [None, Some(401), Some(408), Some(429), Some(500), Some(503)] {
            let failure = after_failure(OutboxKind::MatchReport, 3, NOW, status);
            assert_eq!(failure.next_attempt_at, NOW + backoff_ms(4), "{status:?}");
        }
    }

    // Other kinds give up at `MAX_ATTEMPTS` already; a refusal leaves their
    // backoff as it was.
    #[test]
    fn a_refusal_leaves_other_kinds_as_they_were() {
        let failure = after_failure(OutboxKind::CardStats, 3, NOW, Some(422));
        assert_eq!(failure.next_attempt_at, NOW + backoff_ms(4));
    }

    #[test]
    fn a_packed_payload_reads_back_and_a_broken_one_is_left_out() {
        let value = serde_json::json!({"games": [1, 2]});
        let packed = pack_payload(&value).expect("packed");
        let rows = vec![
            ClaimedRow {
                subject: "q-one".to_string(),
                payload: packed,
            },
            ClaimedRow {
                subject: "q-broken".to_string(),
                payload: b"not zstd".to_vec(),
            },
            ClaimedRow {
                subject: "q-not-json".to_string(),
                payload: zstd::encode_all(&b"{nope"[..], 0).expect("zstd"),
            },
        ];
        assert_eq!(readable_rows(&rows), vec![("q-one".to_string(), value)]);
    }

    #[test]
    fn last_error_keeps_500_characters() {
        assert_eq!(status_error(500), "HTTP 500");
        assert_eq!(REQUEST_THREW, "request threw exception");
        let long = "é".repeat(600);
        assert_eq!(last_error(&long).chars().count(), 500);
        assert_eq!(last_error("HTTP 503"), "HTTP 503");
    }
}
