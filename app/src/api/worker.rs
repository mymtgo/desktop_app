//! The `api` worker thread (07 spec section 7): one thread that enqueues,
//! claims, sends and syncs. It wakes every second, or at once on a signal,
//! asks `Schedule::due` what to run and runs it. Outbox writes go through
//! the tracker (`OutboxEditor`); requests go through `ApiState::send`.
//!
//! Like a Laravel queue worker plus `schedule:work` in one loop: `Worker`
//! is the testable part, `ApiWorker` the thread the app starts and stops.

use std::collections::{BTreeSet, VecDeque};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use mymtgo_api::{
    ApiError, ApiRequest, ApiResponse, CLAIM_LIMIT, Job, LAST_RUNS_KEY, LastRuns, REQUEST_THREW,
    RequestClass, Schedule, gate, refused_observations, status_error,
};
use store::outbox::{ClaimedRow, OutboxEdit, OutboxKind, OutboxOutcome};
use tauri::{AppHandle, Manager};
use tracker::OutboxEditor;

use crate::api::attest::attest_known_accounts;
use crate::api::session::ApiState;
use crate::api::shipping::{
    enqueue_card_stats, enqueue_observations, enqueue_reports, name_tournaments,
    requeue_device_reports, ship_card_stats, ship_observations, ship_reports,
};
use crate::api::tokens::recover_unauthorized;
use crate::commands::{Startup, ready};
use crate::worker_thread::WorkerThread;

/// How long the worker sleeps between ticks when nothing wakes it.
pub const TICK: Duration = Duration::from_secs(1);

/// How often the local accounts are read for logins to attest.
pub const ACCOUNTS_CHECK_MS: i64 = 60_000;

/// What wakes the worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerSignal {
    /// A log match became complete: a sync follows once its send hold is
    /// over (`SYNC_AFTER_COMPLETION_MS`).
    MatchCompleted,
    /// Offline mode changed: run what is due now.
    GateChanged,
    /// A sign-in finished: run what is due now.
    SignedIn,
    /// "Sync now".
    SyncNow,
    /// A tournament row appeared: ask its name.
    TournamentAdded,
    /// "Submit pending" (0.x `RunSubmitMatchesController`): queue and ship
    /// the match reports now (only while they are live).
    SubmitReports,
    Stop,
}

/// Turns claimed rows into requests, each with the subjects it carries.
pub type BuildRequests = fn(&[ClaimedRow]) -> Vec<(Vec<String>, ApiRequest)>;

/// What one `ship` did, in rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShipOutcome {
    pub claimed: u32,
    pub sent: u32,
    pub failed: u32,
    /// Left out by the builder (the stored payload cannot be read): failed
    /// for good with `PAYLOAD_UNREADABLE`, or, for a match report, dropped
    /// so the next enqueue rebuilds it (#264).
    pub unreadable: u32,
    /// Claimed but never sent (offline mode came on): back to `pending`,
    /// no attempt charged.
    pub released: u32,
}

/// Ship one kind (0.x `ShipCardStats`, `ShipTournamentObservations`):
/// claim up to `CLAIM_LIMIT` due rows, send `build`'s requests one by one
/// and mark each request's subjects: `Sent` on 2xx; on 401 recover (refresh
/// or register again), then `Failed`; on any other answer or no answer
/// `Failed` ("HTTP <status>", "request threw exception"). A refusal a
/// smaller request avoids is narrowed first (`narrow`, #498): a 413 is
/// split, an observation 422 fails only the rows it names. Offline mode
/// coming on mid-send releases every claim not yet sent. Nothing is
/// claimed while the gate would refuse community requests. A 429, or a
/// path a 429 paused, releases the rest of the claim with no attempt charged.
pub fn ship(
    api: &ApiState,
    editor: &OutboxEditor,
    kind: OutboxKind,
    build: BuildRequests,
    now_ms: i64,
) -> ShipOutcome {
    ship_with(api, editor, kind, build, now_ms, OnUnauthorized::Fail)
}

/// What a 401 does after the recovery (0.x `RecoverFromUnauthorized`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnUnauthorized {
    /// The failure path (card stats and observations: the batch backs off).
    Fail,
    /// One retry with the recovered credentials (match reports); a refusal
    /// is logged.
    RetryOnce,
}

/// `ship`, choosing what a 401 does.
pub fn ship_with(
    api: &ApiState,
    editor: &OutboxEditor,
    kind: OutboxKind,
    build: BuildRequests,
    now_ms: i64,
    on_unauthorized: OnUnauthorized,
) -> ShipOutcome {
    let mut outcome = ShipOutcome::default();
    if api.client().is_err()
        || gate(RequestClass::Community, &api.gate_now())
            .refusal()
            .is_some()
    {
        return outcome;
    }
    let claim = OutboxEdit::Claim {
        kind,
        now_ms,
        limit: CLAIM_LIMIT,
    };
    let rows = match editor.edit(claim) {
        Ok(OutboxOutcome::Claimed(rows)) => rows,
        Ok(other) => {
            tracing::warn!(?other, "an outbox claim answered something else");
            return outcome;
        }
        Err(error) => {
            tracing::warn!(%error, kind = kind.as_str(), "could not claim outbox rows");
            return outcome;
        }
    };
    if rows.is_empty() {
        return outcome;
    }
    outcome.claimed = rows.len() as u32;
    let mut unsent: BTreeSet<String> = rows.iter().map(|row| row.subject.clone()).collect();
    let requests = build(&rows);
    let unreadable = left_out(&unsent, &requests);
    if !unreadable.is_empty() {
        tracing::warn!(
            kind = kind.as_str(),
            count = unreadable.len(),
            "outbox rows whose payload cannot be read; failed, or dropped to rebuild a report"
        );
        forget(&mut unsent, &unreadable);
        let edit = OutboxEdit::Unreadable {
            kind,
            subjects: unreadable,
        };
        outcome.unreadable = mark(editor, edit);
    }
    let mut queue: VecDeque<(Vec<String>, ApiRequest)> = requests.into();
    while let Some((subjects, request)) = queue.pop_front() {
        // The bearer is the one the request carried, so a 401 never
        // refreshes a token another caller replaced meanwhile (#264).
        let (mut result, mut used) = split_bearer(api.send_with_bearer(&request, now_ms));
        if on_unauthorized == OnUnauthorized::RetryOnce
            && matches!(&result, Ok(response) if response.status == 401)
        {
            recover_unauthorized(api, used.as_deref(), now_ms);
            (result, used) = split_bearer(api.send_with_bearer(&request, now_ms));
        }
        if on_unauthorized == OnUnauthorized::RetryOnce
            && let Ok(response) = &result
            && !response.is_success()
        {
            tracing::warn!(
                status = response.status,
                kind = kind.as_str(),
                "the API refused a shipment; it waits for its next attempt"
            );
        }
        let failure = match result {
            // Rate limited (#499): this request and the rest of the claim
            // go back unsent, no attempt charged, until the pause ends.
            Ok(response) if response.status == 429 => break,
            Err(ApiError::RateLimited(_)) => break,
            Ok(response) if response.is_success() => {
                // A report the API ignored (another claim governs its player)
                // still counts as linked: sending it again changes nothing.
                let linked = used.is_some();
                outcome.sent += mark(editor, sent(kind, &subjects, now_ms, linked));
                forget(&mut unsent, &subjects);
                continue;
            }
            Ok(response) => {
                if let Some(narrowed) = narrow(kind, &subjects, &response, &rows, build) {
                    if !narrowed.refused.is_empty() {
                        let error = status_error(response.status);
                        let status = Some(response.status);
                        let edit = failed(kind, &narrowed.refused, now_ms, error, status);
                        outcome.failed += mark(editor, edit);
                        forget(&mut unsent, &narrowed.refused);
                    }
                    for again in narrowed.again.into_iter().rev() {
                        queue.push_front(again);
                    }
                    continue;
                }
                if response.status == 401 && on_unauthorized == OnUnauthorized::Fail {
                    recover_unauthorized(api, used.as_deref(), now_ms);
                }
                (status_error(response.status), Some(response.status))
            }
            Err(ApiError::Offline) => break,
            Err(error) => {
                tracing::warn!(error = %error.kind(), kind = kind.as_str(), "shipping failed");
                (REQUEST_THREW.to_string(), None)
            }
        };
        let (error, status) = failure;
        outcome.failed += mark(editor, failed(kind, &subjects, now_ms, error, status));
        forget(&mut unsent, &subjects);
    }
    if !unsent.is_empty() {
        let release = OutboxEdit::Release {
            kind,
            subjects: unsent.into_iter().collect(),
        };
        outcome.released = mark(editor, release);
    }
    outcome
}

/// A refusal a smaller request avoids: the rows the API named as refused,
/// and the requests to send in its place.
struct Narrowed {
    refused: Vec<String>,
    again: Vec<(Vec<String>, ApiRequest)>,
}

/// How to narrow a refused request (#498), or `None` to fail it whole. An
/// observation 422 that names rows (`observations.{index}`) fails those and
/// sends the rest again. A 413 (body too large), or an observation 422 that
/// names none and says the body is too large (the API's answer before it
/// had a 413), sends each half again, no attempt charged. A single row is never split.
/// Each step sends fewer rows, so it ends.
fn narrow(
    kind: OutboxKind,
    subjects: &[String],
    response: &ApiResponse,
    rows: &[ClaimedRow],
    build: BuildRequests,
) -> Option<Narrowed> {
    let observation_422 = kind == OutboxKind::TournamentObservation && response.status == 422;
    if observation_422 {
        let refused: Vec<String> = refused_observations(&response.body)
            .into_iter()
            .filter_map(|index| subjects.get(index).cloned())
            .collect();
        if !refused.is_empty() {
            let rest: Vec<String> = subjects
                .iter()
                .filter(|subject| !refused.contains(subject))
                .cloned()
                .collect();
            return Some(Narrowed {
                refused,
                again: build(&claimed_rows(rows, &rest)),
            });
        }
    }
    // Only the API's old answer to a body too large ("... larger than N
    // bytes inflated.") is fixed by splitting; a broken body is not.
    let too_large_422 =
        observation_422 && String::from_utf8_lossy(&response.body).contains("larger than");
    if (response.status != 413 && !too_large_422) || subjects.len() < 2 {
        return None;
    }
    let (first, second) = subjects.split_at(subjects.len() / 2);
    let mut again = build(&claimed_rows(rows, first));
    again.extend(build(&claimed_rows(rows, second)));
    Some(Narrowed {
        refused: Vec::new(),
        again,
    })
}

/// The claimed rows for `subjects`, in claim order.
fn claimed_rows(rows: &[ClaimedRow], subjects: &[String]) -> Vec<ClaimedRow> {
    rows.iter()
        .filter(|row| subjects.contains(&row.subject))
        .cloned()
        .collect()
}

/// A send's answer and, beside it, the bearer it carried (none when it
/// went nowhere).
fn split_bearer(
    sent: Result<(ApiResponse, Option<String>), ApiError>,
) -> (Result<ApiResponse, ApiError>, Option<String>) {
    match sent {
        Ok((response, bearer)) => (Ok(response), bearer),
        Err(error) => (Err(error), None),
    }
}

fn sent(kind: OutboxKind, subjects: &[String], now_ms: i64, linked: bool) -> OutboxEdit {
    OutboxEdit::Sent {
        kind,
        subjects: subjects.to_vec(),
        now_ms,
        linked,
    }
}

fn failed(
    kind: OutboxKind,
    subjects: &[String],
    now_ms: i64,
    error: String,
    status: Option<u16>,
) -> OutboxEdit {
    OutboxEdit::Failed {
        kind,
        subjects: subjects.to_vec(),
        now_ms,
        error,
        status,
    }
}

/// The claimed subjects no request carries: the builders leave out only
/// rows whose stored payload cannot be read.
fn left_out(claimed: &BTreeSet<String>, requests: &[(Vec<String>, ApiRequest)]) -> Vec<String> {
    let mut built: BTreeSet<&String> = BTreeSet::new();
    for (subjects, _) in requests {
        built.extend(subjects);
    }
    claimed
        .iter()
        .filter(|subject| !built.contains(subject))
        .cloned()
        .collect()
}

fn forget(unsent: &mut BTreeSet<String>, subjects: &[String]) {
    for subject in subjects {
        unsent.remove(subject);
    }
}

/// Apply a mark and say how many rows it changed. A mark that does not
/// reach the tracker leaves its rows `sending`; the reaper returns them.
fn mark(editor: &OutboxEditor, edit: OutboxEdit) -> u32 {
    match editor.edit(edit) {
        Ok(OutboxOutcome::Updated(count)) => count,
        Ok(other) => {
            tracing::warn!(?other, "an outbox mark answered something else");
            0
        }
        Err(error) => {
            tracing::warn!(%error, "could not mark outbox rows; the reaper returns them");
            0
        }
    }
}

/// Stores settings rows through the tracker.
pub type SaveSettings<'a> = dyn Fn(Vec<(String, String)>) + 'a;

/// A sync run. Given whether a failure to reach MyMTGO is stored for the
/// Account page (the run was asked for, or the retries were used), it answers whether the run could
/// not reach MyMTGO.
pub type SyncRun<'a> = dyn Fn(bool) -> bool + 'a;

/// The worker's state between ticks.
pub struct Worker<'a> {
    api: &'a ApiState,
    /// A fresh editor per job; `None` while tracking restarts.
    editor: &'a dyn Fn() -> Option<OutboxEditor>,
    /// A read-only connection for the jobs that read (opened per job);
    /// `None` when the database cannot be opened.
    reader: &'a dyn Fn() -> Option<store::Connection>,
    schedule: Schedule,
    runs: LastRuns,
    accounts_checked_at: Option<i64>,
    /// The sync run (`sync_runner::run_app_sync` in the app); `None` runs
    /// nothing.
    sync: Option<&'a SyncRun<'a>>,
    /// Stores settings rows (the persisted last runs, #504); `None` keeps
    /// them in memory only.
    save_settings: Option<&'a SaveSettings<'a>>,
    /// The stored last runs were read back.
    restored: bool,
}

impl<'a> Worker<'a> {
    pub fn new(
        api: &'a ApiState,
        editor: &'a dyn Fn() -> Option<OutboxEditor>,
        reader: &'a dyn Fn() -> Option<store::Connection>,
    ) -> Worker<'a> {
        Worker {
            api,
            editor,
            reader,
            schedule: Schedule::default(),
            runs: LastRuns::default(),
            accounts_checked_at: None,
            sync: None,
            save_settings: None,
            restored: false,
        }
    }

    /// The worker storing the 30-minute jobs' last runs with `save`, and
    /// reading them back at its first tick, so a restart does not run them
    /// again at once (#504).
    pub fn with_saved_runs(mut self, save: &'a SaveSettings<'a>) -> Worker<'a> {
        self.save_settings = Some(save);
        self
    }

    /// The worker with this sync run for `Job::Sync`.
    pub fn with_sync(mut self, sync: &'a SyncRun<'a>) -> Worker<'a> {
        self.sync = Some(sync);
        self
    }

    /// Take note of a signal; the next tick runs what it made due.
    pub fn note(&mut self, signal: WorkerSignal, now_ms: i64) {
        match signal {
            WorkerSignal::MatchCompleted => {
                let gate_state = self.api.gate_now();
                self.runs.match_completed(now_ms, &gate_state);
            }
            WorkerSignal::SyncNow => self.runs.sync_now(now_ms),
            WorkerSignal::TournamentAdded => self.runs.tournament_added(),
            WorkerSignal::SubmitReports => self.runs.submit_reports(),
            // Only a wake: the tick reads the gate again.
            WorkerSignal::GateChanged | WorkerSignal::SignedIn | WorkerSignal::Stop => {}
        }
    }

    /// Run every due job, each with a fresh editor, and return the jobs
    /// run. While tracking restarts (no editor) the rest of the tick waits.
    pub fn tick(&mut self, now_ms: i64) -> Vec<Job> {
        self.restore_runs(now_ms);
        let gate_state = self.api.gate_now();
        self.check_accounts(now_ms, &gate_state);
        let mut ran = Vec::new();
        for job in self.schedule.due(now_ms, &self.runs, &gate_state) {
            let Some(editor) = (self.editor)() else {
                tracing::debug!("tracking is restarting; the API jobs wait");
                break;
            };
            let unreachable = self.run_job(job, &editor, now_ms);
            if job == Job::Sync {
                self.runs.record_sync(now_ms, unreachable);
            } else {
                self.runs.record(job, now_ms);
            }
            if job.persisted_name().is_some()
                && let Some(save) = self.save_settings
            {
                save(vec![self.runs.persisted_row()]);
            }
            ran.push(job);
        }
        ran
    }

    /// Read the stored last runs back once, when they are stored at all
    /// and the database can be read.
    fn restore_runs(&mut self, now_ms: i64) {
        if self.restored || self.save_settings.is_none() {
            return;
        }
        let Some(conn) = (self.reader)() else {
            return;
        };
        self.restored = true;
        match store::read::settings(&conn) {
            Ok(rows) => {
                if let Some((_, value)) = rows.iter().find(|(key, _)| key == LAST_RUNS_KEY) {
                    self.runs.restore(value, now_ms);
                }
            }
            Err(error) => tracing::warn!(%error, "could not read the API jobs' last runs"),
        }
    }

    /// Once a minute, while attestation could be sent, read the local
    /// accounts: a newly learned login asks for an attestation.
    fn check_accounts(&mut self, now_ms: i64, gate_state: &mymtgo_api::GateState) {
        if gate(RequestClass::Sync, gate_state).refusal().is_some() {
            return;
        }
        if self
            .accounts_checked_at
            .is_some_and(|at| now_ms - at < ACCOUNTS_CHECK_MS)
        {
            return;
        }
        self.accounts_checked_at = Some(now_ms);
        self.runs.accounts_seen(&self.api.accounts());
    }

    /// Run one job; true when it could not reach MyMTGO (only the sync
    /// says, for its retry).
    fn run_job(&self, job: Job, editor: &OutboxEditor, now_ms: i64) -> bool {
        match job {
            Job::Reap => {
                let count = mark(editor, OutboxEdit::Reap { now_ms });
                if count > 0 {
                    tracing::warn!(count, "returned outbox claims of a worker that stopped");
                }
            }
            Job::Attest => attest_known_accounts(self.api, now_ms),
            Job::EnqueueObservations => {
                if let Some(reader) = self.read() {
                    enqueue_observations(editor, &reader, now_ms);
                }
            }
            Job::ShipObservations => {
                ship_observations(self.api, editor, now_ms);
            }
            Job::TournamentNames => {
                if let Some(reader) = self.read() {
                    name_tournaments(self.api, editor, &reader, now_ms);
                }
            }
            // `Schedule` runs these only while `MATCH_REPORTS_LIVE` and
            // `CARD_STATS_LIVE` (both on as shipped) are on; the archetypes
            // they need are the stored `match_archetypes` rows.
            Job::EnqueueReports => {
                requeue_device_reports(self.api, editor);
                if let Some(reader) = self.read() {
                    enqueue_reports(editor, &reader, now_ms);
                }
            }
            Job::ShipReports => {
                ship_reports(self.api, editor, now_ms);
            }
            Job::EnqueueCardStats => {
                if let Some(reader) = self.read() {
                    enqueue_card_stats(editor, &reader, now_ms);
                }
            }
            Job::ShipCardStats => {
                ship_card_stats(self.api, editor, now_ms);
            }
            // One run at a time (`SyncRunState`); a trigger noted during it
            // makes the schedule due again for the next tick.
            Job::Sync => {
                if let Some(sync) = self.sync {
                    return sync(self.runs.sync_reports_unreachable(now_ms));
                }
            }
        }
        false
    }

    /// A read-only connection, or `None` (logged) when it cannot be opened.
    fn read(&self) -> Option<store::Connection> {
        let reader = (self.reader)();
        if reader.is_none() {
            tracing::warn!("the API worker could not open the database to read");
        }
        reader
    }
}

/// The worker's loop: wait up to `TICK` for a signal, note it, tick. Ends
/// on `Stop` or when every sender is gone.
pub fn run_worker(signals: &Receiver<WorkerSignal>, worker: &mut Worker, now: &dyn Fn() -> i64) {
    loop {
        match signals.recv_timeout(TICK) {
            Ok(WorkerSignal::Stop) | Err(RecvTimeoutError::Disconnected) => return,
            Ok(signal) => worker.note(signal, now()),
            Err(RecvTimeoutError::Timeout) => {}
        }
        worker.tick(now());
    }
}

/// The running `api` thread, managed by Tauri.
pub struct ApiWorker {
    thread: WorkerThread<WorkerSignal>,
}

impl ApiWorker {
    /// Start the thread. It reads `ApiState` and `Startup` from the app, so
    /// both are managed first.
    pub fn start(app: AppHandle) -> ApiWorker {
        let thread = WorkerThread::spawn(
            "api",
            "could not start the API worker; nothing is shipped",
            move |inbox| {
                let api = app.state::<ApiState>();
                let editor = || outbox_editor(&app);
                let reader = || database_reader(&app);
                let sync = |report_unreachable| {
                    crate::api::sync_runner::run_app_sync(&app, false, report_unreachable)
                };
                let save = |rows| save_settings(&app, rows);
                let mut worker = Worker::new(&api, &editor, &reader)
                    .with_sync(&sync)
                    .with_saved_runs(&save);
                run_worker(&inbox, &mut worker, &crate::wall_clock_ms);
            },
        );
        ApiWorker { thread }
    }

    /// Wake the worker with a signal. Returns false once stopped.
    pub fn signal(&self, signal: WorkerSignal) -> bool {
        self.thread.send(signal)
    }

    /// Stop the worker, waiting up to `STOP_WAIT` for it (a request in
    /// flight can take 30 seconds; a claim it leaves is reaped next run).
    pub fn stop(&self) {
        self.thread.stop(
            WorkerSignal::Stop,
            "the API worker is still sending; leaving it",
        );
    }
}

/// Wake the app's worker, if it runs.
pub fn signal(app: &AppHandle, signal: WorkerSignal) {
    if let Some(worker) = app.try_state::<ApiWorker>() {
        worker.signal(signal);
    }
}

/// A read-only connection to the app's database; `None` when startup failed
/// or it cannot be opened.
fn database_reader(app: &AppHandle) -> Option<store::Connection> {
    let startup = app.try_state::<Startup>()?;
    let state = ready(&startup).ok()?;
    store::open_reader(&state.db_path).ok()
}

/// Store settings rows through the running tracker; logged when it cannot
/// (the rows are only a hint: the jobs run at start without them).
fn save_settings(app: &AppHandle, rows: Vec<(String, String)>) {
    let Some(startup) = app.try_state::<Startup>() else {
        return;
    };
    let Ok(state) = ready(&startup) else {
        return;
    };
    let Ok(saver) = state.tracker_part(tracker::TrackerHandle::settings_saver) else {
        return;
    };
    if let Err(error) = saver.save(rows) {
        tracing::warn!(%error, "could not store the API jobs' last runs");
    }
}

/// A fresh editor from the running tracker; `None` while it restarts or
/// when startup failed.
fn outbox_editor(app: &AppHandle) -> Option<OutboxEditor> {
    let startup = app.try_state::<Startup>()?;
    let state = ready(&startup).ok()?;
    state
        .tracker_part(tracker::TrackerHandle::outbox_editor)
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::secrets::{ACCESS_TOKEN, REFRESH_TOKEN};
    use crate::api::session::testing::{TestApi, test_api};
    use crate::api::stub::{Reply, Stub};
    use crate::bridge::lock;
    use crate::commands::AppState;
    use crate::commands::tests::ready_state;
    use fixture_tools::TestDir;
    use mymtgo_api::{AuthMode, Body, KnownAccount, Method, OFFLINE_MODE, RequestClass};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::sync::{Arc, Barrier};
    use store::outbox::{NewOutboxRow, OutboxRowState, REAP_AFTER_MS};
    use tracker::TrackerEvent;

    const NOW: i64 = 1_800_000_000_000;
    const DAY: i64 = 24 * 3_600_000;
    const OBS: OutboxKind = OutboxKind::TournamentObservation;
    const PATH: &str = "/api/q-ship";

    /// A device with a fresh key; offline when asked.
    fn api(base_url: &str, offline: bool) -> TestApi {
        let key_valid = (NOW + DAY).to_string();
        let offline_row = offline.to_string();
        let rows = [
            ("api_device_id", r#""qdevice""#),
            ("api_key_expires_at", key_valid.as_str()),
            (OFFLINE_MODE, offline_row.as_str()),
        ];
        test_api(base_url, &rows, &[("api_key", "qdevice-key")])
    }

    /// One community request per 100 rows (0.x `HTTP_CHUNK`), the payloads
    /// joined by newlines.
    fn per_hundred(rows: &[ClaimedRow]) -> Vec<(Vec<String>, ApiRequest)> {
        let mut requests = Vec::new();
        for chunk in rows.chunks(100) {
            let mut request = ApiRequest::new(
                Method::Post,
                PATH,
                RequestClass::Community,
                AuthMode::DeviceOrBearer,
            );
            let mut body = Vec::new();
            for row in chunk {
                body.extend_from_slice(&row.payload);
                body.push(b'\n');
            }
            request.body = Body::Json(body);
            let subjects = chunk.iter().map(|row| row.subject.clone()).collect();
            requests.push((subjects, request));
        }
        requests
    }

    fn tracked(dir: &TestDir) -> (AppState, mpsc::Receiver<TrackerEvent>) {
        ready_state(dir)
    }

    fn editor(state: &AppState) -> OutboxEditor {
        lock(&state.tracker)
            .as_ref()
            .expect("tracker")
            .outbox_editor()
    }

    fn enqueue(state: &AppState, kind: OutboxKind, count: usize) {
        let rows = (0..count)
            .map(|index| NewOutboxRow {
                kind,
                subject: format!("1:{index:04}"),
                payload: Some(format!("Qpayload {index}").into_bytes()),
                now_ms: NOW - 1_000 + index as i64,
            })
            .collect();
        let outcome = editor(state).edit(OutboxEdit::Enqueue { rows });
        assert_eq!(outcome, Ok(OutboxOutcome::Enqueued(count as u32)));
    }

    fn rows(state: &AppState) -> Vec<OutboxRowState> {
        store::outbox::rows_for_test(&store::open_reader(&state.db_path).expect("reader"))
            .expect("rows")
    }

    fn count(state: &AppState, status: &str) -> usize {
        rows(state)
            .iter()
            .filter(|row| row.status == status)
            .count()
    }

    fn write(state: &AppState, sql: &str) {
        store::open_writer(&state.db_path)
            .expect("writer")
            .execute_batch(sql)
            .expect("write");
    }

    fn ship_now(api: &TestApi, state: &AppState, kind: OutboxKind) -> ShipOutcome {
        ship(&api.state, &editor(state), kind, per_hundred, NOW)
    }

    // 0.x ShipCardStatsTest "marks queue rows sent on 2xx response" and
    // ShipTournamentObservationsTest "marks observations as sent on 204".
    #[test]
    fn marks_rows_sent_on_2xx_response() {
        for (kind, status) in [(OutboxKind::CardStats, 201), (OBS, 204)] {
            let stub = Stub::start(move |_| Reply::json(status, ""));
            let dir = TestDir::new("worker-ship-sent");
            let (state, _events) = tracked(&dir);
            let api = api(&stub.base_url, false);
            enqueue(&state, kind, 1);
            let outcome = ship_now(&api, &state, kind);
            assert_eq!(
                outcome,
                ShipOutcome {
                    claimed: 1,
                    sent: 1,
                    ..ShipOutcome::default()
                }
            );
            let row = &rows(&state)[0];
            assert_eq!(row.status, "sent", "{kind:?}");
            assert_eq!(row.attempts, 1);
            assert!(row.payload.is_empty());
            let seen = stub.requests();
            assert_eq!(seen.len(), 1);
            assert_eq!(seen[0].target, PATH);
            assert_eq!(seen[0].body, b"Qpayload 0\n");
            assert_eq!(seen[0].header("x-api-key"), Some("qdevice-key"));
            assert_eq!(seen[0].header("x-device-id"), Some("qdevice"));
        }
    }

    /// A stub answering `first` to the first request and 204 after.
    fn first_then_204(first: Reply) -> Stub {
        let calls = Arc::new(AtomicUsize::new(0));
        let first = Mutex::new(Some(first));
        Stub::start(move |_| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0
                && let Some(reply) = lock(&first).take()
            {
                return reply;
            }
            Reply::json(204, "")
        })
    }

    fn statuses(state: &AppState) -> Vec<(String, String, i64)> {
        rows(state)
            .into_iter()
            .map(|row| (row.subject, row.status, row.attempts))
            .collect()
    }

    // #498: a 413 (body too large) halves the request and sends each half,
    // with no attempt charged for the refusal.
    #[test]
    fn a_413_splits_the_request_and_sends_each_half() {
        let stub = first_then_204(Reply::json(413, r#"{"message":"Split the batch"}"#));
        let dir = TestDir::new("worker-ship-413");
        let (state, _events) = tracked(&dir);
        let api = api(&stub.base_url, false);
        enqueue(&state, OutboxKind::CardStats, 4);
        let outcome = ship_now(&api, &state, OutboxKind::CardStats);
        assert_eq!((outcome.sent, outcome.failed), (4, 0));
        let bodies: Vec<Vec<u8>> = stub.requests().into_iter().map(|seen| seen.body).collect();
        assert_eq!(bodies.len(), 3);
        assert_eq!(bodies[1], b"Qpayload 0\nQpayload 1\n");
        assert_eq!(bodies[2], b"Qpayload 2\nQpayload 3\n");
        assert!(rows(&state).iter().all(|row| row.status == "sent"));
    }

    // A single row too large for the API cannot be split: it fails.
    #[test]
    fn a_413_for_one_row_counts_a_failure() {
        let stub = Stub::start(|_| Reply::json(413, ""));
        let dir = TestDir::new("worker-ship-413-one");
        let (state, _events) = tracked(&dir);
        let api = api(&stub.base_url, false);
        enqueue(&state, OBS, 1);
        let outcome = ship_now(&api, &state, OBS);
        assert_eq!(outcome.failed, 1);
        assert_eq!(stub.requests().len(), 1);
        let row = &rows(&state)[0];
        assert_eq!((row.status.as_str(), row.attempts), ("pending", 1));
        assert_eq!(row.last_error.as_deref(), Some("HTTP 413"));
    }

    // #498: an observation 422 that names a row fails that row only; the
    // rest go again at once.
    #[test]
    fn an_observation_422_fails_only_the_rows_it_names() {
        let refusal = r#"{"message":"bad","errors":{"observations.1.event_type":["The selected event type is invalid."]}}"#;
        let stub = first_then_204(Reply::json(422, refusal));
        let dir = TestDir::new("worker-ship-422-named");
        let (state, _events) = tracked(&dir);
        let api = api(&stub.base_url, false);
        enqueue(&state, OBS, 4);
        let outcome = ship_now(&api, &state, OBS);
        assert_eq!((outcome.sent, outcome.failed), (3, 1));
        let seen = stub.requests();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[1].body, b"Qpayload 0\nQpayload 2\nQpayload 3\n");
        assert_eq!(
            statuses(&state),
            vec![
                ("1:0000".to_string(), "sent".to_string(), 1),
                ("1:0001".to_string(), "pending".to_string(), 1),
                ("1:0002".to_string(), "sent".to_string(), 1),
                ("1:0003".to_string(), "sent".to_string(), 1),
            ]
        );
    }

    // An observation 422 that names no row (the API's older answer to a
    // body too large) splits as a 413 does.
    #[test]
    fn an_observation_422_naming_no_row_splits() {
        let refusal = r#"{"message":"Invalid gzip body, or larger than 8388608 bytes inflated.","errors":{"body":"x"}}"#;
        let stub = first_then_204(Reply::json(422, refusal));
        let dir = TestDir::new("worker-ship-422-body");
        let (state, _events) = tracked(&dir);
        let api = api(&stub.base_url, false);
        enqueue(&state, OBS, 2);
        let outcome = ship_now(&api, &state, OBS);
        assert_eq!((outcome.sent, outcome.failed), (2, 0));
        assert_eq!(stub.requests().len(), 3);
    }

    // An observation 422 that names no row and is not about size (a broken
    // body) cannot be fixed by splitting: it fails the request whole.
    #[test]
    fn an_observation_422_about_the_body_fails_whole() {
        let refusal = r#"{"message":"Invalid gzip body.","errors":{"body":"Invalid gzip body."}}"#;
        let stub = Stub::start(move |_| Reply::json(422, refusal));
        let dir = TestDir::new("worker-ship-422-broken");
        let (state, _events) = tracked(&dir);
        let api = api(&stub.base_url, false);
        enqueue(&state, OBS, 4);
        let outcome = ship_now(&api, &state, OBS);
        assert_eq!(outcome.failed, 4);
        assert_eq!(stub.requests().len(), 1);
    }

    // Other kinds keep a 422 as a failure of the whole request.
    #[test]
    fn a_card_stats_422_fails_the_whole_request() {
        let stub = Stub::start(|_| Reply::json(422, r#"{"errors":{"games.0":["x"]}}"#));
        let dir = TestDir::new("worker-ship-422-stats");
        let (state, _events) = tracked(&dir);
        let api = api(&stub.base_url, false);
        enqueue(&state, OutboxKind::CardStats, 2);
        let outcome = ship_now(&api, &state, OutboxKind::CardStats);
        assert_eq!(outcome.failed, 2);
        assert_eq!(stub.requests().len(), 1);
    }

    // #499: a 429 releases the claim with no attempt charged and pauses the
    // path for its Retry-After: the next ship sends nothing until then.
    #[test]
    fn a_429_releases_the_claim_and_pauses_the_path() {
        let stub = Stub::start(|_| Reply::Respond {
            status: 429,
            headers: vec![("Retry-After".to_string(), "120".to_string())],
            body: br#"{"message":"Too Many Attempts."}"#.to_vec(),
        });
        let dir = TestDir::new("worker-ship-429");
        let (state, _events) = tracked(&dir);
        let api = api(&stub.base_url, false);
        enqueue(&state, OutboxKind::CardStats, 150);
        let outcome = ship_now(&api, &state, OutboxKind::CardStats);
        assert_eq!(
            outcome,
            ShipOutcome {
                claimed: 150,
                released: 150,
                ..ShipOutcome::default()
            }
        );
        assert_eq!(stub.requests().len(), 1);
        assert!(
            rows(&state)
                .iter()
                .all(|row| row.status == "pending" && row.attempts == 0)
        );
        let later = ship(
            &api.state,
            &editor(&state),
            OutboxKind::CardStats,
            per_hundred,
            NOW + 119_000,
        );
        assert_eq!(later.released, 150);
        assert_eq!(stub.requests().len(), 1, "paused: nothing sent");
        let after = ship(
            &api.state,
            &editor(&state),
            OutboxKind::CardStats,
            per_hundred,
            NOW + 120_000,
        );
        assert_eq!(after.released, 150);
        assert_eq!(stub.requests().len(), 2, "the pause is over");
    }

    // 0.x "flips failed rows back to pending with exponential backoff".
    #[test]
    fn flips_failed_rows_back_to_pending_with_exponential_backoff() {
        let stub = Stub::start(|_| Reply::json(500, "server error"));
        let dir = TestDir::new("worker-ship-backoff");
        let (state, _events) = tracked(&dir);
        let api = api(&stub.base_url, false);
        enqueue(&state, OBS, 1);
        let outcome = ship_now(&api, &state, OBS);
        assert_eq!(outcome.failed, 1);
        let row = &rows(&state)[0];
        assert_eq!(row.status, "pending");
        assert_eq!(row.attempts, 1);
        assert_eq!(row.next_attempt_at, Some(NOW + 10_000));
        assert!(
            row.last_error
                .as_deref()
                .is_some_and(|error| error.contains("500"))
        );
    }

    // 0.x "marks rows failed after MAX_ATTEMPTS".
    #[test]
    fn marks_rows_failed_after_max_attempts() {
        let stub = Stub::start(|_| Reply::json(500, "boom"));
        let dir = TestDir::new("worker-ship-max");
        let (state, _events) = tracked(&dir);
        let api = api(&stub.base_url, false);
        enqueue(&state, OutboxKind::CardStats, 1);
        write(
            &state,
            &format!(
                "UPDATE outbox SET attempts = 19, next_attempt_at = {}",
                NOW - 60_000
            ),
        );
        ship_now(&api, &state, OutboxKind::CardStats);
        let row = &rows(&state)[0];
        assert_eq!(row.status, "failed");
        assert_eq!(row.attempts, 20);
    }

    // 0.x "skips rows whose next_attempt_at is in the future".
    #[test]
    fn skips_rows_whose_next_attempt_at_is_in_the_future() {
        let stub = Stub::start(|_| Reply::json(201, ""));
        let dir = TestDir::new("worker-ship-future");
        let (state, _events) = tracked(&dir);
        let api = api(&stub.base_url, false);
        enqueue(&state, OBS, 1);
        write(
            &state,
            &format!("UPDATE outbox SET next_attempt_at = {}", NOW + 5 * 60_000),
        );
        assert_eq!(ship_now(&api, &state, OBS), ShipOutcome::default());
        let row = &rows(&state)[0];
        assert_eq!((row.status.as_str(), row.attempts), ("pending", 0));
        assert_eq!(stub.connections(), 0);
    }

    // 0.x ShipCardStatsTest "isolates per-chunk failures so successful
    // chunks remain sent".
    #[test]
    fn isolates_per_chunk_failures_so_successful_chunks_remain_sent() {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&calls);
        let stub = Stub::start(move |_| {
            if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                Reply::json(201, "")
            } else {
                Reply::json(500, "boom")
            }
        });
        let dir = TestDir::new("worker-ship-chunks");
        let (state, _events) = tracked(&dir);
        let api = api(&stub.base_url, false);
        enqueue(&state, OutboxKind::CardStats, 150);
        let outcome = ship_now(&api, &state, OutboxKind::CardStats);
        assert_eq!(
            outcome,
            ShipOutcome {
                claimed: 150,
                sent: 100,
                failed: 50,
                unreadable: 0,
                released: 0,
            }
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(count(&state, "sent"), 100);
        assert_eq!(count(&state, "pending"), 50);
    }

    // 0.x CallSiteGatingTest: "does not transmit card stats while offline,
    // and leaves the row claimable" and the same for observations.
    #[test]
    fn does_not_transmit_while_offline_and_leaves_the_row_claimable() {
        for kind in [OutboxKind::CardStats, OBS] {
            let stub = Stub::start(|_| Reply::json(201, ""));
            let dir = TestDir::new("worker-ship-offline");
            let (state, _events) = tracked(&dir);
            let api = api(&stub.base_url, true);
            enqueue(&state, kind, 1);
            assert_eq!(ship_now(&api, &state, kind), ShipOutcome::default());
            std::thread::sleep(Duration::from_millis(50));
            assert_eq!(stub.connections(), 0, "{kind:?}");
            let row = &rows(&state)[0];
            assert_eq!((row.status.as_str(), row.attempts), ("pending", 0));
        }
    }

    // 0.x ShipCardStats: offline mode switched on between two chunks
    // releases every claim not yet sent, charging nothing.
    #[test]
    fn offline_mid_send_releases_the_unsent_claims() {
        let arrived = Arc::new(Barrier::new(2));
        let switched = Arc::new(Barrier::new(2));
        let (in_handler, out_handler) = (Arc::clone(&arrived), Arc::clone(&switched));
        let stub = Stub::start(move |_| {
            in_handler.wait();
            out_handler.wait();
            Reply::json(201, "")
        });
        let dir = TestDir::new("worker-ship-mid-send");
        let (state, _events) = tracked(&dir);
        let api = api(&stub.base_url, false);
        enqueue(&state, OutboxKind::CardStats, 150);
        let outcome = std::thread::scope(|scope| {
            let shipping = scope.spawn(|| ship_now(&api, &state, OutboxKind::CardStats));
            arrived.wait();
            api.state.set_offline_mode(true, None);
            switched.wait();
            shipping.join().expect("ship")
        });
        assert_eq!(
            outcome,
            ShipOutcome {
                claimed: 150,
                sent: 100,
                failed: 0,
                unreadable: 0,
                released: 50,
            }
        );
        assert_eq!(stub.requests().len(), 1);
        assert_eq!(count(&state, "sent"), 100);
        assert!(
            rows(&state)
                .iter()
                .filter(|row| row.status == "pending")
                .all(|row| row.attempts == 0 && row.claimed_at.is_none()),
        );
        assert_eq!(count(&state, "pending"), 50);
    }

    // 0.x: a 401 runs RecoverFromUnauthorized, then the failure path.
    #[test]
    fn a_401_recovers_then_counts_a_failure() {
        let stub = Stub::start(|request| {
            if request.target == "/api/devices/register" {
                Reply::json(200, r#"{"api_key":"qnew-key"}"#)
            } else {
                Reply::json(401, "")
            }
        });
        let dir = TestDir::new("worker-ship-401");
        let (state, _events) = tracked(&dir);
        let api = api(&stub.base_url, false);
        enqueue(&state, OBS, 1);
        let outcome = ship_now(&api, &state, OBS);
        assert_eq!(outcome.failed, 1);
        let targets: Vec<String> = stub.requests().into_iter().map(|r| r.target).collect();
        assert_eq!(
            targets,
            vec![PATH.to_string(), "/api/devices/register".to_string()]
        );
        let row = &rows(&state)[0];
        assert_eq!(row.status, "pending");
        assert_eq!(row.attempts, 1);
        assert_eq!(row.last_error.as_deref(), Some("HTTP 401"));
    }

    /// A stub whose first `PATH` request with `Bearer qaccess` waits for the
    /// test to swap the token, then answers 401; the rest answer 201. It
    /// counts token refreshes.
    fn held_401_stub(
        arrived: mpsc::Sender<()>,
        go: mpsc::Receiver<()>,
        refreshes: Arc<AtomicUsize>,
    ) -> Stub {
        let arrived = Mutex::new(arrived);
        let go = Mutex::new(go);
        Stub::start(move |request| {
            match (request.target.as_str(), request.header("authorization")) {
                ("/oauth/token", _) => {
                    refreshes.fetch_add(1, Ordering::SeqCst);
                    Reply::json(
                        200,
                        r#"{"token_type":"Bearer","access_token":"qwrong","refresh_token":"qwrong","expires_in":2592000}"#,
                    )
                }
                (PATH, Some("Bearer qaccess")) => {
                    let _ = lock(&arrived).send(());
                    let _ = lock(&go).recv();
                    Reply::json(401, "")
                }
                _ => Reply::json(201, ""),
            }
        })
    }

    // #264 (review 4 finding 3): the 401 recovery weighs the bearer the
    // request carried. Another caller swaps the token while the request is
    // out: no refresh of its own, and a report's retry carries the new one.
    #[test]
    fn a_401_for_a_token_replaced_meanwhile_does_not_refresh_again() {
        for on_unauthorized in [OnUnauthorized::Fail, OnUnauthorized::RetryOnce] {
            let (arrived_tx, arrived_rx) = mpsc::channel();
            let (go_tx, go_rx) = mpsc::channel();
            let refreshes = Arc::new(AtomicUsize::new(0));
            let stub = held_401_stub(arrived_tx, go_rx, Arc::clone(&refreshes));
            let dir = TestDir::new("worker-ship-401-swapped");
            let (state, _events) = tracked(&dir);
            let api = linked(&stub.base_url);
            enqueue(&state, OBS, 1);
            let outcome = std::thread::scope(|scope| {
                let shipping = scope.spawn(|| {
                    ship_with(
                        &api.state,
                        &editor(&state),
                        OBS,
                        per_hundred,
                        NOW,
                        on_unauthorized,
                    )
                });
                arrived_rx.recv().expect("the request is out");
                api.state.set_secret(ACCESS_TOKEN, Some("qother"));
                go_tx.send(()).expect("answer it");
                shipping.join().expect("ship")
            });
            assert_eq!(
                refreshes.load(Ordering::SeqCst),
                0,
                "{on_unauthorized:?}: no refresh of its own"
            );
            let bearers: Vec<Option<String>> = stub
                .requests()
                .iter()
                .map(|request| request.header("authorization").map(str::to_string))
                .collect();
            match on_unauthorized {
                OnUnauthorized::Fail => {
                    assert_eq!(outcome.failed, 1);
                    assert_eq!(bearers, [Some("Bearer qaccess".to_string())]);
                }
                OnUnauthorized::RetryOnce => {
                    assert_eq!(outcome.sent, 1);
                    assert_eq!(
                        bearers,
                        [
                            Some("Bearer qaccess".to_string()),
                            Some("Bearer qother".to_string())
                        ]
                    );
                }
            }
        }
    }

    #[test]
    fn a_request_without_an_answer_is_request_threw_exception() {
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.local_addr().expect("address").port()
        };
        let dir = TestDir::new("worker-ship-threw");
        let (state, _events) = tracked(&dir);
        let api = api(&format!("http://127.0.0.1:{port}"), false);
        enqueue(&state, OBS, 1);
        assert_eq!(ship_now(&api, &state, OBS).failed, 1);
        let row = &rows(&state)[0];
        assert_eq!(row.last_error.as_deref(), Some("request threw exception"));
        assert_eq!(row.attempts, 1);
    }

    // #264 (Q7 M1, Q8 M3): a row the builder leaves out has a payload that
    // cannot be read; it fails for good with the reason instead of going
    // back to pending forever.
    #[test]
    fn rows_a_builder_leaves_out_fail_as_unreadable() {
        fn first_only(rows: &[ClaimedRow]) -> Vec<(Vec<String>, ApiRequest)> {
            let mut requests = per_hundred(&rows[..1]);
            requests.truncate(1);
            requests
        }
        let stub = Stub::start(|_| Reply::json(201, ""));
        let dir = TestDir::new("worker-ship-left-out");
        let (state, _events) = tracked(&dir);
        let api = api(&stub.base_url, false);
        enqueue(&state, OBS, 3);
        let outcome = ship(&api.state, &editor(&state), OBS, first_only, NOW);
        assert_eq!(
            (outcome.sent, outcome.unreadable, outcome.released),
            (1, 2, 0)
        );
        assert_eq!(count(&state, "sending"), 0);
        assert_eq!(count(&state, "failed"), 2);
        let unreadable: Vec<Option<String>> = rows(&state)
            .into_iter()
            .filter(|row| row.status == "failed")
            .map(|row| row.last_error)
            .collect();
        assert_eq!(unreadable, vec![Some("payload unreadable".to_string()); 2]);
        // Never claimed again.
        let again = ship(
            &api.state,
            &editor(&state),
            OBS,
            first_only,
            NOW + 3_600_000,
        );
        assert_eq!(again.claimed, 0);
    }

    // Review Focus 3: a claim whose worker died is reaped 10 minutes later
    // and the next ship sends each subject exactly once.
    #[test]
    fn a_dead_claim_is_reaped_and_sent_once() {
        let stub = Stub::start(|_| Reply::json(201, ""));
        let dir = TestDir::new("worker-reap");
        let (state, _events) = tracked(&dir);
        let api = api(&stub.base_url, false);
        enqueue(&state, OBS, 3);
        {
            // A worker claims, then dies before sending anything.
            let dead = editor(&state);
            let claim = OutboxEdit::Claim {
                kind: OBS,
                now_ms: NOW,
                limit: 200,
            };
            let Ok(OutboxOutcome::Claimed(claimed)) = dead.edit(claim) else {
                panic!("claimed");
            };
            assert_eq!(claimed.len(), 3);
        }
        assert_eq!(
            ship_now(&api, &state, OBS),
            ShipOutcome::default(),
            "still claimed"
        );
        let early = editor(&state).edit(OutboxEdit::Reap {
            now_ms: NOW + REAP_AFTER_MS - 1,
        });
        assert_eq!(early, Ok(OutboxOutcome::Updated(0)));
        let reaped = editor(&state).edit(OutboxEdit::Reap {
            now_ms: NOW + REAP_AFTER_MS,
        });
        assert_eq!(reaped, Ok(OutboxOutcome::Updated(3)));
        assert!(
            rows(&state)
                .iter()
                .all(|row| row.status == "pending" && row.attempts == 0)
        );
        let later = NOW + REAP_AFTER_MS;
        ship(&api.state, &editor(&state), OBS, per_hundred, later);
        ship(&api.state, &editor(&state), OBS, per_hundred, later + 1);
        let seen = stub.requests();
        assert_eq!(seen.len(), 1, "one request");
        assert_eq!(seen[0].body, b"Qpayload 0\nQpayload 1\nQpayload 2\n");
        assert_eq!(count(&state, "sent"), 3);
    }

    #[test]
    fn a_tick_runs_the_reaper_through_the_tracker() {
        let dir = TestDir::new("worker-tick-reap");
        let (state, _events) = tracked(&dir);
        let api = api("http://127.0.0.1:1", true);
        enqueue(&state, OBS, 1);
        let claim = OutboxEdit::Claim {
            kind: OBS,
            now_ms: NOW,
            limit: 200,
        };
        assert!(editor(&state).edit(claim).is_ok());
        let source = || Some(editor(&state));
        let mut worker = Worker::new(&api.state, &source, &no_reader);
        assert_eq!(worker.tick(NOW + REAP_AFTER_MS), vec![Job::Reap]);
        assert_eq!(count(&state, "pending"), 1);
        assert_eq!(
            worker.tick(NOW + REAP_AFTER_MS + 1_000),
            Vec::new(),
            "once a minute"
        );
    }

    // #504: the 30-minute jobs' last runs are stored and read back, so a
    // restart does not run them again at once.
    #[test]
    fn a_restart_reads_back_when_the_long_jobs_last_ran() {
        let dir = TestDir::new("worker-last-runs");
        let (state, _events) = tracked(&dir);
        let api = api("http://127.0.0.1:1", false);
        let source = || Some(editor(&state));
        let reader = || store::open_reader(&state.db_path).ok();
        let saved: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
        let save = |rows: Vec<(String, String)>| lock(&saved).extend(rows);
        let mut first = Worker::new(&api.state, &source, &reader).with_saved_runs(&save);
        assert!(first.tick(NOW).contains(&Job::TournamentNames));
        let row = lock(&saved).last().cloned().expect("saved");
        assert_eq!(row.0, mymtgo_api::LAST_RUNS_KEY);
        write(
            &state,
            &format!(
                "INSERT INTO settings (key, value) VALUES ('{}', '{}')",
                row.0, row.1
            ),
        );
        let mut restarted = Worker::new(&api.state, &source, &reader).with_saved_runs(&save);
        let ran = restarted.tick(NOW + 60_000);
        assert!(!ran.contains(&Job::TournamentNames), "{ran:?}");
        assert!(ran.contains(&Job::ShipObservations), "{ran:?}");
    }

    #[test]
    fn a_tick_waits_while_tracking_restarts() {
        let api = api("http://127.0.0.1:1", true);
        let source = || None;
        let mut worker = Worker::new(&api.state, &source, &no_reader);
        assert_eq!(worker.tick(NOW), Vec::new());
        assert_eq!(worker.runs.last_run(Job::Reap), None, "due again next tick");
    }

    fn linked(base_url: &str) -> TestApi {
        let key_valid = (NOW + DAY).to_string();
        let token_valid = (NOW + 30 * DAY).to_string();
        let rows = [
            ("api_device_id", r#""qdevice""#),
            ("api_key_expires_at", key_valid.as_str()),
            ("api_token_expires_at", token_valid.as_str()),
        ];
        test_api(
            base_url,
            &rows,
            &[
                ("api_key", "qdevice-key"),
                (ACCESS_TOKEN, "qaccess"),
                (REFRESH_TOKEN, "qrefresh"),
            ],
        )
    }

    // Attestation follows a newly learned login on a worker tick, never a
    // timer.
    #[test]
    fn a_newly_learned_login_is_attested_on_a_tick() {
        let stub = Stub::start(|_| {
            Reply::json(
                201,
                r#"{"id":1,"username":"Qalt","login_id":8,"public":false}"#,
            )
        });
        let dir = TestDir::new("worker-attest");
        let (state, _events) = tracked(&dir);
        let api = linked(&stub.base_url);
        *lock(&api.accounts) = vec![KnownAccount {
            username: "Qplayer".to_string(),
            login_id: None,
        }];
        let source = || Some(editor(&state));
        let mut worker = Worker::new(&api.state, &source, &no_reader);
        assert!(!worker.tick(NOW).contains(&Job::Attest), "the baseline");
        lock(&api.accounts).push(KnownAccount {
            username: "Qalt".to_string(),
            login_id: Some(8),
        });
        assert!(
            !worker.tick(NOW + 1_000).contains(&Job::Attest),
            "accounts are read once a minute"
        );
        assert!(worker.tick(NOW + 60_000).contains(&Job::Attest));
        let attests: Vec<_> = stub
            .requests()
            .into_iter()
            .filter(|request| request.target == "/api/account/players")
            .collect();
        assert_eq!(attests.len(), 1);
        assert!(
            !worker.tick(NOW + 120_000).contains(&Job::Attest),
            "asked once"
        );
    }

    /// The sync a completed match asks for runs after that match's send
    /// hold; an earlier run would skip the match and leave it to the 30
    /// minute run.
    #[test]
    fn a_completed_match_asks_for_a_sync_after_its_send_hold() {
        let api = linked("http://127.0.0.1:1");
        let source = || None;
        let mut worker = Worker::new(&api.state, &source, &no_reader);
        worker.runs.record(Job::Sync, NOW);
        let completed = NOW + 1_000;
        worker.note(WorkerSignal::MatchCompleted, completed);
        let gate = api.state.gate_now();
        let schedule = Schedule::default();
        let held_until = completed + store::api_reads::SEND_HOLD_MS;
        assert!(
            !schedule
                .due(held_until, &worker.runs, &gate)
                .contains(&Job::Sync),
            "the match is still held"
        );
        assert!(
            schedule
                .due(held_until + 30_000, &worker.runs, &gate)
                .contains(&Job::Sync)
        );
    }

    #[test]
    fn a_due_sync_runs_the_sync_job_once() {
        let api = linked("http://127.0.0.1:1");
        let runs = AtomicUsize::new(0);
        let sync = |_| {
            runs.fetch_add(1, Ordering::SeqCst);
            false
        };
        let dir = TestDir::new("worker-sync-job");
        let (state, _events) = tracked(&dir);
        let source = || Some(editor(&state));
        let mut worker = Worker::new(&api.state, &source, &no_reader).with_sync(&sync);
        assert!(
            worker.tick(NOW).contains(&Job::Sync),
            "due at the first tick"
        );
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert!(!worker.tick(NOW + 1_000).contains(&Job::Sync));
        worker.note(WorkerSignal::SyncNow, NOW + 2_000);
        assert!(worker.tick(NOW + 2_000).contains(&Job::Sync), "Sync now");
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    // At launch or wake the network is often not up yet: a sync that could
    // not reach MyMTGO is tried again 1, 2, 4 and 8 minutes later, and only
    // the last of those stores its failure for the Account page.
    #[test]
    fn a_sync_that_could_not_reach_mymtgo_is_tried_again_with_backoff() {
        let api = linked("http://127.0.0.1:1");
        let reported: Mutex<Vec<bool>> = Mutex::new(Vec::new());
        let sync = |report_unreachable| {
            lock(&reported).push(report_unreachable);
            true
        };
        let dir = TestDir::new("worker-sync-retry");
        let (state, _events) = tracked(&dir);
        let source = || Some(editor(&state));
        let mut worker = Worker::new(&api.state, &source, &no_reader).with_sync(&sync);
        let mut at = NOW;
        assert!(worker.tick(at).contains(&Job::Sync), "the launch sync");
        for minutes in [1, 2, 4, 8] {
            let wait = minutes * 60_000;
            assert!(!worker.tick(at + wait - 1_000).contains(&Job::Sync));
            at += wait;
            assert!(worker.tick(at).contains(&Job::Sync), "{minutes} min");
        }
        assert_eq!(*lock(&reported), [false, false, false, false, true]);
        assert!(
            !worker.tick(at + 29 * 60_000).contains(&Job::Sync),
            "then the normal 30 minutes"
        );
    }

    #[test]
    fn the_worker_stops_on_stop_and_ticks_on_a_signal() {
        let api = api("http://127.0.0.1:1", true);
        let ticks = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&ticks);
        let source = move || {
            counted.fetch_add(1, Ordering::SeqCst);
            None
        };
        let (signals, inbox) = mpsc::channel();
        signals.send(WorkerSignal::GateChanged).expect("send");
        signals.send(WorkerSignal::Stop).expect("send");
        let started = std::time::Instant::now();
        let mut worker = Worker::new(&api.state, &source, &no_reader);
        run_worker(&inbox, &mut worker, &|| NOW);
        assert!(started.elapsed() < Duration::from_secs(1), "no wait");
        assert_eq!(ticks.load(Ordering::SeqCst), 1, "one tick, then stopped");
    }

    #[test]
    fn the_scheduled_gate_opens_no_secret_store_while_offline() {
        let api = test_api(
            "http://127.0.0.1:1",
            &[("offline_mode", "true")],
            &[("api_key", "qdevice-key")],
        );
        let gate = api.state.gate_now();
        assert!(gate.offline);
        assert_eq!(api.secrets.reads(), 0);
    }

    fn no_reader() -> Option<store::Connection> {
        None
    }

    fn insert_sync_event(state: &AppState, offset: i64) {
        write(
            state,
            &format!(
                "INSERT OR IGNORE INTO log_files (id, path, identity_hash, head_hash, head_len,
                     day_anchor, timezone, clock_date, first_seen_at, last_seen_at)
                 VALUES (7, '/q/obs.log', 'a', 'b', 1, '2026-10-09', 'UTC', '2026-10-09', 1, 1);
                 INSERT INTO log_events (log_file_id, byte_offset, kind, tournament_token,
                     logged_at, raw)
                 VALUES (7, {offset}, 'tournament_sync', 'tok-1', {NOW},
                     CAST('EventSyncData_t {{\"Token\":\"tok-1\"}}' AS BLOB));"
            ),
        );
    }

    // Q7: a tick queues the stored tournament events and ships them in the
    // same tick (each enqueue runs before its ship).
    #[test]
    fn a_tick_queues_and_ships_observations() {
        let stub = Stub::start(|_| Reply::json(204, ""));
        let dir = TestDir::new("worker-observations");
        let (state, _events) = tracked(&dir);
        let api = api(&stub.base_url, false);
        insert_sync_event(&state, 10);
        let source = || Some(editor(&state));
        let reader = || store::open_reader(&state.db_path).ok();
        let mut worker = Worker::new(&api.state, &source, &reader);
        let ran = worker.tick(NOW);
        assert!(ran.contains(&Job::EnqueueObservations), "{ran:?}");
        assert!(ran.contains(&Job::ShipObservations), "{ran:?}");
        assert_eq!(count(&state, "sent"), 1);
        let seen = stub.requests();
        let shipped: Vec<_> = seen
            .iter()
            .filter(|request| request.target == "/api/tournament-observations")
            .collect();
        assert_eq!(shipped.len(), 1);
        assert_eq!(shipped[0].header("content-encoding"), Some("gzip"));
    }

    // Q7: `TournamentAdded` (the bridge's `LeaguesChanged`) asks the names
    // at the next tick, not 30 minutes later; a run with nothing to name
    // makes no request.
    #[test]
    fn a_tournament_added_asks_names_at_the_next_tick() {
        let stub = Stub::start(|_| Reply::json(404, "{}"));
        let dir = TestDir::new("worker-names");
        let (state, _events) = tracked(&dir);
        let api = api(&stub.base_url, false);
        let source = || Some(editor(&state));
        let reader = || store::open_reader(&state.db_path).ok();
        let mut worker = Worker::new(&api.state, &source, &reader);
        assert!(worker.tick(NOW).contains(&Job::TournamentNames));
        assert_eq!(stub.connections(), 0, "nothing to name");
        write(
            &state,
            "INSERT INTO tournaments (mtgo_event_id, name, started_at) VALUES (77, 'Qsynth', 1)",
        );
        assert!(!worker.tick(NOW + 1_000).contains(&Job::TournamentNames));
        worker.note(WorkerSignal::TournamentAdded, NOW + 2_000);
        assert!(worker.tick(NOW + 2_000).contains(&Job::TournamentNames));
        let asked: Vec<String> = stub.requests().into_iter().map(|r| r.target).collect();
        assert_eq!(asked, vec!["/api/tournaments/77".to_string()]);
    }

    // Owner ruling 2026-10-11 (#264): a device that never signed in sends
    // match reports and card stats with its anonymous device key, gated
    // only by offline mode. Duplicates are the API's to drop.
    #[test]
    fn a_device_that_never_signed_in_ships_reports_and_card_stats() {
        // A device that never signed in and holds no key yet: it registers
        // at first need and reports with its anonymous device key.
        let stub = Stub::start(|request| match request.target.as_str() {
            "/api/devices/register" => Reply::json(200, r#"{"api_key":"qfresh-key"}"#),
            _ => Reply::json(200, "{}"),
        });
        let dir = TestDir::new("worker-never-signed-in");
        let (state, _events) = tracked(&dir);
        let api = test_api(&stub.base_url, &[], &[]);
        write(
            &state,
            "INSERT INTO accounts (username, first_seen_at) VALUES ('Qplayer', 0);
             INSERT INTO decks (mtgo_id, name, format, created_at, updated_at, source)
                 VALUES ('900', 'Qdeck', 'Modern', 0, 0, 'file');
             INSERT INTO deck_versions (deck_id, signature, modified_at) VALUES (1, 'cQ==', 0);
             INSERT INTO cards (catalog_id, oracle_id) VALUES (7, 'q-oracle-7');
             INSERT INTO matches (token, account_id, source, format, state, outcome, started_at,
                 last_activity_at, revision, derived_version, deck_version_id)
                 VALUES ('q-match', 1, 'imported', 'CMODERN', 'complete', 'win', 1, 1, 1, 1, 1);
             INSERT INTO games (match_id, mtgo_game_id, game_number, result, end_reason)
                 VALUES (1, 901, 1, 'win', 'win');
             INSERT INTO game_cards (game_id, side, catalog_id, main_qty) VALUES (1, 'local', 7, 4);
             INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
                 VALUES ('q-player-uuid', 'Qarchetype', 'modern', 'downloaded', 0, 0);
             INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
                 VALUES (1, 0, (SELECT id FROM archetypes WHERE uuid = 'q-player-uuid'), 1.0);",
        );
        let source = || Some(editor(&state));
        let reader = || store::open_reader(&state.db_path).ok();
        let mut worker = Worker::new(&api.state, &source, &reader);
        let ran = worker.tick(NOW);
        for job in [Job::ShipReports, Job::EnqueueCardStats, Job::ShipCardStats] {
            assert!(ran.contains(&job), "{job:?} in {ran:?}");
        }
        let kinds: Vec<(String, String)> = rows(&state)
            .into_iter()
            .map(|row| (row.kind, row.status))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("match_report".to_string(), "sent".to_string()),
                ("card_stats".to_string(), "sent".to_string()),
            ]
        );
        for target in ["/api/matches/report", "/api/card-stats/report"] {
            let sent = stub
                .requests()
                .into_iter()
                .find(|request| request.target == target)
                .expect("sent");
            assert_eq!(sent.header("x-api-key"), Some("qfresh-key"), "{target}");
            assert!(sent.header("x-device-id").is_some(), "{target}");
            assert_eq!(sent.header("authorization"), None, "{target}");
        }
    }

    // Archetypes spec section 13 (Review Focus 3, Q5 `enqueue_twice_adds_nothing`
    // rerun with the switches as shipped; Plan T follow-up F21): with
    // `MATCH_REPORTS_LIVE` and `CARD_STATS_LIVE` on, a match with its player
    // seat is queued and reported once, and each of its decided games gets
    // its card stats queued and sent exactly once, however many ticks and
    // "Submit pending" presses follow.
    #[test]
    fn a_match_is_reported_once_and_each_game_card_stats_go_out_once() {
        assert_eq!(
            (mymtgo_api::MATCH_REPORTS_LIVE, mymtgo_api::CARD_STATS_LIVE),
            (true, true)
        );
        let stub = Stub::start(|_| Reply::json(200, "{}"));
        let dir = TestDir::new("worker-live-reports");
        let (state, _events) = tracked(&dir);
        let api = api(&stub.base_url, false);
        write(
            &state,
            "INSERT INTO accounts (username, first_seen_at) VALUES ('Qplayer', 0);
             INSERT INTO decks (mtgo_id, name, format, created_at, updated_at, source)
                 VALUES ('900', 'Qdeck', 'Modern', 0, 0, 'file');
             INSERT INTO deck_versions (deck_id, signature, modified_at) VALUES (1, 'cQ==', 0);
             INSERT INTO cards (catalog_id, oracle_id) VALUES (7, 'q-oracle-7');
             INSERT INTO matches (token, account_id, source, format, state, outcome, started_at,
                 last_activity_at, revision, derived_version, deck_version_id)
                 VALUES ('q-match', 1, 'imported', 'CMODERN', 'complete', 'win', 1, 1, 1, 1, 1);
             INSERT INTO games (match_id, mtgo_game_id, game_number, result, end_reason)
                 VALUES (1, 901, 1, 'win', 'win');
             INSERT INTO games (match_id, mtgo_game_id, game_number, result, end_reason)
                 VALUES (1, 902, 2, 'loss', 'win');
             INSERT INTO game_cards (game_id, side, catalog_id, main_qty) VALUES (1, 'local', 7, 4);
             INSERT INTO game_cards (game_id, side, catalog_id, main_qty) VALUES (2, 'local', 7, 4);
             INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
                 VALUES ('q-player-uuid', 'Qarchetype', 'modern', 'downloaded', 0, 0);
             INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
                 VALUES (1, 0, (SELECT id FROM archetypes WHERE uuid = 'q-player-uuid'), 1.0);",
        );
        let source = || Some(editor(&state));
        let reader = || store::open_reader(&state.db_path).ok();
        let mut worker = Worker::new(&api.state, &source, &reader);
        for at in [NOW, NOW + 60_000, NOW + 120_000, NOW + 180_000] {
            worker.note(WorkerSignal::SubmitReports, at);
            let ran = worker.tick(at);
            for job in [
                Job::EnqueueReports,
                Job::EnqueueCardStats,
                Job::ShipCardStats,
            ] {
                assert!(ran.contains(&job), "{job:?} at {at}");
            }
        }
        let reports: Vec<_> = rows(&state)
            .into_iter()
            .filter(|row| row.kind == "match_report")
            .collect();
        assert_eq!(reports.len(), 1, "{reports:?}");
        assert_eq!(reports[0].status, "sent");
        let card_stats: Vec<(String, String)> = rows(&state)
            .into_iter()
            .filter(|row| row.kind == "card_stats")
            .map(|row| (row.subject, row.status))
            .collect();
        assert_eq!(
            card_stats,
            vec![
                ("q-match:901".to_string(), "sent".to_string()),
                ("q-match:902".to_string(), "sent".to_string()),
            ]
        );
        let requests = stub.requests();
        let posted: Vec<&str> = requests
            .iter()
            .map(|request| request.target.as_str())
            .filter(|target| {
                target.starts_with("/api/matches") || target.starts_with("/api/card-stats")
            })
            .collect();
        assert_eq!(
            posted,
            vec!["/api/matches/report", "/api/card-stats/report"],
            "one report and one card stats request for both games"
        );
    }
}
