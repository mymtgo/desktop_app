//! Ported from 0.x's `BuildSupportBundleTest` (eight) and
//! `DownloadReportBundleTest` (two), on temp folders: v1 has one kind of app
//! log (`mymtgo.<date>.log`) where 0.x had `laravel*.log` and
//! `pipeline-*.log`, so 0.x's pairs of tests per log kind become one.

use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use chrono::NaiveDate;
use flate2::read::DeflateDecoder;

use super::*;
use crate::summary::FileView;
use crate::updates::UpdateChannel;

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A fresh folder under the temp folder, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "mymtgo-support-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn put(&self, relative: &str, text: &str) -> PathBuf {
        let path = self.0.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// The 0.x test setup: an MTGO folder with `mtgo.log`, MyMTGO's log folder
/// with one app log.
struct Setup {
    root: Scratch,
    mtgo_log: PathBuf,
    log_dir: PathBuf,
}

fn setup(label: &str) -> Setup {
    let root = Scratch::new(label);
    let mtgo_log = root.put("mtgo/mtgo.log", "MTGO log contents\n");
    root.put("data/logs/mymtgo.2026-04-13.log", "App log contents\n");
    let log_dir = root.0.join("data/logs");
    Setup {
        root,
        mtgo_log,
        log_dir,
    }
}

impl Setup {
    fn watched(&self) -> Vec<String> {
        vec![self.mtgo_log.to_string_lossy().into_owned()]
    }

    fn app_log(&self, name: &str, text: &str) {
        self.root.put(&format!("data/logs/{name}"), text);
    }

    /// Build the bundle as the command does; the zip's entries.
    fn bundle(&self) -> Result<Vec<(String, Vec<u8>)>, BundleError> {
        let files = find_report_files(&self.watched(), &self.log_dir, None);
        let out = self.root.0.join("out.zip");
        write_bundle(&out, &files, summary(), noon())?;
        Ok(read_zip(&std::fs::read(&out).unwrap()))
    }
}

fn noon() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 10, 10)
        .unwrap()
        .and_hms_opt(12, 0, 0)
        .unwrap()
}

fn summary() -> SummaryInput {
    summary_input(
        "1.0.0-beta.2".to_string(),
        &StatusView::waiting(None),
        UpdateChannel::Edge,
        false,
        (0, 0),
    )
}

fn u16_at(bytes: &[u8], at: usize) -> usize {
    usize::from(u16::from_le_bytes([bytes[at], bytes[at + 1]]))
}

fn u32_at(bytes: &[u8], at: usize) -> usize {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize
}

/// Names and contents of a zip's entries, through its central directory
/// (the `support` crate's tests check the format in full).
fn read_zip(zip: &[u8]) -> Vec<(String, Vec<u8>)> {
    let end = zip.len() - 22;
    assert_eq!(u32_at(zip, end), 0x0605_4b50);
    let mut at = u32_at(zip, end + 16);
    let mut entries = Vec::new();
    for _ in 0..u16_at(zip, end + 10) {
        let method = u16_at(zip, at + 10);
        let compressed = u32_at(zip, at + 20);
        let name_len = u16_at(zip, at + 28);
        let skip = name_len + u16_at(zip, at + 30) + u16_at(zip, at + 32);
        let local = u32_at(zip, at + 42);
        let name = String::from_utf8(zip[at + 46..at + 46 + name_len].to_vec()).unwrap();
        at += 46 + skip;
        let data_at = local + 30 + u16_at(zip, local + 26) + u16_at(zip, local + 28);
        let raw = &zip[data_at..data_at + compressed];
        let mut data = Vec::new();
        assert_eq!(method, 8, "deflated");
        DeflateDecoder::new(raw).read_to_end(&mut data).unwrap();
        entries.push((name, data));
    }
    entries
}

fn names(entries: &[(String, Vec<u8>)]) -> Vec<&str> {
    entries.iter().map(|(name, _)| name.as_str()).collect()
}

fn data<'a>(entries: &'a [(String, Vec<u8>)], name: &str) -> Option<&'a [u8]> {
    entries
        .iter()
        .find(|(entry, _)| entry == name)
        .map(|(_, data)| data.as_slice())
}

#[test]
fn zips_the_mtgo_log_the_app_log_and_the_summary() {
    let setup = setup("all");
    let entries = setup.bundle().unwrap();
    assert_eq!(
        names(&entries),
        ["summary.txt", "mtgo.log", "mymtgo.2026-04-13.log"]
    );
    assert_eq!(
        data(&entries, "mtgo.log"),
        Some(&b"MTGO log contents\n"[..])
    );
    assert_eq!(
        data(&entries, "mymtgo.2026-04-13.log"),
        Some(&b"App log contents\n"[..])
    );
}

#[test]
fn zips_only_the_mtgo_log_when_no_app_logs_exist() {
    let setup = setup("mtgo-only");
    std::fs::remove_file(setup.log_dir.join("mymtgo.2026-04-13.log")).unwrap();
    let entries = setup.bundle().unwrap();
    assert_eq!(names(&entries), ["summary.txt", "mtgo.log"]);
    assert_eq!(
        data(&entries, "mtgo.log"),
        Some(&b"MTGO log contents\n"[..])
    );
}

#[test]
fn zips_only_the_app_log_when_no_mtgo_log_exists() {
    let setup = setup("app-only");
    std::fs::remove_file(&setup.mtgo_log).unwrap();
    let entries = setup.bundle().unwrap();
    assert_eq!(names(&entries), ["summary.txt", "mymtgo.2026-04-13.log"]);
}

#[test]
fn no_logs_is_the_no_log_files_error() {
    let setup = setup("none");
    std::fs::remove_file(&setup.mtgo_log).unwrap();
    std::fs::remove_file(setup.log_dir.join("mymtgo.2026-04-13.log")).unwrap();
    assert_eq!(setup.bundle(), Err(BundleError::NoLogFiles));
    // A missing log folder is the same.
    let files = find_report_files(&[], &setup.root.0.join("nowhere"), None);
    assert!(files.is_empty());
}

#[test]
fn includes_up_to_three_most_recent_app_logs() {
    let setup = setup("three");
    setup.app_log("mymtgo.2026-04-12.log", "Day 2\n");
    setup.app_log("mymtgo.2026-04-11.log", "Day 3\n");
    let entries = setup.bundle().unwrap();
    // summary + mtgo.log + 3 app logs
    assert_eq!(entries.len(), 5);
    assert_eq!(
        data(&entries, "mymtgo.2026-04-13.log"),
        Some(&b"App log contents\n"[..])
    );
    assert_eq!(
        data(&entries, "mymtgo.2026-04-12.log"),
        Some(&b"Day 2\n"[..])
    );
    assert_eq!(
        data(&entries, "mymtgo.2026-04-11.log"),
        Some(&b"Day 3\n"[..])
    );
}

#[test]
fn drops_app_logs_older_than_the_three_most_recent() {
    let setup = setup("older");
    setup.app_log("mymtgo.2026-04-12.log", "Day 2\n");
    setup.app_log("mymtgo.2026-04-11.log", "Day 3\n");
    setup.app_log("mymtgo.2026-04-10.log", "Day 4 (excluded)\n");
    setup.app_log("mymtgo.2026-04-09.log", "Day 5 (excluded)\n");
    let entries = setup.bundle().unwrap();
    assert_eq!(entries.len(), 5);
    assert_eq!(data(&entries, "mymtgo.2026-04-10.log"), None);
    assert_eq!(data(&entries, "mymtgo.2026-04-09.log"), None);
}

#[test]
fn takes_the_most_recently_modified_watched_mtgo_log() {
    // 0.x: `FindMtgoLogPath::all()->last()`, sorted by modified time.
    let setup = setup("newest");
    let older = setup.root.put("mtgo-old/mtgo.log", "Older log\n");
    let old_time = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    File::options()
        .write(true)
        .open(&older)
        .unwrap()
        .set_modified(old_time)
        .unwrap();
    let watched = vec![
        setup.mtgo_log.to_string_lossy().into_owned(),
        older.to_string_lossy().into_owned(),
    ];
    let files = find_report_files(&watched, &setup.log_dir, None);
    assert_eq!(files[0].path, setup.mtgo_log);
    // A watched file that no longer exists is not found.
    let gone = vec![
        setup
            .root
            .0
            .join("gone/mtgo.log")
            .to_string_lossy()
            .into_owned(),
    ];
    assert!(list_watched_logs(&gone).is_empty());
}

#[test]
fn a_file_that_cannot_be_read_is_skipped_and_named_in_the_summary() {
    let setup = setup("skipped");
    let files = find_report_files(&setup.watched(), &setup.log_dir, None);
    // Gone between the listing and the zip.
    std::fs::remove_file(&setup.mtgo_log).unwrap();
    let out = setup.root.0.join("out.zip");
    write_bundle(&out, &files, summary(), noon()).unwrap();
    let entries = read_zip(&std::fs::read(&out).unwrap());
    assert_eq!(names(&entries), ["summary.txt", "mymtgo.2026-04-13.log"]);
    let text = String::from_utf8(data(&entries, "summary.txt").unwrap().to_vec()).unwrap();
    assert!(text.ends_with("Could not read: mtgo.log\n"), "{text}");
}

// DownloadReportBundleTest

#[test]
fn a_zip_named_for_the_local_time_is_written() {
    let setup = setup("download");
    let files = find_report_files(&setup.watched(), &setup.log_dir, None);
    let name = ::support::report_file_name(noon());
    assert_eq!(name, "mymtgo-report-2026-10-10-120000.zip");
    let out = setup.root.0.join(&name);
    write_bundle(&out, &files, summary(), noon()).unwrap();
    let bytes = std::fs::read(&out).unwrap();
    assert_eq!(&bytes[..4], b"PK\x03\x04");
    assert_eq!(read_zip(&bytes).len(), 3);
}

#[test]
fn nothing_is_written_when_no_log_files_exist() {
    let setup = setup("download-none");
    let out = setup.root.0.join("report.zip");
    assert_eq!(
        write_bundle(&out, &[], summary(), noon()),
        Err(BundleError::NoLogFiles)
    );
    assert!(!out.exists());
}

#[test]
fn a_failed_write_leaves_no_partial_zip() {
    let setup = setup("failed");
    let files = find_report_files(&setup.watched(), &setup.log_dir, None);
    // The target is a folder: creating the file fails.
    let out = setup.root.0.join("data");
    assert!(matches!(
        write_bundle(&out, &files, summary(), noon()),
        Err(BundleError::Failed(_))
    ));
    assert!(out.is_dir());
}

/// A log that fails part way through, after the zip file was created.
struct FailingReader;

impl Read for FailingReader {
    fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("the disk went away"))
    }
}

#[test]
fn a_write_failing_after_the_file_was_created_removes_the_partial_zip() {
    let scratch = Scratch::new("partial");
    let out = scratch.0.join("report.zip");
    let logs = vec![::support::ReportEntry {
        name: "mtgo.log".to_string(),
        reader: FailingReader,
    }];
    let dos_time = ::support::DosTime::from_local(noon());
    let written = write_zip(&out, dos_time, "summary", logs);
    assert!(
        matches!(&written, Err(BundleError::Failed(error)) if error.contains("the disk went away")),
        "{written:?}"
    );
    assert!(!out.exists(), "the partial zip is removed");
}

#[test]
fn the_bundle_holds_only_logs_and_the_summary() {
    let setup = setup("only-logs");
    // Everything else the data and MTGO folders hold stays out.
    setup.root.put("data/mymtgo.sqlite", "database");
    setup
        .root
        .put("data/.env.local", "MYMTGO_API_CLIENT_SECRET=q");
    setup.root.put("data/secrets.bin", "tokens");
    setup.root.put("data/logs/settings.json", "{}");
    setup.root.put("data/logs/mymtgo.sqlite-wal", "wal");
    setup.root.put("mtgo/Qplayer_deck.xml", "<Deck/>");
    setup.root.put("mtgo/Match_GameLog_1.dat", "game log");
    let entries = setup.bundle().unwrap();
    assert_eq!(
        names(&entries),
        ["summary.txt", "mtgo.log", "mymtgo.2026-04-13.log"]
    );
}

#[test]
fn the_summary_names_no_path_user_or_token() {
    let mut status = StatusView::waiting(None);
    status.started = true;
    status.searched = vec!["C:\\Users\\Qplayer\\AppData\\Local\\Apps\\2.0".to_string()];
    status.files = vec![FileView {
        path: "C:\\Users\\Qplayer\\AppData\\Local\\Apps\\2.0\\mtgo.log".to_string(),
        size: 10,
        cursor: 10,
        last_live_growth_ms: None,
        bad_timezone: false,
    }];
    status.offline_mode = true;
    let mut input = summary_input(
        "1.0.0-beta.2".to_string(),
        &status,
        UpdateChannel::Edge,
        true,
        (12, 30),
    );
    input.skipped = vec![status.files[0].path.clone()];
    let text = ::support::summary_text(&input);
    assert!(text.contains("Watcher: Watching\n"), "{text}");
    assert!(text.contains("Logs watched: 1\n"));
    assert!(text.contains("Offline mode: on\n"));
    assert!(text.contains("Signed in: yes\n"));
    assert!(text.contains("Matches: 12\nGames: 30\n"));
    for leak in ["Qplayer", "Users", "AppData", "\\", "token"] {
        assert!(!text.contains(leak), "{leak} in {text}");
    }
}

fn file_view(path: &str) -> FileView {
    FileView {
        path: path.to_string(),
        size: 10,
        cursor: 10,
        last_live_growth_ms: None,
        bad_timezone: false,
    }
}

#[test]
fn the_watched_logs_are_mtgo_then_arena() {
    let mut status = StatusView::waiting(None);
    status.files = vec![file_view("/m/mtgo.log")];
    status.arena_files = vec![file_view("/a/Player.log")];
    assert_eq!(watched_logs(&status), ["/m/mtgo.log", "/a/Player.log"]);
}

#[test]
fn zips_the_arena_log_beside_the_mtgo_log() {
    let setup = setup("arena");
    let arena_log = setup.root.put("arena/Player.log", "Arena log contents\n");
    let mut watched = setup.watched();
    watched.push(arena_log.to_string_lossy().into_owned());
    let files = find_report_files(&watched, &setup.log_dir, None);
    let out = setup.root.0.join("out.zip");
    write_bundle(&out, &files, summary(), noon()).unwrap();
    let entries = read_zip(&std::fs::read(&out).unwrap());
    assert_eq!(
        names(&entries),
        [
            "summary.txt",
            "mtgo.log",
            "Player.log",
            "mymtgo.2026-04-13.log"
        ]
    );
    assert_eq!(
        data(&entries, "Player.log"),
        Some(&b"Arena log contents\n"[..])
    );
}

#[test]
fn the_summary_counts_the_arena_logs() {
    let mut status = StatusView::waiting(None);
    status.arena_files = vec![file_view("/a/Player.log")];
    let input = summary_input(
        "1.2.0-rc.1".to_string(),
        &status,
        UpdateChannel::Edge,
        false,
        (0, 0),
    );
    let text = ::support::summary_text(&input);
    assert!(
        text.contains("Logs watched: 0\nArena logs watched: 1\n"),
        "{text}"
    );
}

#[test]
fn the_watcher_line_follows_the_tracker() {
    let mut status = StatusView::waiting(None);
    assert_eq!(watcher_state(&status), "Starting");
    status.started = true;
    assert_eq!(watcher_state(&status), "Watching");
    status.restarting = Some("the tracker stopped".to_string());
    assert_eq!(watcher_state(&status), "Restarting");
}

#[test]
fn a_stopped_watcher_reads_stopped() {
    let mut status = StatusView::waiting(None);
    status.started = true;
    status.paused = true;
    assert_eq!(watcher_state(&status), "Stopped");
    // A restart outranks the pause, as the status bar's order.
    status.restarting = Some("the tracker stopped".to_string());
    assert_eq!(watcher_state(&status), "Restarting");
}

#[test]
fn the_channel_line_is_the_update_channel() {
    let status = StatusView::waiting(None);
    let stable = summary_input(
        "1.0.0".to_string(),
        &status,
        UpdateChannel::Stable,
        false,
        (0, 0),
    );
    assert_eq!(stable.channel, "stable");
    let beta = summary_input(
        "1.0.0".to_string(),
        &status,
        UpdateChannel::Edge,
        false,
        (0, 0),
    );
    assert_eq!(beta.channel, "edge");
}

#[test]
fn the_local_time_is_the_timezone_s_wall_clock() {
    // 2026-10-10 11:00:00 UTC is 12:00 in London (BST).
    let now_ms = 1_791_630_000_000;
    assert_eq!(local_now(chrono_tz::Europe::London, now_ms), noon());
}

#[test]
fn a_failed_write_keeps_the_file_already_at_the_chosen_name() {
    // The save dialog confirmed replacing an older report: a write that
    // fails never truncates it, and no `.partial` is left behind.
    let scratch = Scratch::new("keep-old");
    let out = scratch.put("report.zip", "older report");
    let logs = vec![::support::ReportEntry {
        name: "mtgo.log".to_string(),
        reader: FailingReader,
    }];
    let dos_time = ::support::DosTime::from_local(noon());
    assert!(write_zip(&out, dos_time, "summary", logs).is_err());
    assert_eq!(std::fs::read(&out).unwrap(), b"older report");
    assert!(!scratch.0.join("report.zip.partial").exists());
}

#[test]
fn a_finished_write_replaces_the_file_and_leaves_no_partial() {
    let setup = setup("replace");
    let files = find_report_files(&setup.watched(), &setup.log_dir, None);
    let out = setup.root.put("report.zip", "older report");
    write_bundle(&out, &files, summary(), noon()).unwrap();
    assert_eq!(read_zip(&std::fs::read(&out).unwrap()).len(), 3);
    assert!(!setup.root.0.join("report.zip.partial").exists());
}

#[test]
fn the_partial_name_is_the_chosen_name_plus_partial() {
    let scratch = Scratch::new("partial-name");
    let out = scratch.0.join("mymtgo-report.zip");
    assert_eq!(
        partial_path(&out),
        scratch.0.join("mymtgo-report.zip.partial")
    );
}

// Review M2: a file already at `<name>.partial` is never truncated or
// removed; the zip is written beside it under `<name>.partial-<n>`.
#[test]
fn a_file_already_at_the_partial_name_is_left_alone() {
    let setup = setup("partial-taken");
    let files = find_report_files(&setup.watched(), &setup.log_dir, None);
    let out = setup.root.0.join("report.zip");
    let taken = setup.root.put("report.zip.partial", "user file");
    write_bundle(&out, &files, summary(), noon()).unwrap();
    assert_eq!(read_zip(&std::fs::read(&out).unwrap()).len(), 3);
    assert_eq!(std::fs::read(&taken).unwrap(), b"user file");
    assert!(!setup.root.0.join("report.zip.partial-1").exists());
}

#[test]
fn a_failed_write_removes_only_the_partial_it_created() {
    let scratch = Scratch::new("partial-taken-failed");
    let out = scratch.0.join("report.zip");
    let taken = scratch.put("report.zip.partial", "user file");
    let logs = vec![::support::ReportEntry {
        name: "mtgo.log".to_string(),
        reader: FailingReader,
    }];
    let dos_time = ::support::DosTime::from_local(noon());
    assert!(write_zip(&out, dos_time, "summary", logs).is_err());
    assert_eq!(std::fs::read(&taken).unwrap(), b"user file");
    assert!(!scratch.0.join("report.zip.partial-1").exists());
    assert!(!out.exists());
}

#[test]
fn the_recorder_line_shows_only_when_enabled_or_recorded() {
    assert_eq!(recorder_summary(false, 0, &RecorderStatus::Off), None);
    assert_eq!(
        recorder_summary(true, 0, &RecorderStatus::Recording).as_deref(),
        Some("recording")
    );
    assert_eq!(
        recorder_summary(false, 3, &RecorderStatus::Off).as_deref(),
        Some("off")
    );
    let failed = RecorderStatus::Failed {
        reason: "C:\\Users\\PlayerA\\x".to_string(),
    };
    assert_eq!(recorder_status_word(&failed), "failed");
}

#[test]
fn only_the_recorder_log_is_picked_from_the_recorder_folder() {
    let setup = setup("recorder-folder");
    setup.root.put("rec/recorder-20261109.log", "recorder\n");
    setup.root.put("rec/Match_x.log", "names\n");
    setup.root.put("rec/x.mtgr", "names\n");
    let rec = setup.root.0.join("rec");
    let files = find_report_files(&setup.watched(), &setup.log_dir, Some(&rec));
    let names: Vec<&str> = files.iter().map(|file| file.name.as_str()).collect();
    assert!(names.contains(&"recorder-20261109.log"), "{names:?}");
    assert!(!names.contains(&"Match_x.log") && !names.contains(&"x.mtgr"));
}

#[test]
fn an_unused_recorder_adds_no_file_and_no_line() {
    let rec = Some(PathBuf::from("/rec"));
    let unused = recorder_summary(false, 0, &RecorderStatus::Off);
    assert_eq!(unused, None);
    assert_eq!(gated_recorder_dir(&unused, rec.clone()), None);
    let used = recorder_summary(true, 0, &RecorderStatus::Ready);
    assert_eq!(gated_recorder_dir(&used, rec.clone()), rec);
}
