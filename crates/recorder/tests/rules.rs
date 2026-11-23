use recorder::{
    CrashWindow, ExitKind, PIN, PinnedFile, RecorderState, download_blocked, exit_kind, file_url,
    parse_status, respawns,
};

#[test]
fn the_shipped_pin_is_complete() {
    assert!(!download_blocked(&PIN));
    for file in PIN.files {
        assert_eq!(file.sha256.len(), 64, "{} has a full sha256", file.name);
        assert!(
            file.sha256
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }
}

#[test]
fn an_empty_hash_blocks_the_download() {
    const FILES: &[PinnedFile] = &[PinnedFile {
        name: "a",
        sha256: "",
    }];
    let pin = recorder::Pin {
        version: "0.1.0",
        consent_version: 1,
        exe: "a",
        files: FILES,
    };
    assert!(download_blocked(&pin));
}

#[test]
fn a_fully_hashed_pin_does_not_block() {
    const FILES: &[PinnedFile] = &[PinnedFile {
        name: "a",
        sha256: "00",
    }];
    let pin = recorder::Pin {
        version: "0.1.0",
        consent_version: 1,
        exe: "a",
        files: FILES,
    };
    assert!(!download_blocked(&pin));
}

#[test]
fn file_urls_use_the_release_host() {
    assert_eq!(
        file_url(&PIN, &PIN.files[0]),
        "https://releases.mymtgo.com/mtgr-record/0.2.0/mtgr-record.exe"
    );
}

#[test]
fn the_pin_lists_the_expected_files() {
    let names: Vec<&str> = PIN.files.iter().map(|f| f.name).collect();
    assert_eq!(
        names,
        [
            "mtgr-record.exe",
            "LICENSE",
            "NOTICE",
            "THIRD-PARTY-NOTICES.md",
            "README.txt"
        ]
    );
    assert_eq!(PIN.exe, "mtgr-record.exe");
    assert_eq!(PIN.consent_version, 1);
}

#[test]
fn exit_codes_map_to_kinds() {
    assert_eq!(exit_kind(Some(0)), ExitKind::Stopped);
    assert_eq!(exit_kind(Some(2)), ExitKind::ConsentNotAccepted);
    assert_eq!(exit_kind(Some(3)), ExitKind::BadArguments);
    assert_eq!(exit_kind(Some(4)), ExitKind::MtgoNotSupported);
    assert_eq!(exit_kind(Some(5)), ExitKind::AlreadyRunning);
    assert_eq!(exit_kind(Some(1)), ExitKind::Crashed);
    assert_eq!(exit_kind(Some(137)), ExitKind::Crashed);
    assert_eq!(exit_kind(None), ExitKind::Crashed);
}

#[test]
fn only_stops_and_crashes_respawn() {
    assert!(respawns(ExitKind::Stopped));
    assert!(respawns(ExitKind::Crashed));
    assert!(!respawns(ExitKind::ConsentNotAccepted));
    assert!(!respawns(ExitKind::BadArguments));
    assert!(!respawns(ExitKind::MtgoNotSupported));
    assert!(!respawns(ExitKind::AlreadyRunning));
}

const MIN: i64 = 60_000;

#[test]
fn five_exits_in_ten_minutes_trip_the_limit() {
    let mut w = CrashWindow::default();
    for i in 0..4 {
        assert!(!w.record(i * MIN));
    }
    assert!(w.record(9 * MIN));
}

#[test]
fn old_exits_fall_out_of_the_window() {
    let mut w = CrashWindow::default();
    for i in 0..4 {
        assert!(!w.record(i * MIN));
    }
    assert!(!w.record(11 * MIN));
}

#[test]
fn status_parses_recording_with_its_match() {
    let text = "state=recording\nmatch=abc-123\nat=2026-11-09T10:00:00.0000000Z\n";
    assert_eq!(
        parse_status(text),
        Some(RecorderState::Recording {
            match_token: Some("abc-123".to_string())
        })
    );
}

#[test]
fn status_recording_with_empty_match_has_no_token() {
    let text = "state=recording\nmatch=\nat=x\n";
    assert_eq!(
        parse_status(text),
        Some(RecorderState::Recording { match_token: None })
    );
}

#[test]
fn status_parses_waiting_and_idle() {
    assert_eq!(
        parse_status("state=waiting\nmatch=\nat=x\n"),
        Some(RecorderState::Waiting)
    );
    assert_eq!(
        parse_status("state=idle\r\nmatch=\r\nat=x\r\n"),
        Some(RecorderState::Idle)
    );
}

#[test]
fn status_with_unknown_state_is_none() {
    assert_eq!(parse_status("state=dancing\nmatch=\nat=x\n"), None);
    assert_eq!(parse_status("match=abc\nat=x\n"), None);
    assert_eq!(parse_status(""), None);
}
