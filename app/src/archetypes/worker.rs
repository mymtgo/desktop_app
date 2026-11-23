//! The `archetypes` worker thread (archetypes spec sections 3 and 6.3,
//! ruling 6): one thread, separate from the `api` and `cards` workers, that
//! runs every archetype job one at a time: "Detect" requests, the deck
//! banner, deck prefills, missing definitions and the detection queue (the
//! backfill), so a backfill never delays shipping or card population.
//!
//! The queue is read from the database (`due_matches`); the worker wakes on
//! a signal or every `IDLE`, so a log match is picked up once its game sync
//! has written. The definitions change check runs `CHECK_TIMES.first`
//! after start, then every `CHECK_TIMES.every`, and when offline mode is
//! left (spec section 5). Every API call runs here, paced `API_PAUSE_MS`
//! apart. Like a Laravel queue worker with one queue and a `sleep` of one
//! second, plus a scheduled command.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::{Duration, Instant};

use mymtgo_api::{ArchetypeDeck, CatalogArchetype, TransientRetry};
use store::Connection;
use store::archetype_edit::{ArchetypeEdit, ArchetypeEditOutcome, OfflineHeld};
use store::archetypes::DeckFilter;
use tauri::{AppHandle, Manager};

use crate::api::session::ApiState;
use crate::archetypes::cache::{ArchetypeCaches, Cached, metagame_key};
use crate::archetypes::catalog::fetch_paced_catalog;
use crate::archetypes::definitions::{API_PAUSE_MS, ASKED_API_PAUSE_MS, Pacer};
use crate::archetypes::detect::{ApiLookups, CheckRun, Detector, FailedEstimates, Writer};
use crate::archetypes::metagame::{fetch_global, fetch_live_decklists};
use crate::bridge::lock;
use crate::commands::{Startup, ready};
use crate::worker_thread::WorkerThread;

/// How long the worker sleeps without a signal before it looks at the
/// queue again.
pub const IDLE: Duration = Duration::from_secs(1);
/// The answer a waiting command gets when the worker is gone.
pub const STOPPED: &str = "the archetype worker stopped";

/// When the definitions change check runs (spec section 5).
#[derive(Debug, Clone, Copy)]
pub struct CheckTimes {
    /// After the worker starts (it starts once the tracker is ready).
    pub first: Duration,
    /// Between checks.
    pub every: Duration,
    /// After a check that failed (#300 F6).
    pub retry: Duration,
}

/// 30 s after start, then every 24 hours; 30 minutes after a failure. A
/// check that could not reach MyMTGO is tried again after
/// `TRANSIENT_RETRIES_MS` first.
pub const CHECK_TIMES: CheckTimes = CheckTimes {
    first: Duration::from_secs(30),
    every: Duration::from_secs(24 * 3_600),
    retry: Duration::from_secs(30 * 60),
};

/// What wakes the worker.
#[derive(Debug)]
pub enum ArchetypeSignal {
    /// A log match became complete: look at the queue.
    MatchCompleted,
    /// The deck sync wrote this deck and it has no archetype: prefill it.
    DeckSynced(i64),
    /// "Detect" on matches, answered with how many were written.
    Detect {
        match_ids: Vec<i64>,
        reply: Sender<Result<u32, String>>,
    },
    /// The deck "Detect archetypes" banner, answered with how many were
    /// queued.
    DetectDeck {
        deck_id: i64,
        filter: DeckFilter,
        reply: Sender<Result<u32, String>>,
    },
    /// Run the definitions change check now (leaving offline mode).
    CheckDefinitions,
    /// Download the definitions of rows known only by uuid now (the game
    /// overlay wrote a stub, overlay spec 7.3).
    FetchDefinitions,
    /// Offline mode was switched: online again, the queue and the missing
    /// definitions may ask the API.
    GateChanged,
    /// The Metagame page wants the website's shares of a format slug over
    /// `days`; they go into the cache and `MetagameSharesReady` follows.
    FetchMetagame {
        slug: String,
        days: u32,
    },
    /// The archetype detail wants the live decklists of `uuid` (cached too).
    FetchDecklists {
        uuid: String,
        reply: Sender<Result<Vec<ArchetypeDeck>, String>>,
    },
    /// The picker wants the server's archetypes of a format key (#489),
    /// answered from the cache or fetched into it.
    FetchCatalog {
        format: String,
        reply: Sender<Result<Vec<CatalogArchetype>, String>>,
    },
    /// The offline mode card's "Download archetypes": the whole catalog,
    /// held as Custom archetypes (refused while online).
    DownloadOffline {
        reply: Sender<Result<OfflineHeld, String>>,
    },
    Stop,
}

/// The jobs the loop runs (the app's run them with `Detector`).
pub trait Jobs {
    fn detect(&mut self, match_ids: &[i64], should_yield: &dyn Fn() -> bool)
    -> Result<u32, String>;
    fn queue_deck(&mut self, deck_id: i64, filter: DeckFilter) -> Result<u32, String>;
    fn prefill_deck(&mut self, deck_id: i64) -> Result<(), String>;
    /// The definitions of rows known only by uuid.
    fn fetch_definitions(&mut self, should_yield: &dyn Fn() -> bool) -> Result<CheckRun, String>;
    /// The definitions change check.
    fn check_definitions(&mut self, should_yield: &dyn Fn() -> bool) -> Result<CheckRun, String>;
    /// One backfill slice; whether more may be due.
    fn backfill(&mut self, should_yield: &dyn Fn() -> bool) -> Result<bool, String>;
    /// The website's shares of a format's window, into the cache.
    fn fetch_metagame(&mut self, slug: &str, days: u32);
    /// An archetype's live decklists, into the cache.
    fn fetch_decklists(&mut self, uuid: &str) -> Result<Vec<ArchetypeDeck>, String>;
    /// The server's archetypes of a format key, into the cache.
    fn fetch_catalog(&mut self, format: &str) -> Result<Vec<CatalogArchetype>, String>;
    /// The offline archetype download.
    fn download_offline(&mut self) -> Result<OfflineHeld, String>;
    /// Rows held offline while online are released (true when some were).
    fn reconcile_offline(&mut self) -> Result<bool, String>;
}

/// The worker's inbox and the signals a running job set aside for the
/// loop (read while it ran, answered after it).
pub struct Inbox {
    receiver: Receiver<ArchetypeSignal>,
    pending: RefCell<Vec<ArchetypeSignal>>,
    /// Every sender is gone: the worker is ending.
    closed: Cell<bool>,
}

impl Inbox {
    pub fn new(receiver: Receiver<ArchetypeSignal>) -> Inbox {
        Inbox {
            receiver,
            pending: RefCell::new(Vec::new()),
            closed: Cell::new(false),
        }
    }

    /// The signals set aside, oldest first.
    pub fn take_pending(&self) -> Vec<ArchetypeSignal> {
        self.pending.borrow_mut().drain(..).collect()
    }

    /// The oldest signal set aside, if any.
    fn next_pending(&self) -> Option<ArchetypeSignal> {
        let mut waiting = self.pending.borrow_mut();
        if waiting.is_empty() {
            None
        } else {
            Some(waiting.remove(0))
        }
    }

    /// Whether a waiting signal asks the running job to stop now (or the
    /// worker is ending).
    pub fn should_yield(&self) -> bool {
        let mut waiting = self.pending.borrow_mut();
        loop {
            match self.receiver.try_recv() {
                Ok(signal) => waiting.push(signal),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.closed.set(true);
                    break;
                }
            }
        }
        self.closed.get() || waiting.iter().any(urgent)
    }

    /// Sleep `duration` on the inbox (`recv_timeout`), setting aside what
    /// arrives. With `may_yield` it ends early, returning false, for a
    /// signal the running job stops for (#300 F1); otherwise it always
    /// runs to its end.
    fn pause(&self, duration: Duration, may_yield: bool) -> bool {
        if may_yield && self.should_yield() {
            return false;
        }
        let deadline = Instant::now() + duration;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return true;
            }
            if self.closed.get() {
                std::thread::sleep(left);
                return !may_yield;
            }
            match self.receiver.recv_timeout(left) {
                Ok(signal) => {
                    let stop = urgent(&signal);
                    self.pending.borrow_mut().push(signal);
                    if may_yield && stop {
                        return false;
                    }
                }
                Err(RecvTimeoutError::Timeout) => return true,
                Err(RecvTimeoutError::Disconnected) => {
                    self.closed.set(true);
                    if may_yield {
                        return false;
                    }
                }
            }
        }
    }
}

/// The worker's pacing clock: the wall clock, pausing on the inbox, so a
/// paced wait never hides a signal and a detection's wait ends for one.
pub struct InboxClock(pub Rc<Inbox>);

impl crate::archetypes::definitions::Clock for InboxClock {
    fn now_ms(&self) -> i64 {
        crate::wall_clock_ms()
    }

    fn sleep_ms(&self, ms: i64) {
        let ms = u64::try_from(ms).unwrap_or(0);
        self.0.pause(Duration::from_millis(ms), false);
    }

    fn sleep_or_yield_ms(&self, ms: i64) -> bool {
        let ms = u64::try_from(ms).unwrap_or(0);
        self.0.pause(Duration::from_millis(ms), true)
    }
}

/// A signal a running job stops for: a waiting command or stopping.
fn urgent(signal: &ArchetypeSignal) -> bool {
    matches!(
        signal,
        ArchetypeSignal::Detect { .. }
            | ArchetypeSignal::DetectDeck { .. }
            | ArchetypeSignal::FetchMetagame { .. }
            | ArchetypeSignal::FetchDecklists { .. }
            | ArchetypeSignal::FetchCatalog { .. }
            | ArchetypeSignal::DownloadOffline { .. }
            | ArchetypeSignal::Stop
    )
}

/// How long after a check ran the next one is due: `None` (no check until
/// the next start) once the endpoint answered 404. `retry` counts the
/// checks in a row that could not reach MyMTGO.
fn after_check(
    run: &Result<CheckRun, String>,
    times: &CheckTimes,
    retry: &mut TransientRetry,
) -> Option<Duration> {
    retry.ended(*run == Ok(CheckRun::Unreachable));
    match run {
        Ok(CheckRun::NotDeployed) => None,
        Ok(CheckRun::Applied | CheckRun::Offline) => Some(times.every),
        Ok(CheckRun::Unreachable) => match retry.wait_ms() {
            Some(wait) => Some(Duration::from_millis(wait.unsigned_abs())),
            None => Some(times.retry),
        },
        Err(error) => {
            tracing::warn!(%error, "the archetype definitions check failed");
            Some(times.retry)
        }
    }
}

/// The worker's loop: commands first, then deck prefills, missing
/// definitions, the change check when due and one backfill slice per
/// round. One job at a time, so a match is never detected twice at once
/// (0.x's unique job). Ends on `Stop` or when every sender is gone.
pub fn run_archetypes_worker(
    inbox: &Inbox,
    idle: Duration,
    times: &CheckTimes,
    jobs: &mut dyn Jobs,
) {
    // At start the queue is the backfill (spec 6.3), and rows known only by
    // uuid wait for their definitions.
    let mut backfill_due = true;
    let mut definitions_due = true;
    // Rows held offline while online (a skipped or failed flip-back) are
    // released at start and on each gate change; a failure (the tracker not
    // ready yet) is tried again at the next wake.
    let mut reconcile_due = true;
    // `None`: the check answered 404, none until the next start.
    let mut next_check: Option<Instant> = Some(Instant::now() + times.first);
    let mut check_retry = TransientRetry::default();
    let mut decks: Vec<i64> = Vec::new();
    let should_yield = || inbox.should_yield();
    loop {
        let check_due = next_check.is_some_and(|at| Instant::now() >= at);
        let busy = backfill_due || definitions_due || check_due || !decks.is_empty();
        let mut signals: Vec<ArchetypeSignal> = inbox.take_pending();
        if signals.is_empty() && !busy {
            let wait = match next_check {
                Some(at) => idle.min(at.saturating_duration_since(Instant::now())),
                None => idle,
            };
            match inbox.receiver.recv_timeout(wait) {
                Ok(signal) => signals.push(signal),
                Err(RecvTimeoutError::Timeout) => backfill_due = true,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        while let Ok(signal) = inbox.receiver.try_recv() {
            signals.push(signal);
        }
        // The batch waits in `pending`, taken one at a time, so a running
        // job's `should_yield` sees the signals behind it (a `Stop` drained
        // with a "Detect" stops it, #300 F1 review).
        inbox.pending.borrow_mut().splice(0..0, signals);
        while let Some(signal) = inbox.next_pending() {
            match signal {
                ArchetypeSignal::Stop => return,
                ArchetypeSignal::MatchCompleted => backfill_due = true,
                ArchetypeSignal::GateChanged => {
                    backfill_due = true;
                    definitions_due = true;
                    reconcile_due = true;
                }
                ArchetypeSignal::FetchDefinitions => definitions_due = true,
                ArchetypeSignal::CheckDefinitions => {
                    if next_check.is_some() {
                        next_check = Some(Instant::now());
                    }
                }
                ArchetypeSignal::DeckSynced(deck_id) => {
                    if !decks.contains(&deck_id) {
                        decks.push(deck_id);
                    }
                }
                ArchetypeSignal::Detect { match_ids, reply } => {
                    // Stops for the next waiting command or `Stop`; the
                    // matches left stay queued for the backfill (#300 F1).
                    let _ = reply.send(jobs.detect(&match_ids, &should_yield));
                    backfill_due = true;
                }
                ArchetypeSignal::DetectDeck {
                    deck_id,
                    filter,
                    reply,
                } => {
                    let queued = jobs.queue_deck(deck_id, filter);
                    if queued.is_ok() {
                        backfill_due = true;
                    }
                    let _ = reply.send(queued);
                }
                ArchetypeSignal::FetchMetagame { slug, days } => {
                    jobs.fetch_metagame(&slug, days);
                }
                ArchetypeSignal::FetchDecklists { uuid, reply } => {
                    let _ = reply.send(jobs.fetch_decklists(&uuid));
                }
                ArchetypeSignal::FetchCatalog { format, reply } => {
                    let _ = reply.send(jobs.fetch_catalog(&format));
                }
                ArchetypeSignal::DownloadOffline { reply } => {
                    let _ = reply.send(jobs.download_offline());
                }
            }
        }
        let mut waiting_decks = std::mem::take(&mut decks).into_iter();
        for deck_id in waiting_decks.by_ref() {
            if should_yield() {
                decks.push(deck_id);
                break;
            }
            if let Err(error) = jobs.prefill_deck(deck_id) {
                tracing::warn!(%error, deck_id, "the deck archetype prefill failed");
            }
            if should_yield() {
                // Asked again after the waiting signal (a prefill writes
                // only while the deck has no archetype).
                decks.push(deck_id);
                break;
            }
        }
        decks.extend(waiting_decks);
        if reconcile_due {
            match jobs.reconcile_offline() {
                Ok(released) => {
                    reconcile_due = false;
                    if released {
                        definitions_due = true;
                        if next_check.is_some() {
                            next_check = Some(Instant::now());
                        }
                    }
                }
                Err(error) => {
                    // Usually tracking is not running yet: quiet, as it
                    // is tried again at each wake.
                    tracing::debug!(%error, "archetypes held offline could not be released yet");
                }
            }
        }
        if definitions_due {
            definitions_due = false;
            match jobs.fetch_definitions(&should_yield) {
                // A changed definition may ask matches for detection again.
                Ok(CheckRun::Applied) => backfill_due = true,
                Ok(CheckRun::NotDeployed) => next_check = None,
                Ok(CheckRun::Offline) => {}
                // Asked again at the next start, rejoin or new stub.
                Ok(CheckRun::Unreachable) => {}
                Err(error) => tracing::warn!(%error, "fetching archetype definitions failed"),
            }
        }
        if next_check.is_some_and(|at| Instant::now() >= at) {
            let run = jobs.check_definitions(&should_yield);
            if run == Ok(CheckRun::Applied) {
                backfill_due = true;
            }
            next_check =
                after_check(&run, times, &mut check_retry).map(|wait| Instant::now() + wait);
        }
        if backfill_due {
            backfill_due = match jobs.backfill(&should_yield) {
                Ok(more) => more,
                Err(error) => {
                    // Tried again at the next wake.
                    tracing::warn!(%error, "archetype detection failed");
                    false
                }
            };
        }
    }
}

/// Archetype edits through the tracker's writer.
pub struct TrackerWriter<'a> {
    app: &'a AppHandle,
}

impl<'a> TrackerWriter<'a> {
    pub fn new(app: &'a AppHandle) -> TrackerWriter<'a> {
        TrackerWriter { app }
    }
}

impl Writer for TrackerWriter<'_> {
    fn ready(&self) -> bool {
        let startup = self.app.state::<Startup>();
        match ready(&startup) {
            Ok(state) => lock(&state.tracker).is_some(),
            Err(_) => false,
        }
    }

    fn edit(&mut self, edit: ArchetypeEdit) -> Result<ArchetypeEditOutcome, String> {
        let startup = self.app.state::<Startup>();
        let state = ready(&startup).map_err(|_| "the app is not ready".to_string())?;
        let editor = state
            .tracker_part(tracker::TrackerHandle::archetype_editor)
            .map_err(|_| "tracking is restarting; try again".to_string())?;
        editor.edit(edit)
    }
}

/// Tells the page one match of its "Detect" is written (`MatchDetected`).
fn announce_detected(app: &AppHandle, reader: &Connection, match_id: i64) {
    let token = match store::manual::match_token(reader, match_id) {
        Ok(Some(token)) => token,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(%error, match_id, "could not read a detected match's token");
            return;
        }
    };
    let event = crate::events::MatchDetected { token };
    if let Err(error) = tauri_specta::Event::emit(&event, app) {
        tracing::warn!(%error, "could not push MatchDetected");
    }
}

/// The jobs with the app's state: a read-only connection of its own, the
/// API, the tracker's writer and one pacer for every request.
struct AppJobs {
    app: AppHandle,
    pacer: Pacer,
    reader: Option<Connection>,
    failed: FailedEstimates,
}

impl AppJobs {
    fn run<T>(
        &mut self,
        job: impl FnOnce(&mut Detector) -> Result<T, String>,
    ) -> Result<T, String> {
        let startup = self.app.state::<Startup>();
        let state = ready(&startup).map_err(|_| "the app is not ready".to_string())?;
        if self.reader.is_none() {
            let reader = store::open_reader(&state.db_path).map_err(|error| error.to_string())?;
            self.reader = Some(reader);
        }

        let Some(reader) = self.reader.as_ref() else {
            return Err("the archetype worker has no database".to_string());
        };
        // While a 0.x import runs or one did not finish, imported matches
        // wait (archetypes spec section 15); the import sends `GateChanged`
        // when it ends, so they are detected then.
        let logs_only = crate::legacy::import_gate_closed(state, reader);
        let api = self.app.state::<ApiState>();
        let mut lookups = ApiLookups::new(&api, &mut self.pacer);
        let mut writer = TrackerWriter { app: &self.app };
        let now = crate::wall_clock_ms;
        let mut detector = Detector {
            reader,
            lookups: &mut lookups,
            writer: &mut writer,
            now_ms: &now,
            failed: &mut self.failed,
            logs_only,
        };
        job(&mut detector)
    }
}

impl Jobs for AppJobs {
    fn detect(
        &mut self,
        match_ids: &[i64],
        should_yield: &dyn Fn() -> bool,
    ) -> Result<u32, String> {
        // The player is waiting on these: a shorter pause, then the
        // backfill's again.
        self.pacer.set_pause_ms(ASKED_API_PAUSE_MS);
        let app = self.app.clone();
        let detected = self.run(|detector| {
            let reader = detector.reader;
            detector.detect_matches_reporting(match_ids, should_yield, &mut |match_id| {
                announce_detected(&app, reader, match_id);
            })
        });
        self.pacer.set_pause_ms(API_PAUSE_MS);
        detected
    }

    fn queue_deck(&mut self, deck_id: i64, filter: DeckFilter) -> Result<u32, String> {
        self.run(|detector| detector.queue_deck(deck_id, filter))
    }

    fn prefill_deck(&mut self, deck_id: i64) -> Result<(), String> {
        self.run(|detector| detector.prefill_deck(deck_id).map(|_| ()))
    }

    fn fetch_definitions(&mut self, should_yield: &dyn Fn() -> bool) -> Result<CheckRun, String> {
        self.run(|detector| detector.fetch_missing_definitions(should_yield))
    }

    fn check_definitions(&mut self, should_yield: &dyn Fn() -> bool) -> Result<CheckRun, String> {
        self.run(|detector| detector.check_definitions(should_yield))
    }

    fn backfill(&mut self, should_yield: &dyn Fn() -> bool) -> Result<bool, String> {
        self.run(|detector| detector.backfill(should_yield))
    }

    fn fetch_metagame(&mut self, slug: &str, days: u32) {
        let answer = {
            let api = self.app.state::<ApiState>();
            fetch_global(&api, &mut self.pacer, slug, days)
        };
        if let Err(error) = &answer {
            tracing::warn!(%error, slug, days, "the metagame shares could not be fetched");
        }
        if let Some(caches) = self.app.try_state::<ArchetypeCaches>() {
            lock(&caches.metagame).store(
                &metagame_key(slug, days),
                answer.ok(),
                crate::wall_clock_ms(),
            );
        }
        // Ready or not, the page reads the cache again.
        let event = crate::events::MetagameSharesReady {
            format: slug.to_string(),
            days,
        };
        if let Err(error) = tauri_specta::Event::emit(&event, &self.app) {
            tracing::warn!(%error, "could not push MetagameSharesReady");
        }
    }

    fn fetch_decklists(&mut self, uuid: &str) -> Result<Vec<ArchetypeDeck>, String> {
        let answer = {
            let api = self.app.state::<ApiState>();
            fetch_live_decklists(&api, &mut self.pacer, uuid)
        };
        if let Some(caches) = self.app.try_state::<ArchetypeCaches>() {
            lock(&caches.decklists).store(uuid, answer.clone().ok(), crate::wall_clock_ms());
        }
        answer
    }

    fn fetch_catalog(&mut self, format: &str) -> Result<Vec<CatalogArchetype>, String> {
        let caches = self.app.try_state::<ArchetypeCaches>();
        // Two pickers asking at once send one request.
        if let Some(caches) = &caches
            && let Cached::Fresh(catalog) =
                lock(&caches.catalog).get(format, crate::wall_clock_ms())
        {
            return Ok(catalog);
        }
        let answer = {
            let api = self.app.state::<ApiState>();
            fetch_paced_catalog(&api, &mut self.pacer, format)
        };
        if let Some(caches) = &caches {
            lock(&caches.catalog).store(format, answer.clone().ok(), crate::wall_clock_ms());
        }
        answer
    }

    fn reconcile_offline(&mut self) -> Result<bool, String> {
        let app = self.app.clone();
        self.run(|detector| {
            let held = store::archetypes::has_offline_held(detector.reader)
                .map_err(|error| error.to_string())?;
            let api = app.state::<ApiState>();
            crate::archetypes::offline::reconcile_offline(
                &api,
                held,
                detector.writer,
                crate::wall_clock_ms(),
            )
        })
    }

    fn download_offline(&mut self) -> Result<OfflineHeld, String> {
        let api = self.app.state::<ApiState>();
        let mut writer = TrackerWriter { app: &self.app };
        crate::archetypes::offline::download_offline(
            &api,
            &mut self.pacer,
            &mut writer,
            crate::wall_clock_ms(),
        )
    }
}

/// The running `archetypes` thread, managed by Tauri.
pub struct ArchetypesWorker {
    thread: WorkerThread<ArchetypeSignal>,
}

impl ArchetypesWorker {
    /// Start the thread. It reads `ApiState` and `Startup` from the app, so
    /// both are managed first.
    pub fn start(app: AppHandle) -> ArchetypesWorker {
        let thread = WorkerThread::spawn(
            "archetypes",
            "could not start the archetypes worker; archetypes are not detected",
            move |receiver| {
                let inbox = Rc::new(Inbox::new(receiver));
                let clock = InboxClock(Rc::clone(&inbox));
                let mut jobs = AppJobs {
                    app,
                    pacer: Pacer::new(Box::new(clock)),
                    reader: None,
                    failed: FailedEstimates::default(),
                };
                run_archetypes_worker(&inbox, IDLE, &CHECK_TIMES, &mut jobs);
            },
        );
        ArchetypesWorker { thread }
    }

    /// Wake the worker with a signal. Returns false once stopped.
    pub fn signal(&self, signal: ArchetypeSignal) -> bool {
        self.thread.send(signal)
    }

    /// Stop the worker, waiting up to `STOP_WAIT` for it (a request in
    /// flight, or a pause, can outlast it; the next start picks the queue
    /// up again).
    pub fn stop(&self) {
        self.thread.stop(
            ArchetypeSignal::Stop,
            "the archetypes worker is still asking the API; leaving it",
        );
    }
}

/// Wake the app's archetypes worker, if it runs.
pub fn signal(app: &AppHandle, signal: ArchetypeSignal) {
    if let Some(worker) = app.try_state::<ArchetypesWorker>() {
        worker.signal(signal);
    }
}

/// Send a request with a reply to the worker and wait for the answer.
pub fn ask(
    app: &AppHandle,
    request: impl FnOnce(Sender<Result<u32, String>>) -> ArchetypeSignal,
) -> Result<u32, String> {
    let worker = app
        .try_state::<ArchetypesWorker>()
        .ok_or_else(|| "the archetype worker is not running".to_string())?;
    let (reply, answer) = mpsc::channel();
    if !worker.signal(request(reply)) {
        return Err(STOPPED.to_string());
    }
    answer.recv().unwrap_or_else(|_| Err(STOPPED.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::lock;
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;

    /// Records what the loop asked for, in order.
    #[derive(Default)]
    struct Recorded {
        calls: Vec<String>,
        /// Backfill slices that say more is due before saying none is.
        more_slices: u32,
        /// What the change checks answer, in order (then `Applied`).
        check_runs: Vec<CheckRun>,
        /// Change checks that fail before those answer.
        check_failures: u32,
        /// Whether the reconcile releases held rows.
        reconcile_releases: bool,
    }

    /// The match id of a "Detect" that runs until something waits.
    const LONG_DETECT: i64 = 4_242;

    struct FakeJobs(Arc<Mutex<Recorded>>);

    impl FakeJobs {
        fn note(&self, call: String) {
            lock(&self.0).calls.push(call);
        }
    }

    impl Jobs for FakeJobs {
        fn detect(
            &mut self,
            match_ids: &[i64],
            should_yield: &dyn Fn() -> bool,
        ) -> Result<u32, String> {
            self.note(format!("detect {match_ids:?}"));
            if match_ids == [LONG_DETECT] {
                // A bulk "Detect" that runs until something waits.
                let deadline = Instant::now() + Duration::from_secs(5);
                while !should_yield() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
                self.note("detect yielded".to_string());
            }
            Ok(match_ids.len() as u32)
        }

        fn queue_deck(&mut self, deck_id: i64, filter: DeckFilter) -> Result<u32, String> {
            self.note(format!("queue {deck_id} {filter:?}"));
            Ok(2)
        }

        fn prefill_deck(&mut self, deck_id: i64) -> Result<(), String> {
            self.note(format!("prefill {deck_id}"));
            Ok(())
        }

        fn fetch_definitions(&mut self, _: &dyn Fn() -> bool) -> Result<CheckRun, String> {
            self.note("definitions".to_string());
            Ok(CheckRun::Offline)
        }

        fn check_definitions(&mut self, _: &dyn Fn() -> bool) -> Result<CheckRun, String> {
            self.note("check".to_string());
            let mut recorded = lock(&self.0);
            if recorded.check_failures > 0 {
                recorded.check_failures -= 1;
                return Err("the check failed".to_string());
            }
            if recorded.check_runs.is_empty() {
                return Ok(CheckRun::Applied);
            }
            Ok(recorded.check_runs.remove(0))
        }

        fn fetch_metagame(&mut self, slug: &str, days: u32) {
            self.note(format!("metagame {slug} {days}"));
        }

        fn fetch_decklists(&mut self, uuid: &str) -> Result<Vec<ArchetypeDeck>, String> {
            self.note(format!("decklists {uuid}"));
            Ok(Vec::new())
        }

        fn fetch_catalog(&mut self, format: &str) -> Result<Vec<CatalogArchetype>, String> {
            self.note(format!("catalog {format}"));
            Ok(Vec::new())
        }

        fn reconcile_offline(&mut self) -> Result<bool, String> {
            self.note("reconcile".to_string());
            Ok(lock(&self.0).reconcile_releases)
        }

        fn download_offline(&mut self) -> Result<OfflineHeld, String> {
            self.note("download offline".to_string());
            Ok(OfflineHeld {
                added: 2,
                converted: 1,
                skipped: 0,
            })
        }

        fn backfill(&mut self, _: &dyn Fn() -> bool) -> Result<bool, String> {
            self.note("backfill".to_string());
            let mut recorded = lock(&self.0);
            if recorded.more_slices > 0 {
                recorded.more_slices -= 1;
                return Ok(true);
            }
            Ok(false)
        }
    }

    /// No change check in the test's lifetime unless signalled.
    const NO_CHECK: CheckTimes = CheckTimes {
        first: Duration::from_secs(3_600),
        every: Duration::from_secs(3_600),
        retry: Duration::from_secs(3_600),
    };

    fn spawn(
        more_slices: u32,
    ) -> (
        Sender<ArchetypeSignal>,
        Arc<Mutex<Recorded>>,
        JoinHandle<()>,
    ) {
        spawn_with(more_slices, NO_CHECK, Vec::new(), 0)
    }

    fn spawn_with(
        more_slices: u32,
        times: CheckTimes,
        check_runs: Vec<CheckRun>,
        check_failures: u32,
    ) -> (
        Sender<ArchetypeSignal>,
        Arc<Mutex<Recorded>>,
        JoinHandle<()>,
    ) {
        let recorded = Arc::new(Mutex::new(Recorded {
            calls: Vec::new(),
            more_slices,
            check_runs,
            check_failures,
            reconcile_releases: false,
        }));
        let (signals, receiver) = mpsc::channel();
        let mut jobs = FakeJobs(Arc::clone(&recorded));
        let thread = std::thread::spawn(move || {
            let inbox = Inbox::new(receiver);
            run_archetypes_worker(&inbox, Duration::from_secs(60), &times, &mut jobs);
        });
        (signals, recorded, thread)
    }

    /// Wait until the worker has answered everything sent before this.
    fn settle(signals: &Sender<ArchetypeSignal>) {
        let (reply, answer) = mpsc::channel();
        signals
            .send(ArchetypeSignal::Detect {
                match_ids: Vec::new(),
                reply,
            })
            .expect("send");
        answer.recv().expect("answer").expect("ok");
    }

    /// Wait (at most 5 s) until the recorded calls satisfy `done`.
    fn wait_until(recorded: &Arc<Mutex<Recorded>>, done: impl Fn(&[String]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done(&calls(recorded)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn count(calls: &[String], call: &str) -> usize {
        calls.iter().filter(|seen| *seen == call).count()
    }

    // Spec section 5: the check waits `first` after start, runs once, and
    // its re-detection is picked up by the backfill.
    #[test]
    fn the_change_check_runs_after_the_first_delay() {
        let times = CheckTimes {
            first: Duration::from_millis(500),
            every: Duration::from_secs(3_600),
            retry: Duration::from_secs(3_600),
        };
        let started = Instant::now();
        let (signals, recorded, thread) = spawn_with(0, times, Vec::new(), 0);
        settle(&signals);
        if started.elapsed() < Duration::from_millis(400) {
            assert_eq!(count(&calls(&recorded), "check"), 0, "not before the delay");
        }
        wait_until(&recorded, |calls| count(calls, "check") >= 1);
        settle(&signals);
        signals.send(ArchetypeSignal::Stop).expect("stop");
        thread.join().expect("join");
        let calls = calls(&recorded);
        assert_eq!(count(&calls, "check"), 1, "{calls:?}");
        let checked = calls
            .iter()
            .position(|call| call == "check")
            .expect("check");
        assert!(
            calls[checked..].contains(&"backfill".to_string()),
            "{calls:?}"
        );
    }

    // 0.x UpdateOfflineModeControllerTest "resyncs the archetype catalog
    // when rejoining": leaving offline mode signals the check, which runs
    // at once.
    #[test]
    fn leaving_offline_mode_runs_the_change_check() {
        let (signals, recorded, thread) = spawn(0);
        settle(&signals);
        for signal in crate::api::offline::archetype_rejoin_signals() {
            signals.send(signal).expect("send");
        }
        settle(&signals);
        signals.send(ArchetypeSignal::Stop).expect("stop");
        thread.join().expect("join");
        assert_eq!(count(&calls(&recorded), "check"), 1);
    }

    // Spec section 5: after a 404 there is no change check until the next
    // start, rejoin or not.
    #[test]
    fn a_missing_check_endpoint_stops_the_checks_until_the_next_start() {
        let times = CheckTimes {
            first: Duration::ZERO,
            every: Duration::ZERO,
            retry: Duration::ZERO,
        };
        let (signals, recorded, thread) = spawn_with(0, times, vec![CheckRun::NotDeployed], 0);
        settle(&signals);
        signals
            .send(ArchetypeSignal::CheckDefinitions)
            .expect("send");
        std::thread::sleep(Duration::from_millis(100));
        settle(&signals);
        signals.send(ArchetypeSignal::Stop).expect("stop");
        thread.join().expect("join");
        assert_eq!(count(&calls(&recorded), "check"), 1);
    }

    fn calls(recorded: &Arc<Mutex<Recorded>>) -> Vec<String> {
        lock(recorded).calls.clone()
    }

    #[test]
    fn the_backfill_runs_at_start_until_the_queue_is_empty() {
        let (signals, recorded, thread) = spawn(2);
        let (reply, answer) = mpsc::channel();
        // A command waits for the start-up work in front of it, then runs.
        signals
            .send(ArchetypeSignal::Detect {
                match_ids: vec![7],
                reply,
            })
            .expect("send");
        assert_eq!(answer.recv().expect("answer"), Ok(1));
        // The command may be answered before the remaining slices run: wait
        // for them (bounded) before stopping, so the count is not a race.
        wait_until(&recorded, |calls| count(calls, "backfill") >= 3);
        signals.send(ArchetypeSignal::Stop).expect("stop");
        thread.join().expect("join");
        // Three slices drain the queue. A fourth is right when the worker
        // drained it before `Detect` arrived: a detection asks again.
        assert_eq!(lock(&recorded).more_slices, 0, "the queue is drained");
        let calls = calls(&recorded);
        assert!(count(&calls, "backfill") >= 3, "{calls:?}");
        assert_eq!(
            calls.iter().filter(|call| *call == "definitions").count(),
            1
        );
        assert!(calls.contains(&"detect [7]".to_string()));
    }

    // 0.x `DetermineMatchArchetypesJobUniqueTest` as "one detection per
    // match in flight": one thread runs one job at a time, in order, and a
    // deck synced twice is prefilled once.
    #[test]
    fn one_detection_per_match_in_flight() {
        let (signals, recorded, thread) = spawn(0);
        signals.send(ArchetypeSignal::DeckSynced(3)).expect("send");
        signals.send(ArchetypeSignal::DeckSynced(3)).expect("send");
        let (first, first_answer) = mpsc::channel();
        let (second, second_answer) = mpsc::channel();
        signals
            .send(ArchetypeSignal::Detect {
                match_ids: vec![5],
                reply: first,
            })
            .expect("send");
        signals
            .send(ArchetypeSignal::DetectDeck {
                deck_id: 3,
                filter: DeckFilter::NoOpponentArchetype,
                reply: second,
            })
            .expect("send");
        assert_eq!(first_answer.recv().expect("answer"), Ok(1));
        assert_eq!(second_answer.recv().expect("answer"), Ok(2));
        // The urgent detections set the prefill aside (#300 F1): a Stop sent
        // before the worker returns to it would drop it (#321). The backfill
        // waited for is the one after the deck banner's queue entry: the
        // start-up backfill can come before the signals.
        wait_until(&recorded, |calls| {
            let backfilled = calls
                .iter()
                .position(|call| call == "queue 3 NoOpponentArchetype")
                .is_some_and(|queued| calls[queued..].iter().any(|call| call == "backfill"));
            count(calls, "prefill 3") >= 1 && backfilled
        });
        signals.send(ArchetypeSignal::Stop).expect("stop");
        thread.join().expect("join");
        let calls = calls(&recorded);
        assert_eq!(
            calls.iter().filter(|call| *call == "prefill 3").count(),
            1,
            "{calls:?}"
        );
        assert_eq!(
            calls.iter().filter(|call| *call == "detect [5]").count(),
            1,
            "{calls:?}"
        );
        // The deck banner queues, then the backfill detects.
        let queued = calls
            .iter()
            .position(|call| call == "queue 3 NoOpponentArchetype")
            .expect("queued");
        assert!(
            calls[queued..].contains(&"backfill".to_string()),
            "{calls:?}"
        );
    }

    /// Overlay spec 7.3: a stub the overlay wrote asks for its definition.
    #[test]
    fn a_new_stub_asks_for_the_missing_definitions() {
        let (signals, recorded, thread) = spawn(0);
        // The worker's start-up fetch runs on its own, and may come after
        // `settle` is answered: wait for it, then count only what follows.
        wait_until(&recorded, |calls| count(calls, "definitions") >= 1);
        settle(&signals);
        let before = count(&calls(&recorded), "definitions");
        assert_eq!(before, 1, "the start-up fetch");
        signals
            .send(ArchetypeSignal::FetchDefinitions)
            .expect("send");
        settle(&signals);
        signals.send(ArchetypeSignal::Stop).expect("stop");
        thread.join().expect("join");
        assert_eq!(count(&calls(&recorded), "definitions"), before + 1);
    }

    #[test]
    fn a_completed_match_and_the_gate_wake_the_queue() {
        let (signals, recorded, thread) = spawn(0);
        // Past the start-up round first.
        let (reply, answer) = mpsc::channel();
        signals
            .send(ArchetypeSignal::Detect {
                match_ids: Vec::new(),
                reply,
            })
            .expect("send");
        answer.recv().expect("answer").expect("ok");
        signals.send(ArchetypeSignal::MatchCompleted).expect("send");
        signals.send(ArchetypeSignal::GateChanged).expect("send");
        signals
            .send(ArchetypeSignal::CheckDefinitions)
            .expect("send");
        let (reply, answer) = mpsc::channel();
        signals
            .send(ArchetypeSignal::Detect {
                match_ids: Vec::new(),
                reply,
            })
            .expect("send");
        answer.recv().expect("answer").expect("ok");
        signals.send(ArchetypeSignal::Stop).expect("stop");
        thread.join().expect("join");
        let calls = calls(&recorded);
        assert!(
            calls.iter().filter(|call| *call == "backfill").count() >= 2,
            "{calls:?}"
        );
        assert!(
            calls.iter().filter(|call| *call == "definitions").count() >= 2,
            "{calls:?}"
        );
    }

    // Spec sections 9 and 10 (ruling C10): the global shares and the live
    // decklists are fetched on the worker, never on the IPC thread.
    /// PR #517 review: at start and on a gate change the worker releases
    /// rows held offline while online, then downloads their definitions
    /// and runs the check.
    #[test]
    fn a_released_reconcile_fetches_and_checks_definitions() {
        let (signals, recorded, thread) = spawn(0);
        settle(&signals);
        // The worker answers a Detect in the same pass, before it reaches
        // the reconcile, so the start's reconcile may land just after.
        wait_until(&recorded, |calls| count(calls, "reconcile") >= 1);
        assert_eq!(count(&calls(&recorded), "reconcile"), 1, "at start");
        assert_eq!(count(&calls(&recorded), "check"), 0);
        lock(&recorded).reconcile_releases = true;
        signals.send(ArchetypeSignal::GateChanged).expect("send");
        settle(&signals);
        signals.send(ArchetypeSignal::Stop).expect("stop");
        thread.join().expect("join");
        let calls = calls(&recorded);
        let reconcile = calls
            .iter()
            .rposition(|c| c == "reconcile")
            .expect("reconciled");
        let fetch = calls
            .iter()
            .rposition(|c| c == "definitions")
            .expect("fetched");
        assert!(reconcile < fetch, "{calls:?}");
        assert!(count(&calls, "check") >= 1, "{calls:?}");
    }

    #[test]
    fn the_offline_download_runs_on_the_worker_and_replies() {
        let (signals, recorded, thread) = spawn(0);
        let (reply, answer) = mpsc::channel();
        signals
            .send(ArchetypeSignal::DownloadOffline { reply })
            .expect("send");
        assert_eq!(
            answer.recv().expect("answer"),
            Ok(OfflineHeld {
                added: 2,
                converted: 1,
                skipped: 0,
            })
        );
        signals.send(ArchetypeSignal::Stop).expect("stop");
        thread.join().expect("join");
        assert_eq!(count(&calls(&recorded), "download offline"), 1);
        assert!(urgent(&ArchetypeSignal::DownloadOffline {
            reply: mpsc::channel().0
        }));
    }

    #[test]
    fn the_metagame_and_the_decklists_are_fetched_on_the_worker() {
        let (signals, recorded, thread) = spawn(0);
        signals
            .send(ArchetypeSignal::FetchMetagame {
                slug: "modern".to_string(),
                days: 30,
            })
            .expect("send");
        let (reply, answer) = mpsc::channel();
        signals
            .send(ArchetypeSignal::FetchDecklists {
                uuid: "q-uuid".to_string(),
                reply,
            })
            .expect("send");
        assert_eq!(answer.recv().expect("answer"), Ok(Vec::new()));
        signals.send(ArchetypeSignal::Stop).expect("stop");
        thread.join().expect("join");
        let calls = calls(&recorded);
        assert_eq!(count(&calls, "metagame modern 30"), 1, "{calls:?}");
        assert_eq!(count(&calls, "decklists q-uuid"), 1, "{calls:?}");
        // A page waits: a running backfill yields to it.
        assert!(urgent(&ArchetypeSignal::FetchMetagame {
            slug: "modern".to_string(),
            days: 7,
        }));
    }

    /// #300 F1: a running bulk "Detect" stops for `Stop`, so stopping the
    /// worker never waits minutes for it.
    #[test]
    fn a_running_detect_stops_for_stop() {
        let (signals, recorded, thread) = spawn(0);
        settle(&signals);
        let (reply, answer) = mpsc::channel();
        signals
            .send(ArchetypeSignal::Detect {
                match_ids: vec![LONG_DETECT],
                reply,
            })
            .expect("send");
        wait_until(&recorded, |calls| {
            calls.contains(&format!("detect [{LONG_DETECT}]"))
        });
        let stopped = Instant::now();
        signals.send(ArchetypeSignal::Stop).expect("stop");
        thread.join().expect("join");
        assert!(stopped.elapsed() < Duration::from_secs(2));
        assert_eq!(answer.recv().expect("answered"), Ok(1));
        assert!(calls(&recorded).contains(&"detect yielded".to_string()));
    }

    /// #300 F1 review: a Detect drained in the same batch as `Stop` still
    /// sees the `Stop` behind it and stops at once.
    #[test]
    fn a_detect_batched_with_stop_stops() {
        let recorded = Arc::new(Mutex::new(Recorded::default()));
        let (signals, receiver) = mpsc::channel();
        let (reply, answer) = mpsc::channel();
        signals
            .send(ArchetypeSignal::Detect {
                match_ids: vec![LONG_DETECT],
                reply,
            })
            .expect("send");
        signals.send(ArchetypeSignal::Stop).expect("stop");
        let mut jobs = FakeJobs(Arc::clone(&recorded));
        let started = Instant::now();
        let thread = std::thread::spawn(move || {
            let inbox = Inbox::new(receiver);
            run_archetypes_worker(&inbox, Duration::from_secs(60), &NO_CHECK, &mut jobs);
        });
        thread.join().expect("join");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(answer.recv().expect("answered"), Ok(1));
        drop(signals);
        assert!(calls(&recorded).contains(&"detect yielded".to_string()));
    }

    /// #300 F1: the pacing pause sleeps on the inbox: a signal the job
    /// stops for ends it at once, others are set aside for the loop; the
    /// plain pause sets them aside and runs to its end.
    #[test]
    fn the_pacing_pause_sleeps_on_the_inbox() {
        use crate::archetypes::definitions::Clock;
        let (signals, receiver) = mpsc::channel();
        let inbox = std::rc::Rc::new(Inbox::new(receiver));
        let clock = InboxClock(std::rc::Rc::clone(&inbox));
        signals
            .send(ArchetypeSignal::FetchDefinitions)
            .expect("send");
        let started = Instant::now();
        clock.sleep_ms(30);
        assert!(started.elapsed() >= Duration::from_millis(30));
        let sender = signals.clone();
        let late = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            sender.send(ArchetypeSignal::MatchCompleted).expect("send");
            sender.send(ArchetypeSignal::Stop).expect("send");
        });
        let started = Instant::now();
        assert!(!clock.sleep_or_yield_ms(60_000));
        assert!(started.elapsed() < Duration::from_secs(5));
        late.join().expect("join");
        let pending: Vec<String> = inbox
            .take_pending()
            .iter()
            .map(|signal| format!("{signal:?}"))
            .collect();
        assert_eq!(pending, ["FetchDefinitions", "MatchCompleted", "Stop"]);
        assert!(inbox.should_yield() || inbox.take_pending().is_empty());
    }

    /// #300 F6: a failed change check is tried again after `retry`, not a
    /// whole `every`.
    #[test]
    fn a_failed_change_check_is_tried_again_soon() {
        let times = CheckTimes {
            first: Duration::ZERO,
            every: Duration::from_secs(3_600),
            retry: Duration::from_millis(50),
        };
        let (signals, recorded, thread) = spawn_with(0, times, Vec::new(), 1);
        wait_until(&recorded, |calls| count(calls, "check") >= 2);
        settle(&signals);
        signals.send(ArchetypeSignal::Stop).expect("stop");
        thread.join().expect("join");
        assert_eq!(count(&calls(&recorded), "check"), 2);
    }

    // At launch or wake the network is often not up yet: a check that could
    // not reach MyMTGO is tried again after 1, 2, 4 and 8 minutes, then
    // after the usual failure wait. Any other end starts the count over.
    #[test]
    fn a_check_that_could_not_reach_mymtgo_is_tried_again_with_backoff() {
        let minutes = |count: u64| Some(Duration::from_secs(count * 60));
        let mut retry = TransientRetry::default();
        let unreachable = Ok(CheckRun::Unreachable);
        let waits: Vec<Option<Duration>> = (0..5)
            .map(|_| after_check(&unreachable, &CHECK_TIMES, &mut retry))
            .collect();
        assert_eq!(
            waits,
            [
                minutes(1),
                minutes(2),
                minutes(4),
                minutes(8),
                Some(CHECK_TIMES.retry)
            ]
        );
        let applied = after_check(&Ok(CheckRun::Applied), &CHECK_TIMES, &mut retry);
        assert_eq!(applied, Some(CHECK_TIMES.every));
        let again = after_check(&unreachable, &CHECK_TIMES, &mut retry);
        assert_eq!(again, minutes(1), "the count started over");
        let failed = after_check(&Err("HTTP 500".to_string()), &CHECK_TIMES, &mut retry);
        assert_eq!(failed, Some(CHECK_TIMES.retry));
        assert_eq!(retry, TransientRetry::default());
    }

    #[test]
    fn the_worker_ends_when_every_sender_is_gone() {
        let (signals, _recorded, thread) = spawn(0);
        drop(signals);
        thread.join().expect("join");
    }
}
