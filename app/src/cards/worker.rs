//! The `cards` worker thread (cards spec sections 6 and 7, ruling 1): one
//! thread, separate from the `api` worker, that runs card population and the
//! local images (downloads and the 0.x copy), one at a time (0.x's
//! single `card_downloads` queue worker plus its hourly schedule).
//!
//! When it runs: hourly from the last run; `STUBS_DEBOUNCE` after new stubs
//! (so a deck sync with 60 new ids runs once); at once after a sync pull
//! (with a stub scan first) and when asked with a reply ("Download card
//! data"). One run at a time: signals that arrive during a run are folded
//! into one more run after it. Like a Laravel queue worker whose jobs are
//! unique while queued.

use std::cell::RefCell;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use crate::api::session::ApiState;
use crate::bridge::lock;
use crate::cards::folder::{MoveRequest, run_move, switch_folder};
use crate::cards::images::{Fetcher, ImageRun, ImagesState, image_round};
use crate::cards::legacy_images::PendingCopy;
use crate::cards::populate::{Population, card_data_source, populate};
use crate::commands::{Startup, ready};
use crate::worker_thread::WorkerThread;

/// How often population runs without a trigger (0.x's hourly schedule).
pub const EVERY: Duration = Duration::from_secs(3_600);
/// How long after new stubs population runs; more stubs in the meantime
/// join the same run.
pub const STUBS_DEBOUNCE: Duration = Duration::from_secs(5);
/// How long queued image downloads that failed wait before they are asked
/// again (0.x `backoff` 30 s).
pub const IMAGES_RETRY: Duration = Duration::from_secs(30);
/// The answer a waiting button gets when the worker stops first.
pub const STOPPED: &str = "the card worker stopped";

/// What wakes the worker.
#[derive(Debug)]
pub enum CardsSignal {
    /// Run now; the reply (if any) gets the run's result.
    Populate {
        reply: Option<Sender<Result<(), String>>>,
        /// Stubs the caller's own scan added just before.
        scanned: u32,
    },
    /// The tracker wrote `count` new card stubs (`CardStubsAdded`).
    StubsAdded {
        count: u32,
    },
    /// A sync pull imported something: scan for stubs, then run.
    SyncPulled,
    /// Image work is waiting (a 0.x import handed its folder): copy it,
    /// then download.
    ImagesQueued,
    /// Move the images folder (validated and marked running already).
    Move(MoveRequest),
    Stop,
}

/// What one run does. A population run is followed by the image downloads
/// (the rows it resolved, and failures retried).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Round {
    /// Card population.
    pub populate: bool,
    /// A stub scan before population (after a sync pull).
    pub scan_first: bool,
    /// A button waits on population: the ids the API did not know are
    /// asked about again (#501).
    pub ask_unknown: bool,
    /// Image work is waiting: download.
    pub download: bool,
    /// Folder moves, run first, so no download lands mid-move.
    pub moves: Vec<MoveRequest>,
    /// Where a cut download run stopped: the downloads go on after it.
    pub resume_after: Option<u32>,
}

/// What one run did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    /// Population's result (answers the waiting replies).
    pub population: Result<(), String>,
    /// The downloads stopped for waiting work: they go on after it.
    pub downloads_cut: bool,
    /// The catalog id a cut run finished last.
    pub resume_after: Option<u32>,
    /// The round's queued downloads did not run (the app not ready, the
    /// setting unreadable, the run failed): asked again after
    /// `Timing::retry`.
    pub downloads_failed: bool,
}

impl Ran {
    /// A run whose image work, if any, is done.
    pub fn done(population: Result<(), String>) -> Ran {
        Ran {
            population,
            downloads_cut: false,
            resume_after: None,
            downloads_failed: false,
        }
    }
}

/// Whether a signal waiting during a run makes the image downloads stop for
/// it: a move, a button waiting for population, or stopping.
fn interrupts(signal: &CardsSignal) -> bool {
    match signal {
        CardsSignal::Populate { reply, .. } => reply.is_some(),
        CardsSignal::Move(_) | CardsSignal::Stop => true,
        CardsSignal::StubsAdded { .. } | CardsSignal::SyncPulled | CardsSignal::ImagesQueued => {
            false
        }
    }
}

/// When the worker runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// Between runs without a trigger.
    pub every: Duration,
    /// After `StubsAdded`.
    pub debounce: Duration,
    /// After the worker starts (stubs left unresolved by the last session).
    pub first_run: Duration,
    /// After queued downloads failed (0.x's first backoff, 30 s).
    pub retry: Duration,
}

impl Default for Timing {
    fn default() -> Timing {
        Timing {
            every: EVERY,
            debounce: STUBS_DEBOUNCE,
            first_run: STUBS_DEBOUNCE,
            retry: IMAGES_RETRY,
        }
    }
}

/// The worker's loop. `run(round, should_yield)` is one run (moves, then
/// population with a stub scan first after a sync pull, then the image
/// work). Its population result answers the waiting replies. The image
/// downloads ask `should_yield` between files: true when a move,
/// a waiting button or a stop arrived, and the downloads then go on after
/// it. Ends on `Stop` or when every sender is gone; replies still waiting
/// then get `STOPPED`.
pub fn run_cards_worker(
    inbox: &Receiver<CardsSignal>,
    timing: Timing,
    run: &mut dyn FnMut(&Round, &dyn Fn() -> bool) -> Ran,
) {
    // Population: hourly from the last population run, and when a signal
    // makes it due. Moves run at once. Queued downloads: at
    // once, after a cut run from where it stopped, and after a round that
    // could not run them, once `retry` has passed.
    let mut last_run = Instant::now();
    let mut populate_at: Option<Instant> = Some(last_run + timing.first_run);
    let mut scan_first = false;
    let mut ask_unknown = false;
    let mut download_at: Option<Instant> = None;
    let mut resume_after: Option<u32> = None;
    let mut moves: Vec<MoveRequest> = Vec::new();
    let mut replies: Vec<Sender<Result<(), String>>> = Vec::new();
    let mut echo = Echo::default();
    // Signals read while a run asked whether to yield, handled next.
    let pending: RefCell<Vec<CardsSignal>> = RefCell::new(Vec::new());
    loop {
        let hourly = last_run + timing.every;
        let mut wake = populate_at.map_or(hourly, |at| at.min(hourly));
        if let Some(at) = download_at {
            wake = wake.min(at);
        }
        if !moves.is_empty() {
            wake = Instant::now();
        }
        let mut signals: Vec<CardsSignal> = pending.borrow_mut().drain(..).collect();
        if signals.is_empty() {
            match inbox.recv_timeout(wake.saturating_duration_since(Instant::now())) {
                Ok(signal) => signals.push(signal),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return answer_stopped(replies),
            }
        }
        // Everything already waiting joins this round, so a burst runs once.
        while let Ok(signal) = inbox.try_recv() {
            signals.push(signal);
        }
        let now = Instant::now();
        for signal in signals {
            match signal {
                CardsSignal::Stop => return answer_stopped(replies),
                CardsSignal::Populate { reply, scanned } => {
                    ask_unknown |= reply.is_some();
                    replies.extend(reply);
                    populate_at = Some(now);
                    echo.expect(scanned, now + timing.debounce);
                }
                CardsSignal::SyncPulled => {
                    scan_first = true;
                    populate_at = Some(now);
                }
                CardsSignal::StubsAdded { count } => {
                    if echo.absorb(count, now) > 0 {
                        let at = now + timing.debounce;
                        populate_at = Some(populate_at.map_or(at, |earlier| earlier.min(at)));
                    }
                }
                CardsSignal::ImagesQueued => {
                    // New rows may sit before a cut run's cursor.
                    download_at = Some(now);
                    resume_after = None;
                }
                CardsSignal::Move(request) => moves.push(request),
            }
        }
        let now = Instant::now();
        let populate = populate_at.is_some_and(|at| at <= now) || hourly <= now;
        let download = download_at.is_some_and(|at| at <= now);
        if !populate && !download && moves.is_empty() {
            continue;
        }
        let round = Round {
            populate,
            scan_first: populate && scan_first,
            ask_unknown: populate && ask_unknown,
            download,
            moves: std::mem::take(&mut moves),
            resume_after: resume_after.filter(|_| download),
        };
        let should_yield = || {
            let mut waiting = pending.borrow_mut();
            while let Ok(signal) = inbox.try_recv() {
                waiting.push(signal);
            }
            waiting.iter().any(interrupts)
        };
        let ran = run(&round, &should_yield);
        if let Err(error) = &ran.population {
            tracing::warn!(%error, "card population failed");
        }
        // A population run downloads too (from the start of the queue).
        let downloads_ran = download || populate;
        if ran.downloads_cut {
            download_at = Some(Instant::now());
            resume_after = ran.resume_after;
        } else if downloads_ran && ran.downloads_failed {
            download_at = Some(Instant::now() + timing.retry);
        } else if downloads_ran {
            download_at = None;
            resume_after = None;
        }
        if populate {
            for reply in replies.drain(..) {
                let _ = reply.send(ran.population.clone());
            }
            last_run = Instant::now();
            populate_at = None;
            scan_first = false;
            ask_unknown = false;
            // The scan's own report may still be on its way.
            echo.extend_to(Instant::now() + timing.debounce);
        }
    }
}

/// The stubs a caller's own scan added before it asked for population
/// (S2-2): the tracker reports them as `StubsAdded` a tick later, and the
/// run already covers them, so that many are not a reason for another run.
/// Only within `until`: a report that never came is not waited for.
#[derive(Debug, Default)]
struct Echo {
    count: u32,
    until: Option<Instant>,
}

impl Echo {
    fn expect(&mut self, scanned: u32, until: Instant) {
        if scanned > 0 {
            self.count += scanned;
            self.until = Some(until);
        }
    }

    fn extend_to(&mut self, until: Instant) {
        if self.count > 0 {
            self.until = Some(until);
        }
    }

    /// The stubs of `count` left after the expected ones are taken off.
    fn absorb(&mut self, count: u32, now: Instant) -> u32 {
        if self.until.is_none_or(|until| now > until) {
            self.count = 0;
            self.until = None;
            return count;
        }
        let absorbed = count.min(self.count);
        self.count -= absorbed;
        count - absorbed
    }
}

fn answer_stopped(replies: Vec<Sender<Result<(), String>>>) {
    for reply in replies {
        let _ = reply.send(Err(STOPPED.to_string()));
    }
}

/// The running `cards` thread, managed by Tauri.
pub struct CardsWorker {
    thread: WorkerThread<CardsSignal>,
    /// 0.x's image folder after an import, copied before any download
    /// (cards spec section 13; the copy is wired into the run with the
    /// download queue).
    pub legacy_images: PendingCopy,
}

impl CardsWorker {
    /// Start the thread. It reads `ApiState` and `Startup` from the app, so
    /// both are managed first.
    pub fn start(app: AppHandle) -> CardsWorker {
        let worker_app = app.clone();
        let thread = WorkerThread::spawn(
            "cards",
            "could not start the cards worker; card data is not filled",
            move |inbox| {
                let app = worker_app;
                let mut run = |round: &Round, should_yield: &dyn Fn() -> bool| {
                    run_in_app(&app, round, should_yield)
                };
                run_cards_worker(&inbox, Timing::default(), &mut run);
            },
        );
        let legacy_images = PendingCopy::default();
        if let Some(folder) = legacy_copy_at_start(&app) {
            legacy_images.hand(folder);
        }
        CardsWorker {
            thread,
            legacy_images,
        }
    }

    /// Wake the worker with a signal. Returns false once stopped.
    pub fn signal(&self, signal: CardsSignal) -> bool {
        self.thread.send(signal)
    }

    /// Stop the worker, waiting up to `STOP_WAIT` for it (a request in
    /// flight can take 15 seconds; a run cut short is asked again by the
    /// next start).
    pub fn stop(&self) {
        self.thread.stop(
            CardsSignal::Stop,
            "the cards worker is still asking the API; leaving it",
        );
    }
}

/// 0.x's image folder to copy at start (S3-4): rebuilt from the stored
/// import, so the copy survives a quit before it ran. The first run copies it.
fn legacy_copy_at_start(app: &AppHandle) -> Option<std::path::PathBuf> {
    let startup = app.try_state::<Startup>()?;
    let state = ready(&startup).ok()?;
    let rows = match store::read::settings(&lock(&state.reader)) {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, "could not read the settings for the 0.x image copy");
            return None;
        }
    };
    crate::cards::legacy_images::pending_at_start(&rows)
}

/// Wake the app's cards worker, if it runs.
pub fn signal(app: &AppHandle, signal: CardsSignal) {
    if let Some(worker) = app.try_state::<CardsWorker>() {
        worker.signal(signal);
    }
}

/// Hand 0.x's image folder to the app's worker after an import, and wake it:
/// the copy runs, then the downloads.
pub fn hand_legacy_images(app: &AppHandle, folder: std::path::PathBuf) {
    if let Some(worker) = app.try_state::<CardsWorker>() {
        worker.legacy_images.hand(folder);
        worker.signal(CardsSignal::ImagesQueued);
    }
}

/// Ask the app's worker for a run now and wait for its result. `scanned` is
/// how many stubs the caller's own scan just added.
pub fn populate_now(app: &AppHandle, scanned: u32) -> Result<(), String> {
    let worker = app
        .try_state::<CardsWorker>()
        .ok_or_else(|| "the card worker is not running".to_string())?;
    let (reply, answer) = mpsc::channel();
    if !worker.signal(CardsSignal::Populate {
        reply: Some(reply),
        scanned,
    }) {
        return Err(STOPPED.to_string());
    }
    answer.recv().unwrap_or_else(|_| Err(STOPPED.to_string()))
}

/// One run with the app's state: moves, population when due (the stub
/// scan first when asked), then the image downloads. A run that cannot start (the app not ready,
/// tracking restarting) fails population and marks the queued downloads
/// for a retry.
fn run_in_app(app: &AppHandle, round: &Round, should_yield: &dyn Fn() -> bool) -> Ran {
    match run_parts(app, round, should_yield) {
        Ok(ran) => ran,
        Err(error) => Ran {
            population: Err(error),
            downloads_cut: false,
            resume_after: None,
            downloads_failed: round.download,
        },
    }
}

fn run_parts(
    app: &AppHandle,
    round: &Round,
    should_yield: &dyn Fn() -> bool,
) -> Result<Ran, String> {
    let startup = app
        .try_state::<Startup>()
        .ok_or_else(|| "the app is not ready".to_string())?;
    let state = ready(&startup).map_err(|_| "the app is not ready".to_string())?;
    // Moves first, and always ended: a tracker that is restarting fails the
    // move's switch, which records the error.
    if let Some(images) = app.try_state::<ImagesState>() {
        let present = |files| {
            state
                .tracker_part(tracker::TrackerHandle::card_images)
                .map_err(|_| "tracking is restarting; try again".to_string())?
                .present(files)
        };
        for request in &round.moves {
            run_move(
                &images,
                request,
                &|to| switch_folder(state, &images, to),
                &present,
            );
        }
    }
    let (scanner, resolver, recorder) = {
        let tracker = lock(&state.tracker);
        let tracker = tracker
            .as_ref()
            .ok_or_else(|| "tracking is restarting; try again".to_string())?;
        (
            tracker.card_stub_scanner(),
            tracker.cards_resolver(),
            tracker.card_images(),
        )
    };
    let db_path = state.db_path.clone();
    let reader = move || store::open_reader(&db_path).ok();
    let population = if round.populate {
        if round.scan_first
            && let Err(error) = scanner.scan()
        {
            tracing::warn!(%error, "the card stub scan after a sync pull failed");
        }
        let data_folder = lock(&state.tracking_folders)
            .mtgo_data_folder
            .clone()
            .map(std::path::PathBuf::from);
        let api = app.state::<ApiState>();
        let card_data =
            move || card_data_source(&mtgo_log::discover::Env::current(), data_folder.as_deref());
        let resolve = |tokens, resolutions, unknown_at_ms| {
            resolver.resolve(tokens, resolutions, unknown_at_ms)
        };
        let save_settings = |rows| resolver.save_settings(rows);
        populate(&Population {
            api: &api,
            reader: &reader,
            card_data: &card_data,
            resolve: &resolve,
            save_settings: &save_settings,
            now: &crate::wall_clock_ms,
            ask_unknown: round.ask_unknown,
        })
    } else {
        Ok(())
    };
    let mut ran = Ran::done(population);
    let images = app.try_state::<ImagesState>();
    let worker = app.try_state::<CardsWorker>();
    let (Some(images), Some(worker)) = (images, worker) else {
        return Ok(ran);
    };
    // Each run resolves the folder at its start.
    let folder = images.folder();
    let fetcher = Fetcher::standard();
    let run = ImageRun {
        folder: &folder.path,
        is_default: folder.is_default,
        // A population run may have resolved rows anywhere in the queue.
        start_after: if round.populate {
            0
        } else {
            round.resume_after.unwrap_or(0)
        },
        fetcher: &fetcher,
        reader: &reader,
        recorder: &recorder,
        present: &|files| recorder.present(files),
        legacy: &worker.legacy_images,
        should_yield,
        fetch_pause: crate::cards::images::FETCH_PAUSE,
    };
    let queued = round.populate || round.download;
    match image_round(&run, queued) {
        Ok(report) => {
            ran.downloads_cut = report.interrupted;
            ran.resume_after = report.resume_after;
            ran.downloads_failed = report.rate_limited;
        }
        Err(error) => {
            tracing::warn!(%error, "the card images run failed");
            ran.downloads_failed = round.download;
        }
    }
    Ok(ran)
}

#[cfg(test)]
mod tests;
