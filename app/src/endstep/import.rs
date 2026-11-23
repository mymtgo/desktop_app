//! Importing the capture files the Endstep helper drops in the inbox
//! (endstep support spec). The inbox is ours: a file is deleted once the
//! tracker's writer (`TrackerHandle::endstep_capture_importer`, the single
//! writer) stored it, and a file that cannot be read as a capture is moved
//! to `inbox/failed/`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, SystemTime};

use store::endstep_captures::NewCapture;
use tauri::{AppHandle, Manager};

use crate::commands::{AppState, Startup, read_settings, ready};
use crate::hash::hash_of;
use crate::worker_thread::WorkerThread;

/// The helper's inbox inside the app's data folder (`AppState::data_dir`).
pub fn inbox_folder(data_dir: &Path) -> PathBuf {
    data_dir.join("endstep").join("inbox")
}

/// How often the inbox is read again.
const POLL: Duration = Duration::from_secs(15);

/// How long the worker waits while the switch reads off before reading it
/// again. A settings read that fails reads as off, so off must look again
/// on its own, never wait only for a `Wake`.
const OFF_RECHECK: Duration = Duration::from_secs(60);

/// One scan handles at most this many files, and holds at most this many
/// bytes of captures; the rest wait for the next poll. A file over the byte
/// cap but within `MAX_CAPTURE_BYTES` (a match that ran to the chess clocks)
/// is read alone, as a batch of one. Only a file over `MAX_CAPTURE_BYTES`
/// can never be read, so it is failed.
const MAX_BATCH_FILES: usize = 50;
const MAX_BATCH_BYTES: u64 = 64 * 1024 * 1024;

/// The name of the folder (inside the inbox) bad files are moved to.
const FAILED_FOLDER: &str = "failed";

/// What one read of the inbox found.
#[derive(Debug, Default)]
pub struct InboxScan {
    /// The captures to store.
    pub captures: Vec<NewCapture>,
    /// The files of `captures`, deleted once they are stored.
    pub files: Vec<PathBuf>,
    /// Files that are not captures (or too large), moved to `inbox/failed/`.
    pub failed: Vec<PathBuf>,
    /// Developer captures not imported in this build, deleted with `files`.
    pub skipped_developer: Vec<PathBuf>,
    /// The version of each file above that this scan saw. `settle` deletes
    /// or moves a file only while it is still that version: the helper may
    /// put a resend (with more frames) under the same name meanwhile.
    pub versions: HashMap<PathBuf, FileVersion>,
}

/// What a scan saw of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileVersion {
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// Lowercase hex sha256 of the bytes read; `None` for a file too large
    /// to read, which is then known by its size and modified time only.
    pub hash: Option<String>,
}

impl FileVersion {
    /// Whether `path` still holds this version. A file that is gone, or
    /// cannot be read, is not. Read files are hashed again (modified times
    /// can be too coarse to see a quick same-size rewrite); unread ones are
    /// compared by size and modified time.
    fn still_at(&self, path: &Path) -> bool {
        let Ok(metadata) = std::fs::metadata(path) else {
            return false;
        };
        if metadata.len() != self.size {
            return false;
        }
        match &self.hash {
            Some(hash) => std::fs::read(path).is_ok_and(|bytes| &hash_of(&bytes) == hash),
            None => metadata.modified().ok() == self.modified,
        }
    }
}

impl InboxScan {
    fn handled(&self) -> usize {
        self.files.len() + self.failed.len() + self.skipped_developer.len()
    }
}

/// A `<id>.<pid>.partial` file older than this is a leftover of a helper
/// that was killed mid-write.
const STALE_PARTIAL_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// Whether the file name is `<unit id>.<pid>.partial`, the helper's shape.
fn is_helper_partial_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(stem) = name.strip_suffix(".partial") else {
        return false;
    };
    let Some((unit_id, pid)) = stem.rsplit_once('.') else {
        return false;
    };
    endstep_capture::valid_unit_id(unit_id)
        && !pid.is_empty()
        && pid.bytes().all(|byte| byte.is_ascii_digit())
}

/// Delete helper `<id>.<pid>.partial` files in `inbox` last modified more than `max_age`
/// before `now`. Fresh partials (a helper may be writing them) and every
/// other file are left alone. Returns how many were deleted.
pub fn sweep_stale_partials(inbox: &Path, now: SystemTime, max_age: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(inbox) else {
        return 0;
    };
    let mut deleted = 0;
    for path in entries.filter_map(|entry| entry.ok()).map(|e| e.path()) {
        if !path.is_file() || !is_helper_partial_name(&path) {
            continue;
        }
        let Ok(modified) = std::fs::metadata(&path).and_then(|meta| meta.modified()) else {
            continue;
        };
        // A modified time in the future gives an error: not stale.
        let Ok(age) = now.duration_since(modified) else {
            continue;
        };
        if age <= max_age {
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => deleted += 1,
            Err(error) => {
                tracing::warn!(file = %path.display(), %error,
                    "could not delete a stale Endstep partial file");
            }
        }
    }
    deleted
}

/// A finished capture: `*.jsonl`. The helper writes `<id>.<pid>.partial` and
/// renames it when done, so a partial file is never read.
fn is_capture_name(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
}

/// Read the finished capture files in `inbox`, in path order. Developer
/// captures are kept only when `keep_developer`. A missing inbox gives an
/// empty scan. Reading never changes a file; `settle` does that.
pub fn scan_inbox(inbox: &Path, keep_developer: bool) -> InboxScan {
    scan_inbox_within(
        inbox,
        keep_developer,
        MAX_BATCH_FILES,
        MAX_BATCH_BYTES,
        endstep_capture::MAX_CAPTURE_BYTES,
    )
}

fn scan_inbox_within(
    inbox: &Path,
    keep_developer: bool,
    max_files: usize,
    max_bytes: u64,
    max_capture_bytes: u64,
) -> InboxScan {
    let mut scan = InboxScan::default();
    let mut batch_bytes: u64 = 0;
    let Ok(entries) = std::fs::read_dir(inbox) else {
        return scan;
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && is_capture_name(path))
        .collect();
    paths.sort();
    for path in paths {
        if scan.handled() >= max_files {
            break;
        }
        let name = path.display().to_string();
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        let size = metadata.len();
        if size > max_capture_bytes {
            tracing::warn!(file = %name, bytes = size, "Endstep capture failed: too large");
            let version = FileVersion {
                size,
                modified: metadata.modified().ok(),
                hash: None,
            };
            scan.versions.insert(path.clone(), version);
            scan.failed.push(path);
            continue;
        }
        if batch_bytes + size > max_bytes && !scan.files.is_empty() {
            // Over the batch cap with files already in it: this one is read
            // first in the next scan, alone when it is over the cap by itself.
            break;
        }
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            // Left in place: a file that cannot be read now may be later.
            Err(error) => {
                tracing::warn!(file = %name, %error, "could not read an Endstep capture");
                continue;
            }
        };
        let file_hash = hash_of(&bytes);
        let version = FileVersion {
            size: bytes.len() as u64,
            modified: None,
            hash: Some(file_hash.clone()),
        };
        scan.versions.insert(path.clone(), version);
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => {
                tracing::warn!(file = %name, "Endstep capture failed: not UTF-8 text");
                scan.failed.push(path);
                continue;
            }
        };
        let parsed = match endstep_capture::parse_capture(&text) {
            Ok(parsed) => parsed,
            Err(error) => {
                tracing::warn!(file = %name, %error, "Endstep capture failed");
                scan.failed.push(path);
                continue;
            }
        };
        if parsed.header.developer && !keep_developer {
            scan.skipped_developer.push(path);
            continue;
        }
        batch_bytes += size;
        scan.files.push(path);
        scan.captures.push(NewCapture {
            header: parsed.header,
            frame_count: parsed.frame_count,
            text,
            file_hash,
        });
    }
    scan
}

/// Clear the inbox after a store attempt: when `stored`, delete the stored
/// and the skipped developer files; always move the failed files to
/// `inbox/failed/`. A file that changed since the scan is left for the next
/// poll. A delete or move that fails is logged and the rest go on.
pub fn settle(scan: &InboxScan, stored: bool) {
    if stored {
        for path in scan.files.iter().chain(&scan.skipped_developer) {
            if !unchanged_since_scan(scan, path) {
                continue;
            }
            if let Err(error) = std::fs::remove_file(path) {
                tracing::warn!(file = %path.display(), %error,
                    "could not delete an imported Endstep capture");
            }
        }
    }
    for path in &scan.failed {
        if unchanged_since_scan(scan, path) {
            move_to_failed(path);
        }
    }
}

/// Whether `path` is still the version `scan` saw; logs when it is not.
fn unchanged_since_scan(scan: &InboxScan, path: &Path) -> bool {
    let unchanged = scan
        .versions
        .get(path)
        .is_some_and(|version| version.still_at(path));
    if !unchanged {
        tracing::info!(file = %path.display(),
            "Endstep capture changed or went since it was read; left for the next poll");
    }
    unchanged
}

/// `folder/name`, or `folder/<stem>.<n>.<ext>` (n from 1) when that exists,
/// so an earlier failed file is never overwritten.
fn free_name_in(folder: &Path, name: &std::ffi::OsStr) -> PathBuf {
    let first = folder.join(name);
    if !first.exists() {
        return first;
    }
    let as_path = Path::new(name);
    let stem = as_path
        .file_stem()
        .unwrap_or(name)
        .to_string_lossy()
        .into_owned();
    let extension = as_path
        .extension()
        .map(|e| e.to_string_lossy().into_owned());
    let mut number = 1u32;
    loop {
        let candidate = match &extension {
            Some(extension) => format!("{stem}.{number}.{extension}"),
            None => format!("{stem}.{number}"),
        };
        let path = folder.join(candidate);
        if !path.exists() {
            return path;
        }
        number += 1;
    }
}

fn move_to_failed(path: &Path) {
    let (Some(inbox), Some(name)) = (path.parent(), path.file_name()) else {
        return;
    };
    let folder = inbox.join(FAILED_FOLDER);
    let moved = std::fs::create_dir_all(&folder)
        .and_then(|()| std::fs::rename(path, free_name_in(&folder, name)));
    if let Err(error) = moved {
        tracing::warn!(file = %path.display(), %error,
            "could not move a failed Endstep capture aside");
    }
}

/// What wakes the inbox worker besides its poll.
pub enum InboxSignal {
    Stop,
    /// The Endstep switch changed: look at the setting again.
    Wake,
}

/// The running `endstep-import` thread, managed by Tauri.
pub struct InboxWorker {
    thread: WorkerThread<InboxSignal>,
}

impl InboxWorker {
    /// Start the thread: one read at once, then one every 15 seconds.
    pub fn start(app: AppHandle) -> InboxWorker {
        let thread = WorkerThread::spawn(
            "endstep-import",
            "could not start the Endstep import; captures are not imported",
            move |signals| run(&app, &signals),
        );
        InboxWorker { thread }
    }

    /// Tell the worker the Endstep switch changed, so it reads the inbox at
    /// once (switched on) or goes quiet (switched off).
    pub fn wake(&self) {
        self.thread.send(InboxSignal::Wake);
    }

    /// Stop the worker, waiting up to `STOP_WAIT` for it.
    pub fn stop(&self) {
        self.thread.stop(
            InboxSignal::Stop,
            "the Endstep import is still storing captures; leaving it",
        );
    }
}

/// Whether the Endstep switch is on (off when the settings cannot be read).
fn enabled(state: &AppState) -> bool {
    read_settings(state).is_ok_and(|settings| settings.endstep_enabled)
}

fn run(app: &AppHandle, signals: &Receiver<InboxSignal>) {
    run_loop(signals, OFF_RECHECK, || {
        let on = app
            .try_state::<Startup>()
            .is_some_and(|startup| ready(&startup).is_ok_and(enabled));
        if on {
            poll(app);
        }
        on
    });
}

/// The worker loop. `step` reads the switch, polls when it is on and says
/// whether it was. On, the next step comes after `POLL`; off, after
/// `off_wait` or a `Wake`, and the inbox is never scanned.
fn run_loop(signals: &Receiver<InboxSignal>, off_wait: Duration, mut step: impl FnMut() -> bool) {
    loop {
        let on = step();
        let wait = if on { POLL } else { off_wait };
        match signals.recv_timeout(wait) {
            Err(RecvTimeoutError::Timeout) | Ok(InboxSignal::Wake) => {}
            Ok(InboxSignal::Stop) | Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// One read of the inbox, one store of its captures, then `settle`. A
/// missing inbox, or a tracker that is restarting, ends the poll quietly
/// and leaves every file for the next one.
fn poll(app: &AppHandle) {
    let Some(startup) = app.try_state::<Startup>() else {
        return;
    };
    let Ok(state) = ready(&startup) else {
        return;
    };
    poll_state(state);
}

/// `poll` for a ready app. Nothing happens while the Endstep switch is off:
/// the inbox is not scanned and a file in it stays where it is.
fn poll_state(state: &AppState) {
    if !enabled(state) {
        return;
    }
    let inbox = inbox_folder(&state.data_dir);
    if !inbox.is_dir() {
        return;
    }
    let swept = sweep_stale_partials(&inbox, SystemTime::now(), STALE_PARTIAL_AGE);
    if swept > 0 {
        tracing::info!(swept, "deleted stale Endstep partial files");
    }
    // The extension no longer sends developer AI captures; one left from an older build
    // imports only in `tauri dev` builds.
    let mut scan = scan_inbox(&inbox, cfg!(debug_assertions));
    if scan.handled() == 0 {
        return;
    }
    let captures = std::mem::take(&mut scan.captures);
    let failed = scan.failed.len();
    let skipped = scan.skipped_developer.len();
    if captures.is_empty() {
        settle(&scan, true);
        tracing::info!(
            added = 0,
            replaced = 0,
            unchanged = 0,
            failed,
            skipped,
            "imported Endstep captures"
        );
        return;
    }
    let importer = match state.tracker_part(tracker::TrackerHandle::endstep_capture_importer) {
        Ok(importer) => importer,
        Err(_) => return,
    };
    match importer.store(captures, crate::wall_clock_ms()) {
        Ok(report) => {
            settle(&scan, true);
            tracing::info!(
                added = report.added,
                replaced = report.replaced,
                unchanged = report.unchanged,
                failed,
                skipped,
                "imported Endstep captures"
            );
        }
        Err(error) => {
            settle(&scan, false);
            tracing::warn!(%error, failed, "could not store the Endstep captures");
        }
    }
}

#[cfg(test)]
mod tests {
    use fixture_tools::TestDir;

    use crate::bridge::lock;
    use crate::commands::tests::ready_state;

    use super::*;

    const UNIT_A: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
    const UNIT_B: &str = "1a2b3c4d-d9cb-469f-a165-70867728950e";
    const FRAME: &str = r#"{"at":1,"src":"game_ws","dir":"in","url":"wss://x","body":{"a":1}}"#;

    fn capture_text(unit_id: &str, developer: bool) -> String {
        format!(
            "{{\"mymtgo_endstep_capture\":1,\"unit_id\":\"{unit_id}\",\"kind\":\"match\",\"started_at\":1700000000000,\"counter\":1,\"extension\":\"0.1.0\",\"browser\":\"firefox\",\"developer\":{developer},\"classification\":\"human\"}}\n{FRAME}\n"
        )
    }

    fn captures_stored(state: &AppState) -> i64 {
        store::endstep_captures::capture_count(&lock(&state.reader)).expect("count")
    }

    /// Off (or a settings read that failed), the worker still reads the
    /// switch again after `off_wait`, with no signal (final review M7).
    #[test]
    fn a_worker_that_reads_off_looks_again_without_a_signal() {
        let (sender, signals) = std::sync::mpsc::channel();
        let (steps_sender, steps) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut count = 0;
            run_loop(&signals, Duration::from_millis(5), || {
                count += 1;
                let _ = steps_sender.send(count);
                false
            });
        });
        let mut seen = 0;
        while seen < 3 {
            match steps.recv_timeout(Duration::from_secs(5)) {
                Ok(count) => seen = count,
                Err(_) => break,
            }
        }
        sender.send(InboxSignal::Stop).expect("stop");
        worker.join().expect("the worker ends");
        assert!(seen >= 3, "the switch was read {seen} times");
    }

    #[test]
    fn a_switched_off_worker_leaves_the_inbox_alone() {
        let dir = TestDir::new("endstep-import-off");
        let (state, _events) = ready_state(&dir);
        let inbox = inbox_folder(&state.data_dir);
        let file = std::fs::create_dir_all(&inbox)
            .map(|()| inbox.join("a.jsonl"))
            .expect("inbox");
        std::fs::write(&file, capture_text(UNIT_A, false)).expect("capture");
        assert!(!read_settings(&state).expect("settings").endstep_enabled);

        poll_state(&state);

        assert_eq!(captures_stored(&state), 0);
        assert!(file.is_file(), "the file stays in the inbox");

        // Switched on, the same poll stores it and clears the file.
        let rows = vec![("endstep_enabled".to_string(), "true".to_string())];
        state.save_setting_rows(rows).expect("saved");
        poll_state(&state);
        assert_eq!(captures_stored(&state), 1);
        assert!(!file.exists());
        crate::platform::tests::stop(&state);
    }

    #[test]
    fn the_inbox_is_endstep_inbox_in_the_data_folder() {
        assert_eq!(
            inbox_folder(Path::new("/data")),
            Path::new("/data").join("endstep").join("inbox")
        );
    }

    #[test]
    fn only_finished_jsonl_files_are_read() {
        let dir = TestDir::new("endstep-scan-finished");
        let a = dir.write("a.jsonl", capture_text(UNIT_A, false).as_bytes());
        dir.write("b.jsonl.partial", capture_text(UNIT_B, false).as_bytes());
        dir.write("c.txt", capture_text(UNIT_B, false).as_bytes());
        let scan = scan_inbox(dir.path(), false);
        assert_eq!(scan.captures.len(), 1);
        assert_eq!(scan.captures[0].header.unit_id, UNIT_A);
        assert_eq!(scan.captures[0].frame_count, 1);
        assert_eq!(scan.captures[0].text, capture_text(UNIT_A, false));
        assert_eq!(scan.files, vec![a]);
        assert!(scan.failed.is_empty());
        assert!(scan.skipped_developer.is_empty());
    }

    #[test]
    fn stale_partials_are_swept_and_nothing_else() {
        let dir = TestDir::new("endstep-sweep");
        let old = dir.write(&format!("{UNIT_A}.123.partial"), b"half");
        let fresh = dir.write(&format!("{UNIT_B}.456.partial"), b"half");
        let foreign = dir.write("notes.partial", b"mine");
        let bad_pid = dir.write(&format!("{UNIT_A}.x1.partial"), b"half");
        let finished = dir.write("c.jsonl", capture_text(UNIT_A, false).as_bytes());
        let other = dir.write("d.txt", b"x");
        let two_days_ago = SystemTime::now() - Duration::from_secs(48 * 60 * 60);
        // Old files of every kind: only the partial may go.
        for path in [&old, &finished, &other, &foreign, &bad_pid] {
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(two_days_ago)
                .unwrap();
        }
        let deleted = sweep_stale_partials(dir.path(), SystemTime::now(), STALE_PARTIAL_AGE);
        assert_eq!(deleted, 1);
        assert!(!old.exists());
        assert!(fresh.exists());
        assert!(finished.exists());
        assert!(other.exists());
        assert!(foreign.exists());
        assert!(bad_pid.exists());
    }

    #[test]
    fn a_partial_dated_in_the_future_is_kept() {
        let dir = TestDir::new("endstep-sweep-future");
        let partial = dir.write(&format!("{UNIT_A}.123.partial"), b"half");
        let later = SystemTime::now() + Duration::from_secs(60 * 60);
        std::fs::File::options()
            .write(true)
            .open(&partial)
            .unwrap()
            .set_modified(later)
            .unwrap();
        let deleted = sweep_stale_partials(dir.path(), SystemTime::now(), STALE_PARTIAL_AGE);
        assert_eq!(deleted, 0);
        assert!(partial.exists());
    }

    #[test]
    fn sweeping_a_missing_inbox_is_fine() {
        let dir = TestDir::new("endstep-sweep-missing");
        let deleted = sweep_stale_partials(&dir.join("nope"), SystemTime::now(), STALE_PARTIAL_AGE);
        assert_eq!(deleted, 0);
    }

    #[test]
    fn a_failed_file_never_overwrites_an_earlier_one() {
        let dir = TestDir::new("endstep-failed-suffix");
        for text in [&b"first"[..], b"second", b"third"] {
            let path = dir.write("c.jsonl", text);
            move_to_failed(&path);
        }
        assert_eq!(std::fs::read(dir.join("failed/c.jsonl")).unwrap(), b"first");
        assert_eq!(
            std::fs::read(dir.join("failed/c.1.jsonl")).unwrap(),
            b"second"
        );
        assert_eq!(
            std::fs::read(dir.join("failed/c.2.jsonl")).unwrap(),
            b"third"
        );
    }

    #[test]
    fn a_bad_file_is_failed_and_others_still_read() {
        let dir = TestDir::new("endstep-scan-bad");
        let bad = dir.write("a.jsonl", b"not a capture");
        let not_utf8 = dir.write("b.jsonl", &[0xff, 0xfe, 0x00]);
        dir.write("c.jsonl", capture_text(UNIT_B, false).as_bytes());
        let scan = scan_inbox(dir.path(), false);
        assert_eq!(scan.captures.len(), 1);
        assert_eq!(scan.captures[0].header.unit_id, UNIT_B);
        assert_eq!(scan.failed, vec![bad, not_utf8]);
    }

    #[test]
    fn developer_captures_are_skipped_unless_kept() {
        let dir = TestDir::new("endstep-scan-developer");
        let path = dir.write("a.jsonl", capture_text(UNIT_A, true).as_bytes());
        let skipped = scan_inbox(dir.path(), false);
        assert!(skipped.captures.is_empty());
        assert!(skipped.files.is_empty());
        assert_eq!(skipped.skipped_developer, vec![path.clone()]);
        let kept = scan_inbox(dir.path(), true);
        assert_eq!(kept.captures.len(), 1);
        assert!(kept.captures[0].header.developer);
        assert_eq!(kept.files, vec![path]);
        assert!(kept.skipped_developer.is_empty());
    }

    #[test]
    fn the_hash_is_the_files_sha256() {
        let dir = TestDir::new("endstep-scan-hash");
        let text = capture_text(UNIT_A, false);
        dir.write("a.jsonl", text.as_bytes());
        let scan = scan_inbox(dir.path(), false);
        assert_eq!(scan.captures[0].file_hash, hash_of(text.as_bytes()));
    }

    #[test]
    fn failed_files_move_to_the_failed_folder() {
        // A store that failed: bad files still move, nothing is deleted.
        let dir = TestDir::new("endstep-settle-not-stored");
        let good = dir.write("a.jsonl", capture_text(UNIT_A, false).as_bytes());
        let developer = dir.write("b.jsonl", capture_text(UNIT_B, true).as_bytes());
        let bad = dir.write("c.jsonl", b"junk");
        let scan = scan_inbox(dir.path(), false);
        settle(&scan, false);
        assert!(good.exists());
        assert!(developer.exists());
        assert!(!bad.exists());
        assert_eq!(std::fs::read(dir.join("failed/c.jsonl")).unwrap(), b"junk");

        // A store that succeeded: stored and developer files are deleted.
        let dir = TestDir::new("endstep-settle-stored");
        let good = dir.write("a.jsonl", capture_text(UNIT_A, false).as_bytes());
        let developer = dir.write("b.jsonl", capture_text(UNIT_B, true).as_bytes());
        let bad = dir.write("c.jsonl", b"junk");
        let scan = scan_inbox(dir.path(), false);
        settle(&scan, true);
        assert!(!good.exists());
        assert!(!developer.exists());
        assert!(!bad.exists());
        assert!(dir.join("failed/c.jsonl").exists());
        // The failed folder is not read as part of the inbox.
        let again = scan_inbox(dir.path(), false);
        assert!(again.captures.is_empty() && again.failed.is_empty());
    }

    #[test]
    fn a_file_rewritten_before_settle_is_kept() {
        let dir = TestDir::new("endstep-settle-rewritten");
        let path = dir.write("a.jsonl", capture_text(UNIT_A, false).as_bytes());
        let scan = scan_inbox(dir.path(), false);
        assert_eq!(scan.files, vec![path.clone()]);
        // A resend with more frames lands while the first version is stored.
        let resend = format!("{}{FRAME}\n", capture_text(UNIT_A, false));
        dir.write("a.jsonl", resend.as_bytes());
        settle(&scan, true);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), resend);
    }

    #[test]
    fn a_same_size_rewrite_before_settle_is_kept() {
        let dir = TestDir::new("endstep-settle-same-size");
        let path = dir.write("a.jsonl", capture_text(UNIT_A, false).as_bytes());
        let scan = scan_inbox(dir.path(), false);
        let other = capture_text(UNIT_B, false);
        assert_eq!(other.len(), capture_text(UNIT_A, false).len());
        dir.write("a.jsonl", other.as_bytes());
        settle(&scan, true);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), other);
    }

    #[test]
    fn a_failed_file_replaced_by_a_good_one_is_not_moved() {
        let dir = TestDir::new("endstep-settle-failed-rewritten");
        let path = dir.write("a.jsonl", b"junk");
        let scan = scan_inbox(dir.path(), false);
        assert_eq!(scan.failed, vec![path.clone()]);
        let good = capture_text(UNIT_A, false);
        dir.write("a.jsonl", good.as_bytes());
        settle(&scan, true);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), good);
        assert!(!dir.join("failed/a.jsonl").exists());
        // The next poll reads the good version.
        assert_eq!(scan_inbox(dir.path(), false).captures.len(), 1);
    }

    #[test]
    fn settle_carries_on_when_a_file_is_already_gone() {
        let dir = TestDir::new("endstep-settle-gone");
        let good = dir.write("a.jsonl", capture_text(UNIT_A, false).as_bytes());
        let bad = dir.write("b.jsonl", b"junk");
        let scan = scan_inbox(dir.path(), false);
        std::fs::remove_file(&good).unwrap();
        std::fs::remove_file(&bad).unwrap();
        settle(&scan, true);
    }

    #[test]
    fn a_scan_is_capped_by_files_and_bytes() {
        let dir = TestDir::new("endstep-scan-cap");
        for n in 0..5 {
            dir.write(
                &format!("{n}.jsonl"),
                capture_text(UNIT_A, false).as_bytes(),
            );
        }
        let by_files = scan_inbox_within(dir.path(), false, 2, u64::MAX, u64::MAX);
        assert_eq!(by_files.captures.len(), 2);
        let size = capture_text(UNIT_A, false).len() as u64;
        let by_bytes = scan_inbox_within(dir.path(), false, 50, size * 3 - 1, u64::MAX);
        assert_eq!(by_bytes.captures.len(), 2);
    }

    #[test]
    fn a_file_over_the_capture_cap_is_failed() {
        let dir = TestDir::new("endstep-scan-huge");
        let bigger = format!("{}{FRAME}\n", capture_text(UNIT_A, false));
        let huge = dir.write("a.jsonl", bigger.as_bytes());
        dir.write("b.jsonl", capture_text(UNIT_B, false).as_bytes());
        let cap = capture_text(UNIT_B, false).len() as u64;
        let scan = scan_inbox_within(dir.path(), false, 50, cap, cap);
        assert_eq!(scan.failed, vec![huge]);
        assert_eq!(scan.captures.len(), 1);
        assert_eq!(scan.captures[0].header.unit_id, UNIT_B);
    }

    #[test]
    fn a_file_over_the_batch_cap_is_read_alone() {
        let dir = TestDir::new("endstep-scan-alone");
        let bigger = format!("{}{FRAME}\n", capture_text(UNIT_A, false));
        dir.write("a.jsonl", bigger.as_bytes());
        let cap = capture_text(UNIT_B, false).len() as u64;
        let scan = scan_inbox_within(dir.path(), false, 50, cap, u64::MAX);
        assert!(scan.failed.is_empty());
        assert_eq!(scan.captures.len(), 1);
        assert_eq!(scan.captures[0].header.unit_id, UNIT_A);
        assert!(scan.versions.values().all(|version| version.hash.is_some()));
    }

    #[test]
    fn a_file_over_the_batch_cap_waits_when_the_batch_has_files() {
        let dir = TestDir::new("endstep-scan-alone-later");
        dir.write("a.jsonl", capture_text(UNIT_B, false).as_bytes());
        let bigger = format!("{}{FRAME}\n", capture_text(UNIT_A, false));
        let big = dir.write("b.jsonl", bigger.as_bytes());
        let cap = capture_text(UNIT_B, false).len() as u64;
        let first = scan_inbox_within(dir.path(), false, 50, cap, u64::MAX);
        assert_eq!(first.captures.len(), 1);
        assert_eq!(first.captures[0].header.unit_id, UNIT_B);
        assert_eq!(first.files.len(), 1);
        assert!(!first.files.contains(&big));
        settle(&first, true);
        let second = scan_inbox_within(dir.path(), false, 50, cap, u64::MAX);
        assert_eq!(second.captures.len(), 1);
        assert_eq!(second.captures[0].header.unit_id, UNIT_A);
    }

    #[test]
    fn a_missing_inbox_is_not_an_error() {
        let dir = TestDir::new("endstep-scan-missing");
        let scan = scan_inbox(&dir.join("nope"), false);
        assert!(scan.captures.is_empty() && scan.failed.is_empty());
    }
}
