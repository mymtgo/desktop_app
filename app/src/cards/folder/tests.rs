//! 0.x `CardImagesFolderTest`, with Laravel's disk read as v1's folder
//! resolution (`ImagesState`), and v1's own rules (its subfolder, its own
//! files only).

use std::collections::BTreeSet;
use std::fs;

use cards::images::{
    ALREADY_MOVING, NOT_ABSOLUTE, NOT_WRITABLE, OVERLAPS, V1_SUBFOLDER, image_file,
};
use fixture_tools::TestDir;

use super::*;
use crate::api::stub::{Reply, Stub};
use crate::cards::images::{
    DEFAULT_FOLDER, Fetcher, ImageRun, ImagesFolder, download_images, images_folder,
};
use crate::cards::legacy_images::PendingCopy;
use crate::cards::populate::tests::{Tracked, tracked};
use crate::cards::scheme::respond;
use crate::commands::read_settings;
use crate::settings::Settings;

/// A data folder and the images state over it, with `custom` as the chosen
/// folder when given.
struct Sandbox {
    dir: TestDir,
    images: ImagesState,
}

impl Sandbox {
    fn new(label: &str, custom: Option<&str>) -> Sandbox {
        let dir = TestDir::new(label);
        let settings = Settings {
            card_images_path: custom.map(|path| dir.join(path).to_string_lossy().into_owned()),
            ..Settings::default()
        };
        let images = ImagesState::new(&settings, &dir.join("data"));
        Sandbox { dir, images }
    }

    fn default(&self) -> PathBuf {
        self.dir.join("data").join(DEFAULT_FOLDER)
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.dir.join(relative)
    }
}

fn put(folder: &Path, name: &str, bytes: &str) {
    fs::create_dir_all(folder).expect("folder");
    fs::write(folder.join(name), bytes).expect("file");
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).expect("read")
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The app's switch over a running tracker.
fn switch<'a>(
    run: &'a Tracked,
    images: &'a ImagesState,
) -> impl Fn(&Path) -> Result<(), String> + 'a {
    move |to| switch_folder(&run.state, images, to)
}

/// The app's "forget files not in the new folder" over a running tracker.
fn present(run: &Tracked) -> impl Fn(BTreeSet<String>) -> Result<u32, String> + '_ {
    move |files| {
        run.state
            .tracker_part(tracker::TrackerHandle::card_images)
            .expect("tracker")
            .present(files)
    }
}

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

fn stored_path(run: &Tracked) -> Option<String> {
    read_settings(&run.state)
        .expect("settings")
        .card_images_path
}

// "points the cards disk at the configured folder"
#[test]
fn points_the_images_folder_at_the_configured_folder() {
    let sandbox = Sandbox::new(
        "folder-configured",
        Some("custom/mymtgo-desktop-card-images"),
    );
    let folder = sandbox.images.folder();
    assert_eq!(
        folder.path,
        sandbox.path("custom/mymtgo-desktop-card-images")
    );
    assert!(!folder.is_default);
    assert_ne!(folder.path, sandbox.default());
}

// "falls back to the default folder when none is configured"
#[test]
fn falls_back_to_the_default_folder_when_none_is_configured() {
    let sandbox = Sandbox::new("folder-default", None);
    assert_eq!(
        sandbox.images.folder(),
        ImagesFolder {
            path: sandbox.default(),
            is_default: true
        }
    );
    assert_eq!(
        images_folder(&Settings::default(), &sandbox.dir.join("data")),
        sandbox.default()
    );
}

// "leaves an already resolved disk alone when the folder has not changed":
// the folder changes only when a move switches it.
#[test]
fn the_folder_stays_until_a_move_switches_it() {
    let sandbox = Sandbox::new("folder-stays", None);
    let before = sandbox.images.folder();
    assert_eq!(sandbox.images.folder(), before);
    assert_eq!(sandbox.images.move_state(), (false, None));
    let custom = sandbox.path("custom");
    sandbox.images.switch_to(&custom);
    assert_eq!(
        sandbox.images.folder(),
        ImagesFolder {
            path: custom,
            is_default: false
        }
    );
    sandbox.images.switch_to(&sandbox.default());
    assert!(sandbox.images.folder().is_default);
}

// "serves images from a custom folder at the same url"
#[test]
fn serves_images_from_a_custom_folder_at_the_same_url() {
    let sandbox = Sandbox::new("folder-serves", Some("custom/mymtgo-desktop-card-images"));
    let folder = sandbox.images.folder().path;
    put(&folder, "42.jpg", "custom-bytes");
    let response = respond(Some(&folder), "/42.jpg");
    assert_eq!(response.status(), tauri::http::StatusCode::OK);
    assert_eq!(response.body(), b"custom-bytes");
}

// "re-applies the folder before each queued job": each download resolves
// the folder at its start.
#[test]
fn each_download_resolves_the_folder_at_its_start() {
    let stub = Stub::start(|_| Reply::Respond {
        status: 200,
        headers: Vec::new(),
        body: b"img".to_vec(),
    });
    let run = tracked("folder-each-run");
    let sandbox = Sandbox::new("folder-each-run-dirs", None);
    let custom = sandbox.path("custom");
    fs::create_dir_all(&custom).expect("custom");
    // Another command switches the folder after the app started.
    sandbox.images.switch_to(&custom);
    run.write(&format!(
        "INSERT INTO cards (catalog_id, name, image) VALUES (7, 'Qcard', '{}/7.jpg')",
        stub.base_url
    ));
    let folder = sandbox.images.folder();
    with_run(&run, &folder, &stub.base_url, download_images).expect("ran");
    assert_eq!(read(&custom.join("7.jpg")), "img");
    assert!(!sandbox.default().join("7.jpg").exists());
}

fn with_run<T>(
    run: &Tracked,
    folder: &ImagesFolder,
    origin: &str,
    body: impl FnOnce(&ImageRun) -> T,
) -> T {
    let db = run.state.db_path.clone();
    let reader = move || store::open_reader(&db).ok();
    let fetcher = Fetcher::new(origin);
    let recorder = crate::bridge::lock(&run.state.tracker)
        .as_ref()
        .expect("tracker")
        .card_images();
    let legacy = PendingCopy::default();
    body(&ImageRun {
        folder: &folder.path,
        is_default: folder.is_default,
        start_after: 0,
        fetcher: &fetcher,
        reader: &reader,
        recorder: &recorder,
        present: &|files| recorder.present(files),
        legacy: &legacy,
        should_yield: &|| false,
        fetch_pause: std::time::Duration::ZERO,
    })
}

// "queues a move into a dedicated subfolder of the chosen folder"
#[test]
fn a_chosen_folder_gets_v1s_own_subfolder() {
    let sandbox = Sandbox::new("folder-subfolder", None);
    let chosen = sandbox.path("chosen");
    fs::create_dir_all(&chosen).expect("chosen");
    let target = chosen_target(&format!("{}/", text(&chosen)));
    assert_eq!(target, chosen.join(V1_SUBFOLDER));
    assert_eq!(
        chosen_target("D:\\Q\\Images\\"),
        PathBuf::from("D:\\Q\\Images").join(V1_SUBFOLDER)
    );
    let request = queue_move(&sandbox.images, &target).expect("queued");
    assert_eq!(
        request,
        MoveRequest {
            from: sandbox.default(),
            to: chosen.join(V1_SUBFOLDER),
            made: vec![chosen.join(V1_SUBFOLDER)],
        }
    );
    assert_eq!(sandbox.images.move_state(), (true, None));
    assert!(
        sandbox.images.folder().is_default,
        "the setting is not switched yet"
    );
}

// "creates a chosen folder that does not exist yet"
#[test]
fn creates_a_chosen_folder_that_does_not_exist_yet() {
    let sandbox = Sandbox::new("folder-creates", None);
    let target = chosen_target(&text(&sandbox.path("new/nested")));
    queue_move(&sandbox.images, &target).expect("queued");
    assert!(target.is_dir());
}

// "rejects a folder that is the current folder or inside it"
#[test]
fn rejects_a_folder_that_is_the_current_folder_or_inside_it() {
    let current = format!("custom/{V1_SUBFOLDER}");
    for relative in [
        current.clone(),
        format!("{current}/deeper"),
        "custom".to_string(),
    ] {
        let sandbox = Sandbox::new("folder-overlap", Some(&current));
        fs::create_dir_all(sandbox.path(&current)).expect("current");
        assert_eq!(
            queue_move(&sandbox.images, &sandbox.path(&relative)),
            Err(OVERLAPS.to_string()),
            "{relative}"
        );
        assert_eq!(sandbox.images.move_state(), (false, None), "nothing queued");
    }
}

// "rejects a relative folder"
#[test]
fn rejects_a_relative_folder() {
    let sandbox = Sandbox::new("folder-relative", None);
    assert_eq!(
        queue_move(&sandbox.images, Path::new("relative/folder")),
        Err(NOT_ABSOLUTE.to_string())
    );
    assert!(!Path::new("relative").exists());
    assert_eq!(sandbox.images.move_state(), (false, None));
}

// "rejects a folder that cannot be written to" (0.x skips it on Windows and
// as root, where chmod does not restrict).
#[cfg(unix)]
#[test]
fn rejects_a_folder_that_cannot_be_written_to() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = Sandbox::new("folder-readonly", None);
    let chosen = sandbox.path("readonly");
    fs::create_dir_all(&chosen).expect("chosen");
    fs::set_permissions(&chosen, fs::Permissions::from_mode(0o555)).expect("chmod");
    let restricted = fs::write(chosen.join("probe"), b"q").is_err();
    let result = queue_move(&sandbox.images, &chosen);
    fs::set_permissions(&chosen, fs::Permissions::from_mode(0o755)).expect("chmod back");
    if !restricted {
        return; // root: chmod does not restrict this user
    }
    assert_eq!(result, Err(NOT_WRITABLE.to_string()));
    assert_eq!(sandbox.images.move_state(), (false, None));
}

// "refuses to start a second move while one is running"
#[test]
fn refuses_to_start_a_second_move_while_one_is_running() {
    let sandbox = Sandbox::new("folder-second", None);
    let first = chosen_target(&text(&sandbox.path("one")));
    queue_move(&sandbox.images, &first).expect("first");
    let second = chosen_target(&text(&sandbox.path("two")));
    assert_eq!(
        queue_move(&sandbox.images, &second),
        Err(ALREADY_MOVING.to_string())
    );
    assert!(!second.exists());
}

// "queues a move back to the default folder on reset"
#[test]
fn queues_a_move_back_to_the_default_folder_on_reset() {
    let current = format!("custom/{V1_SUBFOLDER}");
    let sandbox = Sandbox::new("folder-reset", Some(&current));
    fs::create_dir_all(sandbox.path(&current)).expect("current");
    let request = queue_move(&sandbox.images, &sandbox.images.default_folder()).expect("queued");
    assert_eq!(
        request,
        MoveRequest {
            from: sandbox.path(&current),
            to: sandbox.default(),
            made: vec![sandbox.default(), sandbox.path("data")],
        }
    );
}

// "moves images, then switches the folder, then removes the originals"
#[test]
fn moves_images_then_switches_the_folder_then_removes_the_originals() {
    let run = tracked("folder-move");
    let sandbox = Sandbox::new("folder-move-dirs", None);
    put(&sandbox.default(), "1.jpg", "one");
    put(&sandbox.default(), "1_art.jpg", "art");
    let to = sandbox.path("chosen").join(V1_SUBFOLDER);
    let request = queue_move(&sandbox.images, &to).expect("queued");
    run_move(
        &sandbox.images,
        &request,
        &switch(&run, &sandbox.images),
        &present(&run),
    );
    assert_eq!(read(&to.join("1.jpg")), "one");
    assert_eq!(read(&to.join("1_art.jpg")), "art");
    assert!(!sandbox.default().join("1.jpg").exists());
    assert_eq!(stored_path(&run), Some(text(&to)));
    assert_eq!(sandbox.images.folder().path, to);
    let served = respond(Some(&sandbox.images.folder().path), "/1.jpg");
    assert_eq!(served.body(), b"one");
    assert_eq!(sandbox.images.move_state(), (false, None));
    assert!(sandbox.default().is_dir(), "the default folder stays");
}

// "stores null when moving back to the default folder"
#[test]
fn stores_null_when_moving_back_to_the_default_folder() {
    let run = tracked("folder-back");
    let current = format!("custom/{V1_SUBFOLDER}");
    let sandbox = Sandbox::new("folder-back-dirs", Some(&current));
    let custom = sandbox.path(&current);
    put(&custom, "1.jpg", "one");
    switch_folder(&run.state, &sandbox.images, &custom).expect("custom stored");
    assert_eq!(stored_path(&run), Some(text(&custom)));
    let request = queue_move(&sandbox.images, &sandbox.default()).expect("queued");
    run_move(
        &sandbox.images,
        &request,
        &switch(&run, &sandbox.images),
        &present(&run),
    );
    assert_eq!(stored_path(&run), None);
    assert_eq!(read(&sandbox.default().join("1.jpg")), "one");
    assert!(!custom.is_dir(), "the empty custom folder is removed");
    assert!(sandbox.images.folder().is_default);
}

// "is safe to run twice"
#[test]
fn is_safe_to_run_twice() {
    let run = tracked("folder-twice");
    let sandbox = Sandbox::new("folder-twice-dirs", None);
    put(&sandbox.default(), "1.jpg", "one");
    let to = sandbox.path("chosen").join(V1_SUBFOLDER);
    let request = MoveRequest {
        from: sandbox.default(),
        to: to.clone(),
        made: Vec::new(),
    };
    assert_eq!(
        move_images(&request, &sandbox.default(), &switch(&run, &sandbox.images)),
        Ok(())
    );
    assert_eq!(
        move_images(&request, &sandbox.default(), &switch(&run, &sandbox.images)),
        Ok(())
    );
    assert_eq!(read(&to.join("1.jpg")), "one");
    assert_eq!(stored_path(&run), Some(text(&to)));
}

// "keeps the old folder and removes partial copies when a copy fails"
#[cfg(unix)]
#[test]
fn keeps_the_old_folder_and_removes_partial_copies_when_a_copy_fails() {
    use std::os::unix::fs::PermissionsExt;
    let run = tracked("folder-fails");
    let sandbox = Sandbox::new("folder-fails-dirs", None);
    put(&sandbox.default(), "1.jpg", "one");
    put(&sandbox.default(), "2.jpg", "two");
    let locked = sandbox.default().join("2.jpg");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("chmod");
    if fs::read(&locked).is_ok() {
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).expect("chmod back");
        return; // root: chmod does not restrict this user
    }
    let to = sandbox.path("chosen").join(V1_SUBFOLDER);
    let request = queue_move(&sandbox.images, &to).expect("queued");
    run_move(
        &sandbox.images,
        &request,
        &switch(&run, &sandbox.images),
        &present(&run),
    );
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).expect("chmod back");
    assert_eq!(stored_path(&run), None);
    assert!(sandbox.default().join("1.jpg").exists());
    assert!(sandbox.default().join("2.jpg").exists());
    assert!(
        !to.exists(),
        "partial copies and the subfolder made removed"
    );
    let (moving, error) = sandbox.images.move_state();
    assert!(!moving);
    assert!(
        error
            .as_deref()
            .is_some_and(|error| error.starts_with("Moving card images failed: ")),
        "{error:?}"
    );
    assert!(sandbox.images.folder().is_default);
}

// "falls back to the default folder instead of failing when the custom
// folder is gone" (deviation): a missing custom folder pauses downloads and
// the scheme 404s; nothing is written to the default folder.
#[test]
fn a_missing_custom_folder_pauses_downloads_and_the_scheme_404s() {
    let sandbox = Sandbox::new("folder-gone", Some("gone/drive/mymtgo-desktop-card-images"));
    let folder = sandbox.images.folder();
    assert!(!folder.is_default);
    assert!(!folder.path.exists());
    assert_eq!(
        respond(Some(&folder.path), "/1.jpg").status(),
        tauri::http::StatusCode::NOT_FOUND
    );
    assert_eq!(
        folder.path,
        sandbox.path("gone/drive/mymtgo-desktop-card-images")
    );
}

// "skips image downloads while the custom folder is gone"
#[test]
fn skips_image_downloads_while_the_custom_folder_is_gone() {
    let stub = Stub::start(|_| Reply::Respond {
        status: 200,
        headers: Vec::new(),
        body: b"bytes".to_vec(),
    });
    let run = tracked("folder-gone-downloads");
    let sandbox = Sandbox::new(
        "folder-gone-downloads-dirs",
        Some("gone/mymtgo-desktop-card-images"),
    );
    run.write(&format!(
        "INSERT INTO cards (catalog_id, name, image, local_image)
         VALUES (123, 'Qcard', '{}/123.jpg', '123.jpg')",
        stub.base_url
    ));
    let folder = sandbox.images.folder();
    let report = with_run(&run, &folder, &stub.base_url, download_images).expect("ran");
    assert_eq!(report.downloaded, 0);
    assert!(stub.requests().is_empty());
    assert!(!folder.path.exists(), "the gone folder is not recreated");
    assert!(!sandbox.default().join(image_file(123)).exists());
    let local: Option<String> = store::open_reader(&run.state.db_path)
        .expect("reader")
        .query_row(
            "SELECT local_image FROM cards WHERE catalog_id = 123",
            [],
            |row| row.get(0),
        )
        .expect("row");
    // S5-5: a gone folder says nothing about its files, so the row keeps
    // its file until the folder is back.
    assert_eq!(local.as_deref(), Some("123.jpg"));
}

#[test]
fn a_move_never_touches_foreign_files() {
    let run = tracked("folder-foreign");
    let current = format!("custom/{V1_SUBFOLDER}");
    let sandbox = Sandbox::new("folder-foreign-dirs", Some(&current));
    let custom = sandbox.path(&current);
    put(&custom, "1.jpg", "one");
    put(&custom, "notes.txt", "keep");
    put(&custom, "2.jpg.part", "keep");
    put(&custom.join("sub"), "3.jpg", "keep");
    let request = queue_move(&sandbox.images, &sandbox.default()).expect("queued");
    run_move(
        &sandbox.images,
        &request,
        &switch(&run, &sandbox.images),
        &present(&run),
    );
    assert_eq!(read(&sandbox.default().join("1.jpg")), "one");
    for name in ["notes.txt", "2.jpg.part", "sub"] {
        assert!(
            !sandbox.default().join(name).exists(),
            "{name} is not copied"
        );
    }
    assert!(!custom.join("1.jpg").exists());
    assert_eq!(read(&custom.join("notes.txt")), "keep");
    assert_eq!(read(&custom.join("2.jpg.part")), "keep");
    assert_eq!(read(&custom.join("sub").join("3.jpg")), "keep");
    assert!(custom.is_dir(), "a folder with other files is kept");
}

/// The same folder under another spelling: refused, and a move that got
/// past validation still deletes nothing (the second guard).
fn same_folder_by_another_name(label: &str, alias: impl Fn(&Sandbox) -> PathBuf) {
    let run = tracked(label);
    let current = format!("x/{V1_SUBFOLDER}");
    let sandbox = Sandbox::new(&format!("{label}-dirs"), Some(&current));
    let custom = sandbox.path(&current);
    put(&custom, "1.jpg", "one");
    put(&custom, "1_art.jpg", "art");
    let to = chosen_target(&text(&alias(&sandbox)));
    assert_eq!(queue_move(&sandbox.images, &to), Err(OVERLAPS.to_string()));
    assert_eq!(sandbox.images.move_state(), (false, None));
    let request = MoveRequest {
        from: custom.clone(),
        to,
        made: Vec::new(),
    };
    let _ = move_images(&request, &sandbox.default(), &switch(&run, &sandbox.images));
    assert_eq!(read(&custom.join("1.jpg")), "one", "nothing deleted");
    assert_eq!(read(&custom.join("1_art.jpg")), "art");
    assert!(custom.is_dir());
}

#[test]
fn a_move_to_the_same_folder_spelt_with_a_dot_deletes_nothing() {
    same_folder_by_another_name("folder-same-dot", |sandbox| sandbox.path("x/."));
}

#[cfg(unix)]
#[test]
fn a_move_to_the_same_folder_through_a_link_deletes_nothing() {
    same_folder_by_another_name("folder-same-link", |sandbox| {
        let link = sandbox.path("link");
        std::os::unix::fs::symlink(sandbox.path("x"), &link).expect("symlink");
        link
    });
}

#[test]
fn a_move_from_a_gone_folder_queues_its_files_again() {
    let run = tracked("folder-gone-move");
    let current = format!("gone/{V1_SUBFOLDER}");
    let sandbox = Sandbox::new("folder-gone-move-dirs", Some(&current));
    run.write(
        "INSERT INTO cards (catalog_id, name, image, local_image, local_art_crop)
         VALUES (1, 'Qcard', 'https://q/1.jpg', '1.jpg', '1_art.jpg')",
    );
    let request = queue_move(&sandbox.images, &sandbox.default()).expect("queued");
    run_move(
        &sandbox.images,
        &request,
        &switch(&run, &sandbox.images),
        &present(&run),
    );
    assert_eq!(sandbox.images.move_state(), (false, None));
    assert_eq!(local(&run, 1), (None, None), "downloaded again");
}

#[test]
fn a_move_missing_some_files_keeps_only_the_rows_it_carried() {
    let run = tracked("folder-partial-move");
    let sandbox = Sandbox::new("folder-partial-move-dirs", None);
    put(&sandbox.default(), "1.jpg", "one");
    run.write(
        "INSERT INTO cards (catalog_id, name, image, local_image, local_art_crop)
         VALUES (1, 'Qcard', 'https://q/1.jpg', '1.jpg', '1_art.jpg'),
                (2, 'Qcard', 'https://q/2.jpg', '2.jpg', NULL)",
    );
    let to = sandbox.path("chosen").join(V1_SUBFOLDER);
    let request = queue_move(&sandbox.images, &to).expect("queued");
    run_move(
        &sandbox.images,
        &request,
        &switch(&run, &sandbox.images),
        &present(&run),
    );
    assert_eq!(local(&run, 1), (Some("1.jpg".to_string()), None));
    assert_eq!(local(&run, 2), (None, None));
}

// ---- Fix round 2: file identity, not path text --------------------------------

#[test]
fn a_probe_seen_in_the_target_means_one_folder_under_two_names() {
    let sandbox = Sandbox::new("folder-probe", None);
    let folder = sandbox.path("x");
    fs::create_dir_all(&folder).expect("folder");
    let alias = folder.join(".");
    assert_eq!(probe_folders(&folder, &alias), Probe::Alias);
    let other = sandbox.path("y");
    fs::create_dir_all(&other).expect("other");
    assert_eq!(probe_folders(&folder, &other), Probe::Separate);
    for dir in [&folder, &other] {
        assert_eq!(fs::read_dir(dir).expect("dir").count(), 0, "no probe left");
    }
}

#[test]
fn a_gone_source_has_nothing_to_probe() {
    let sandbox = Sandbox::new("folder-probe-gone", None);
    assert_eq!(
        probe_folders(&sandbox.path("gone"), &sandbox.path("y")),
        Probe::Separate
    );
}

#[test]
fn a_move_into_an_alias_of_the_source_is_refused_and_deletes_nothing() {
    let sandbox = Sandbox::new("folder-alias-move", None);
    let folder = sandbox.path("x");
    put(&folder, "12.jpg", "12-bytes");
    let request = MoveRequest {
        from: folder.clone(),
        to: folder.join("."),
        made: Vec::new(),
    };
    let result = move_images(&request, &sandbox.default(), &|_| Ok(()));
    assert_eq!(result, Err(OVERLAPS.to_string()));
    assert_eq!(read(&folder.join("12.jpg")), "12-bytes");
}

#[cfg(unix)]
#[test]
fn a_source_that_is_the_same_file_as_its_copy_is_never_deleted() {
    let sandbox = Sandbox::new("folder-hard-link", None);
    let from = sandbox.path("from");
    let to = sandbox.path("to");
    put(&from, "12.jpg", "12-bytes");
    fs::create_dir_all(&to).expect("to");
    fs::hard_link(from.join("12.jpg"), to.join("12.jpg")).expect("hard link");
    assert!(!safe_to_delete(&from.join("12.jpg"), &to));
    // A separate copy of the same size is still fine to delete.
    put(&from, "13.jpg", "13-bytes");
    put(&to, "13.jpg", "13-bytes");
    assert!(safe_to_delete(&from.join("13.jpg"), &to));
}

#[test]
fn a_drive_or_filesystem_root_keeps_its_separator() {
    assert_eq!(trimmed_choice("E:\\"), "E:\\");
    assert_eq!(trimmed_choice("E:/"), "E:/");
    assert_eq!(trimmed_choice("/"), "/");
    assert_eq!(trimmed_choice("D:\\Q\\Images\\"), "D:\\Q\\Images");
    assert_eq!(trimmed_choice("/q/images//"), "/q/images");
    assert_eq!(trimmed_choice("/q/images"), "/q/images");
}

#[cfg(unix)]
#[test]
fn a_filesystem_root_target_is_absolute() {
    let target = chosen_target("/");
    assert_eq!(target, PathBuf::from("/").join(V1_SUBFOLDER));
    assert!(is_absolute(&target.to_string_lossy()));
}

#[cfg(windows)]
#[test]
fn a_drive_root_target_is_absolute() {
    let target = chosen_target("E:\\");
    assert_eq!(target, PathBuf::from("E:\\mymtgo-desktop-card-images"));
    assert!(is_absolute(&target.to_string_lossy()));
}

// ---- Sweep 8 (#296) -----------------------------------------------------------

// S5-1: a move that fails leaves no empty v1 subfolder behind.
#[cfg(unix)]
#[test]
fn a_failed_move_leaves_no_empty_subfolder() {
    use std::os::unix::fs::PermissionsExt;
    let run = tracked("folder-failed-tidy");
    let sandbox = Sandbox::new("folder-failed-tidy-dirs", None);
    put(&sandbox.default(), "1.jpg", "one");
    let locked = sandbox.default().join("1.jpg");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("chmod");
    if fs::read(&locked).is_ok() {
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).expect("chmod back");
        return; // root: chmod does not restrict this user
    }
    fs::create_dir_all(sandbox.path("chosen")).expect("chosen");
    let to = sandbox.path("chosen").join(V1_SUBFOLDER);
    let request = queue_move(&sandbox.images, &to).expect("queued");
    assert!(to.is_dir(), "made by the queue");
    run_move(
        &sandbox.images,
        &request,
        &switch(&run, &sandbox.images),
        &present(&run),
    );
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).expect("chmod back");
    assert!(sandbox.images.move_state().1.is_some(), "the move failed");
    assert!(!to.exists(), "the subfolder the move made is removed");
    assert!(sandbox.path("chosen").is_dir(), "the chosen folder stays");
    assert_eq!(read(&locked), "one");
}

// S5-1: an existing subfolder is never removed by a failed move.
#[cfg(unix)]
#[test]
fn a_failed_move_keeps_a_subfolder_that_was_there() {
    use std::os::unix::fs::PermissionsExt;
    let run = tracked("folder-failed-keeps");
    let sandbox = Sandbox::new("folder-failed-keeps-dirs", None);
    put(&sandbox.default(), "1.jpg", "one");
    let locked = sandbox.default().join("1.jpg");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("chmod");
    if fs::read(&locked).is_ok() {
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).expect("chmod back");
        return;
    }
    let to = sandbox.path("chosen").join(V1_SUBFOLDER);
    fs::create_dir_all(&to).expect("already there");
    let request = queue_move(&sandbox.images, &to).expect("queued");
    run_move(
        &sandbox.images,
        &request,
        &switch(&run, &sandbox.images),
        &present(&run),
    );
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).expect("chmod back");
    assert!(to.is_dir(), "it was there before the move");
}

// R1-3: `.` and `..` are refused, so a `..` under a missing parent cannot
// slip past the overlap check unresolved.
#[test]
fn a_target_with_dot_components_is_refused() {
    let current = format!("x/{V1_SUBFOLDER}");
    let sandbox = Sandbox::new("folder-dot-dot", Some(&current));
    fs::create_dir_all(sandbox.path(&current)).expect("current");
    let sneaky = sandbox
        .path("missing")
        .join("..")
        .join(&current)
        .join("deeper");
    assert_eq!(
        queue_move(&sandbox.images, &sneaky),
        Err(NOT_ABSOLUTE.to_string())
    );
    assert!(!sandbox.path("missing").exists(), "nothing made");
    assert!(!sandbox.path(&current).join("deeper").exists());
    assert_eq!(sandbox.images.move_state(), (false, None));
}

// R1-4: a source that is a link to a file elsewhere is neither copied nor
// deleted, and its target is never touched.
#[cfg(unix)]
#[test]
fn a_linked_source_file_is_neither_copied_nor_deleted() {
    let run = tracked("folder-linked-source");
    let sandbox = Sandbox::new("folder-linked-source-dirs", None);
    put(&sandbox.default(), "1.jpg", "one");
    let outside = sandbox.path("outside.jpg");
    fs::write(&outside, "outside").expect("outside");
    std::os::unix::fs::symlink(&outside, sandbox.default().join("2.jpg")).expect("link");
    let to = sandbox.path("chosen").join(V1_SUBFOLDER);
    let request = queue_move(&sandbox.images, &to).expect("queued");
    run_move(
        &sandbox.images,
        &request,
        &switch(&run, &sandbox.images),
        &present(&run),
    );
    assert_eq!(sandbox.images.move_state(), (false, None));
    assert_eq!(read(&to.join("1.jpg")), "one");
    assert!(!to.join("2.jpg").exists(), "a link is not v1's file");
    assert!(fs::symlink_metadata(sandbox.default().join("2.jpg")).is_ok());
    assert_eq!(read(&outside), "outside");
}

// R1-4: a link with an owned name in the target is never written through;
// the move fails and deletes nothing.
#[cfg(unix)]
#[test]
fn a_linked_target_file_is_never_written_through() {
    let run = tracked("folder-linked-target");
    let sandbox = Sandbox::new("folder-linked-target-dirs", None);
    put(&sandbox.default(), "1.jpg", "one");
    let to = sandbox.path("chosen").join(V1_SUBFOLDER);
    fs::create_dir_all(&to).expect("to");
    let outside = sandbox.path("outside.jpg");
    fs::write(&outside, "outside-bytes").expect("outside");
    std::os::unix::fs::symlink(&outside, to.join("1.jpg")).expect("link");
    let request = queue_move(&sandbox.images, &to).expect("queued");
    run_move(
        &sandbox.images,
        &request,
        &switch(&run, &sandbox.images),
        &present(&run),
    );
    assert_eq!(read(&outside), "outside-bytes", "never written through");
    assert_eq!(read(&sandbox.default().join("1.jpg")), "one", "kept");
    assert!(sandbox.images.move_state().1.is_some(), "the move failed");
    assert!(sandbox.images.folder().is_default);
}

// R1-4: a whole move whose target already holds a hard link of the source
// keeps the source (one file, two names).
#[cfg(unix)]
#[test]
fn a_move_onto_a_hard_link_keeps_the_source() {
    let run = tracked("folder-hard-link-move");
    let sandbox = Sandbox::new("folder-hard-link-move-dirs", None);
    put(&sandbox.default(), "1.jpg", "one");
    let to = sandbox.path("chosen").join(V1_SUBFOLDER);
    fs::create_dir_all(&to).expect("to");
    fs::hard_link(sandbox.default().join("1.jpg"), to.join("1.jpg")).expect("hard link");
    let request = queue_move(&sandbox.images, &to).expect("queued");
    run_move(
        &sandbox.images,
        &request,
        &switch(&run, &sandbox.images),
        &present(&run),
    );
    assert_eq!(sandbox.images.move_state(), (false, None));
    assert_eq!(read(&sandbox.default().join("1.jpg")), "one");
    assert_eq!(read(&to.join("1.jpg")), "one");
}

// R3-2: a source folder that cannot be read moves nothing and fails, so the
// setting is not switched and no row is forgotten.
#[cfg(unix)]
#[test]
fn an_unreadable_source_folder_fails_the_move() {
    use std::os::unix::fs::PermissionsExt;
    let run = tracked("folder-unreadable-source");
    let sandbox = Sandbox::new("folder-unreadable-source-dirs", None);
    put(&sandbox.default(), "1.jpg", "one");
    run.write(
        "INSERT INTO cards (catalog_id, name, image, local_image)
         VALUES (1, 'Qcard', 'https://q/1.jpg', '1.jpg')",
    );
    let to = sandbox.path("chosen").join(V1_SUBFOLDER);
    let request = queue_move(&sandbox.images, &to).expect("queued");
    fs::set_permissions(sandbox.default(), fs::Permissions::from_mode(0o300)).expect("chmod");
    let listable = fs::read_dir(sandbox.default()).is_ok();
    if !listable {
        run_move(
            &sandbox.images,
            &request,
            &switch(&run, &sandbox.images),
            &present(&run),
        );
    }
    fs::set_permissions(sandbox.default(), fs::Permissions::from_mode(0o755)).expect("chmod back");
    if listable {
        return; // root: chmod does not restrict this user
    }
    let (_, error) = sandbox.images.move_state();
    assert!(
        error
            .as_deref()
            .is_some_and(|error| error.starts_with("Moving card images failed: ")),
        "{error:?}"
    );
    assert_eq!(stored_path(&run), None, "not switched");
    assert_eq!(
        local(&run, 1).0,
        Some("1.jpg".to_string()),
        "nothing forgotten"
    );
    assert_eq!(read(&sandbox.default().join("1.jpg")), "one");
}

// ---- Fixes B3: safe writes and probes (#314 row 7, #296 R2-1, R2-2) ----------

/// Make `folder` read only; false when this user can still write in it
/// (root), so the test has nothing to check.
#[cfg(unix)]
fn read_only(folder: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(folder, fs::Permissions::from_mode(0o555)).expect("chmod");
    let probe = folder.join(".mymtgo-root-check");
    if fs::write(&probe, b"").is_ok() {
        let _ = fs::remove_file(&probe);
        writable_again(folder);
        return false;
    }
    true
}

#[cfg(unix)]
fn writable_again(folder: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(folder, fs::Permissions::from_mode(0o755)).expect("chmod back");
}

// #314 row 7: a target of another size is replaced through a temporary and
// a rename, so a hard link of it elsewhere is never truncated in place.
#[cfg(unix)]
#[test]
fn a_replaced_target_never_writes_into_a_hard_link_elsewhere() {
    let sandbox = Sandbox::new("folder-replace-hard-link", None);
    let from = sandbox.path("from");
    let to = sandbox.path("to");
    put(&from, "1.jpg", "one");
    fs::create_dir_all(&to).expect("to");
    let outside = sandbox.path("outside.jpg");
    fs::write(&outside, "outside-bytes").expect("outside");
    fs::hard_link(&outside, to.join("1.jpg")).expect("hard link");
    let mut created = Vec::new();
    let sources = copy_all(&from, &to, &mut created).expect("copied");
    assert_eq!(sources, vec![from.join("1.jpg")]);
    assert_eq!(read(&to.join("1.jpg")), "one");
    assert_eq!(read(&outside), "outside-bytes", "never truncated in place");
    assert!(!to.join("1.jpg.part").exists(), "no temporary left");
}

// #296 R2-2: a source folder that cannot be probed is copied, but nothing in
// it is deleted.
#[cfg(unix)]
#[test]
fn a_source_that_cannot_be_written_cannot_be_probed() {
    let sandbox = Sandbox::new("folder-probe-read-only-source", None);
    let from = sandbox.path("from");
    let to = sandbox.path("to");
    put(&from, "1.jpg", "one");
    fs::create_dir_all(&to).expect("to");
    if !read_only(&from) {
        return;
    }
    let probe = probe_folders(&from, &to);
    writable_again(&from);
    assert_eq!(probe, Probe::Unknown);
}

#[cfg(unix)]
#[test]
fn a_source_that_cannot_be_probed_is_copied_and_never_deleted() {
    let sandbox = Sandbox::new("folder-copy-only", None);
    let from = sandbox.path("from");
    let to = sandbox.path("to");
    put(&from, "1.jpg", "one");
    let request = MoveRequest {
        from: from.clone(),
        to: to.clone(),
        made: Vec::new(),
    };
    if !read_only(&from) {
        return;
    }
    let switched = std::cell::Cell::new(false);
    let result = move_images(&request, &sandbox.default(), &|_| {
        switched.set(true);
        Ok(())
    });
    let kept = from.join("1.jpg").exists();
    writable_again(&from);
    assert_eq!(result, Ok(()));
    assert!(switched.get(), "the copy-only move still switches");
    assert_eq!(read(&to.join("1.jpg")), "one");
    assert!(kept, "copy only: the source stays");
}

// #296 R2-1: the probe runs both ways, so a target that cannot be written
// is not taken for a separate folder.
#[cfg(unix)]
#[test]
fn the_probe_runs_in_both_directions() {
    let sandbox = Sandbox::new("folder-probe-both-ways", None);
    let from = sandbox.path("from");
    let to = sandbox.path("to");
    fs::create_dir_all(&from).expect("from");
    fs::create_dir_all(&to).expect("to");
    if !read_only(&to) {
        return;
    }
    let probe = probe_folders(&from, &to);
    writable_again(&to);
    assert_eq!(probe, Probe::Unknown);
    assert_eq!(
        fs::read_dir(&from).expect("from").count(),
        0,
        "no probe left"
    );
}

// #296 R2-1: the folders are probed again right before the deletes; a probe
// that no longer works keeps every source.
#[cfg(unix)]
#[test]
fn the_folders_are_probed_again_before_the_deletes() {
    let sandbox = Sandbox::new("folder-reprobe", None);
    let from = sandbox.path("from");
    let to = sandbox.path("to");
    put(&from, "1.jpg", "one");
    let request = MoveRequest {
        from: from.clone(),
        to: to.clone(),
        made: Vec::new(),
    };
    let restricted = std::cell::Cell::new(false);
    let result = move_images(&request, &sandbox.default(), &|to| {
        // The target turns read only between the copy and the deletes.
        restricted.set(read_only(to));
        Ok(())
    });
    writable_again(&to);
    if !restricted.get() {
        return;
    }
    assert_eq!(result, Ok(()));
    assert_eq!(read(&to.join("1.jpg")), "one");
    assert_eq!(
        read(&from.join("1.jpg")),
        "one",
        "kept: the re-probe failed"
    );
}

// Review M3: a target that was there before the move (another size, so it
// is replaced) is not "made by the move": a failed move keeps it.
#[test]
fn a_failed_move_keeps_a_target_that_was_there_before() {
    let sandbox = Sandbox::new("folder-failed-keeps-target", None);
    let from = sandbox.path("from");
    let to = sandbox.path("to");
    put(&from, "1.jpg", "one");
    put(&to, "1.jpg", "older-bytes");
    let request = MoveRequest {
        from: from.clone(),
        to: to.clone(),
        made: Vec::new(),
    };
    let result = move_images(&request, &sandbox.default(), &|_| {
        Err("the switch failed".to_string())
    });
    assert!(result.is_err());
    assert!(to.join("1.jpg").exists(), "a file that was there stays");
    assert_eq!(read(&from.join("1.jpg")), "one");
}

// Review M4: the probe is injected, so copy-only and the re-probe are
// checked on every platform.
fn request_between(sandbox: &Sandbox) -> MoveRequest {
    let from = sandbox.path("from");
    put(&from, "1.jpg", "one");
    MoveRequest {
        from,
        to: sandbox.path("to"),
        made: Vec::new(),
    }
}

#[test]
fn folders_that_cannot_be_probed_are_copied_and_never_deleted() {
    let sandbox = Sandbox::new("folder-copy-only-any-os", None);
    let request = request_between(&sandbox);
    let switched = std::cell::Cell::new(false);
    let result = move_images_with(
        &request,
        &sandbox.default(),
        &|_| {
            switched.set(true);
            Ok(())
        },
        &|_, _| Probe::Unknown,
    );
    assert_eq!(result, Ok(()));
    assert!(switched.get());
    assert_eq!(read(&request.to.join("1.jpg")), "one");
    assert_eq!(read(&request.from.join("1.jpg")), "one", "copy only");
}

#[test]
fn a_probe_that_fails_before_the_deletes_keeps_every_source() {
    let sandbox = Sandbox::new("folder-reprobe-any-os", None);
    let request = request_between(&sandbox);
    let calls = std::cell::Cell::new(0);
    let probe = |_: &Path, _: &Path| {
        calls.set(calls.get() + 1);
        if calls.get() == 1 {
            Probe::Separate
        } else {
            Probe::Unknown
        }
    };
    let result = move_images_with(&request, &sandbox.default(), &|_| Ok(()), &probe);
    assert_eq!(result, Ok(()));
    assert_eq!(
        calls.get(),
        2,
        "probed before the copy and again before the deletes"
    );
    assert_eq!(read(&request.from.join("1.jpg")), "one");
    // The same move with two good probes deletes the source.
    let sandbox = Sandbox::new("folder-reprobe-any-os-good", None);
    let request = request_between(&sandbox);
    move_images_with(&request, &sandbox.default(), &|_| Ok(()), &|_, _| {
        Probe::Separate
    })
    .expect("moved");
    assert!(!request.from.join("1.jpg").exists());
}

#[test]
fn a_missing_target_cannot_be_probed() {
    let sandbox = Sandbox::new("folder-probe-missing-target", None);
    let from = sandbox.path("from");
    fs::create_dir_all(&from).expect("from");
    assert_eq!(
        probe_folders(&from, &sandbox.path("missing")),
        Probe::Unknown
    );
    assert!(!sandbox.path("missing").exists(), "never made");
    assert_eq!(
        fs::read_dir(&from).expect("from").count(),
        0,
        "no probe left"
    );
}
