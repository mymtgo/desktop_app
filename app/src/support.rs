//! Get help's "Download report data" (release spec section 8.1, 0.x's
//! `BuildSupportBundle` and `DownloadReportBundleController`): finds the
//! report's files on disk, asks where to save the zip and writes it there on
//! a blocking thread, streamed. The choice of files, the summary and the zip
//! format are the pure `support` crate's.
//!
//! Never in the bundle: the database, settings, `.env.local`, keychain
//! values, tokens, deck files or any path. The logs are not redacted, as in
//! 0.x (spec ruling 13); the popover says so.

use std::fs::File;
use std::io::{BufWriter, Read};
use std::path::{Path, PathBuf};

use ::support::{FileRef, ReportEntry, SummaryInput};
use chrono::NaiveDateTime;
use chrono_tz::Tz;
use serde::Serialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

use crate::bridge::lock;
use crate::commands::{AppState, CommandError, Startup, ready};
use crate::logging::{LOG_FOLDER, Timer};
use crate::recorder::supervisor::{
    RecorderStatus, RecorderWorker, SUPPORTED, needs_acceptance, view_status,
};
use crate::summary::StatusView;
use crate::updates::UpdateChannel;
use ::recorder::{PIN, download_blocked};

/// How a "Download report data" ended. A failure is the command's error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum ReportBundleOutcome {
    Saved,
    /// The save dialog was closed: nothing happens.
    Cancelled,
    /// No `mtgo.log`, no Arena `Player.log` and no MyMTGO log: "Couldn't
    /// find log files".
    NoLogFiles,
}

/// Why a bundle was not written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleError {
    NoLogFiles,
    Failed(String),
}

fn file_metadata(path: &Path) -> Option<std::fs::Metadata> {
    std::fs::metadata(path).ok().filter(|meta| meta.is_file())
}

fn file_ref(path: PathBuf, meta: &std::fs::Metadata) -> Option<FileRef> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    let modified_ms = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_millis() as i64);
    Some(FileRef {
        path,
        name,
        modified_ms,
    })
}

/// The game logs the tracker watches: MTGO's `mtgo.log` files, then
/// Arena's `Player.log` (never `Player-prev.log`, which the status leaves
/// out).
pub fn watched_logs(status: &StatusView) -> Vec<String> {
    let mut paths = Vec::new();
    for file in status.files.iter().chain(status.arena_files.iter()) {
        paths.push(file.path.clone());
    }
    paths
}

/// The watched game logs that still exist, with their modified times.
pub fn list_watched_logs(watched: &[String]) -> Vec<FileRef> {
    let mut found = Vec::new();
    for path in watched {
        let path = PathBuf::from(path);
        if let Some(meta) = file_metadata(&path)
            && let Some(file) = file_ref(path, &meta)
        {
            found.push(file);
        }
    }
    found
}

/// Every file in MyMTGO's log folder (the pure crate keeps the log files).
pub fn list_app_logs(log_dir: &Path) -> Vec<FileRef> {
    let Ok(entries) = std::fs::read_dir(log_dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if let Some(meta) = file_metadata(&path)
            && let Some(file) = file_ref(path, &meta)
        {
            found.push(file);
        }
    }
    found
}

/// The files the bundle holds (0.x's choice): see `support::pick_report_files`.
///
/// `recorder_log_dir` is the recorder's log folder (`None` where there is
/// none). It is listed for `recorder-*.log` only: recordings, match logs,
/// captures and `status.txt` hold player names and are never picked.
pub fn find_report_files(
    watched: &[String],
    log_dir: &Path,
    recorder_log_dir: Option<&Path>,
) -> Vec<FileRef> {
    let recorder_logs = recorder_log_dir.map(list_app_logs).unwrap_or_default();
    ::support::pick_report_files(
        &list_watched_logs(watched),
        &list_app_logs(log_dir),
        &recorder_logs,
    )
}

/// The recorder's log folder, `%LOCALAPPDATA%\MtgoReplay\logs`. Only
/// Windows runs the recorder, so elsewhere there is none.
fn recorder_log_dir(app: &AppHandle) -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let local = app.path().local_data_dir().ok()?;
    Some(local.join("MtgoReplay").join("logs"))
}

/// The summary's recorder word, from the status the Replays page shows.
/// Never the failure reason (it can name paths).
pub fn recorder_status_word(status: &RecorderStatus) -> &'static str {
    match status {
        RecorderStatus::Off => "off",
        RecorderStatus::Unsupported => "unsupported",
        RecorderStatus::NoBuild => "noBuild",
        RecorderStatus::Downloading => "downloading",
        RecorderStatus::Failed { .. } => "failed",
        RecorderStatus::BlockedByAntivirus => "blockedByAntivirus",
        RecorderStatus::Waiting => "waiting",
        RecorderStatus::Ready => "ready",
        RecorderStatus::Recording => "recording",
        RecorderStatus::AnotherRunning => "anotherRunning",
        RecorderStatus::MtgoNotSupported => "mtgoNotSupported",
        RecorderStatus::StoppedAfterCrashes => "stoppedAfterCrashes",
    }
}

/// The summary's `Recorder:` value: `None` unless the recorder is enabled
/// or something was recorded (spec 6).
pub fn recorder_summary(enabled: bool, recordings: i64, status: &RecorderStatus) -> Option<String> {
    (enabled || recordings > 0).then(|| recorder_status_word(status).to_string())
}

/// Write the bundle of `files` to `out_path`. A file that cannot be opened
/// is skipped and named in the summary. On failure the partial zip is
/// removed.
pub fn write_bundle(
    out_path: &Path,
    files: &[FileRef],
    mut summary: SummaryInput,
    local_now: NaiveDateTime,
) -> Result<(), BundleError> {
    if files.is_empty() {
        return Err(BundleError::NoLogFiles);
    }
    let mut logs = Vec::new();
    for file in files {
        match File::open(&file.path) {
            Ok(reader) => logs.push(ReportEntry {
                name: file.name.clone(),
                reader,
            }),
            Err(error) => {
                tracing::warn!(%error, file = %file.name, "a report file could not be read");
                summary.skipped.push(file.name.clone());
            }
        }
    }
    let text = ::support::summary_text(&summary);
    let dos_time = ::support::DosTime::from_local(local_now);
    write_zip(out_path, dos_time, &text, logs)
}

/// Where the zip is written before it takes the chosen name:
/// `<name>.partial` next to it.
fn partial_path(out_path: &Path) -> PathBuf {
    partial_named(out_path, 0)
}

/// `<name>.partial` for `n` 0, else `<name>.partial-<n>`.
fn partial_named(out_path: &Path, n: u32) -> PathBuf {
    let mut name = out_path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".partial");
    if n > 0 {
        name.push(format!("-{n}"));
    }
    out_path.with_file_name(name)
}

/// Other partial names tried when one is taken.
const PARTIAL_TRIES: u32 = 9;

/// A new partial file next to `out_path`: never one already there (a user
/// file or a link is not opened, truncated or later removed).
fn create_partial(out_path: &Path) -> Result<(File, PathBuf), String> {
    for n in 0..=PARTIAL_TRIES {
        let path = partial_named(out_path, n);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((file, path)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    Err(format!(
        "{} and the next {PARTIAL_TRIES} partial names already exist",
        partial_path(out_path).display()
    ))
}

/// Write the zip of `summary` and `logs` to `out_path`. The zip is written
/// to a new `<name>.partial`, flushed and then renamed, so a crash or a
/// failure never leaves a truncated zip under the chosen name nor truncates
/// the file the user chose to replace. A failure removes only the partial
/// file this run created.
fn write_zip<R: Read>(
    out_path: &Path,
    dos_time: ::support::DosTime,
    summary: &str,
    logs: Vec<ReportEntry<R>>,
) -> Result<(), BundleError> {
    let (out, partial) = create_partial(out_path).map_err(BundleError::Failed)?;
    let written = ::support::write_report(
        BufWriter::with_capacity(64 * 1024, out),
        dos_time,
        summary,
        logs,
    )
    .map_err(|error| error.to_string())
    .and_then(|writer| {
        let file = writer
            .into_inner()
            .map_err(|error| error.error().to_string())?;
        file.sync_all().map_err(|error| error.to_string())
    })
    .and_then(|()| std::fs::rename(&partial, out_path).map_err(|error| error.to_string()));
    if let Err(error) = written {
        std::fs::remove_file(&partial).ok();
        return Err(BundleError::Failed(error));
    }
    Ok(())
}

/// The watcher line of the summary: the status bar's word for the tracker,
/// "Stopped" while the user has switched the watcher off (spec 4.1).
pub fn watcher_state(status: &StatusView) -> String {
    if status.restarting.is_some() {
        "Restarting".to_string()
    } else if status.paused {
        "Stopped".to_string()
    } else if status.started {
        "Watching".to_string()
    } else {
        "Starting".to_string()
    }
}

/// The summary's facts, from the app's state.
pub fn summary_input(
    app_version: String,
    status: &StatusView,
    channel: UpdateChannel,
    signed_in: bool,
    counts: (u64, u64),
) -> SummaryInput {
    SummaryInput {
        app_version,
        channel: crate::updates::channel_name(channel),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        watcher: watcher_state(status),
        logs_watched: status.files.len(),
        arena_logs_watched: status.arena_files.len(),
        offline_mode: status.offline_mode,
        signed_in,
        matches: counts.0,
        games: counts.1,
        recorder_status: None,
        skipped: Vec::new(),
    }
}

/// Local wall-clock time in `timezone`, for the file name and zip times.
pub fn local_now(timezone: Tz, now_ms: i64) -> NaiveDateTime {
    chrono::DateTime::from_timestamp_millis(now_ms)
        .unwrap_or_default()
        .with_timezone(&timezone)
        .naive_local()
}

/// The recorder log folder, only when the summary has a recorder line: one
/// gate for the log file and the line (enabled, or something recorded).
pub fn gated_recorder_dir(status: &Option<String>, dir: Option<PathBuf>) -> Option<PathBuf> {
    status.as_ref().and(dir)
}

fn recorder_status_for_summary(app: &AppHandle, state: &AppState) -> Option<String> {
    let enabled = crate::commands::read_settings(state)
        .map(|settings| {
            settings.recorder_enabled
                && !needs_acceptance(settings.recorder_acceptance.as_ref(), &PIN)
        })
        .unwrap_or(false);
    let recordings = store::recorded_replays::recording_count(&lock(&state.reader)).unwrap_or(0);
    let worker = app
        .try_state::<RecorderWorker>()
        .map_or(RecorderStatus::Off, |worker| worker.status());
    recorder_summary(
        enabled,
        recordings,
        &view_status(download_blocked(&PIN), SUPPORTED, enabled, &worker),
    )
}

/// Signed in to the MyMTGO API (tokens stored). Never the tokens.
fn signed_in(app: &AppHandle) -> bool {
    app.try_state::<crate::api::session::ApiState>()
        .is_some_and(|state| state.session().linked())
}

/// "Download report data": the files, then the save dialog, then the zip,
/// all on a blocking thread.
#[tauri::command]
#[specta::specta]
pub async fn save_report_bundle(app: AppHandle) -> Result<ReportBundleOutcome, CommandError> {
    let _timer = Timer::start("save_report_bundle");
    crate::commands::off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let status = lock(&state.shared.status).clone();
        let recorder_status = recorder_status_for_summary(&app, state);
        let recorder_dir = gated_recorder_dir(&recorder_status, recorder_log_dir(&app));
        let files = find_report_files(
            &watched_logs(&status),
            &state.data_dir.join(LOG_FOLDER),
            recorder_dir.as_deref(),
        );
        if files.is_empty() {
            return Ok(ReportBundleOutcome::NoLogFiles);
        }
        let local = local_now(state.timezone, crate::wall_clock_ms());

        let mut dialog = app
            .dialog()
            .file()
            .set_title("Save report data")
            .set_file_name(::support::report_file_name(local))
            .add_filter("Zip archive", &["zip"]);
        if let Some(window) = app.get_webview_window("main") {
            dialog = dialog.set_parent(&window);
        }
        let Some(picked) = dialog.blocking_save_file() else {
            return Ok(ReportBundleOutcome::Cancelled);
        };
        let path = picked.into_path().map_err(|error| {
            CommandError::Storage(format!("could not save the report: {error}"))
        })?;

        let counts = store::read::match_and_game_counts(&lock(&state.reader)).unwrap_or((0, 0));
        let version = app.package_info().version.to_string();
        let mut summary = summary_input(
            version,
            &status,
            crate::updates::current_channel(&app),
            signed_in(&app),
            counts,
        );
        summary.recorder_status = recorder_status;
        match write_bundle(&path, &files, summary, local) {
            Ok(()) => Ok(ReportBundleOutcome::Saved),
            Err(BundleError::NoLogFiles) => Ok(ReportBundleOutcome::NoLogFiles),
            Err(BundleError::Failed(error)) => {
                tracing::warn!(%error, "the report bundle could not be written");
                Err(CommandError::Storage(format!(
                    "could not write the report: {error}"
                )))
            }
        }
    })
    .await
}

#[cfg(test)]
mod tests;
