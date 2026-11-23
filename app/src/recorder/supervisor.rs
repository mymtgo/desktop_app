//! Keeping the recorder running while MyMTGO runs (recorded replays spec 6).
//! The decisions are plain functions with tests; the loop that downloads,
//! spawns and watches the process is `run` (checked by hand on Windows).

use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use recorder::{
    CrashWindow, ExitKind, PIN, Pin, RecorderState, download_blocked, exit_kind, respawns,
};
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::api::offline::offline_mode_now;
use crate::api::session::ApiState;
use crate::bridge::lock;
use crate::cards::images::Fetcher;
use crate::commands::{Startup, read_settings, ready};
use crate::recorder::import::recorder_folder;
use crate::recorder::install::{InstallError, install, verified_exe};
use crate::recorder::process::{read_status, spawn, status_file_in, stop_child};
use crate::settings::{RecorderAcceptance, Settings};
use crate::worker_thread::WorkerThread;

/// The risk notice's wording version, bumped when its text changes.
pub const NOTICE_VERSION: u32 = 1;

/// What the Replays page shows under the switch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum RecorderStatus {
    Off,
    Unsupported,
    /// This MyMTGO version pins no recorder build yet (`download_blocked`).
    NoBuild,
    Downloading,
    Failed {
        reason: String,
    },
    BlockedByAntivirus,
    Waiting,
    /// Attached to MTGO, between matches.
    Ready,
    Recording,
    AnotherRunning,
    /// The recorder could not attach to MTGO (exit 4): an MTGO it does not
    /// support, or one still starting. Tried again every minute.
    MtgoNotSupported,
    StoppedAfterCrashes,
}

/// What follows a recorder exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Respawn,
    Stop(RecorderStatus),
}

/// Whether the notice must be shown (again) before the recorder may run.
pub fn needs_acceptance(acc: Option<&RecorderAcceptance>, pin: &Pin) -> bool {
    match acc {
        None => true,
        Some(acc) => {
            acc.notice_version < NOTICE_VERSION || acc.consent_version < pin.consent_version
        }
    }
}

/// What to do after an exit. `tripped` is the crash window's verdict.
pub fn next_step(kind: ExitKind, tripped: bool) -> Step {
    if tripped && respawns(kind) {
        return Step::Stop(RecorderStatus::StoppedAfterCrashes);
    }
    match kind {
        ExitKind::Stopped | ExitKind::Crashed => Step::Respawn,
        ExitKind::AlreadyRunning => Step::Stop(RecorderStatus::AnotherRunning),
        ExitKind::MtgoNotSupported => Step::Stop(RecorderStatus::MtgoNotSupported),
        ExitKind::ConsentNotAccepted => Step::Stop(RecorderStatus::Failed {
            reason: "The recorder says its notice was not accepted".to_string(),
        }),
        ExitKind::BadArguments => Step::Stop(RecorderStatus::Failed {
            reason: "The recorder did not accept its start arguments".to_string(),
        }),
    }
}

/// How long a halted supervisor waits before trying again by itself; `None`
/// waits for a Retry. Another recorder may close, and exit 4 also covers an
/// MTGO that is still starting, so both are tried again every minute.
pub fn retry_after(status: &RecorderStatus) -> Option<Duration> {
    match status {
        RecorderStatus::AnotherRunning | RecorderStatus::MtgoNotSupported => Some(HALT_RETRY),
        _ => None,
    }
}

/// Whether a download may start now.
pub fn may_download(offline: bool, pin: &Pin) -> bool {
    !offline && !download_blocked(pin)
}

/// What a busy loop (downloading or watching) does with a signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Busy {
    /// Carry on with what it was doing.
    Continue,
    /// Stop what it was doing and look at the settings from the top.
    Again,
    /// Close down.
    Quit,
}

/// Any signal while busy: Stop quits; otherwise a switch that is no longer
/// wanted ends the work, and a wanted one carries on.
pub fn on_signal_while_busy(signal: &RecorderSignal, still_wanted: bool) -> Busy {
    match signal {
        RecorderSignal::Stop => Busy::Quit,
        _ if !still_wanted => Busy::Again,
        _ => Busy::Continue,
    }
}

/// What the exe on disk is after an install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExeCheck {
    Ready,
    BlockedByAntivirus,
    Failed(String),
}

/// Judge the exe after an install: `exists` (the file is there) and
/// `verified` (it matches its pin).
pub fn check_after_install(exists: bool, verified: bool) -> ExeCheck {
    match (exists, verified) {
        (_, true) => ExeCheck::Ready,
        (false, false) => ExeCheck::BlockedByAntivirus,
        (true, false) => {
            ExeCheck::Failed("The downloaded recorder does not match its pin".to_string())
        }
    }
}

/// The recorder's state as the page shows it. `Idle` (attached, between
/// matches) is `Ready`.
pub fn status_for(state: RecorderState) -> RecorderStatus {
    match state {
        RecorderState::Recording { .. } => RecorderStatus::Recording,
        RecorderState::Waiting => RecorderStatus::Waiting,
        RecorderState::Idle => RecorderStatus::Ready,
    }
}

/// What the Replays page may show: the worker's status, except that a pin
/// with no build (`blocked`), the platform and the switch come first, and a
/// switch that was just turned on (the worker has not reported yet) reads as
/// a neutral waiting.
pub fn view_status(
    blocked: bool,
    supported: bool,
    enabled: bool,
    worker: &RecorderStatus,
) -> RecorderStatus {
    if blocked {
        RecorderStatus::NoBuild
    } else if !supported {
        RecorderStatus::Unsupported
    } else if !enabled {
        RecorderStatus::Off
    } else if *worker == RecorderStatus::Off {
        RecorderStatus::Waiting
    } else {
        worker.clone()
    }
}

/// Whether this platform runs the recorder at all.
pub const SUPPORTED: bool = cfg!(windows);

/// Pause before starting an exited recorder again.
const RESPAWN_PAUSE: Duration = Duration::from_secs(2);
/// How often the process and its status file are looked at.
const WATCH: Duration = Duration::from_secs(5);
/// How long closing MyMTGO waits for the worker (grace plus a margin).
pub const EXIT_WAIT: Duration = Duration::from_secs(12);
/// Waits between download tries (0.x: 30 s, then 120 s).
const BACKOFF: [Duration; 2] = [Duration::from_secs(30), Duration::from_secs(120)];
/// How often "another recorder is running" and "MTGO not supported" look again.
const HALT_RETRY: Duration = Duration::from_secs(60);
/// The download's total time limit and size cap.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
const DOWNLOAD_MAX_BYTES: u64 = 256 * 1024 * 1024;

/// What wakes the supervisor.
#[derive(Debug, PartialEq, Eq)]
pub enum RecorderSignal {
    /// The switch was turned off or on: look at the settings again. Sent by
    /// `set_recorder_enabled`.
    Wake,
    /// Start again after a failure or a stop (`retry_recorder`, and
    /// `accept_recorder_notice`, which also turns the switch on).
    Retry,
    Stop,
}

/// The running `recorder-supervisor` thread and the status it reports,
/// managed by Tauri.
pub struct RecorderWorker {
    thread: WorkerThread<RecorderSignal>,
    status: Arc<Mutex<RecorderStatus>>,
}

impl RecorderWorker {
    pub fn start(app: AppHandle) -> RecorderWorker {
        let status = Arc::new(Mutex::new(RecorderStatus::Off));
        let shared = Arc::clone(&status);
        let thread = WorkerThread::spawn(
            "recorder-supervisor",
            "could not start the recorder supervisor; the recorder is not run",
            move |inbox| run(&app, &shared, &inbox),
        );
        RecorderWorker { thread, status }
    }

    pub fn status(&self) -> RecorderStatus {
        lock(&self.status).clone()
    }

    pub fn send(&self, signal: RecorderSignal) {
        self.thread.send(signal);
    }

    /// Stop the recorder and the thread, waiting up to `EXIT_WAIT`.
    pub fn stop(&self) {
        self.thread.stop_within(
            RecorderSignal::Stop,
            EXIT_WAIT,
            "the recorder is still stopping; leaving it",
        );
    }
}

fn set_status(shared: &Mutex<RecorderStatus>, status: RecorderStatus) {
    *lock(shared) = status;
}

/// Whether the settings ask for a running recorder.
fn wanted(settings: &Settings) -> bool {
    settings.recorder_enabled && !needs_acceptance(settings.recorder_acceptance.as_ref(), &PIN)
}

/// Wait for a signal, or `timeout` (None waits for ever). `None` back means
/// the timeout passed; a closed inbox reads as Stop.
fn wait(inbox: &Receiver<RecorderSignal>, timeout: Option<Duration>) -> Option<RecorderSignal> {
    match timeout {
        None => Some(inbox.recv().unwrap_or(RecorderSignal::Stop)),
        Some(timeout) => match inbox.recv_timeout(timeout) {
            Ok(signal) => Some(signal),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(RecorderSignal::Stop),
        },
    }
}

fn still_wanted(app: &AppHandle) -> bool {
    current_settings(app).is_some_and(|settings| wanted(&settings))
}

fn current_settings(app: &AppHandle) -> Option<Settings> {
    let startup = app.try_state::<Startup>()?;
    let state = ready(&startup).ok()?;
    read_settings(state).ok()
}

/// How a pass ends.
enum Pass {
    /// Look at the settings again and go on.
    Again,
    /// Stay stopped with this status until a Retry (or a timeout).
    Halt(RecorderStatus),
    /// Close down.
    Quit,
}

fn run(app: &AppHandle, shared: &Mutex<RecorderStatus>, inbox: &Receiver<RecorderSignal>) {
    let mut crashes = CrashWindow::default();
    loop {
        let on = current_settings(app).is_some_and(|settings| wanted(&settings));
        if !SUPPORTED || !on {
            set_status(
                shared,
                if SUPPORTED {
                    RecorderStatus::Off
                } else {
                    RecorderStatus::Unsupported
                },
            );
            if wait(inbox, None) == Some(RecorderSignal::Stop) {
                return;
            }
            continue;
        }
        match one_pass(app, shared, inbox, &mut crashes) {
            Pass::Again => {}
            Pass::Quit => return,
            Pass::Halt(status) => {
                let pause = retry_after(&status);
                set_status(shared, status);
                loop {
                    match wait(inbox, pause) {
                        Some(RecorderSignal::Stop) => return,
                        Some(RecorderSignal::Wake) => {
                            // Off now? Then the head of the loop says so.
                            if !current_settings(app).is_some_and(|s| wanted(&s)) {
                                break;
                            }
                        }
                        Some(RecorderSignal::Retry) | None => {
                            crashes = CrashWindow::default();
                            break;
                        }
                    }
                }
            }
        }
    }
}

/// Make sure the exe is installed and verified, run it, and say how that
/// ended.
fn one_pass(
    app: &AppHandle,
    shared: &Mutex<RecorderStatus>,
    inbox: &Receiver<RecorderSignal>,
    crashes: &mut CrashWindow,
) -> Pass {
    let Some(startup) = app.try_state::<Startup>() else {
        return Pass::Halt(RecorderStatus::Failed {
            reason: "MyMTGO is not ready".to_string(),
        });
    };
    let Ok(state) = ready(&startup) else {
        return Pass::Halt(RecorderStatus::Failed {
            reason: "MyMTGO is not ready".to_string(),
        });
    };
    let root = state.data_dir.clone();
    let exe = match verified_exe(&PIN, &root) {
        Some(exe) => exe,
        None => match download(app, shared, inbox, &root) {
            Ok(exe) => exe,
            Err(pass) => return pass,
        },
    };
    // Switched off while the download ran: never start it.
    if !still_wanted(app) {
        return Pass::Again;
    }
    let Ok(documents) = app.path().document_dir() else {
        return Pass::Halt(RecorderStatus::Failed {
            reason: "The Documents folder was not found".to_string(),
        });
    };
    let folder = recorder_folder(&documents);
    let _ = std::fs::create_dir_all(&folder);
    set_status(shared, RecorderStatus::Waiting);
    let child = match spawn(&exe, &folder) {
        Ok(child) => child,
        Err(error) => {
            tracing::warn!(%error, "could not start the recorder");
            return Pass::Halt(RecorderStatus::BlockedByAntivirus);
        }
    };
    let ended = watch(app, shared, inbox, child);
    let code = match ended {
        Watched::Exited(code) => code,
        Watched::Quit => return Pass::Quit,
        Watched::SwitchedOff => return Pass::Again,
    };
    let kind = exit_kind(code);
    let tripped = crashes.record(crate::wall_clock_ms());
    tracing::info!(?code, ?kind, tripped, "the recorder exited");
    match next_step(kind, tripped) {
        Step::Stop(status) => Pass::Halt(status),
        Step::Respawn => match wait(inbox, Some(RESPAWN_PAUSE)) {
            Some(RecorderSignal::Stop) => Pass::Quit,
            _ => Pass::Again,
        },
    }
}

/// Download and install, up to three tries; the installed exe is then
/// checked. `Err` carries how the pass ends instead.
fn download(
    app: &AppHandle,
    shared: &Mutex<RecorderStatus>,
    inbox: &Receiver<RecorderSignal>,
    root: &Path,
) -> Result<PathBuf, Pass> {
    let offline = app
        .try_state::<ApiState>()
        .is_some_and(|api| offline_mode_now(&api).enabled);
    if download_blocked(&PIN) {
        return Err(Pass::Halt(RecorderStatus::NoBuild));
    }
    if !may_download(offline, &PIN) {
        return Err(Pass::Halt(RecorderStatus::Failed {
            reason: "Offline mode is on".to_string(),
        }));
    }
    set_status(shared, RecorderStatus::Downloading);
    let fetcher = Fetcher::for_origins(vec!["https://releases.mymtgo.com".to_string()])
        .with_max_bytes(DOWNLOAD_MAX_BYTES);
    let fetch = move |url: &str| fetcher.fetch_within(url, DOWNLOAD_TIMEOUT);
    let mut last = String::new();
    for attempt in 0..=BACKOFF.len() {
        match install(&PIN, root, &fetch) {
            Ok(exe) => {
                let verified = verified_exe(&PIN, root);
                return match check_after_install(exe.is_file(), verified.is_some()) {
                    ExeCheck::Ready => Ok(verified.unwrap_or(exe)),
                    ExeCheck::BlockedByAntivirus => {
                        Err(Pass::Halt(RecorderStatus::BlockedByAntivirus))
                    }
                    ExeCheck::Failed(reason) => Err(Pass::Halt(RecorderStatus::Failed { reason })),
                };
            }
            Err(InstallError::Blocked) => {
                last = "No recorder build is pinned in this version".to_string();
                break;
            }
            Err(InstallError::Offline) => {
                last = "Offline mode is on".to_string();
                break;
            }
            // A bad hash will not fix itself by asking again.
            Err(InstallError::HashMismatch(file)) => {
                last = format!("The downloaded {file} does not match its pin");
                break;
            }
            Err(InstallError::Fetch(error)) => last = format!("The download failed: {error}"),
            Err(InstallError::Move(error)) => last = format!("Could not install it: {error}"),
        }
        tracing::warn!(attempt, reason = %last, "the recorder download failed");
        if let Some(pause) = BACKOFF.get(attempt)
            && let Some(signal) = wait(inbox, Some(*pause))
        {
            match on_signal_while_busy(&signal, still_wanted(app)) {
                Busy::Quit => return Err(Pass::Quit),
                Busy::Again => return Err(Pass::Again),
                Busy::Continue => {}
            }
        }
    }
    Err(Pass::Halt(RecorderStatus::Failed { reason: last }))
}

/// How watching the process ended.
enum Watched {
    Exited(Option<i32>),
    SwitchedOff,
    Quit,
}

/// Watch the child: every `WATCH` look at the process and its status file;
/// a Stop signal, or the switch going off, stops it gracefully.
fn watch(
    app: &AppHandle,
    shared: &Mutex<RecorderStatus>,
    inbox: &Receiver<RecorderSignal>,
    mut child: Child,
) -> Watched {
    let started = SystemTime::now();
    let status_file = status_file_in(std::env::var_os("LOCALAPPDATA"));
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Watched::Exited(status.code()),
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(%error, "could not look at the recorder process");
                stop_child(&mut child);
                return Watched::Exited(None);
            }
        }
        if let Some(path) = &status_file
            && let Some(state) = read_status(path, started)
        {
            set_status(shared, status_for(state));
        }
        if let Some(signal) = wait(inbox, Some(WATCH)) {
            match on_signal_while_busy(&signal, still_wanted(app)) {
                Busy::Quit => {
                    stop_child(&mut child);
                    return Watched::Quit;
                }
                Busy::Again => {
                    stop_child(&mut child);
                    return Watched::SwitchedOff;
                }
                Busy::Continue => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recorder::PIN;

    fn accepted(notice: u32, consent: u32) -> RecorderAcceptance {
        RecorderAcceptance {
            notice_version: notice,
            consent_version: consent,
            accepted_at_ms: 1,
        }
    }

    #[test]
    fn acceptance_is_needed_until_given() {
        assert!(needs_acceptance(None, &PIN));
        let acc = accepted(NOTICE_VERSION, PIN.consent_version);
        assert!(!needs_acceptance(Some(&acc), &PIN));
    }

    #[test]
    fn a_higher_pinned_consent_version_asks_again() {
        let acc = accepted(NOTICE_VERSION, PIN.consent_version);
        let newer = Pin {
            consent_version: PIN.consent_version + 1,
            ..PIN
        };
        assert!(needs_acceptance(Some(&acc), &newer));
    }

    #[test]
    fn a_new_notice_version_asks_again() {
        let acc = accepted(NOTICE_VERSION - 1, PIN.consent_version);
        assert!(needs_acceptance(Some(&acc), &PIN));
    }

    #[test]
    fn a_crash_respawns_until_the_limit() {
        assert_eq!(next_step(ExitKind::Crashed, false), Step::Respawn);
        assert_eq!(next_step(ExitKind::Stopped, false), Step::Respawn);
        assert_eq!(
            next_step(ExitKind::Crashed, true),
            Step::Stop(RecorderStatus::StoppedAfterCrashes)
        );
        assert_eq!(
            next_step(ExitKind::Stopped, true),
            Step::Stop(RecorderStatus::StoppedAfterCrashes)
        );
    }

    #[test]
    fn mtgo_not_supported_stops_with_its_status() {
        assert_eq!(
            next_step(ExitKind::MtgoNotSupported, false),
            Step::Stop(RecorderStatus::MtgoNotSupported)
        );
    }

    /// Exit 4 also covers an MTGO still starting, so like another recorder
    /// running it is tried again every minute (the crash window is reset by
    /// the timed retry, as for a Retry).
    #[test]
    fn mtgo_not_supported_is_retried_every_minute() {
        let Step::Stop(status) = next_step(ExitKind::MtgoNotSupported, false) else {
            panic!("exit 4 halts first");
        };
        assert_eq!(retry_after(&status), Some(Duration::from_secs(60)));
        assert_eq!(
            retry_after(&RecorderStatus::AnotherRunning),
            Some(Duration::from_secs(60))
        );
        assert_eq!(retry_after(&RecorderStatus::StoppedAfterCrashes), None);
        assert_eq!(retry_after(&RecorderStatus::NoBuild), None);
        let failed = RecorderStatus::Failed {
            reason: "x".to_string(),
        };
        assert_eq!(retry_after(&failed), None);
    }

    #[test]
    fn another_recorder_running_stops_with_its_status() {
        assert_eq!(
            next_step(ExitKind::AlreadyRunning, false),
            Step::Stop(RecorderStatus::AnotherRunning)
        );
    }

    #[test]
    fn bad_arguments_never_respawn() {
        for tripped in [false, true] {
            assert!(matches!(
                next_step(ExitKind::BadArguments, tripped),
                Step::Stop(_)
            ));
            assert!(matches!(
                next_step(ExitKind::ConsentNotAccepted, tripped),
                Step::Stop(_)
            ));
        }
    }

    #[test]
    fn switching_off_during_a_download_never_spawns() {
        for signal in [RecorderSignal::Wake, RecorderSignal::Retry] {
            assert_eq!(on_signal_while_busy(&signal, false), Busy::Again);
            assert_eq!(on_signal_while_busy(&signal, true), Busy::Continue);
        }
        assert_eq!(
            on_signal_while_busy(&RecorderSignal::Stop, true),
            Busy::Quit
        );
        assert_eq!(
            on_signal_while_busy(&RecorderSignal::Stop, false),
            Busy::Quit
        );
    }

    #[test]
    fn offline_mode_blocks_the_download() {
        let hashed = Pin {
            files: &[recorder::PinnedFile {
                name: "mtgr-record.exe",
                sha256: "ab",
            }],
            ..PIN
        };
        assert!(may_download(false, &hashed));
        assert!(!may_download(true, &hashed));
        // An unhashed pin never downloads, online or not.
        let unhashed = Pin {
            files: &[recorder::PinnedFile {
                name: "mtgr-record.exe",
                sha256: "",
            }],
            ..PIN
        };
        assert!(!may_download(false, &unhashed));
    }

    #[test]
    fn recorder_states_map_to_statuses() {
        let recording = RecorderState::Recording { match_token: None };
        assert_eq!(status_for(recording), RecorderStatus::Recording);
        assert_eq!(status_for(RecorderState::Waiting), RecorderStatus::Waiting);
        assert_eq!(status_for(RecorderState::Idle), RecorderStatus::Ready);
    }

    #[test]
    fn the_page_status_follows_platform_then_switch() {
        let worker = RecorderStatus::Recording;
        assert_eq!(
            view_status(false, false, true, &worker),
            RecorderStatus::Unsupported
        );
        assert_eq!(
            view_status(false, true, false, &worker),
            RecorderStatus::Off
        );
        assert_eq!(
            view_status(false, true, true, &worker),
            RecorderStatus::Recording
        );
        assert_eq!(
            view_status(false, true, true, &RecorderStatus::Off),
            RecorderStatus::Waiting
        );
    }

    #[test]
    fn an_unpinned_build_reports_no_build_before_anything_else() {
        for supported in [false, true] {
            for enabled in [false, true] {
                assert_eq!(
                    view_status(true, supported, enabled, &RecorderStatus::Recording),
                    RecorderStatus::NoBuild
                );
            }
        }
    }

    #[test]
    fn a_missing_exe_after_install_is_blocked_by_antivirus() {
        assert_eq!(
            check_after_install(false, false),
            ExeCheck::BlockedByAntivirus
        );
        assert_eq!(check_after_install(true, true), ExeCheck::Ready);
        assert!(matches!(
            check_after_install(true, false),
            ExeCheck::Failed(_)
        ));
    }
}
