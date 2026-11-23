//! Downloads, the 0.x copy and clearing against Plan Q's stub server and a
//! running tracker (no test talks to Scryfall).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use fixture_tools::TestDir;

use super::*;
use crate::api::stub::{Recorded, Reply, Stub};
use crate::bridge::lock;
use crate::cards::populate::tests::{Tracked, tracked};

/// A JPEG-ish body for a path (`/10.jpg` answers `img:/10.jpg`).
fn image_body(request: &Recorded) -> Reply {
    Reply::Respond {
        status: 200,
        headers: vec![("Content-Type".to_string(), "image/jpeg".to_string())],
        body: format!("img:{}", request.target).into_bytes(),
    }
}

/// `(local_image, local_art_crop)` of a row.
fn local(run: &Tracked, catalog_id: u32) -> (Option<String>, Option<String>) {
    store::open_reader(&run.state.db_path)
        .expect("reader")
        .query_row(
            "SELECT local_image, local_art_crop FROM cards WHERE catalog_id = ?1",
            [catalog_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("row")
}

fn card(run: &Tracked, catalog_id: u32, base: &str) {
    run.write(&format!(
        "INSERT INTO cards (catalog_id, name, image, art_crop)
         VALUES ({catalog_id}, 'Qcard', '{base}/{catalog_id}.jpg', '{base}/{catalog_id}a.jpg')"
    ));
}

fn recorder(run: &Tracked) -> CardImagesRecorder {
    lock(&run.state.tracker)
        .as_ref()
        .expect("tracker")
        .card_images()
}

/// Run `body` with an `ImageRun` over `folder`, fetching from `origin`.
fn with_run<T>(
    run: &Tracked,
    folder: &Path,
    origin: &str,
    legacy: &PendingCopy,
    body: impl FnOnce(&ImageRun) -> T,
) -> T {
    let db = run.state.db_path.clone();
    let reader = move || store::open_reader(&db).ok();
    let fetcher = Fetcher::new(origin);
    let recorder = recorder(run);
    body(&ImageRun {
        folder,
        is_default: true,
        start_after: 0,
        fetcher: &fetcher,
        reader: &reader,
        recorder: &recorder,
        present: &|files| recorder.present(files),
        legacy,
        should_yield: &|| false,
        fetch_pause: std::time::Duration::ZERO,
    })
}

fn targets(stub: &Stub) -> Vec<String> {
    let mut targets: Vec<String> = stub.requests().into_iter().map(|r| r.target).collect();
    targets.sort();
    targets
}

// 0.x CardImagesFolderTest: "stores no custom folder by default".
#[test]
fn stores_no_custom_folder_by_default() {
    let settings = Settings::default();
    assert_eq!(settings.card_images_path, None);
    assert_eq!(
        images_folder(&settings, Path::new("/q/data")),
        Path::new("/q/data").join(DEFAULT_FOLDER)
    );
}

#[test]
fn downloads_store_both_files_and_columns() {
    let stub = Stub::start(image_body);
    let run = tracked("images-download");
    card(&run, 10, &stub.base_url);
    card(&run, 11, &stub.base_url);
    let dir = TestDir::new("images-download-folder");
    let folder = dir.join("card-images");
    let report = with_run(
        &run,
        &folder,
        &stub.base_url,
        &PendingCopy::default(),
        download_images,
    )
    .expect("ran");
    assert_eq!(report.downloaded, 4);
    assert_eq!(
        targets(&stub),
        vec!["/10.jpg", "/10a.jpg", "/11.jpg", "/11a.jpg"]
    );
    assert_eq!(
        fs::read(folder.join("10.jpg")).expect("image"),
        b"img:/10.jpg"
    );
    assert_eq!(
        fs::read(folder.join("10_art.jpg")).expect("art"),
        b"img:/10a.jpg"
    );
    assert_eq!(
        local(&run, 11),
        (Some("11.jpg".to_string()), Some("11_art.jpg".to_string()))
    );
    let leftovers: Vec<String> = fs::read_dir(&folder)
        .expect("folder")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !is_owned_file(name))
        .collect();
    assert!(
        leftovers.is_empty(),
        "no temporary file left: {leftovers:?}"
    );
    // Nothing is fetched twice.
    let again = with_run(
        &run,
        &folder,
        &stub.base_url,
        &PendingCopy::default(),
        download_images,
    )
    .expect("ran");
    assert_eq!(again.downloaded, 0);
    assert_eq!(stub.requests().len(), 4);
}

#[test]
fn a_failed_download_leaves_the_column_and_is_retried() {
    let failing = Arc::new(AtomicBool::new(true));
    let fail = Arc::clone(&failing);
    let stub = Stub::start(move |request| {
        if request.target == "/10a.jpg" && fail.load(Ordering::SeqCst) {
            Reply::json(500, "{}")
        } else {
            image_body(request)
        }
    });
    let run = tracked("images-retry");
    card(&run, 10, &stub.base_url);
    let dir = TestDir::new("images-retry-folder");
    let folder = dir.join("card-images");
    let first = with_run(
        &run,
        &folder,
        &stub.base_url,
        &PendingCopy::default(),
        download_images,
    )
    .expect("ran");
    assert_eq!((first.downloaded, first.failed), (1, 1));
    assert_eq!(local(&run, 10), (Some("10.jpg".to_string()), None));
    assert!(!folder.join("10_art.jpg").exists());
    failing.store(false, Ordering::SeqCst);
    let second = with_run(
        &run,
        &folder,
        &stub.base_url,
        &PendingCopy::default(),
        download_images,
    )
    .expect("ran");
    assert_eq!((second.downloaded, second.failed), (1, 0));
    assert_eq!(
        targets(&stub),
        vec!["/10.jpg", "/10a.jpg", "/10a.jpg"],
        "only the failed file is asked again"
    );
    assert_eq!(
        local(&run, 10),
        (Some("10.jpg".to_string()), Some("10_art.jpg".to_string()))
    );
}

/// A 429 or 503 stops the run: the card is not marked failed, nothing more
/// is asked, and the next run takes up where it was.
fn rate_limit_stops_the_run(status: u16, label: &str) {
    let limited = Arc::new(AtomicBool::new(true));
    let flag = Arc::clone(&limited);
    let stub = Stub::start(move |request| {
        if request.target == "/11.jpg" && flag.load(Ordering::SeqCst) {
            Reply::json(status, "{}")
        } else {
            image_body(request)
        }
    });
    let run = tracked(label);
    card(&run, 10, &stub.base_url);
    card(&run, 11, &stub.base_url);
    card(&run, 12, &stub.base_url);
    let dir = TestDir::new(&format!("{label}-folder"));
    let folder = dir.join("card-images");
    let first = with_run(
        &run,
        &folder,
        &stub.base_url,
        &PendingCopy::default(),
        download_images,
    )
    .expect("ran");
    assert!(first.rate_limited);
    assert_eq!((first.downloaded, first.failed), (2, 0));
    assert_eq!(
        targets(&stub),
        vec!["/10.jpg", "/10a.jpg", "/11.jpg"],
        "nothing after the refused request"
    );
    limited.store(false, Ordering::SeqCst);
    let second = with_run(
        &run,
        &folder,
        &stub.base_url,
        &PendingCopy::default(),
        download_images,
    )
    .expect("ran");
    assert!(!second.rate_limited);
    assert_eq!((second.downloaded, second.failed), (4, 0));
}

#[test]
fn a_429_stops_the_run_without_failing_the_card() {
    rate_limit_stops_the_run(429, "images-429");
}

#[test]
fn a_503_stops_the_run_without_failing_the_card() {
    rate_limit_stops_the_run(503, "images-503");
}

#[test]
fn the_run_waits_between_fetches() {
    let stub = Stub::start(image_body);
    let run = tracked("images-pause");
    card(&run, 10, &stub.base_url);
    let dir = TestDir::new("images-pause-folder");
    let folder = dir.join("card-images");
    let db = run.state.db_path.clone();
    let reader = move || store::open_reader(&db).ok();
    let fetcher = Fetcher::new(&stub.base_url);
    let recorder = recorder(&run);
    let legacy = PendingCopy::default();
    let started = std::time::Instant::now();
    let report = download_images(&ImageRun {
        folder: &folder,
        is_default: true,
        start_after: 0,
        fetcher: &fetcher,
        reader: &reader,
        recorder: &recorder,
        present: &|files| recorder.present(files),
        legacy: &legacy,
        should_yield: &|| false,
        fetch_pause: std::time::Duration::from_millis(60),
    })
    .expect("ran");
    assert_eq!(report.downloaded, 2);
    assert!(
        started.elapsed() >= std::time::Duration::from_millis(120),
        "one pause after each of the two fetches"
    );
}

#[test]
fn a_non_scryfall_host_is_never_fetched() {
    let stub = Stub::start(image_body);
    let run = tracked("images-host");
    // The allowed origin is the stub's; these rows point elsewhere.
    run.write(&format!(
        "INSERT INTO cards (catalog_id, name, image, art_crop) VALUES
             (10, 'Qcard', 'http://localhost:{port}/10.jpg', 'https://127.0.0.1:{port}/10a.jpg'),
             (11, 'Qcard', '{base}.evil/11.jpg', '{base}@evil.test/11a.jpg');",
        port = stub.base_url.rsplit(':').next().expect("port"),
        base = stub.base_url,
    ));
    let dir = TestDir::new("images-host-folder");
    let folder = dir.join("card-images");
    let report = with_run(
        &run,
        &folder,
        &stub.base_url,
        &PendingCopy::default(),
        download_images,
    )
    .expect("ran");
    assert_eq!((report.downloaded, report.failed), (0, 4));
    assert_eq!(stub.connections(), 0, "no connection was opened");
    assert_eq!(local(&run, 10), (None, None));
    // The app's fetcher allows only the card image hosts.
    assert!(
        Fetcher::standard()
            .fetch(&format!("{}/10.jpg", stub.base_url))
            .is_err()
    );
    assert_eq!(stub.connections(), 0);
}

#[test]
fn an_oversized_image_is_refused() {
    let stub = Stub::start(|request| {
        let size = if request.target == "/10.jpg" {
            MAX_IMAGE_BYTES as usize + 1
        } else {
            MAX_IMAGE_BYTES as usize
        };
        Reply::Respond {
            status: 200,
            headers: Vec::new(),
            body: vec![b'q'; size],
        }
    });
    let run = tracked("images-oversized");
    card(&run, 10, &stub.base_url);
    let dir = TestDir::new("images-oversized-folder");
    let folder = dir.join("card-images");
    let report = with_run(
        &run,
        &folder,
        &stub.base_url,
        &PendingCopy::default(),
        download_images,
    )
    .expect("ran");
    assert_eq!((report.downloaded, report.failed), (1, 1));
    assert_eq!(local(&run, 10), (None, Some("10_art.jpg".to_string())));
    assert!(!folder.join("10.jpg").exists());
    assert_eq!(
        fs::metadata(folder.join("10_art.jpg")).expect("art").len(),
        MAX_IMAGE_BYTES,
        "exactly the cap is fine"
    );
}

fn planted(folder: &Path, names: &[&str]) {
    fs::create_dir_all(folder).expect("folder");
    for name in names {
        fs::write(folder.join(name), b"q").expect("file");
    }
}

#[test]
fn an_import_copies_then_downloads_only_the_rest() {
    let stub = Stub::start(image_body);
    let run = tracked("images-import-copy");
    card(&run, 10, &stub.base_url);
    card(&run, 11, &stub.base_url);
    let dir = TestDir::new("images-import-copy-folders");
    let legacy_folder = dir.join("0x-cards");
    fs::create_dir_all(&legacy_folder).expect("0.x folder");
    fs::write(legacy_folder.join("10.jpg"), b"0x-10").expect("file");
    fs::write(legacy_folder.join("10_art.jpg"), b"0x-10-art").expect("file");
    fs::write(legacy_folder.join("11_art.jpg"), b"0x-11-art").expect("file");
    let folder = dir.join("card-images");
    let pending = PendingCopy::default();
    pending.hand(legacy_folder.clone());
    let report = with_run(&run, &folder, &stub.base_url, &pending, |run| {
        image_round(run, true)
    })
    .expect("ran");
    assert_eq!((report.copied, report.downloaded, report.failed), (3, 1, 0));
    assert_eq!(
        targets(&stub),
        vec!["/11.jpg"],
        "copied files cause no request"
    );
    assert_eq!(fs::read(folder.join("10.jpg")).expect("copy"), b"0x-10");
    assert_eq!(
        fs::read(folder.join("11.jpg")).expect("download"),
        b"img:/11.jpg"
    );
    for id in [10, 11] {
        assert_eq!(
            local(&run, id),
            (Some(format!("{id}.jpg")), Some(format!("{id}_art.jpg")))
        );
    }
    assert!(pending.take().is_none(), "the copy runs once");
    assert_eq!(
        fs::read(legacy_folder.join("10.jpg")).expect("0.x file kept"),
        b"0x-10"
    );
    assert_eq!(owned_size(&folder), 5 + 9 + 9 + "img:/11.jpg".len() as u64);
}

#[test]
fn image_rounds_download_when_queued_and_otherwise_wait() {
    let stub = Stub::start(image_body);
    let run = tracked("images-rounds");
    card(&run, 10, &stub.base_url);
    let dir = TestDir::new("images-rounds-folder");
    let folder = dir.join("card-images");
    let round = |queued| {
        with_run(
            &run,
            &folder,
            &stub.base_url,
            &PendingCopy::default(),
            |run| image_round(run, queued),
        )
        .expect("ran")
    };
    // Nothing queued: an hourly run without work neither downloads nor clears.
    assert_eq!(round(false), ImageReport::default());
    assert!(stub.requests().is_empty());
    // Queued (a population run, or a 0.x import): the rows are downloaded,
    // with no setting to switch on first.
    assert_eq!(round(true).downloaded, 2);
}

#[test]
fn an_old_local_images_row_changes_nothing_and_a_settings_save_keeps_the_folder() {
    let run = tracked("images-setting");
    run.state
        .save_setting_rows(vec![("local_images".to_string(), "true".to_string())])
        .expect("old row");
    let read =
        crate::commands::read_settings(&run.state).expect("an old row does not break reading");
    assert_eq!(read.card_images_path, None);
    // The Settings page saves every field from what it last read: the
    // folder belongs to its own command.
    let stale = Settings {
        card_images_path: Some("/q/elsewhere".to_string()),
        close_to_tray: false,
        ..Settings::default()
    };
    let applied = crate::commands::apply_settings(&run.state, stale).expect("saved");
    assert_eq!(applied.settings.card_images_path, None);
    let stored = crate::commands::read_settings(&run.state).expect("read");
    assert_eq!(stored.card_images_path, None);
    assert!(!stored.close_to_tray, "the other fields are saved");
}

#[test]
fn the_standard_fetcher_allows_scryfall_and_the_mymtgo_image_host_only() {
    let fetcher = Fetcher::standard();
    assert!(fetcher.allows("https://cards.scryfall.io/large/front/a.jpg"));
    assert!(fetcher.allows("https://cards.mymtgo.com/cards/1.jpg"));
    for url in [
        "https://mymtgo.com/cards/1.jpg",
        "https://cards.mymtgo.com.evil.test/1.jpg",
        "http://cards.mymtgo.com/1.jpg",
        "https://evil.test/https://cards.mymtgo.com/1.jpg",
    ] {
        assert!(!fetcher.allows(url), "{url}");
        assert!(fetcher.fetch(url).is_err(), "{url}");
    }
}

// The origins are a list: a card on any allowed host downloads, here the
// second one (the stub stands in for cards.mymtgo.com).
#[test]
fn a_card_on_the_second_allowed_origin_downloads() {
    let stub = Stub::start(image_body);
    let run = tracked("images-second-origin");
    card(&run, 10, &stub.base_url);
    let dir = TestDir::new("images-second-origin-folder");
    let folder = dir.join("card-images");
    let db = run.state.db_path.clone();
    let reader = move || store::open_reader(&db).ok();
    let fetcher = Fetcher::for_origins(vec![
        "https://cards.scryfall.io".to_string(),
        stub.base_url.clone(),
    ]);
    let recorder = recorder(&run);
    let legacy = PendingCopy::default();
    let report = download_images(&ImageRun {
        folder: &folder,
        is_default: true,
        start_after: 0,
        fetcher: &fetcher,
        reader: &reader,
        recorder: &recorder,
        present: &|files| recorder.present(files),
        legacy: &legacy,
        should_yield: &|| false,
        fetch_pause: std::time::Duration::ZERO,
    })
    .expect("ran");
    assert_eq!((report.downloaded, report.failed), (2, 0));
}

#[test]
fn the_status_shows_the_usage_and_folder() {
    let dir = TestDir::new("images-status");
    let folder = ImagesFolder {
        path: dir.join("card-images"),
        is_default: true,
    };
    planted(&folder.path, &["1.jpg", "notes.txt"]);
    fs::write(folder.path.join("2_art.jpg"), vec![b'q'; 2_047]).expect("file");
    let status = images_status(&folder, (false, None));
    assert_eq!(
        status,
        CardImagesStatus {
            usage: "2 KB".to_string(),
            folder: FolderStatus {
                path: folder.path.to_string_lossy().into_owned(),
                is_default: true,
                missing: false,
                moving: false,
                error: None,
            },
        }
    );
    let moving = images_status(&folder, (true, None));
    assert!(moving.folder.moving);
    let failed = images_status(
        &folder,
        (false, Some("Moving card images failed: q".to_string())),
    );
    assert_eq!(
        failed.folder.error.as_deref(),
        Some("Moving card images failed: q")
    );
    let gone = ImagesFolder {
        path: dir.join("gone"),
        is_default: false,
    };
    let status = images_status(&gone, (false, None));
    assert_eq!(status.usage, "0 B");
    assert!(status.folder.missing, "a chosen folder that is gone");
    let default_not_made = ImagesFolder {
        path: dir.join("not-yet"),
        is_default: true,
    };
    assert!(
        !images_status(&default_not_made, (false, None))
            .folder
            .missing,
        "the default folder is made on demand"
    );
}

#[test]
fn a_download_run_stops_between_files_for_waiting_work_and_goes_on_later() {
    let stub = Stub::start(image_body);
    let run = tracked("images-yield");
    card(&run, 10, &stub.base_url);
    card(&run, 11, &stub.base_url);
    let dir = TestDir::new("images-yield-folder");
    let folder = dir.join("card-images");
    let db = run.state.db_path.clone();
    let reader = move || store::open_reader(&db).ok();
    let fetcher = Fetcher::new(&stub.base_url);
    let recorder = recorder(&run);
    let legacy = PendingCopy::default();
    // Work arrives once the first card is done.
    let asked = std::cell::Cell::new(0);
    let should_yield = || {
        asked.set(asked.get() + 1);
        asked.get() > 1
    };
    let image_run = ImageRun {
        folder: &folder,
        is_default: true,
        start_after: 0,
        fetcher: &fetcher,
        reader: &reader,
        recorder: &recorder,
        present: &|files| recorder.present(files),
        legacy: &legacy,
        should_yield: &should_yield,
        fetch_pause: std::time::Duration::ZERO,
    };
    let cut = download_images(&image_run).expect("ran");
    assert!(cut.interrupted);
    assert_eq!(cut.downloaded, 2, "card 10's two files, then it stopped");
    assert_eq!(targets(&stub), vec!["/10.jpg", "/10a.jpg"]);
    let rest = with_run(&run, &folder, &stub.base_url, &legacy, download_images).expect("ran");
    assert!(!rest.interrupted);
    assert_eq!(rest.downloaded, 2, "card 11 on the next run");
}

// Fix round 2 (S5 I1): every download run starts by forgetting the local
// files that are not in the folder, so a presence check that failed after a
// move is corrected by the next run.
#[test]
fn a_failed_presence_check_is_corrected_by_the_next_run() {
    let stub = Stub::start(image_body);
    let run = tracked("images-self-heal");
    run.write(
        "INSERT INTO cards (catalog_id, name, image, local_image)
         VALUES (7, 'Qcard', 'https://q/7.jpg', '7.jpg')",
    );
    let dir = TestDir::new("images-self-heal-folder");
    let folder = dir.join("card-images");
    let db = run.state.db_path.clone();
    let reader = move || store::open_reader(&db).ok();
    let fetcher = Fetcher::new(&stub.base_url);
    let recorder = recorder(&run);
    let legacy = PendingCopy::default();
    let broken = |_files: std::collections::BTreeSet<String>| Err("tracker busy".to_string());
    let working = |files| recorder.present(files);
    let attempt = |present: &dyn Fn(std::collections::BTreeSet<String>) -> Result<u32, String>| {
        download_images(&ImageRun {
            folder: &folder,
            is_default: true,
            start_after: 0,
            fetcher: &fetcher,
            reader: &reader,
            recorder: &recorder,
            present,
            legacy: &legacy,
            should_yield: &|| false,
            fetch_pause: std::time::Duration::ZERO,
        })
    };
    attempt(&broken).expect("a failed check does not fail the run");
    assert_eq!(local(&run, 7).0, Some("7.jpg".to_string()), "still stale");
    attempt(&working).expect("ran");
    assert_eq!(
        local(&run, 7).0,
        None,
        "the next run forgot the missing file"
    );
}

// Fix round 3: a folder that cannot be listed says nothing about which files
// are there, so no row is forgotten.
fn stale_row(run: &Tracked) {
    run.write(
        "INSERT INTO cards (catalog_id, name, image, local_image)
         VALUES (7, 'Qcard', 'https://q/7.jpg', '7.jpg')",
    );
}

fn run_over(run: &Tracked, folder: &Path, is_default: bool) {
    let stub = Stub::start(image_body);
    let db = run.state.db_path.clone();
    let reader = move || store::open_reader(&db).ok();
    let fetcher = Fetcher::new(&stub.base_url);
    let recorder = recorder(run);
    let legacy = PendingCopy::default();
    download_images(&ImageRun {
        folder,
        is_default,
        start_after: 0,
        fetcher: &fetcher,
        reader: &reader,
        recorder: &recorder,
        present: &|files| recorder.present(files),
        legacy: &legacy,
        should_yield: &|| false,
        fetch_pause: std::time::Duration::ZERO,
    })
    .expect("ran");
}

#[test]
fn a_gone_folder_cannot_be_listed() {
    let dir = TestDir::new("images-list-gone");
    assert!(listed_files(&dir.join("gone")).is_err());
}

#[test]
fn a_gone_custom_folder_keeps_its_rows() {
    let run = tracked("images-gone-custom");
    stale_row(&run);
    let dir = TestDir::new("images-gone-custom-folder");
    run_over(&run, &dir.join("gone"), false);
    assert_eq!(local(&run, 7).0, Some("7.jpg".to_string()));
}

#[cfg(unix)]
#[test]
fn an_unreadable_folder_keeps_its_rows() {
    use std::os::unix::fs::PermissionsExt;
    let run = tracked("images-unreadable");
    stale_row(&run);
    let dir = TestDir::new("images-unreadable-folder");
    let folder = dir.join("card-images");
    fs::create_dir_all(&folder).expect("folder");
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o000)).expect("chmod");
    let readable = fs::read_dir(&folder).is_ok(); // root ignores modes
    if !readable {
        assert!(listed_files(&folder).is_err());
        run_over(&run, &folder, true);
    }
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o755)).expect("restore");
    assert_eq!(local(&run, 7).0, Some("7.jpg".to_string()));
}

// ---- Sweep 8 (#296) -----------------------------------------------------------

/// An `ImageRun` over `folder` with every part given.
struct Parts<'a> {
    folder: &'a Path,
    is_default: bool,
    start_after: u32,
    legacy: &'a PendingCopy,
    should_yield: &'a dyn Fn() -> bool,
    reader: &'a dyn Fn() -> Option<Connection>,
}

fn run_parts(run: &Tracked, origin: &str, parts: Parts) -> Result<ImageReport, String> {
    let fetcher = Fetcher::new(origin);
    let recorder = recorder(run);
    download_images(&ImageRun {
        folder: parts.folder,
        is_default: parts.is_default,
        start_after: parts.start_after,
        fetcher: &fetcher,
        reader: parts.reader,
        recorder: &recorder,
        present: &|files| recorder.present(files),
        legacy: parts.legacy,
        should_yield: parts.should_yield,
        fetch_pause: std::time::Duration::ZERO,
    })
}

// S4-1: a record the tracker cannot take (busy or stopped) is logged and the
// run goes on with the next card.
#[test]
fn a_record_that_cannot_be_stored_does_not_stop_the_run() {
    let stub = Stub::start(image_body);
    let run = tracked("images-record-fails");
    card(&run, 10, &stub.base_url);
    card(&run, 11, &stub.base_url);
    let dir = TestDir::new("images-record-fails-folder");
    let folder = dir.join("card-images");
    let recorder = recorder(&run);
    lock(&run.state.tracker).take().expect("tracker").stop();
    let db = run.state.db_path.clone();
    let reader = move || store::open_reader(&db).ok();
    let fetcher = Fetcher::new(&stub.base_url);
    let legacy = PendingCopy::default();
    let report = download_images(&ImageRun {
        folder: &folder,
        is_default: true,
        start_after: 0,
        fetcher: &fetcher,
        reader: &reader,
        recorder: &recorder,
        present: &|files| recorder.present(files),
        legacy: &legacy,
        should_yield: &|| false,
        fetch_pause: std::time::Duration::ZERO,
    })
    .expect("the run goes on");
    assert_eq!(report.downloaded, 4);
    assert_eq!(
        targets(&stub),
        vec!["/10.jpg", "/10a.jpg", "/11.jpg", "/11a.jpg"]
    );
}

// S4-2: a `.part` link planted in the folder is never written through or
// deleted (the write fails and the next run asks again); a stale `.part`
// file of v1's own is replaced.
#[cfg(unix)]
#[test]
fn a_planted_part_link_is_never_written_through() {
    let dir = TestDir::new("images-part-link");
    let folder = dir.join("card-images");
    fs::create_dir_all(&folder).expect("folder");
    let outside = dir.join("outside.txt");
    fs::write(&outside, b"keep").expect("outside");
    std::os::unix::fs::symlink(&outside, folder.join("10.jpg.part")).expect("link");
    assert!(write_image(&folder, "10.jpg", b"new", true).is_err());
    assert_eq!(fs::read(&outside).expect("outside"), b"keep");
    assert!(
        fs::symlink_metadata(folder.join("10.jpg.part")).is_ok(),
        "not deleted"
    );
    assert!(!folder.join("10.jpg").exists());
    fs::write(folder.join("11.jpg.part"), b"half").expect("stale");
    write_image(&folder, "11.jpg", b"new", true).expect("written");
    assert_eq!(fs::read(folder.join("11.jpg")).expect("image"), b"new");
    assert!(!folder.join("11.jpg.part").exists());
    // A link with the image's own name is not replaced either.
    std::os::unix::fs::symlink(&outside, folder.join("12.jpg")).expect("link");
    assert!(write_image(&folder, "12.jpg", b"new", true).is_err());
    assert_eq!(fs::read(&outside).expect("outside"), b"keep");
    assert!(
        fs::symlink_metadata(folder.join("12.jpg"))
            .expect("link")
            .is_symlink()
    );
}

// S4-3: settings that cannot be read leave the images unmanaged (the scheme
// 404s and pages show the remote images) instead of the default folder.
#[test]
fn unreadable_settings_leave_the_images_unmanaged() {
    let data = Path::new("/q/data");
    assert!(images_state_at_start(Err("q broken".to_string()), data).is_none());
    let chosen = Settings {
        card_images_path: Some("/q/chosen".to_string()),
        ..Settings::default()
    };
    let state = images_state_at_start(Ok::<_, String>(chosen), data).expect("managed");
    assert_eq!(state.folder().path, PathBuf::from("/q/chosen"));
}

// S4-4: links with v1's names are never served or counted.
#[cfg(unix)]
#[test]
fn links_with_owned_names_are_never_served_or_counted() {
    let dir = TestDir::new("images-links-folder");
    let folder = dir.join("card-images");
    planted(&folder, &["12.jpg"]);
    let outside = dir.join("outside.jpg");
    fs::write(&outside, b"outside-bytes").expect("outside");
    std::os::unix::fs::symlink(&outside, folder.join("13.jpg")).expect("file link");
    let elsewhere = dir.join("elsewhere");
    planted(&elsewhere, &["14.jpg"]);
    std::os::unix::fs::symlink(&elsewhere, folder.join("15.jpg")).expect("folder link");
    let served = crate::cards::scheme::respond(Some(&folder), "/13.jpg");
    assert_eq!(served.status(), tauri::http::StatusCode::NOT_FOUND);
    assert_eq!(
        listed_names(&folder).expect("listed"),
        BTreeSet::from(["12.jpg".to_string()])
    );
    assert_eq!(owned_size(&folder), 1);
    assert_eq!(fs::read(&outside).expect("outside"), b"outside-bytes");
    assert!(elsewhere.join("14.jpg").exists());
    assert!(
        fs::symlink_metadata(folder.join("13.jpg")).is_ok(),
        "a link is not v1's file"
    );
}

// S5-4: a chosen folder whose drive goes mid-run is never made again, and the
// run stops fetching.
#[test]
fn a_chosen_folder_gone_mid_run_is_not_recreated() {
    let dir = TestDir::new("images-gone-mid-run");
    let folder = dir.join("drive").join("mymtgo-desktop-card-images");
    fs::create_dir_all(&folder).expect("folder");
    let unplug = dir.join("drive");
    let stub = Stub::start(move |request| {
        let _ = fs::remove_dir_all(&unplug);
        image_body(request)
    });
    let run = tracked("images-gone-mid-run-db");
    card(&run, 10, &stub.base_url);
    card(&run, 11, &stub.base_url);
    let db = run.state.db_path.clone();
    let reader = move || store::open_reader(&db).ok();
    let legacy = PendingCopy::default();
    let report = run_parts(
        &run,
        &stub.base_url,
        Parts {
            folder: &folder,
            is_default: false,
            start_after: 0,
            legacy: &legacy,
            should_yield: &|| false,
            reader: &reader,
        },
    )
    .expect("ran");
    assert!(!folder.exists(), "the gone folder is not made again");
    assert_eq!(report.downloaded, 0);
    assert!(
        !targets(&stub).contains(&"/11.jpg".to_string()),
        "the run stopped: {:?}",
        targets(&stub)
    );
    assert_eq!(local(&run, 10), (None, None));
}

// R1-1: a cut run goes on from the card after the last one it finished, so a
// failing card at the start is not asked again by every resumed run.
#[test]
fn a_cut_run_resumes_after_its_last_card() {
    let stub = Stub::start(|request| {
        if request.target.starts_with("/10") {
            Reply::json(500, "{}")
        } else {
            image_body(request)
        }
    });
    let run = tracked("images-resume");
    for id in [10, 11, 12] {
        card(&run, id, &stub.base_url);
    }
    let dir = TestDir::new("images-resume-folder");
    let folder = dir.join("card-images");
    let db = run.state.db_path.clone();
    let reader = move || store::open_reader(&db).ok();
    let legacy = PendingCopy::default();
    let asked = std::cell::Cell::new(0);
    let third = || {
        asked.set(asked.get() + 1);
        asked.get() > 2
    };
    let cut = run_parts(
        &run,
        &stub.base_url,
        Parts {
            folder: &folder,
            is_default: true,
            start_after: 0,
            legacy: &legacy,
            should_yield: &third,
            reader: &reader,
        },
    )
    .expect("ran");
    assert!(cut.interrupted);
    assert_eq!(cut.resume_after, Some(11));
    let before = stub.requests().len();
    let rest = run_parts(
        &run,
        &stub.base_url,
        Parts {
            folder: &folder,
            is_default: true,
            start_after: 11,
            legacy: &legacy,
            should_yield: &|| false,
            reader: &reader,
        },
    )
    .expect("ran");
    assert!(!rest.interrupted);
    assert_eq!(rest.resume_after, None);
    let mut asked_again: Vec<String> = stub.requests()[before..]
        .iter()
        .map(|request| request.target.clone())
        .collect();
    asked_again.sort();
    assert_eq!(asked_again, vec!["/12.jpg", "/12a.jpg"]);
}

// S3-4: a copy that fails keeps 0.x's folder for the next run.
#[test]
fn a_failed_0x_copy_keeps_its_folder_for_the_next_run() {
    let run = tracked("images-copy-fails");
    let dir = TestDir::new("images-copy-fails-folder");
    let folder = dir.join("card-images");
    let legacy = PendingCopy::default();
    legacy.hand(dir.join("0x-cards"));
    let no_database = || None;
    let failed = run_parts(
        &run,
        "https://q.invalid",
        Parts {
            folder: &folder,
            is_default: true,
            start_after: 0,
            legacy: &legacy,
            should_yield: &|| false,
            reader: &no_database,
        },
    );
    assert!(failed.is_err());
    assert_eq!(legacy.take(), Some(dir.join("0x-cards")));
}
