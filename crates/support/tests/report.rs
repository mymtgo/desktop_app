//! The report bundle's contents (release spec section 8.1), ported from
//! 0.x's `BuildSupportBundleTest` where it tests the file choice: the
//! newest `mtgo.log`, the three newest app logs, the summary.

mod common;

use std::path::PathBuf;

use chrono::NaiveDate;
use common::{CountingSink, GeneratedLog, read_zip};
use support::{
    DosTime, FileRef, ReportEntry, SUMMARY_NAME, SummaryInput, is_app_log_name, pick_report_files,
    report_file_name, summary_text, write_report,
};

fn file(name: &str, modified_ms: i64) -> FileRef {
    FileRef {
        path: PathBuf::from(format!("/q/logs/{name}")),
        name: name.to_string(),
        modified_ms: Some(modified_ms),
    }
}

fn watched(folder: &str, modified_ms: i64) -> FileRef {
    FileRef {
        path: PathBuf::from(format!("/q/{folder}/mtgo.log")),
        name: "mtgo.log".to_string(),
        modified_ms: Some(modified_ms),
    }
}

fn names(files: &[FileRef]) -> Vec<&str> {
    files.iter().map(|file| file.name.as_str()).collect()
}

#[test]
fn picks_the_mtgo_log_and_the_app_log() {
    let picked = pick_report_files(&[watched("a", 1)], &[file("mymtgo.2026-04-13.log", 1)], &[]);
    assert_eq!(names(&picked), ["mtgo.log", "mymtgo.2026-04-13.log"]);
}

#[test]
fn picks_only_the_mtgo_log_when_there_are_no_app_logs() {
    let picked = pick_report_files(&[watched("a", 1)], &[], &[]);
    assert_eq!(names(&picked), ["mtgo.log"]);
}

#[test]
fn picks_only_the_app_log_when_there_is_no_mtgo_log() {
    let picked = pick_report_files(&[], &[file("mymtgo.2026-04-13.log", 1)], &[]);
    assert_eq!(names(&picked), ["mymtgo.2026-04-13.log"]);
}

#[test]
fn picks_nothing_when_no_logs_exist() {
    assert!(pick_report_files(&[], &[], &[]).is_empty());
    // Files in the log folder that are not app logs do not count.
    assert!(
        pick_report_files(&[], &[file("notes.txt", 1), file("mymtgo.sqlite", 1)], &[]).is_empty()
    );
}

fn arena(folder: &str, modified_ms: i64) -> FileRef {
    FileRef {
        path: PathBuf::from(format!("/q/{folder}/Player.log")),
        name: "Player.log".to_string(),
        modified_ms: Some(modified_ms),
    }
}

#[test]
fn picks_the_newest_of_each_game_log_mtgo_first_then_arena() {
    let watched_logs = [
        watched("a", 1),
        arena("arena-old", 1),
        watched("b", 3),
        arena("arena", 2),
    ];
    let picked = pick_report_files(&watched_logs, &[file("mymtgo.2026-04-13.log", 1)], &[]);
    assert_eq!(
        names(&picked),
        ["mtgo.log", "Player.log", "mymtgo.2026-04-13.log"]
    );
    assert_eq!(picked[0].path, PathBuf::from("/q/b/mtgo.log"));
    assert_eq!(picked[1].path, PathBuf::from("/q/arena/Player.log"));
}

#[test]
fn picks_the_arena_log_alone_when_mtgo_is_not_watched() {
    let picked = pick_report_files(&[arena("arena", 1)], &[], &[]);
    assert_eq!(names(&picked), ["Player.log"]);
}

#[test]
fn picks_up_to_three_newest_app_logs() {
    let app = [
        file("mymtgo.2026-04-11.log", 1),
        file("mymtgo.2026-04-13.log", 1),
        file("mymtgo.2026-04-12.log", 1),
    ];
    let picked = pick_report_files(&[watched("a", 1)], &app, &[]);
    assert_eq!(
        names(&picked),
        [
            "mtgo.log",
            "mymtgo.2026-04-13.log",
            "mymtgo.2026-04-12.log",
            "mymtgo.2026-04-11.log"
        ]
    );
}

#[test]
fn drops_app_logs_older_than_the_three_newest() {
    // Newest by name (the dates are padded), not by modified time.
    let app = [
        file("mymtgo.2026-04-09.log", 50),
        file("mymtgo.2026-04-13.log", 1),
        file("mymtgo.2026-04-10.log", 40),
        file("mymtgo.2026-04-12.log", 2),
        file("mymtgo.2026-04-11.log", 3),
    ];
    let picked = pick_report_files(&[], &app, &[]);
    assert_eq!(
        names(&picked),
        [
            "mymtgo.2026-04-13.log",
            "mymtgo.2026-04-12.log",
            "mymtgo.2026-04-11.log"
        ]
    );
}

#[test]
fn picks_the_newest_watched_mtgo_log_by_modified_time() {
    let picked = pick_report_files(
        &[watched("old", 10), watched("new", 30), watched("mid", 20)],
        &[],
        &[],
    );
    assert_eq!(picked.len(), 1);
    assert_eq!(picked[0].path, PathBuf::from("/q/new/mtgo.log"));
}

#[test]
fn app_log_names_are_mymtgo_dot_anything_dot_log() {
    assert!(is_app_log_name("mymtgo.2026-10-10.log"));
    assert!(is_app_log_name("mymtgo.log"));
    assert!(!is_app_log_name("mymtgo.sqlite"));
    assert!(!is_app_log_name("laravel.log"));
    assert!(!is_app_log_name("mymtgo-2026.log"));
    assert!(!is_app_log_name("mtgo.log"));
}

#[test]
fn the_newest_recorder_log_is_included() {
    let logs = [
        file("recorder-2026-11-07.log", 1),
        file("recorder-2026-11-09.log", 1),
        file("recorder-2026-11-08.log", 1),
    ];
    let picked = pick_report_files(&[watched("a", 1)], &[], &logs);
    assert_eq!(names(&picked), ["mtgo.log", "recorder-2026-11-09.log"]);
}

#[test]
fn no_recorder_logs_adds_nothing() {
    let picked = pick_report_files(&[watched("a", 1)], &[file("mymtgo.2026-04-13.log", 1)], &[]);
    assert_eq!(names(&picked), ["mtgo.log", "mymtgo.2026-04-13.log"]);
}

#[test]
fn recordings_are_never_in_the_report() {
    let folder = [
        file("2026-11-09_PlayerA_PlayerB.mtgr", 9),
        file("Match_GameLog_1.log", 9),
        file("capture.pcap", 9),
        file("status.txt", 9),
        file("recorder.log", 9),
    ];
    assert!(pick_report_files(&[], &[], &folder).is_empty());
}

#[test]
fn the_summary_names_the_recorder_status() {
    let mut input = summary_input();
    input.recorder_status = Some("recording".to_string());
    let text = summary_text(&input);
    assert!(text.contains("Games: 310\nRecorder: recording\n"), "{text}");
    assert!(!summary_text(&summary_input()).contains("Recorder:"));
}

fn summary_input() -> SummaryInput {
    SummaryInput {
        app_version: "1.0.0-beta.2".to_string(),
        channel: "beta".to_string(),
        os: "windows".to_string(),
        arch: "x86_64".to_string(),
        watcher: "Watching".to_string(),
        logs_watched: 2,
        arena_logs_watched: 1,
        offline_mode: false,
        signed_in: true,
        matches: 120,
        games: 310,
        recorder_status: None,
        skipped: Vec::new(),
    }
}

#[test]
fn the_summary_lists_the_app_and_its_state() {
    assert_eq!(
        summary_text(&summary_input()),
        "MyMTGO report\n\
         App version: 1.0.0-beta.2\n\
         Update channel: beta\n\
         OS: windows (x86_64)\n\
         Watcher: Watching\n\
         Logs watched: 2\n\
         Arena logs watched: 1\n\
         Offline mode: off\n\
         Signed in: yes\n\
         Matches: 120\n\
         Games: 310\n"
    );
}

#[test]
fn the_summary_names_skipped_files() {
    let mut input = summary_input();
    input.offline_mode = true;
    input.signed_in = false;
    input.skipped = vec!["mtgo.log".to_string(), "mymtgo.2026-10-09.log".to_string()];
    let text = summary_text(&input);
    assert!(text.contains("Offline mode: on\n"));
    assert!(text.contains("Signed in: no\n"));
    assert!(
        text.ends_with("Could not read: mtgo.log, mymtgo.2026-10-09.log\n"),
        "{text}"
    );
}

#[test]
fn the_summary_names_no_path_user_or_token() {
    let mut input = summary_input();
    // A skipped file given with its full path is named by its file name only.
    input.skipped = vec![
        "C:\\Users\\Qplayer\\AppData\\Local\\Apps\\2.0\\mtgo.log".to_string(),
        "/home/Qplayer/.local/share/com.mymtgo.desktop/logs/mymtgo.2026-10-09.log".to_string(),
    ];
    let text = summary_text(&input);
    assert!(
        text.contains("Could not read: mtgo.log, mymtgo.2026-10-09.log\n"),
        "{text}"
    );
    for leak in [
        "Qplayer", "Users", "home", "AppData", "\\", "/", "token", "@",
    ] {
        assert!(!text.contains(leak), "{leak} in {text}");
    }
}

#[test]
fn the_report_file_name_is_dated_in_local_time() {
    let local = NaiveDate::from_ymd_opt(2026, 10, 9)
        .unwrap()
        .and_hms_opt(7, 5, 3)
        .unwrap();
    assert_eq!(
        report_file_name(local),
        "mymtgo-report-2026-10-09-070503.zip"
    );
}

fn noon() -> DosTime {
    DosTime::from_local(
        NaiveDate::from_ymd_opt(2026, 10, 10)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap(),
    )
}

#[test]
fn the_bundle_holds_only_logs_and_the_summary() {
    let logs = vec![
        ReportEntry {
            name: "mtgo.log".to_string(),
            reader: &b"MTGO log contents\n"[..],
        },
        ReportEntry {
            name: "mymtgo.2026-10-10.log".to_string(),
            reader: &b"App log contents\n"[..],
        },
    ];
    let summary = summary_text(&summary_input());
    let zip = write_report(Vec::new(), noon(), &summary, logs).unwrap();
    let entries = read_zip(&zip);
    let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(names, [SUMMARY_NAME, "mtgo.log", "mymtgo.2026-10-10.log"]);
    assert_eq!(entries[0].data, summary.as_bytes());
    assert_eq!(entries[1].data, b"MTGO log contents\n");
    assert_eq!(entries[2].data, b"App log contents\n");
    assert!(entries.iter().all(|entry| entry.method == 8), "deflated");
}

/// Spec section 19: a 200 MB `mtgo.log` bundles in under 10 s, memory flat.
/// Measured in release builds (`cargo test --release -p support`).
#[test]
#[cfg_attr(debug_assertions, ignore = "speed budget: run with --release")]
fn bundle_with_a_200_mb_log_under_10_s() {
    const LEN: u64 = 200 * 1024 * 1024;
    let mut log = GeneratedLog::new(LEN);
    let started = std::time::Instant::now();
    let logs = vec![ReportEntry {
        name: "mtgo.log".to_string(),
        reader: &mut log,
    }];
    let sink = write_report(CountingSink::default(), noon(), "summary", logs).unwrap();
    let elapsed = started.elapsed();
    eprintln!(
        "bundle of a 200 MB log: {:.2} s, {} bytes out, largest write {}",
        elapsed.as_secs_f64(),
        sink.total,
        sink.largest_write
    );
    assert_eq!(log.remaining, 0);
    assert!(elapsed.as_secs_f64() < 10.0, "took {elapsed:?}");
}

#[test]
fn report_never_sends_endstep() {
    // Endstep's inbox files (capture units, the last-seen file, the helper's
    // log) sit among the app and recorder files: the bundle holds logs only,
    // and the tracker's watched game logs have no Endstep file.
    let noise = [
        file("endstep-unit-1.jsonl", 9),
        file("last_seen.json", 9),
        file("mymtgo.sqlite", 9),
        file("endstep-host.log", 9),
    ];
    let picked = pick_report_files(&[watched("a", 1)], &noise, &noise);
    assert_eq!(names(&picked), ["mtgo.log"]);
}
