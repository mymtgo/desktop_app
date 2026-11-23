//! Importing the recorder's finished `.mtgr` files (recorded replays spec).
//! `scan_folder` only reads the folder: it never writes, renames or deletes
//! a file, whatever it finds. The files it returns go to the store through
//! the tracker's writer (`TrackerHandle::recording_importer`), so there is
//! still one writer.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, SystemTime};

use store::recorded_replays::NewRecording;
use tauri::{AppHandle, Manager};

use crate::bridge::lock;
use crate::commands::{Startup, ready};
use crate::hash::hash_of;
use crate::worker_thread::WorkerThread;

/// The recorder's folder inside the user's Documents folder.
pub fn recorder_folder(documents: &Path) -> PathBuf {
    documents.join("MtgReplay")
}

/// How often the folder is read again.
const POLL: Duration = Duration::from_secs(30);

/// A finished file, read and ready to store.
pub type ScannedRecording = NewRecording;

/// A file's size and modified time: unchanged means not worth reading again.
type Signature = (u64, SystemTime);

/// Files over this size are not read (a real recording is far smaller).
const MAX_RECORDING_BYTES: u64 = 20 * 1024 * 1024;

/// One poll's batch stops at this many files to store, or once their bytes
/// reach `MAX_BATCH_BYTES`; the rest (not remembered) wait for the next poll.
const MAX_BATCH_FILES: usize = 50;
const MAX_BATCH_BYTES: u64 = 50 * 1024 * 1024;

/// The files a worker has dealt with, by size and modified time, kept across
/// polls so an unchanged file (stored, unchanged or rejected) is neither
/// read nor logged again.
#[derive(Debug, Default)]
pub struct ScanMemory {
    seen: HashMap<PathBuf, Signature>,
}

impl ScanMemory {
    /// Remember the files of a batch the store accepted.
    pub fn remember(&mut self, stored: Vec<(PathBuf, Signature)>) {
        self.seen.extend(stored);
    }
}

/// What one read of the folder found.
#[derive(Debug, Default)]
pub struct ScanResult {
    /// Files not stored yet (or stored with another hash), one per token.
    pub recordings: Vec<ScannedRecording>,
    /// The files of `recordings`, for `ScanMemory::remember` once stored. A
    /// batch that failed to store is not remembered, so the next poll tries
    /// it again.
    pub pending: Vec<(PathBuf, Signature)>,
    /// Files rejected in this scan (not read before, or changed since).
    pub skipped: u32,
    /// Files whose hash is already stored.
    pub unchanged: u32,
    /// Files whose bytes were read in this scan.
    pub read: u32,
}

fn is_recording_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("Match_") && name.ends_with(".mtgr"))
}

/// Read every finished `Match_*.mtgr` file in `folder` that `memory` has not
/// dealt with and whose hash is not in `known`. `.mtgr.writing`, `.tmp`,
/// `.log` and failed files are not finished recordings and are left alone
/// without a mention. A folder that is missing gives an empty result. Only
/// the first file (in path order) of a token is kept. A batch holds at most
/// `MAX_BATCH_FILES` files and `MAX_BATCH_BYTES` bytes (the first in path
/// order); the files after it are left for the next poll.
pub fn scan_folder(folder: &Path, known: &HashSet<String>, memory: &mut ScanMemory) -> ScanResult {
    scan_folder_within(folder, known, memory, MAX_BATCH_FILES, MAX_BATCH_BYTES)
}

fn scan_folder_within(
    folder: &Path,
    known: &HashSet<String>,
    memory: &mut ScanMemory,
    max_files: usize,
    max_bytes: u64,
) -> ScanResult {
    let mut result = ScanResult::default();
    let mut batch_bytes: u64 = 0;
    let Ok(entries) = std::fs::read_dir(folder) else {
        return result;
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && is_recording_name(path))
        .collect();
    paths.sort();
    // Files that are gone are forgotten, so a new file under the name is read.
    memory.seen.retain(|path, _| paths.contains(path));
    let mut tokens: HashSet<String> = HashSet::new();
    for path in paths {
        if result.recordings.len() >= max_files {
            break;
        }
        let name = path.display().to_string();
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        let signature = (metadata.len(), modified);
        if memory.seen.get(&path) == Some(&signature) {
            continue;
        }
        if signature.0 > MAX_RECORDING_BYTES {
            tracing::warn!(file = %name, bytes = signature.0, "recording skipped: too large");
            memory.seen.insert(path, signature);
            result.skipped += 1;
            continue;
        }
        // A batch is never empty, so one file over the byte cap still goes.
        if !result.recordings.is_empty() && batch_bytes + signature.0 > max_bytes {
            break;
        }
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            // Not remembered: a file that cannot be read now may be later.
            Err(error) => {
                tracing::warn!(file = %name, %error, "recording skipped: could not read it");
                result.skipped += 1;
                continue;
            }
        };
        result.read += 1;
        let file_hash = hash_of(&bytes);
        if known.contains(&file_hash) {
            memory.seen.insert(path, signature);
            result.unchanged += 1;
            continue;
        }
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => {
                tracing::warn!(file = %name, "recording skipped: not UTF-8 text");
                memory.seen.insert(path, signature);
                result.skipped += 1;
                continue;
            }
        };
        let header = match mtgr::read_header(&text) {
            Ok(header) => header,
            Err(error) => {
                tracing::warn!(file = %name, ?error, "recording skipped: unreadable header");
                memory.seen.insert(path, signature);
                result.skipped += 1;
                continue;
            }
        };
        if !tokens.insert(header.match_token.clone()) {
            tracing::warn!(file = %name, token = %header.match_token,
                "recording skipped: an earlier file has the same match token");
            memory.seen.insert(path, signature);
            result.skipped += 1;
            continue;
        }
        batch_bytes += signature.0;
        result.pending.push((path, signature));
        result.recordings.push(NewRecording {
            header,
            text,
            file_hash,
        });
    }
    result
}

/// What wakes the import worker besides its poll.
pub enum ImportSignal {
    Stop,
}

/// The running `recorder-import` thread, managed by Tauri.
pub struct ImportWorker {
    thread: WorkerThread<ImportSignal>,
}

impl ImportWorker {
    /// Start the thread: one read at once, then one every 30 seconds.
    pub fn start(app: AppHandle) -> ImportWorker {
        let thread = WorkerThread::spawn(
            "recorder-import",
            "could not start the recorder import; recordings are not imported",
            move |inbox| run(&app, &inbox),
        );
        ImportWorker { thread }
    }

    /// Stop the worker, waiting up to `STOP_WAIT` for it.
    pub fn stop(&self) {
        self.thread.stop(
            ImportSignal::Stop,
            "the recorder import is still storing files; leaving it",
        );
    }
}

fn run(app: &AppHandle, inbox: &Receiver<ImportSignal>) {
    let mut memory = ScanMemory::default();
    loop {
        poll(app, &mut memory);
        match inbox.recv_timeout(POLL) {
            Err(RecvTimeoutError::Timeout) => {}
            Ok(ImportSignal::Stop) | Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// One read of the folder and one store of what is new. A folder that is
/// missing, or a tracker that is restarting, ends the poll quietly.
fn poll(app: &AppHandle, memory: &mut ScanMemory) {
    let Ok(documents) = app.path().document_dir() else {
        return;
    };
    let folder = recorder_folder(&documents);
    if !folder.is_dir() {
        return;
    }
    let Some(startup) = app.try_state::<Startup>() else {
        return;
    };
    let Ok(state) = ready(&startup) else {
        return;
    };
    let known: HashSet<String> = {
        let reader = lock(&state.reader);
        match store::recorded_replays::known_hashes(&reader) {
            Ok(hashes) => hashes.into_values().collect(),
            Err(error) => {
                tracing::warn!(%error, "could not read the stored recording hashes");
                return;
            }
        }
    };
    let scanned = scan_folder(&folder, &known, memory);
    if scanned.recordings.is_empty() {
        return;
    }
    let importer = match state.tracker_part(tracker::TrackerHandle::recording_importer) {
        Ok(importer) => importer,
        Err(_) => return,
    };
    match importer.store(scanned.recordings, crate::wall_clock_ms()) {
        Ok(report) => {
            memory.remember(scanned.pending);
            tracing::info!(
                added = report.added,
                replaced = report.replaced,
                unchanged = report.unchanged,
                skipped = scanned.skipped,
                "imported recordings"
            );
        }
        Err(error) => tracing::warn!(%error, "could not store the recordings"),
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use fixture_tools::TestDir;

    use super::*;

    const GUID_A: &str = "6f1c2d3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f";
    const GUID_B: &str = "7a2d3e4f-5b6c-4d7e-9f80-0b1c2d3e4f5a";

    fn mtgr_text(token: &str) -> String {
        format!("[Mtgr \"0.3.0\"]\n[Match \"{token}\"]\n[Game \"1\"]\n\nT1 PlayerA plays a land.\n")
    }

    fn none() -> HashSet<String> {
        HashSet::new()
    }

    #[test]
    fn only_finished_mtgr_files_are_read() {
        let dir = TestDir::new("recorder-scan-finished");
        dir.write("Match_a.mtgr", mtgr_text(GUID_A).as_bytes());
        dir.write("Match_b.mtgr.writing", mtgr_text(GUID_B).as_bytes());
        dir.write("Match_c.tmp", mtgr_text(GUID_B).as_bytes());
        dir.write("recorder.log", b"log");
        dir.write("Match_d.tmp.failed", mtgr_text(GUID_B).as_bytes());
        let scanned = scan_folder(dir.path(), &none(), &mut ScanMemory::default());
        assert_eq!(scanned.recordings.len(), 1);
        assert_eq!(scanned.recordings[0].header.match_token, GUID_A);
        assert_eq!(scanned.skipped, 0);
    }

    #[test]
    fn a_bad_file_is_skipped_and_others_still_read() {
        let dir = TestDir::new("recorder-scan-bad");
        dir.write("Match_a.mtgr", b"not a recording");
        dir.write("Match_b.mtgr", &[0xff, 0xfe, 0x00]);
        dir.write("Match_c.mtgr", mtgr_text(GUID_B).as_bytes());
        let scanned = scan_folder(dir.path(), &none(), &mut ScanMemory::default());
        assert_eq!(scanned.recordings.len(), 1);
        assert_eq!(scanned.recordings[0].header.match_token, GUID_B);
        assert_eq!(scanned.skipped, 2);
    }

    #[test]
    fn a_known_hash_is_not_read_again() {
        let dir = TestDir::new("recorder-scan-known");
        let text = mtgr_text(GUID_A);
        dir.write("Match_a.mtgr", text.as_bytes());
        let known: HashSet<String> = [hash_of(text.as_bytes())].into();
        let scanned = scan_folder(dir.path(), &known, &mut ScanMemory::default());
        assert!(scanned.recordings.is_empty());
        assert_eq!(scanned.unchanged, 1);
        assert_eq!(scanned.skipped, 0);
    }

    #[test]
    fn a_missing_folder_is_not_an_error() {
        let dir = TestDir::new("recorder-scan-missing");
        let scanned = scan_folder(&dir.join("nope"), &none(), &mut ScanMemory::default());
        assert!(scanned.recordings.is_empty());
        assert_eq!(scanned.skipped, 0);
    }

    #[test]
    fn the_folder_is_mtgreplay_in_documents() {
        assert_eq!(
            recorder_folder(Path::new("/docs")),
            Path::new("/docs").join("MtgReplay")
        );
    }

    #[test]
    fn a_rejected_file_is_logged_once_and_not_reread() {
        let dir = TestDir::new("recorder-scan-rejected-once");
        dir.write("Match_a.mtgr", b"junk");
        let mut memory = ScanMemory::default();
        let first = scan_folder(dir.path(), &none(), &mut memory);
        assert_eq!((first.skipped, first.read), (1, 1));
        let second = scan_folder(dir.path(), &none(), &mut memory);
        assert_eq!((second.skipped, second.read), (0, 0));
    }

    #[test]
    fn an_unchanged_file_is_not_reread() {
        let dir = TestDir::new("recorder-scan-unchanged");
        dir.write("Match_a.mtgr", mtgr_text(GUID_A).as_bytes());
        let mut memory = ScanMemory::default();
        let first = scan_folder(dir.path(), &none(), &mut memory);
        assert_eq!(first.recordings.len(), 1);
        memory.remember(first.pending);
        let second = scan_folder(dir.path(), &none(), &mut memory);
        assert_eq!((second.recordings.len(), second.read), (0, 0));
    }

    #[test]
    fn a_file_whose_store_failed_is_read_again() {
        let dir = TestDir::new("recorder-scan-not-stored");
        dir.write("Match_a.mtgr", mtgr_text(GUID_A).as_bytes());
        let mut memory = ScanMemory::default();
        scan_folder(dir.path(), &none(), &mut memory);
        let again = scan_folder(dir.path(), &none(), &mut memory);
        assert_eq!(again.recordings.len(), 1);
    }

    #[test]
    fn a_changed_file_is_read_again() {
        let dir = TestDir::new("recorder-scan-changed");
        dir.write("Match_a.mtgr", b"junk");
        let mut memory = ScanMemory::default();
        scan_folder(dir.path(), &none(), &mut memory);
        dir.write("Match_a.mtgr", mtgr_text(GUID_A).as_bytes());
        let again = scan_folder(dir.path(), &none(), &mut memory);
        assert_eq!(again.recordings.len(), 1);
        assert_eq!(again.read, 1);
    }

    #[test]
    fn files_without_the_match_prefix_are_ignored() {
        let dir = TestDir::new("recorder-scan-prefix");
        dir.write("other.mtgr", mtgr_text(GUID_A).as_bytes());
        let scanned = scan_folder(dir.path(), &none(), &mut ScanMemory::default());
        assert_eq!(
            (scanned.recordings.len(), scanned.skipped, scanned.read),
            (0, 0, 0)
        );
    }

    #[test]
    fn files_over_the_size_cap_are_skipped() {
        let dir = TestDir::new("recorder-scan-size");
        let path = dir.write("Match_a.mtgr", mtgr_text(GUID_A).as_bytes());
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(MAX_RECORDING_BYTES + 1).unwrap();
        let scanned = scan_folder(dir.path(), &none(), &mut ScanMemory::default());
        assert_eq!(
            (scanned.recordings.len(), scanned.skipped, scanned.read),
            (0, 1, 0)
        );
    }

    #[test]
    fn one_file_per_token_per_batch() {
        let dir = TestDir::new("recorder-scan-one-token");
        dir.write("Match_a.mtgr", mtgr_text(GUID_A).as_bytes());
        let other = format!(
            "{}T2 PlayerB plays a land.\n",
            mtgr_text(&GUID_A.to_uppercase())
        );
        dir.write("Match_b.mtgr", other.as_bytes());
        let scanned = scan_folder(dir.path(), &none(), &mut ScanMemory::default());
        assert_eq!(scanned.recordings.len(), 1);
        assert_eq!(scanned.recordings[0].text, mtgr_text(GUID_A));
        assert_eq!(scanned.skipped, 1);
    }

    #[test]
    fn a_large_folder_is_imported_over_several_polls() {
        let dir = TestDir::new("recorder-scan-batches");
        let tokens: Vec<String> = (0..5)
            .map(|n| format!("6f1c2d3e-4a5b-4c6d-8e7f-9a0b1c2d3e4{n}"))
            .collect();
        for (n, token) in tokens.iter().enumerate() {
            dir.write(&format!("Match_{n}.mtgr"), mtgr_text(token).as_bytes());
        }
        let mut memory = ScanMemory::default();
        let mut polls: Vec<Vec<String>> = Vec::new();
        loop {
            let scanned = scan_folder_within(dir.path(), &none(), &mut memory, 2, u64::MAX);
            if scanned.recordings.is_empty() {
                break;
            }
            polls.push(
                scanned
                    .recordings
                    .iter()
                    .map(|r| r.header.match_token.clone())
                    .collect(),
            );
            memory.remember(scanned.pending);
        }
        assert_eq!(
            polls,
            vec![
                tokens[0..2].to_vec(),
                tokens[2..4].to_vec(),
                tokens[4..5].to_vec()
            ]
        );
        // The byte cap ends a batch too: room for two files, not three.
        let size = mtgr_text(&tokens[0]).len() as u64;
        let mut fresh = ScanMemory::default();
        let capped = scan_folder_within(dir.path(), &none(), &mut fresh, 50, size * 3 - 1);
        assert_eq!(capped.recordings.len(), 2);
    }

    #[test]
    fn files_are_never_modified() {
        let dir = TestDir::new("recorder-scan-readonly");
        let path = dir.write("Match_a.mtgr", mtgr_text(GUID_A).as_bytes());
        let bad = dir.write("Match_b.mtgr", b"junk");
        let before = |p: &Path| -> (Vec<u8>, SystemTime) {
            (
                std::fs::read(p).unwrap(),
                std::fs::metadata(p).unwrap().modified().unwrap(),
            )
        };
        let (a, b) = (before(&path), before(&bad));
        scan_folder(dir.path(), &none(), &mut ScanMemory::default());
        scan_folder(dir.path(), &none(), &mut ScanMemory::default());
        assert_eq!(before(&path), a);
        assert_eq!(before(&bad), b);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }
}
