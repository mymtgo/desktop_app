//! What goes into the report bundle (release spec section 8.1, 0.x's
//! `BuildSupportBundle`): the newest `mtgo.log` and the newest Arena
//! `Player.log` among the watched files, the three newest MyMTGO log files,
//! the newest recorder log and `summary.txt`. Nothing else: no
//! database, settings, deck files, paths, tokens or usernames. The logs are
//! not redacted, as in 0.x (spec ruling 13); the popover says so.

use std::io::{Read, Write};
use std::path::PathBuf;

use chrono::NaiveDateTime;

use crate::zip::{DosTime, ZipError, ZipWriter};

/// How many MyMTGO log files go in (0.x: three of each log kind).
pub const APP_LOG_LIMIT: usize = 3;
/// The summary's name inside the zip.
pub const SUMMARY_NAME: &str = "summary.txt";

/// A file the app found on disk. `name` is the file name only: it is the
/// entry's name in the zip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRef {
    pub path: PathBuf,
    pub name: String,
    /// When it was last modified, UTC milliseconds; `None` when unknown.
    pub modified_ms: Option<i64>,
}

/// What `summary.txt` says. Every field is a count, a flag or a fixed
/// word: never a path, a name or a secret.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SummaryInput {
    pub app_version: String,
    /// The update channel ("beta" or "stable").
    pub channel: String,
    /// `std::env::consts::OS` and `ARCH`.
    pub os: String,
    pub arch: String,
    /// The status bar's watcher word ("Watching", "Stopped", ...).
    pub watcher: String,
    /// How many `mtgo.log` files are watched.
    pub logs_watched: usize,
    /// How many Arena `Player.log` files are watched.
    pub arena_logs_watched: usize,
    pub offline_mode: bool,
    pub signed_in: bool,
    pub matches: u64,
    pub games: u64,
    /// The recorder's state as a short word, `None` when the recorder is
    /// off and nothing was recorded.
    pub recorder_status: Option<String>,
    /// Files picked for the bundle that could not be read. Named by file
    /// name only, even when a path is given.
    pub skipped: Vec<String>,
}

/// One log for the bundle, opened by the app.
pub struct ReportEntry<R: Read> {
    pub name: String,
    pub reader: R,
}

/// A MyMTGO log file: `mymtgo.<date>.log`, as the daily log writer names
/// them (`app/src/logging.rs`), or `mymtgo.log`.
pub fn is_app_log_name(name: &str) -> bool {
    name.starts_with("mymtgo.") && name.ends_with(".log") && name.len() >= "mymtgo.log".len()
}

/// A recorder log: `recorder-<anything>.log`. Recordings (`.mtgr`),
/// `Match_*.log` files, captures and `status.txt` are not logs.
pub fn is_recorder_log_name(name: &str) -> bool {
    name.starts_with("recorder-") && name.ends_with(".log") && name.len() > "recorder-.log".len()
}

/// The files the bundle holds: the most recently modified watched log of
/// each name (`mtgo.log`, Arena's `Player.log`), in the order the names
/// first appear (0.x takes the last of its `mtgo.log` files sorted by
/// modified time), then up to three MyMTGO logs, newest first by name (their
/// dates are zero padded, as 0.x sorts its logs), then the newest recorder
/// log (by modified time, then name). Other files are ignored.
/// Empty means there is nothing to send.
pub fn pick_report_files(
    watched_logs: &[FileRef],
    app_logs: &[FileRef],
    recorder_logs: &[FileRef],
) -> Vec<FileRef> {
    let mut newest: Vec<&FileRef> = Vec::new();
    for log in watched_logs {
        match newest.iter_mut().find(|best| best.name == log.name) {
            // `>=`: on a tie the later file wins, as 0.x's `last()`.
            Some(best) => {
                if log.modified_ms >= best.modified_ms {
                    *best = log;
                }
            }
            None => newest.push(log),
        }
    }
    let mut picked: Vec<FileRef> = newest.into_iter().cloned().collect();

    let mut apps: Vec<&FileRef> = app_logs
        .iter()
        .filter(|log| is_app_log_name(&log.name))
        .collect();
    apps.sort_by(|a, b| b.name.cmp(&a.name));
    for log in apps.into_iter().take(APP_LOG_LIMIT) {
        picked.push(log.clone());
    }

    let newest_recorder = recorder_logs
        .iter()
        .filter(|log| is_recorder_log_name(&log.name))
        .max_by(|a, b| (a.modified_ms, &a.name).cmp(&(b.modified_ms, &b.name)));
    if let Some(log) = newest_recorder {
        picked.push(log.clone());
    }
    picked
}

/// The last part of a path, splitting on both `/` and `\`.
fn file_name_only(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}

/// `summary.txt`: one fact per line.
pub fn summary_text(input: &SummaryInput) -> String {
    let mut text = String::new();
    text.push_str("MyMTGO report\n");
    text.push_str(&format!("App version: {}\n", input.app_version));
    text.push_str(&format!("Update channel: {}\n", input.channel));
    text.push_str(&format!("OS: {} ({})\n", input.os, input.arch));
    text.push_str(&format!("Watcher: {}\n", input.watcher));
    text.push_str(&format!("Logs watched: {}\n", input.logs_watched));
    text.push_str(&format!(
        "Arena logs watched: {}\n",
        input.arena_logs_watched
    ));
    text.push_str(&format!("Offline mode: {}\n", on_off(input.offline_mode)));
    text.push_str(&format!("Signed in: {}\n", yes_no(input.signed_in)));
    text.push_str(&format!("Matches: {}\n", input.matches));
    text.push_str(&format!("Games: {}\n", input.games));
    if let Some(status) = &input.recorder_status {
        text.push_str(&format!("Recorder: {status}\n"));
    }
    if !input.skipped.is_empty() {
        let names: Vec<&str> = input
            .skipped
            .iter()
            .map(|name| file_name_only(name))
            .collect();
        text.push_str(&format!("Could not read: {}\n", names.join(", ")));
    }
    text
}

/// The suggested file name, `mymtgo-report-YYYY-MM-DD-HHMMSS.zip`, in the
/// user's local time (0.x: `mtgo-report-<Y-m-d-His>.zip`).
pub fn report_file_name(local: NaiveDateTime) -> String {
    format!("mymtgo-report-{}.zip", local.format("%Y-%m-%d-%H%M%S"))
}

/// Write the bundle to `out`: the summary first, then each log, all
/// deflated and streamed. Returns the writer.
pub fn write_report<W: Write, R: Read>(
    out: W,
    dos_time: DosTime,
    summary: &str,
    logs: Vec<ReportEntry<R>>,
) -> Result<W, ZipError> {
    let mut zip = ZipWriter::new(out);
    zip.add_deflated(SUMMARY_NAME, dos_time, summary.as_bytes())?;
    for log in logs {
        zip.add_deflated(&log.name, dos_time, log.reader)?;
    }
    zip.finish()
}
