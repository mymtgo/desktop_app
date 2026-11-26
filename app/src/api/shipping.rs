//! Tournament observations, real tournament names, and the match reports
//! and card stats (behind their flags) as IO (07 spec section 8, rulings 3, 6, 12 and
//! 15), run by the `api` worker. Reads go through a read-only connection;
//! every write goes through the tracker (`OutboxEditor`).

use mymtgo_api::{
    ApiError, NewOutboxRow, OutboxKind, PER_RUN_CAP, RequestClass, TournamentLookup,
    card_stats_requests, card_stats_row, card_stats_skip_reason, gate, observation_row,
    observation_skip_reason, observations_requests, parse_tournament, report_requests, report_row,
    tournament_request,
};
use store::Connection;
use store::api_reads::{
    ENQUEUE_LIMIT, card_stats_candidates, report_candidates, tournaments_to_name,
    unqueued_tournament_events,
};
use store::outbox::{OutboxEdit, OutboxOutcome, TournamentName};
use tauri::{AppHandle, Manager};
use tracker::OutboxEditor;

use crate::api::session::ApiState;
use crate::api::worker::{OnUnauthorized, ShipOutcome, WorkerSignal, ship, ship_with};
use crate::bridge::lock;
use crate::commands::{CommandError, Startup, off_async_workers, ready};

/// What one enqueue run did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EnqueueOutcome {
    /// Unqueued tournament events read (at most `ENQUEUE_LIMIT`).
    pub read: u32,
    /// Queued to send.
    pub queued: u32,
    /// Queued as `skipped`: nothing to send, never read again.
    pub skipped: u32,
}

/// Queue the tournament events not yet in the outbox (0.x
/// `EnqueueTournamentObservations`, 500 a run). The worker runs it only
/// while community requests could go out.
pub fn enqueue_observations(
    editor: &OutboxEditor,
    reader: &Connection,
    now_ms: i64,
) -> EnqueueOutcome {
    let events = match unqueued_tournament_events(reader, ENQUEUE_LIMIT) {
        Ok(events) => events,
        Err(error) => {
            tracing::warn!(%error, "could not read the tournament events to queue");
            return EnqueueOutcome::default();
        }
    };
    if events.is_empty() {
        return EnqueueOutcome::default();
    }
    let mut outcome = EnqueueOutcome {
        read: events.len() as u32,
        ..EnqueueOutcome::default()
    };
    let mut rows = Vec::new();
    for event in &events {
        let row = observation_row(event, now_ms);
        if row.payload.is_some() {
            outcome.queued += 1;
        } else {
            // Logged once: a skipped row is never read again. The line itself
            // is not logged (it can hold other players' names).
            tracing::warn!(
                kind = event.kind.as_str(),
                subject = row.subject.as_str(),
                reason = observation_skip_reason(event).unwrap_or("unknown"),
                "skipping a tournament event"
            );
            outcome.skipped += 1;
        }
        rows.push(row);
    }
    match editor.edit(OutboxEdit::Enqueue { rows }) {
        Ok(OutboxOutcome::Enqueued(_)) => outcome,
        Ok(other) => {
            tracing::warn!(?other, "an outbox enqueue answered something else");
            EnqueueOutcome::default()
        }
        Err(error) => {
            tracing::warn!(%error, "could not queue tournament observations");
            EnqueueOutcome::default()
        }
    }
}

/// Ship one claim of observations (0.x `ShipTournamentObservations`): one
/// gzip request per claim of up to 200, split at 6 MiB of JSON.
pub fn ship_observations(api: &ApiState, editor: &OutboxEditor, now_ms: i64) -> ShipOutcome {
    ship(
        api,
        editor,
        OutboxKind::TournamentObservation,
        observations_requests,
        now_ms,
    )
}

/// The version reports carry (0.x `config('nativephp.version')`).
pub const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Queue rows through the tracker; the outcome when it took them, else
/// nothing (logged).
fn enqueue(
    editor: &OutboxEditor,
    rows: Vec<NewOutboxRow>,
    outcome: EnqueueOutcome,
) -> EnqueueOutcome {
    if rows.is_empty() {
        return outcome;
    }
    match editor.edit(OutboxEdit::Enqueue { rows }) {
        Ok(OutboxOutcome::Enqueued(_)) => outcome,
        Ok(other) => {
            tracing::warn!(?other, "an outbox enqueue answered something else");
            EnqueueOutcome::default()
        }
        Err(error) => {
            tracing::warn!(%error, "could not queue outbox rows");
            EnqueueOutcome::default()
        }
    }
}

/// Queue the matches to report (0.x `scopeSubmittable`, up to 500 a run),
/// each with its body built now. A match needs an archetype on its player
/// seat (`match_archetypes`, archetypes spec section 13). The worker runs it
/// only while `MATCH_REPORTS_LIVE` is on and community requests could go
/// out.
pub fn enqueue_reports(editor: &OutboxEditor, reader: &Connection, now_ms: i64) -> EnqueueOutcome {
    let inputs = match report_candidates(reader, ENQUEUE_LIMIT, CLIENT_VERSION, now_ms) {
        Ok(inputs) => inputs,
        Err(error) => {
            tracing::warn!(%error, "could not read the matches to report");
            return EnqueueOutcome::default();
        }
    };
    let rows: Vec<NewOutboxRow> = inputs
        .iter()
        .map(|input| report_row(input, now_ms))
        .collect();
    let count = rows.len() as u32;
    let outcome = EnqueueOutcome {
        read: count,
        queued: count,
        skipped: 0,
    };
    enqueue(editor, rows, outcome)
}

/// Signed in, drop the match reports this device sent in device mode, so
/// `enqueue_reports` queues them again and they go with the bearer: the API
/// ties a report to the account only then. Signed out, nothing changes.
pub fn requeue_device_reports(api: &ApiState, editor: &OutboxEditor) -> u32 {
    if !api.session().linked() {
        return 0;
    }
    match editor.edit(OutboxEdit::RequeueDeviceReports) {
        Ok(OutboxOutcome::Updated(count)) => {
            if count > 0 {
                tracing::info!(count, "match reports sent signed out go again signed in");
            }
            count
        }
        Ok(other) => {
            tracing::warn!(?other, "an outbox requeue answered something else");
            0
        }
        Err(error) => {
            tracing::warn!(%error, "could not requeue the match reports sent signed out");
            0
        }
    }
}

/// Ship one claim of match reports (0.x `SubmitMatchToApi`): one JSON
/// request per match; a 401 recovers and retries once; any other refusal
/// is logged and the row waits for its next attempt.
pub fn ship_reports(api: &ApiState, editor: &OutboxEditor, now_ms: i64) -> ShipOutcome {
    ship_with(
        api,
        editor,
        OutboxKind::MatchReport,
        report_requests,
        now_ms,
        OnUnauthorized::RetryOnce,
    )
}

/// Queue the games to send card stats for (0.x `EnqueueCardStats`, up to
/// 500 a run). A game without a player archetype is never read (F23); a
/// game without a payload (no format, date or card with an oracle id) is
/// logged and read again next run.
pub fn enqueue_card_stats(
    editor: &OutboxEditor,
    reader: &Connection,
    now_ms: i64,
) -> EnqueueOutcome {
    let games = match card_stats_candidates(reader, ENQUEUE_LIMIT, now_ms) {
        Ok(games) => games,
        Err(error) => {
            tracing::warn!(%error, "could not read the games to send card stats for");
            return EnqueueOutcome::default();
        }
    };
    let mut outcome = EnqueueOutcome {
        read: games.len() as u32,
        ..EnqueueOutcome::default()
    };
    let mut rows = Vec::new();
    for game in &games {
        match card_stats_row(game, now_ms) {
            Some(row) => {
                outcome.queued += 1;
                rows.push(row);
            }
            None => {
                tracing::warn!(
                    match_token = game.match_token.as_str(),
                    mtgo_game_id = game.mtgo_game_id,
                    reason = card_stats_skip_reason(game).unwrap_or("unknown"),
                    "skipping card stats for a game"
                );
                outcome.skipped += 1;
            }
        }
    }
    enqueue(editor, rows, outcome)
}

/// Ship one claim of card stats (0.x `ShipCardStats`): gzip chunks of 100.
pub fn ship_card_stats(api: &ApiState, editor: &OutboxEditor, now_ms: i64) -> ShipOutcome {
    ship(
        api,
        editor,
        OutboxKind::CardStats,
        card_stats_requests,
        now_ms,
    )
}

/// The Data & Privacy page's pending match line (0.x OfflineModeCard).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PendingReports {
    /// `MATCH_REPORTS_LIVE`: the line and its button show only when true.
    pub live: bool,
    /// Matches to report or queued and not yet sent.
    pub count: u32,
}

/// What the pending line shows at `now_ms`, from a read-only connection (a
/// match still in its `SEND_HOLD_MS` is not counted yet).
pub fn pending_reports(
    reader: &Connection,
    live: bool,
    now_ms: i64,
) -> Result<PendingReports, String> {
    let count = store::api_reads::pending_report_count(reader, now_ms)
        .map_err(|error| error.to_string())?;
    Ok(PendingReports { live, count })
}

#[tauri::command]
#[specta::specta]
pub async fn pending_match_reports(app: AppHandle) -> Result<PendingReports, CommandError> {
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        pending_reports(
            &lock(&state.reader),
            mymtgo_api::MATCH_REPORTS_LIVE,
            crate::wall_clock_ms(),
        )
        .map_err(CommandError::Storage)
    })
    .await
}

/// "Submit pending" (0.x `RunSubmitMatchesController`): the worker queues
/// and ships the match reports at its next tick (only while they are live).
#[tauri::command]
#[specta::specta]
pub fn submit_pending_matches(app: AppHandle) {
    crate::api::worker::signal(&app, WorkerSignal::SubmitReports);
}

/// What one tournament names run did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NamesOutcome {
    pub asked: u32,
    pub named: u32,
    pub not_found: u32,
    pub failed: u32,
}

/// Ask the API for the real names of up to `PER_RUN_CAP` tournaments whose
/// name is synthesized and was never asked about, newest first. A 200
/// writes the name, a 404 only the check; any other failure writes nothing,
/// so the next run asks again (ruling 12). Nothing goes out while the
/// community gate is closed; offline mode mid-run stops the run.
pub fn name_tournaments(
    api: &ApiState,
    editor: &OutboxEditor,
    reader: &Connection,
    now_ms: i64,
) -> NamesOutcome {
    let mut outcome = NamesOutcome::default();
    if api.client().is_err()
        || gate(RequestClass::Community, &api.gate_now())
            .refusal()
            .is_some()
    {
        return outcome;
    }
    let events = match tournaments_to_name(reader, PER_RUN_CAP) {
        Ok(events) => events,
        Err(error) => {
            tracing::warn!(%error, "could not read the tournaments to name");
            return outcome;
        }
    };
    let mut rows = Vec::new();
    for mtgo_event_id in events {
        let mut stop = false;
        let lookup = match api.send(&tournament_request(mtgo_event_id), now_ms) {
            Ok(response) => parse_tournament(&response),
            Err(ApiError::Offline) => break,
            Err(error) => {
                tracing::warn!(error = %error.kind(), mtgo_event_id, "a tournament lookup failed");
                // The server is not answering: the rest are asked next run.
                stop = matches!(error, ApiError::Timeout | ApiError::Connect(..));
                TournamentLookup::Failed
            }
        };
        outcome.asked += 1;
        if stop {
            outcome.failed += 1;
            break;
        }
        let name = match lookup {
            TournamentLookup::Named(name) => {
                outcome.named += 1;
                Some(name)
            }
            TournamentLookup::NotFound => {
                outcome.not_found += 1;
                None
            }
            TournamentLookup::Failed => {
                outcome.failed += 1;
                continue;
            }
        };
        rows.push(TournamentName {
            mtgo_event_id,
            name,
            checked_at_ms: now_ms,
        });
    }
    if !rows.is_empty()
        && let Err(error) = editor.edit(OutboxEdit::NameTournaments { rows })
    {
        tracing::warn!(%error, "could not store tournament names; they are asked again");
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::session::testing::{TestApi, test_api};
    use crate::api::stub::{Recorded, Reply, Stub};
    use crate::api::worker::ShipOutcome;
    use crate::bridge::lock;
    use crate::commands::AppState;
    use crate::commands::tests::ready_state;
    use fixture_tools::TestDir;
    use mymtgo_api::OFFLINE_MODE;
    use std::io::Read;
    use std::sync::mpsc;
    use store::outbox::OutboxRowState;
    use tracker::TrackerEvent;

    const NOW: i64 = 1_800_000_000_000;
    const DAY: i64 = 24 * 3_600_000;
    /// When `reportable_match` ends: a day before `NOW`, long past its
    /// `SEND_HOLD_MS`.
    const ENDED: i64 = NOW - DAY;
    const SYNC_LINE: &str = r#"12:00:00 [INF] (Tournament|Sync) EventSyncData_t {"Token":"tok-1"}"#;

    /// A device with a fresh key; offline when asked.
    fn api(base_url: &str, offline: bool) -> TestApi {
        let key_valid = (NOW + DAY).to_string();
        let offline_row = offline.to_string();
        let rows = [
            ("api_device_id", r#""test-device""#),
            ("api_key_expires_at", key_valid.as_str()),
            (OFFLINE_MODE, offline_row.as_str()),
        ];
        test_api(base_url, &rows, &[("api_key", "test-key")])
    }

    fn tracked(label: &str) -> (TestDir, AppState, mpsc::Receiver<TrackerEvent>) {
        let dir = TestDir::new(label);
        let (state, events) = ready_state(&dir);
        (dir, state, events)
    }

    fn editor(state: &AppState) -> OutboxEditor {
        lock(&state.tracker)
            .as_ref()
            .expect("tracker")
            .outbox_editor()
    }

    fn reader(state: &AppState) -> store::Connection {
        store::open_reader(&state.db_path).expect("reader")
    }

    fn write(state: &AppState, sql: &str) {
        store::open_writer(&state.db_path)
            .expect("writer")
            .execute_batch(sql)
            .expect("write");
    }

    /// A SQL text literal.
    fn quoted(text: &str) -> String {
        format!("'{}'", text.replace('\'', "''"))
    }

    /// A classified log event, in a log file of its own.
    fn log_event(state: &AppState, offset: i64, kind: &str, token: Option<&str>, raw: &str) {
        write(
            state,
            "INSERT OR IGNORE INTO log_files (id, path, identity_hash, head_hash, head_len,
                 day_anchor, timezone, clock_date, first_seen_at, last_seen_at)
             VALUES (7, '/q/obs.log', 'a', 'b', 1, '2026-10-09', 'UTC', '2026-10-09', 1, 1)",
        );
        let token = token.map_or("NULL".to_string(), quoted);
        write(
            state,
            &format!(
                "INSERT INTO log_events (log_file_id, byte_offset, kind, tournament_token,
                     logged_at, raw)
                 VALUES (7, {offset}, {}, {token}, {}, CAST({} AS BLOB))",
                quoted(kind),
                NOW - 60_000,
                quoted(raw)
            ),
        );
    }

    fn rows(state: &AppState) -> Vec<OutboxRowState> {
        store::outbox::rows_for_test(&reader(state)).expect("rows")
    }

    fn gunzip(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(bytes)
            .read_to_end(&mut out)
            .expect("gzip");
        out
    }

    fn sent_body(request: &Recorded) -> serde_json::Value {
        serde_json::from_slice(&gunzip(&request.body)).expect("a JSON body")
    }

    fn tournament(state: &AppState, event: i64, started_at: i64) {
        write(
            state,
            &format!(
                "INSERT INTO tournaments (mtgo_event_id, name, started_at)
                 VALUES ({event}, 'Qsynth {event}', {started_at})"
            ),
        );
    }

    fn tournament_row(state: &AppState, event: i64) -> (String, bool, Option<i64>) {
        reader(state)
            .query_row(
                "SELECT name, name_synthesized, name_checked_at FROM tournaments
                 WHERE mtgo_event_id = ?1",
                [event],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("tournament")
    }

    // 0.x EnqueueTournamentObservationsTest: "enqueues an observation for a
    // newly classified tournament event".
    #[test]
    fn enqueues_an_observation_for_a_newly_classified_tournament_event() {
        let (_dir, state, _events) = tracked("shipping-enqueue");
        log_event(&state, 10, "tournament_sync", Some("tok-1"), SYNC_LINE);
        let outcome = enqueue_observations(&editor(&state), &reader(&state), NOW);
        assert_eq!(
            outcome,
            EnqueueOutcome {
                read: 1,
                queued: 1,
                skipped: 0
            }
        );
        let stored = rows(&state);
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].kind, "tournament_observation");
        assert_eq!(stored[0].subject, "7:10");
        assert_eq!(stored[0].status, "pending");
        assert_eq!(stored[0].created_at, NOW);
        let payload: serde_json::Value =
            serde_json::from_slice(&zstd::decode_all(stored[0].payload.as_slice()).expect("zstd"))
                .expect("json");
        assert_eq!(payload["tournament_token"], "tok-1");
        assert_eq!(payload["event_type"], "tournament_sync");
        assert_eq!(payload["payload"], serde_json::json!({ "Token": "tok-1" }));
    }

    // 0.x: "skips log events that are already enqueued".
    #[test]
    fn skips_log_events_that_are_already_enqueued() {
        let (_dir, state, _events) = tracked("shipping-enqueue-twice");
        log_event(&state, 10, "tournament_sync", Some("tok-1"), SYNC_LINE);
        enqueue_observations(&editor(&state), &reader(&state), NOW);
        let again = enqueue_observations(&editor(&state), &reader(&state), NOW + 1);
        assert_eq!(again, EnqueueOutcome::default());
        assert_eq!(rows(&state).len(), 1);
    }

    // 0.x: "ignores non-tournament log events".
    #[test]
    fn ignores_non_tournament_log_events() {
        let (_dir, state, _events) = tracked("shipping-enqueue-other");
        log_event(&state, 10, "match_state_changed", None, "something");
        let outcome = enqueue_observations(&editor(&state), &reader(&state), NOW);
        assert_eq!(outcome.read, 0);
        assert!(rows(&state).is_empty());
    }

    // 0.x: "records a terminal skipped row for events whose extracted
    // payload is empty" and "does not re-process empty-payload events on
    // subsequent runs".
    #[test]
    fn an_empty_payload_is_a_terminal_skipped_row_read_once() {
        let (_dir, state, _events) = tracked("shipping-enqueue-skipped");
        log_event(
            &state,
            10,
            "tournament_state_changed",
            Some("deadbeef-dead-beef-dead-beefdeadbeef"),
            "garbage line with no from/to",
        );
        let first = enqueue_observations(&editor(&state), &reader(&state), NOW);
        assert_eq!(
            first,
            EnqueueOutcome {
                read: 1,
                queued: 0,
                skipped: 1
            }
        );
        for later in 1..3 {
            let outcome = enqueue_observations(&editor(&state), &reader(&state), NOW + later);
            assert_eq!(outcome, EnqueueOutcome::default(), "run {later}");
        }
        let stored = rows(&state);
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].status, "skipped");
        assert!(stored[0].payload.is_empty());
    }

    #[test]
    fn the_enqueue_reads_at_most_500_events_a_run() {
        let (_dir, state, _events) = tracked("shipping-enqueue-500");
        for offset in 0..501 {
            log_event(&state, offset, "tournament_sync", Some("tok-1"), SYNC_LINE);
        }
        assert_eq!(
            enqueue_observations(&editor(&state), &reader(&state), NOW).read,
            500
        );
        assert_eq!(
            enqueue_observations(&editor(&state), &reader(&state), NOW).read,
            1
        );
        assert_eq!(rows(&state).len(), 501);
    }

    fn enqueued(state: &AppState, kind: &str, raw: &str) {
        log_event(state, 10, kind, Some("tok-1"), raw);
        enqueue_observations(&editor(state), &reader(state), NOW - 1_000);
    }

    // 0.x ShipTournamentObservationsTest: "marks observations as sent on 204
    // response" and "sends gzipped body with auth headers".
    #[test]
    fn ships_a_gzipped_body_with_auth_headers_and_marks_it_sent() {
        let stub = Stub::start(|_| Reply::json(204, ""));
        let (_dir, state, _events) = tracked("shipping-ship");
        let api = api(&stub.base_url, false);
        enqueued(&state, "tournament_sync", SYNC_LINE);
        let outcome = ship_observations(&api.state, &editor(&state), NOW);
        assert_eq!(
            outcome,
            ShipOutcome {
                claimed: 1,
                sent: 1,
                ..ShipOutcome::default()
            }
        );
        let stored = &rows(&state)[0];
        assert_eq!((stored.status.as_str(), stored.attempts), ("sent", 1));
        let seen = stub.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].method, "POST");
        assert_eq!(seen[0].target, "/api/tournament-observations");
        assert_eq!(seen[0].header("x-device-id"), Some("test-device"));
        assert_eq!(seen[0].header("x-api-key"), Some("test-key"));
        assert_eq!(seen[0].header("content-encoding"), Some("gzip"));
        assert_eq!(seen[0].header("content-type"), Some("application/json"));
        assert!(!seen[0].body.is_empty());
    }

    // 0.x: "sends a body that satisfies the API validator: array of
    // observations with required fields".
    #[test]
    fn sends_a_body_that_satisfies_the_api_validator() {
        let stub = Stub::start(|_| Reply::json(204, ""));
        let (_dir, state, _events) = tracked("shipping-validator");
        let api = api(&stub.base_url, false);
        enqueued(
            &state,
            "tournament_state_changed",
            "12:00:00 [INF] (Game Management|Tournament State Changed for b197b9e8-0d08-4227-aa17-ba38cb4c1731 from TournamentUninitializedState to PremierNotJoinedAwaitingMinPlayersState)",
        );
        ship_observations(&api.state, &editor(&state), NOW);
        let body = sent_body(&stub.requests()[0]);
        let list = body.as_array().expect("a bare array");
        assert_eq!(list.len(), 1);
        let observation = &list[0];
        for key in [
            "tournament_token",
            "match_token",
            "event_type",
            "payload",
            "client_observed_at",
        ] {
            assert!(observation.get(key).is_some(), "{key}");
        }
        assert_eq!(observation["tournament_token"], "tok-1");
        assert_eq!(observation["event_type"], "tournament_state_changed");
        assert!(
            observation["payload"]
                .as_object()
                .is_some_and(|payload| !payload.is_empty())
        );
        let observed = observation["client_observed_at"].as_str().expect("a date");
        assert_eq!(observed, mymtgo_api::php::iso8601_utc(NOW - 60_000));
    }

    // 0.x ShipTournamentObservationsTest: "flips failed observations back to
    // pending with backoff", "marks observations failed after 20 attempts",
    // "skips observations whose next_attempt_at is in the future", through
    // the real builder.
    #[test]
    fn the_outbox_mechanics_hold_through_the_real_builder() {
        let stub = Stub::start(|_| Reply::json(500, "server error"));
        let (_dir, state, _events) = tracked("shipping-mechanics");
        let api = api(&stub.base_url, false);
        enqueued(&state, "tournament_sync", SYNC_LINE);
        assert_eq!(
            ship_observations(&api.state, &editor(&state), NOW).failed,
            1
        );
        let stored = &rows(&state)[0];
        assert_eq!((stored.status.as_str(), stored.attempts), ("pending", 1));
        assert_eq!(stored.next_attempt_at, Some(NOW + 10_000));
        assert!(
            stored
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("500"))
        );
        assert_eq!(
            ship_observations(&api.state, &editor(&state), NOW + 5_000),
            ShipOutcome::default(),
            "not due yet"
        );
        assert_eq!(stub.requests().len(), 1);
        write(
            &state,
            &format!(
                "UPDATE outbox SET attempts = 19, next_attempt_at = {}",
                NOW - 60_000
            ),
        );
        ship_observations(&api.state, &editor(&state), NOW);
        let stored = &rows(&state)[0];
        assert_eq!((stored.status.as_str(), stored.attempts), ("failed", 20));
    }

    #[test]
    fn nothing_is_shipped_or_asked_offline() {
        let stub = Stub::start(|_| Reply::json(204, ""));
        let (_dir, state, _events) = tracked("shipping-offline");
        let api = api(&stub.base_url, true);
        enqueued(&state, "tournament_sync", SYNC_LINE);
        tournament(&state, 77, NOW);
        assert_eq!(
            ship_observations(&api.state, &editor(&state), NOW),
            ShipOutcome::default()
        );
        assert_eq!(
            name_tournaments(&api.state, &editor(&state), &reader(&state), NOW),
            NamesOutcome::default()
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(stub.connections(), 0);
        assert_eq!(rows(&state)[0].status, "pending");
        assert_eq!(tournament_row(&state, 77).2, None);
    }

    fn lookup_stub() -> Stub {
        Stub::start(|request| match request.target.as_str() {
            "/api/tournaments/12841367" => Reply::json(
                200,
                r#"{"mtgo_event_id":12841367,"name":"Qlegacy Challenge 32","format":"CLEGACY",
                    "started_at":"2026-05-03T05:00:00.000000Z"}"#,
            ),
            "/api/tournaments/500" => Reply::json(500, "{}"),
            _ => Reply::json(404, "{}"),
        })
    }

    // 0.x FetchTournamentMetadataTest: "returns metadata when API responds
    // 200", "returns null on 404" (only the check is written) and the
    // failure (nothing written, asked again next run).
    #[test]
    fn a_200_names_a_404_checks_and_a_failure_writes_nothing() {
        let stub = lookup_stub();
        let (_dir, state, _events) = tracked("shipping-names");
        let api = api(&stub.base_url, false);
        tournament(&state, 12_841_367, NOW - DAY);
        tournament(&state, 99_999_999, NOW - 2 * DAY);
        tournament(&state, 500, NOW - 3 * DAY);
        let outcome = name_tournaments(&api.state, &editor(&state), &reader(&state), NOW);
        assert_eq!(
            outcome,
            NamesOutcome {
                asked: 3,
                named: 1,
                not_found: 1,
                failed: 1
            }
        );
        assert_eq!(
            tournament_row(&state, 12_841_367),
            ("Qlegacy Challenge 32".to_string(), false, Some(NOW))
        );
        assert_eq!(
            tournament_row(&state, 99_999_999),
            ("Qsynth 99999999".to_string(), true, Some(NOW))
        );
        assert_eq!(
            tournament_row(&state, 500),
            ("Qsynth 500".to_string(), true, None)
        );
        let seen = stub.requests();
        assert_eq!(seen[0].target, "/api/tournaments/12841367", "newest first");
        assert_eq!(seen[0].method, "GET");
        assert_eq!(seen[0].header("x-api-key"), Some("test-key"));
        // RefreshTournamentMetadataTest "skips the event ... when API returns
        // 404": a 404 is not asked again, a failure is.
        let again = name_tournaments(&api.state, &editor(&state), &reader(&state), NOW + 1);
        assert_eq!(again.asked, 1);
        assert_eq!(
            stub.requests().last().map(|r| r.target.clone()).as_deref(),
            Some("/api/tournaments/500")
        );
    }

    // 0.x FetchTournamentMetadataTest: "returns null when request throws".
    #[test]
    fn a_request_that_throws_writes_nothing() {
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.local_addr().expect("address").port()
        };
        let (_dir, state, _events) = tracked("shipping-names-throw");
        let api = api(&format!("http://127.0.0.1:{port}"), false);
        tournament(&state, 12_841_367, NOW);
        let outcome = name_tournaments(&api.state, &editor(&state), &reader(&state), NOW);
        assert_eq!((outcome.asked, outcome.failed), (1, 1));
        assert_eq!(tournament_row(&state, 12_841_367).2, None);
    }

    // A silent or unreachable server would hold the worker for every
    // remaining lookup (up to 50 at 5 s); the run stops at the first
    // transport error and the rest are asked at the next run.
    #[test]
    fn a_transport_error_ends_the_names_run() {
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.local_addr().expect("address").port()
        };
        let (_dir, state, _events) = tracked("shipping-names-transport");
        let api = api(&format!("http://127.0.0.1:{port}"), false);
        for event in 1..=3 {
            tournament(&state, event, NOW - DAY + event);
        }
        let outcome = name_tournaments(&api.state, &editor(&state), &reader(&state), NOW);
        assert_eq!(
            (outcome.asked, outcome.failed),
            (1, 1),
            "stopped at the first"
        );
    }

    // 0.x RefreshTournamentMetadataTest: "caps API lookups per run and
    // reports remaining events" and "is idempotent".
    #[test]
    fn lookups_are_capped_per_run_and_a_second_pass_asks_nothing() {
        let stub = Stub::start(|_| Reply::json(404, "{}"));
        let (_dir, state, _events) = tracked("shipping-names-cap");
        let api = api(&stub.base_url, false);
        for event in 1..=60 {
            tournament(&state, event, NOW - DAY + event);
        }
        let first = name_tournaments(&api.state, &editor(&state), &reader(&state), NOW);
        assert_eq!(first.asked, 50);
        assert_eq!(
            stub.requests()[0].target,
            "/api/tournaments/60",
            "newest first"
        );
        let second = name_tournaments(&api.state, &editor(&state), &reader(&state), NOW);
        assert_eq!(second.asked, 10);
        let third = name_tournaments(&api.state, &editor(&state), &reader(&state), NOW);
        assert_eq!(third, NamesOutcome::default());
        assert_eq!(stub.requests().len(), 60);
        // "returns zero counts and makes no HTTP calls when no matches need
        // backfill".
        let quiet = Stub::start(|_| Reply::json(200, "{}"));
        let api = self::api(&quiet.base_url, false);
        let (_dir, state, _events) = tracked("shipping-names-none");
        assert_eq!(
            name_tournaments(&api.state, &editor(&state), &reader(&state), NOW),
            NamesOutcome::default()
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(quiet.connections(), 0);
    }

    // ---- Match reports and card stats (behind their flags, ruling 3) -------

    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const PLAYER_UUID: &str = "aaaaaaaa-1111-4111-8111-111111111111";

    /// Clear every match's archetype rows (a match never detected).
    fn without_archetypes(state: &AppState) {
        write(state, "DELETE FROM match_archetypes");
    }

    /// A complete CMODERN match of Qplayer, ended at `ENDED`, with a deck
    /// version and one won game (mtgo game 901) whose local 1001 has an oracle id, and a
    /// downloaded player archetype (`PLAYER_UUID`) on its player seat.
    /// Imported, so the running tracker never rederives it.
    fn reportable_match(state: &AppState, token: &str) {
        write(
            state,
            &format!(
                "INSERT INTO accounts (username, first_seen_at) VALUES ('Qplayer', 0)
                     ON CONFLICT (username) DO NOTHING;
                 INSERT INTO decks (mtgo_id, name, format, created_at, updated_at, source)
                     VALUES ('900', 'Qdeck', 'Modern', 0, 0, 'file')
                     ON CONFLICT (mtgo_id) DO NOTHING;
                 INSERT INTO deck_versions (deck_id, signature, modified_at)
                     VALUES ((SELECT id FROM decks), 'cXNpZw==', 0)
                     ON CONFLICT (deck_id, signature) DO NOTHING;
                 INSERT INTO cards (catalog_id, oracle_id) VALUES (1001, 'q-oracle-1')
                     ON CONFLICT (catalog_id) DO NOTHING;
                 INSERT INTO matches (token, account_id, source, format, state, outcome,
                     started_at, ended_at, last_activity_at, revision, derived_version,
                     deck_version_id)
                     VALUES ({}, (SELECT id FROM accounts WHERE username = 'Qplayer'), 'imported', 'CMODERN', 'complete',
                         'win',
                         {ENDED}, {ENDED}, {ENDED}, 1, 1, (SELECT id FROM deck_versions));
                 INSERT INTO games (match_id, mtgo_game_id, game_number, result, end_reason,
                     started_at)
                     VALUES (last_insert_rowid(), 901, 1, 'win', 'win', {ENDED});
                 INSERT INTO game_cards (game_id, side, catalog_id, main_qty, opening_hand,
                     starting_hand)
                     VALUES (last_insert_rowid(), 'local', 1001, 4, 1, 1);
                 INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
                     VALUES ('{PLAYER_UUID}', 'Qarchetype', 'CMODERN', 'downloaded', 0, 0)
                     ON CONFLICT (uuid) DO NOTHING;
                 INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
                     VALUES ((SELECT id FROM matches WHERE token = {}), 0,
                         (SELECT id FROM archetypes WHERE uuid = '{PLAYER_UUID}'), 1.0);",
                quoted(token),
                quoted(token)
            ),
        );
    }

    fn of_kind(state: &AppState, kind: &str) -> Vec<OutboxRowState> {
        rows(state)
            .into_iter()
            .filter(|row| row.kind == kind)
            .collect()
    }

    // 0.x: a submittable match is reported once (`submitted_at`); v1 queues
    // it once by its token.
    #[test]
    fn a_reportable_match_is_queued_once_with_its_body() {
        let (_dir, state, _events) = tracked("shipping-reports-enqueue");
        reportable_match(&state, "q-match-1");
        let outcome = enqueue_reports(&editor(&state), &reader(&state), NOW);
        assert_eq!(outcome.queued, 1);
        let again = enqueue_reports(&editor(&state), &reader(&state), NOW);
        assert_eq!(again, EnqueueOutcome::default());
        let queued = of_kind(&state, "match_report");
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].subject, "q-match-1");
        let body: serde_json::Value =
            serde_json::from_slice(&zstd::decode_all(queued[0].payload.as_slice()).expect("zstd"))
                .expect("json");
        assert_eq!(body["match_token"], "q-match-1");
        assert_eq!(body["client_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(body["player_archetype_uuid"], PLAYER_UUID);
    }

    // 0.x SubmitMatchToApi: a match without a player archetype is never
    // reported, so none is queued.
    #[test]
    fn no_match_is_queued_without_a_player_archetype() {
        let (_dir, state, _events) = tracked("shipping-reports-none");
        reportable_match(&state, "q-match-1");
        without_archetypes(&state);
        let outcome = enqueue_reports(&editor(&state), &reader(&state), NOW);
        assert_eq!(outcome, EnqueueOutcome::default());
        assert!(rows(&state).is_empty());
    }

    // 0.x SubmitMatchToApi: one JSON POST per match; 2xx marks it reported.
    #[test]
    fn a_report_is_posted_as_json_and_marked_sent() {
        let stub = Stub::start(|_| Reply::json(200, "{}"));
        let (_dir, state, _events) = tracked("shipping-reports-ship");
        let api = api(&stub.base_url, false);
        reportable_match(&state, "q-match-1");
        reportable_match(&state, "q-match-2");
        enqueue_reports(&editor(&state), &reader(&state), NOW - 1);
        let outcome = ship_reports(&api.state, &editor(&state), NOW);
        assert_eq!((outcome.claimed, outcome.sent), (2, 2));
        let seen = stub.requests();
        assert_eq!(seen.len(), 2, "one request per match");
        assert_eq!(seen[0].target, "/api/matches/report");
        assert_eq!(seen[0].header("content-type"), Some("application/json"));
        assert_eq!(seen[0].header("x-api-key"), Some("test-key"));
        let body: serde_json::Value = serde_json::from_slice(&seen[0].body).expect("json");
        assert_eq!(body["match_token"], "q-match-1");
        assert!(
            of_kind(&state, "match_report")
                .iter()
                .all(|row| row.status == "sent")
        );
    }

    /// `api` signed in with the bearer `q-access`.
    fn signed_in_api(base_url: &str) -> TestApi {
        let key_valid = (NOW + DAY).to_string();
        let token_valid = (NOW + 7 * DAY).to_string();
        let rows = [
            ("api_device_id", r#""test-device""#),
            ("api_key_expires_at", key_valid.as_str()),
            ("api_token_expires_at", token_valid.as_str()),
            (OFFLINE_MODE, "false"),
        ];
        let secrets = [
            ("api_key", "test-key"),
            ("access_token", "q-access"),
            ("refresh_token", "q-refresh"),
        ];
        test_api(base_url, &rows, &secrets)
    }

    fn sent_linked(state: &AppState, subject: &str) -> Option<bool> {
        of_kind(state, "match_report")
            .into_iter()
            .find(|row| row.subject == subject)
            .expect("the report row")
            .sent_linked
    }

    // A report sent signed out stays unowned on the API: once signed in, the
    // device sends it again with the bearer.
    #[test]
    fn reports_sent_signed_out_go_again_with_the_bearer_once_signed_in() {
        let stub = Stub::start(|_| Reply::json(200, "{}"));
        let (_dir, state, _events) = tracked("shipping-reports-relink");
        let signed_out = api(&stub.base_url, false);
        reportable_match(&state, "q-match-1");
        reportable_match(&state, "q-hidden");
        enqueue_reports(&editor(&state), &reader(&state), NOW - 1);
        ship_reports(&signed_out.state, &editor(&state), NOW);
        assert_eq!(sent_linked(&state, "q-match-1"), Some(false));
        assert_eq!(
            requeue_device_reports(&signed_out.state, &editor(&state)),
            0,
            "signed out, nothing to gain"
        );
        write(
            &state,
            "UPDATE matches SET hidden_at = 1 WHERE token = 'q-hidden'",
        );

        let signed_in = signed_in_api(&stub.base_url);
        assert_eq!(requeue_device_reports(&signed_in.state, &editor(&state)), 1);
        assert_eq!(
            enqueue_reports(&editor(&state), &reader(&state), NOW + 1).queued,
            1
        );
        let outcome = ship_reports(&signed_in.state, &editor(&state), NOW + 2);
        assert_eq!(outcome.sent, 1);
        let seen = stub.requests();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[2].header("authorization"), Some("Bearer q-access"));
        let body: serde_json::Value = serde_json::from_slice(&seen[2].body).expect("json");
        assert_eq!(body["match_token"], "q-match-1");
        assert_eq!(sent_linked(&state, "q-match-1"), Some(true));
        assert_eq!(
            sent_linked(&state, "q-hidden"),
            Some(false),
            "a hidden match is not reported again"
        );
        assert_eq!(
            requeue_device_reports(&signed_in.state, &editor(&state)),
            0,
            "sent once with the bearer"
        );
    }

    /// A complete, game synced and archetype detected Arena limited match
    /// `token` of `Qarena#12345` against `Opp`, ended at `ENDED`: event
    /// `Qevent`, Bo1, one won game, a run `arena:c1` with a course deck
    /// snapshot, and a limited rank reading before the match. No archetype
    /// seat.
    fn arena_match(state: &AppState, token: &str) {
        write(
            state,
            &format!(
                "INSERT INTO accounts (username, first_seen_at, platform, arena_user_id)
                     VALUES ('Qarena#12345', 0, 'arena', 'Qarena-id')
                     ON CONFLICT (username) DO NOTHING;
                 INSERT INTO opponents (username, first_seen_at, platform, display_name)
                     VALUES ('arena:opp-id', 0, 'arena', 'Opp')
                     ON CONFLICT (username) DO NOTHING;
                 INSERT INTO matches (token, account_id, opponent_id, source, format, match_type,
                     game_structure, state, outcome, started_at, ended_at, last_activity_at,
                     revision, derived_version, games_synced_at, archetypes_detected_at)
                     VALUES ({}, (SELECT id FROM accounts WHERE username = 'Qarena#12345'),
                         (SELECT id FROM opponents WHERE username = 'arena:opp-id'), 'log',
                         'S6EOE', 'limited', 'bo1', 'complete', 'win',
                         {ENDED}, {ENDED}, {ENDED}, 1, 1, {ENDED}, {ENDED});
                 INSERT INTO games (match_id, mtgo_game_id, game_number, result, end_reason,
                     on_play, local_mulligans, opp_mulligans, local_starting_hand_size,
                     turn_count, started_at, ended_at)
                     VALUES (last_insert_rowid(), 901, 1, 'win', 'win', 1, 0, 0, 7, 9, 1, 2);
                 INSERT INTO arena_match_events (match_token, event_name)
                     VALUES ({}, 'Qevent');
                 INSERT INTO leagues (run_key, account_id, source, kind, state, token, name,
                     started_at, created_at)
                     SELECT 'arena:c1', account_id, 'log', 'draft', 'active', 'arena:c1', 'Qrun',
                         1, 1
                     FROM matches WHERE token = {};
                 UPDATE matches SET league_id = last_insert_rowid() WHERE token = {};
                 INSERT OR IGNORE INTO arena_log_files (id, path, identity_hash, head_hash,
                     head_len, first_seen_at) VALUES (1, '/arena/Player.log', 1, 2, 3, 4);
                 INSERT INTO arena_course_observations (arena_log_file_id, byte_offset,
                     observed_at, account_id, course_id, event_name, module, wins, losses,
                     deck_id, deck_name, cards)
                     SELECT 1, 500, {}, account_id, 'c1', 'Qevent', 'Qmodule', 0, 0, 'd',
                         'Qdeck',
                         '{{\"main\":[[100090001,2]],\"side\":[[100090002,1]],\"pool\":[]}}'
                     FROM matches WHERE token = {};
                 INSERT INTO arena_log_events (arena_log_file_id, byte_offset, seq, kind,
                     match_token, logged_at, raw)
                     VALUES (1, 1000, 0, 'room_state', {}, 1, x'');
                 INSERT INTO arena_rank_observations (account_id, ladder, season, observed_at,
                     class, level, step, matches_won, matches_lost, arena_log_file_id,
                     byte_offset)
                     VALUES ((SELECT id FROM accounts WHERE username = 'Qarena#12345'),
                         'limited', 95, {}, 'Gold', 2, 3, 0, 0, 1, 900);",
                quoted(token),
                quoted(token),
                quoted(token),
                quoted(token),
                ENDED - 1_000,
                quoted(token),
                quoted(token),
                ENDED - 500,
            ),
        );
    }

    fn report_requests_seen(stub: &Stub) -> Vec<Recorded> {
        stub.requests()
            .into_iter()
            .filter(|request| request.target == "/api/matches/report")
            .collect()
    }

    /// The API's `ReportMatchRequest` rules for an Arena body (`arenaRules`
    /// and the shared rules), as assertions: a twin of the validator.
    fn assert_satisfies_arena_validator(body: &serde_json::Value) {
        use serde_json::Value;
        // PHP `string|max:N`: a string of at most N characters.
        let text = |value: &Value, max: usize, what: &str| {
            let text = value
                .as_str()
                .unwrap_or_else(|| panic!("{what} is a string"));
            assert!(text.chars().count() <= max, "{what} max {max}");
        };
        let nullable_text = |value: &Value, max: usize, what: &str| {
            if !value.is_null() {
                text(value, max, what);
            }
        };
        // PHP `nullable|integer|min:lo|max:hi`.
        let nullable_int = |value: &Value, lo: i64, hi: i64, what: &str| {
            if !value.is_null() {
                let n = value
                    .as_i64()
                    .unwrap_or_else(|| panic!("{what} is an integer"));
                assert!((lo..=hi).contains(&n), "{what} in {lo}..={hi}");
            }
        };
        // PHP `date`: a string that parses as a date (ISO 8601 here).
        let date = |value: &Value, what: &str| {
            let text = value
                .as_str()
                .unwrap_or_else(|| panic!("{what} is a date string"));
            assert!(
                text.len() >= 10
                    && text.as_bytes()[4] == b'-'
                    && text.as_bytes()[7] == b'-'
                    && text[..4].bytes().all(|b| b.is_ascii_digit()),
                "{what} parses as a date"
            );
        };
        let nullable_date = |value: &Value, what: &str| {
            if !value.is_null() {
                date(value, what);
            }
        };
        let is_uuid = |value: &Value, what: &str| {
            let text = value
                .as_str()
                .unwrap_or_else(|| panic!("{what} is a uuid string"));
            let parts: Vec<&str> = text.split('-').collect();
            let lengths: Vec<usize> = parts.iter().map(|part| part.len()).collect();
            assert!(
                lengths == [8, 4, 4, 4, 12]
                    && text.chars().all(|c| c == '-' || c.is_ascii_hexdigit()),
                "{what} is a uuid"
            );
        };

        assert_eq!(body["platform"], "arena");
        for key in ["match_token", "username", "format"] {
            assert!(!body[key].as_str().expect(key).is_empty(), "{key} required");
        }
        assert!(body["is_tournament"].is_boolean());
        assert!(["win", "loss", "draw"].contains(&body["result"].as_str().expect("result")));
        date(&body["played_at"], "played_at");
        nullable_int(&body["login_id"], 1, i64::MAX, "login_id");
        for key in ["player_archetype_uuid", "opponent_archetype_uuid"] {
            if !body[key].is_null() {
                is_uuid(&body[key], key);
            }
        }
        nullable_text(&body["arena_user_id"], 255, "arena_user_id");
        nullable_text(&body["league_token"], usize::MAX, "league_token");
        nullable_int(&body["league_run"], i64::MIN, i64::MAX, "league_run");
        nullable_text(&body["league_client_id"], 100, "league_client_id");
        nullable_text(&body["challenge_token"], usize::MAX, "challenge_token");
        nullable_int(&body["tournament_round"], 1, 20, "tournament_round");
        nullable_text(&body["client_version"], 32, "client_version");
        nullable_text(&body["opponent_name"], 64, "opponent_name");
        for list in ["deck", "opponent_deck"] {
            let Some(cards) = body[list].as_array() else {
                assert!(list == "opponent_deck", "{list} must be present");
                continue;
            };
            for card in cards {
                assert!(card["arena_id"].is_i64(), "{list} arena_id");
                assert!(card.get("mtgo_id").is_none(), "{list} mtgo_id");
                assert!(card["quantity"].as_i64().expect("quantity") >= 1);
                assert!(["main", "side"].contains(&card["zone"].as_str().expect("zone")));
            }
        }
        for game in body["games"].as_array().expect("games") {
            let number = game["game_number"].as_i64().expect("game_number");
            assert!((1..=10).contains(&number));
            assert!(
                game.get("won")
                    .is_some_and(|w| w.is_null() || w.is_boolean())
            );
            assert!(game["on_play"].is_boolean());
            let mulligans = game["mulligan_count"].as_i64().expect("mulligan_count");
            assert!((0..=15).contains(&mulligans));
            let hand = game["starting_hand_size"]
                .as_i64()
                .expect("starting_hand_size");
            assert!((0..=10).contains(&hand));
            let kept = game["kept_hand"].as_array().expect("kept_hand present");
            assert!(kept.iter().all(|id| id.is_i64()));
            let opponent = game["opponent_mulligan_count"]
                .as_i64()
                .expect("opp mulligans");
            assert!((0..=15).contains(&opponent));
            nullable_int(&game["turn_count"], 0, 500, "turn_count");
            nullable_date(&game["started_at"], "started_at");
            nullable_date(&game["ended_at"], "ended_at");
            nullable_int(
                &game["clock_remaining_ms"],
                0,
                3_600_000,
                "clock_remaining_ms",
            );
            nullable_int(
                &game["opponent_clock_remaining_ms"],
                0,
                3_600_000,
                "opponent_clock_remaining_ms",
            );
            for key in ["timed_out", "opponent_timed_out"] {
                assert!(game[key].is_null() || game[key].is_boolean(), "{key}");
            }
        }
        let arena = body["arena"].as_object().expect("arena object");
        text(&arena["event_name"], 128, "event_name");
        assert!(!arena["event_name"].as_str().expect("event").is_empty());
        assert!(["bo1", "bo3"].contains(&arena["best_of"].as_str().expect("best_of")));
        assert!(
            ["ranked", "unranked", "limited"]
                .contains(&arena["match_type"].as_str().expect("match_type"))
        );
        nullable_int(&arena["season"], 0, i64::MAX, "season");
        // `arena.commanders` has no rule in the PHP: null or an object.
        let commanders = arena.get("commanders").unwrap_or(&Value::Null);
        assert!(commanders.is_null() || commanders.is_object());
        if let Some(rank) = arena["rank"].as_object() {
            // `required_with:arena.rank` on ladder, label and class.
            assert!(["constructed", "limited"].contains(&rank["ladder"].as_str().expect("ladder")));
            text(&rank["label"], 32, "rank label");
            text(&rank["class"], 32, "rank class");
            nullable_int(&rank["level"], 0, 255, "rank level");
            nullable_int(&rank["step"], 0, 255, "rank step");
            let percentile = &rank["mythic_percentile"];
            assert!(
                percentile.is_null()
                    || percentile
                        .as_f64()
                        .is_some_and(|n| (0.0..=100.0).contains(&n))
            );
            nullable_int(&rank["mythic_placement"], 0, i64::MAX, "mythic_placement");
            nullable_int(&rank["season"], 0, i64::MAX, "rank season");
            nullable_date(&rank["observed_at"], "rank observed_at");
        } else {
            assert!(arena["rank"].is_null(), "rank is null or an object");
        }
    }

    #[test]
    fn an_arena_match_is_enqueued_and_sent() {
        let stub = Stub::start(|_| Reply::json(201, r#"{"message":"Match reported"}"#));
        let (_dir, state, _events) = tracked("shipping-arena-sent");
        let api = api(&stub.base_url, false);
        arena_match(&state, "q-arena-1");
        let outcome = enqueue_reports(&editor(&state), &reader(&state), NOW);
        assert_eq!(outcome.queued, 1);
        let outcome = ship_reports(&api.state, &editor(&state), NOW);
        assert_eq!((outcome.claimed, outcome.sent), (1, 1));
        let seen = report_requests_seen(&stub);
        assert_eq!(seen.len(), 1);
        let body: serde_json::Value = serde_json::from_slice(&seen[0].body).expect("json");
        assert_eq!(body["platform"], "arena");
        assert_eq!(body["match_token"], "q-arena-1");
        assert_eq!(body["username"], "Qarena#12345");
        assert_eq!(body["arena"]["event_name"], "Qevent");
        assert_eq!(body["arena"]["best_of"], "bo1");
        assert_eq!(body["arena"]["match_type"], "limited");
        assert_eq!(body["league_token"], "arena:c1");
        assert!(body["league_run"].is_i64());
        assert_eq!(body["arena"]["season"], 95);
        assert_eq!(body["arena"]["rank"]["ladder"], "limited");
        assert!(body["arena"]["commanders"].is_null());
        let deck = body["deck"].as_array().expect("deck");
        assert_eq!(deck.len(), 2);
        assert!(deck.iter().all(|card| card.get("mtgo_id").is_none()));
        assert_eq!(deck[0]["arena_id"], 90001);
        assert_eq!(deck[1]["zone"], "side");
        let stored = of_kind(&state, "match_report");
        assert_eq!((stored[0].status.as_str(), stored[0].attempts), ("sent", 1));
    }

    #[test]
    fn an_arena_body_satisfies_the_api_validator() {
        let stub = Stub::start(|_| Reply::json(201, "{}"));
        let (_dir, state, _events) = tracked("shipping-arena-validator");
        let api = api(&stub.base_url, false);
        arena_match(&state, "q-arena-1");
        enqueue_reports(&editor(&state), &reader(&state), NOW);
        ship_reports(&api.state, &editor(&state), NOW);
        let body: serde_json::Value =
            serde_json::from_slice(&report_requests_seen(&stub)[0].body).expect("json");
        assert_satisfies_arena_validator(&body);
    }

    // The API answers 201 with `ignored: true` for a row another account
    // owns; the client treats it as sent.
    #[test]
    fn an_ignored_answer_counts_as_sent() {
        let stub =
            Stub::start(|_| Reply::json(201, r#"{"message":"Match reported","ignored":true}"#));
        let (_dir, state, _events) = tracked("shipping-arena-ignored");
        let api = api(&stub.base_url, false);
        arena_match(&state, "q-arena-1");
        enqueue_reports(&editor(&state), &reader(&state), NOW);
        let outcome = ship_reports(&api.state, &editor(&state), NOW);
        assert_eq!((outcome.claimed, outcome.sent), (1, 1));
        let stored = &of_kind(&state, "match_report")[0];
        assert_eq!((stored.status.as_str(), stored.attempts), ("sent", 1));
    }

    #[test]
    fn a_rejected_arena_report_does_not_hold_back_mtgo_reports() {
        let stub = Stub::start(|request| {
            let text = String::from_utf8_lossy(&request.body).into_owned();
            if text.contains(r#""platform":"arena""#) {
                Reply::json(422, r#"{"message":"invalid"}"#)
            } else {
                Reply::json(201, r#"{"message":"Match reported"}"#)
            }
        });
        let (_dir, state, _events) = tracked("shipping-arena-rejected");
        let api = api(&stub.base_url, false);
        // The Arena row is queued first, so it fails before the MTGO row.
        arena_match(&state, "q-arena-1");
        enqueue_reports(&editor(&state), &reader(&state), NOW);
        reportable_match(&state, "q-mtgo-1");
        enqueue_reports(&editor(&state), &reader(&state), NOW);
        let outcome = ship_reports(&api.state, &editor(&state), NOW);
        let order: Vec<String> = report_requests_seen(&stub)
            .iter()
            .map(|request| {
                let text = String::from_utf8_lossy(&request.body);
                if text.contains(r#""platform":"arena""#) {
                    "arena"
                } else {
                    "mtgo"
                }
                .to_string()
            })
            .collect();
        assert_eq!(order, ["arena", "mtgo"]);
        assert_eq!((outcome.claimed, outcome.sent, outcome.failed), (2, 1, 1));
        let stored = of_kind(&state, "match_report");
        let status_of = |subject: &str| {
            stored
                .iter()
                .find(|row| row.subject == subject)
                .expect("row")
        };
        assert_eq!(status_of("q-mtgo-1").status, "sent");
        let arena = status_of("q-arena-1");
        assert_eq!(arena.status, "pending");
        assert_eq!(
            arena.next_attempt_at,
            Some(NOW + mymtgo_api::REFUSED_REPORT_BACKOFF_MS)
        );
    }

    // 0.x: a 401 runs RecoverFromUnauthorized, then retries once.
    #[test]
    fn a_401_report_recovers_and_is_retried_once() {
        let reports = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&reports);
        let stub = Stub::start(move |request| {
            if request.target == "/api/devices/register" {
                return Reply::json(200, r#"{"api_key":"qnew-key"}"#);
            }
            if counted.fetch_add(1, Ordering::SeqCst) == 0 {
                Reply::json(401, "")
            } else {
                Reply::json(201, "{}")
            }
        });
        let (_dir, state, _events) = tracked("shipping-reports-401");
        let api = api(&stub.base_url, false);
        reportable_match(&state, "q-match-1");
        enqueue_reports(&editor(&state), &reader(&state), NOW - 1);
        let outcome = ship_reports(&api.state, &editor(&state), NOW);
        assert_eq!(outcome.sent, 1);
        let targets: Vec<String> = stub.requests().into_iter().map(|r| r.target).collect();
        assert_eq!(
            targets,
            vec![
                "/api/matches/report".to_string(),
                "/api/devices/register".to_string(),
                "/api/matches/report".to_string(),
            ]
        );
        assert_eq!(stub.requests()[2].header("x-api-key"), Some("qnew-key"));
        // A second 401 is not retried again: the failure path.
        let refused = Stub::start(|request| {
            if request.target == "/api/devices/register" {
                Reply::json(200, r#"{"api_key":"qnew-key"}"#)
            } else {
                Reply::json(401, "")
            }
        });
        let (_dir, state, _events) = tracked("shipping-reports-401-twice");
        let api = self::api(&refused.base_url, false);
        reportable_match(&state, "q-match-1");
        enqueue_reports(&editor(&state), &reader(&state), NOW - 1);
        assert_eq!(ship_reports(&api.state, &editor(&state), NOW).failed, 1);
        let reported = refused
            .requests()
            .iter()
            .filter(|request| request.target == "/api/matches/report")
            .count();
        assert_eq!(reported, 2);
    }

    // 0.x: any other status logs a warning and the match stays unsubmitted
    // (here: pending; a refusal waits a day, #500).
    #[test]
    fn a_refused_report_stays_pending() {
        let stub = Stub::start(|_| Reply::json(422, r#"{"message":"invalid"}"#));
        let (_dir, state, _events) = tracked("shipping-reports-422");
        let api = api(&stub.base_url, false);
        reportable_match(&state, "q-match-1");
        enqueue_reports(&editor(&state), &reader(&state), NOW - 1);
        assert_eq!(ship_reports(&api.state, &editor(&state), NOW).failed, 1);
        let row = &of_kind(&state, "match_report")[0];
        assert_eq!(row.status, "pending");
        assert_eq!(row.last_error.as_deref(), Some("HTTP 422"));
        assert_eq!(
            row.next_attempt_at,
            Some(NOW + mymtgo_api::REFUSED_REPORT_BACKOFF_MS)
        );
    }

    // Review 4 finding 1: a report whose stored body cannot be read is
    // dropped, so the next enqueue builds a fresh one from the match and it
    // is still reported (0.x builds the body at send time).
    #[test]
    fn an_unreadable_report_is_queued_again_with_a_fresh_body() {
        let stub = Stub::start(|_| Reply::json(200, "{}"));
        let (_dir, state, _events) = tracked("shipping-reports-unreadable");
        let api = api(&stub.base_url, false);
        reportable_match(&state, "q-match-1");
        enqueue_reports(&editor(&state), &reader(&state), NOW - 2);
        write(&state, "UPDATE outbox SET payload = X'00'");
        let outcome = ship_reports(&api.state, &editor(&state), NOW - 1);
        assert_eq!((outcome.unreadable, outcome.sent), (1, 0));
        assert!(stub.requests().is_empty(), "nothing sent");
        assert!(of_kind(&state, "match_report").is_empty());

        let outcome = enqueue_reports(&editor(&state), &reader(&state), NOW - 1);
        assert_eq!(outcome.queued, 1, "queued again");
        let outcome = ship_reports(&api.state, &editor(&state), NOW);
        assert_eq!(outcome.sent, 1);
        let body: serde_json::Value =
            serde_json::from_slice(&stub.requests()[0].body).expect("json");
        assert_eq!(body["match_token"], "q-match-1");
    }

    // 0.x EnqueueCardStatsTest "enqueues an eligible game exactly once" and
    // "is idempotent on re-run".
    #[test]
    fn a_game_is_queued_for_card_stats_once() {
        let (_dir, state, _events) = tracked("shipping-card-stats-enqueue");
        reportable_match(&state, "q-match-1");
        let outcome = enqueue_card_stats(&editor(&state), &reader(&state), NOW);
        assert_eq!((outcome.read, outcome.queued), (1, 1));
        let again = enqueue_card_stats(&editor(&state), &reader(&state), NOW);
        assert_eq!(again, EnqueueOutcome::default());
        let queued = of_kind(&state, "card_stats");
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].subject, "q-match-1:901");
        let payload: serde_json::Value =
            serde_json::from_slice(&zstd::decode_all(queued[0].payload.as_slice()).expect("zstd"))
                .expect("json");
        assert_eq!(payload["format"], "CMODERN");
        assert_eq!(payload["cards"][0]["oracle_id"], "q-oracle-1");
        // No archetype on either side: not read at all.
        let (_dir, state, _events) = tracked("shipping-card-stats-none");
        reportable_match(&state, "q-match-1");
        without_archetypes(&state);
        let none = enqueue_card_stats(&editor(&state), &reader(&state), NOW);
        assert_eq!(none, EnqueueOutcome::default());
    }

    // A game whose payload is skipped (here: no oracle id) is not queued and
    // is read again next run, as 0.x.
    #[test]
    fn a_skipped_card_stats_game_is_not_queued() {
        let (_dir, state, _events) = tracked("shipping-card-stats-skip");
        reportable_match(&state, "q-match-1");
        write(&state, "UPDATE cards SET oracle_id = NULL");
        let outcome = enqueue_card_stats(&editor(&state), &reader(&state), NOW);
        assert_eq!((outcome.read, outcome.queued, outcome.skipped), (1, 0, 1));
        assert!(rows(&state).is_empty());
    }

    // 0.x ShipCardStatsTest "sends gzipped body with auth headers" and
    // "chunks claimed rows into HTTP_CHUNK requests" (150 rows: 100 + 50).
    #[test]
    fn card_stats_ship_gzipped_in_chunks_of_100() {
        let stub = Stub::start(|_| Reply::json(201, ""));
        let (_dir, state, _events) = tracked("shipping-card-stats-ship");
        let api = api(&stub.base_url, false);
        reportable_match(&state, "q-match-1");
        enqueue_card_stats(&editor(&state), &reader(&state), NOW - 1);
        let payload = of_kind(&state, "card_stats")[0].payload.clone();
        let more = (0..149)
            .map(|index| store::outbox::NewOutboxRow {
                kind: OutboxKind::CardStats,
                subject: format!("q-more:{index}"),
                payload: Some(payload.clone()),
                now_ms: NOW - 1,
            })
            .collect();
        editor(&state)
            .edit(OutboxEdit::Enqueue { rows: more })
            .expect("queued");
        let outcome = ship_card_stats(&api.state, &editor(&state), NOW);
        assert_eq!((outcome.claimed, outcome.sent), (150, 150));
        let seen = stub.requests();
        assert_eq!(seen.len(), 2);
        for request in &seen {
            assert_eq!(request.target, "/api/card-stats/report");
            assert_eq!(request.header("x-device-id"), Some("test-device"));
            assert_eq!(request.header("x-api-key"), Some("test-key"));
            assert_eq!(request.header("content-encoding"), Some("gzip"));
        }
        assert_eq!(
            sent_body(&seen[0])["games"]
                .as_array()
                .expect("games")
                .len(),
            100
        );
        assert_eq!(
            sent_body(&seen[1])["games"]
                .as_array()
                .expect("games")
                .len(),
            50
        );
    }

    // Edit a match spec section 5: a match is queued and counted only once
    // its 5 minute hold is over.
    #[test]
    fn a_match_waits_5_minutes_before_it_is_queued() {
        let (_dir, state, _events) = tracked("shipping-hold");
        reportable_match(&state, "q-match-1");
        let held = ENDED + 4 * 60_000;
        assert_eq!(
            enqueue_reports(&editor(&state), &reader(&state), held).queued,
            0
        );
        assert_eq!(
            enqueue_card_stats(&editor(&state), &reader(&state), held).queued,
            0
        );
        assert_eq!(
            pending_reports(&reader(&state), true, held).map(|pending| pending.count),
            Ok(0)
        );
        let over = ENDED + store::api_reads::SEND_HOLD_MS;
        assert_eq!(
            pending_reports(&reader(&state), true, over).map(|pending| pending.count),
            Ok(1)
        );
        assert_eq!(
            enqueue_reports(&editor(&state), &reader(&state), over).queued,
            1
        );
        assert_eq!(
            enqueue_card_stats(&editor(&state), &reader(&state), over).queued,
            1
        );
    }

    // 0.x "N matches pending.": to queue plus queued and not yet sent; a
    // match without a player archetype is neither.
    #[test]
    fn pending_reports_count_the_unsent_reports() {
        let (_dir, state, _events) = tracked("shipping-reports-pending");
        reportable_match(&state, "q-match-1");
        reportable_match(&state, "q-match-2");
        write(
            &state,
            "DELETE FROM match_archetypes
             WHERE match_id = (SELECT id FROM matches WHERE token = 'q-match-2')",
        );
        assert_eq!(
            pending_reports(&reader(&state), false, NOW),
            Ok(PendingReports {
                live: false,
                count: 1
            }),
            "one to queue"
        );
        enqueue_reports(&editor(&state), &reader(&state), NOW);
        assert_eq!(
            pending_reports(&reader(&state), true, NOW),
            Ok(PendingReports {
                live: true,
                count: 1
            }),
            "one queued and not yet sent"
        );
    }
}
