//! The league card's custom background (overlay spec 4 and 9.4; 0.x
//! `UploadOverlayBackgroundController`, `DeleteOverlayBackgroundController`).
//!
//! - **Choose** (`choose_overlay_background`): the dialog plugin's open
//!   dialog, filtered to jpeg, png, webp and gif. The file is checked (its
//!   extension and its first bytes name the same image type, at most
//!   5 MiB, Laravel's messages) and copied into `<app data>/overlay/` as
//!   `background-{ms}.{ext}`. The row `overlay_background_file` names it;
//!   the previous file is deleted once the new row is stored.
//! - **Remove** (`remove_overlay_background`): the row is cleared, then the
//!   file deleted. The uploaded copy is deleted by the publish side
//!   (`publish::forget_background`) on the `BackgroundChanged` that follows.
//! - **Serve**: the `overlay` URI scheme (`overlay://localhost/background`,
//!   `http://overlay.localhost/background` on Windows, as Plan S's `card`
//!   scheme) answers only the path `/background`, with the file the row
//!   names, when that name is one v1 writes and it is a plain file in the
//!   folder itself. Nothing else can be named, so no path traversal.
//!
//! Each change wakes the overlay worker, which recomputes the league card
//! (0.x "pushes an overlay reload after uploading and deleting").

use std::path::{Path, PathBuf};

use tauri::http::{Response, StatusCode, header};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

use crate::bridge::lock;
use crate::commands::{AppState, CommandError, Startup, off_async_workers, ready, storage};
use crate::logging::Timer;
use crate::overlay::worker::OverlayMsg;
use crate::settings::{overlay_background_file, overlay_background_file_row};

/// The scheme's name.
pub const SCHEME: &str = "overlay";
/// The folder in the app data folder that holds the background.
pub const FOLDER: &str = "overlay";
/// The one path the scheme serves.
pub const BACKGROUND_PATH: &str = "/background";
/// The largest background, in bytes (Laravel `max:5120`, kilobytes).
pub const MAX_BYTES: u64 = 5120 * 1024;
/// The extensions the dialog offers and a stored file may have.
pub const EXTENSIONS: [&str; 5] = ["jpeg", "jpg", "png", "webp", "gif"];
/// Laravel's messages for the `image` field (spec 9.4).
pub const TYPE_MESSAGE: &str = "The image must be a file of type: jpeg, png, webp, gif.";
pub const SIZE_MESSAGE: &str = "The image field must not be greater than 5120 kilobytes.";

/// The start of every background file name.
const NAME_PREFIX: &str = "background-";

/// An image type the background may have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageKind {
    Jpeg,
    Png,
    Webp,
    Gif,
}

impl ImageKind {
    pub(crate) fn content_type(self) -> &'static str {
        match self {
            ImageKind::Jpeg => "image/jpeg",
            ImageKind::Png => "image/png",
            ImageKind::Webp => "image/webp",
            ImageKind::Gif => "image/gif",
        }
    }
}

/// Why a background was not stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackgroundError {
    /// The file is not one the card takes; the message is for the user.
    Refused(&'static str),
    /// Reading, writing or saving failed.
    Failed(String),
}

/// `<app data>/overlay/`.
pub fn folder(data_dir: &Path) -> PathBuf {
    data_dir.join(FOLDER)
}

/// A scheme's URL prefix as the webview loads it: `{scheme}://localhost/`,
/// or `http://{scheme}.localhost/` on Windows (Tauri's custom protocols).
pub fn scheme_base(scheme: &str) -> String {
    if cfg!(windows) {
        format!("http://{scheme}.localhost/")
    } else {
        format!("{scheme}://localhost/")
    }
}

/// The window's URL of the background `name`. The query names the file, so
/// a replaced background is a new URL the card loads again; the scheme
/// ignores it.
pub fn background_url(name: &str) -> String {
    let stamp = name
        .strip_prefix(NAME_PREFIX)
        .and_then(|rest| rest.split('.').next())
        .unwrap_or("");
    format!("{}{}?v={stamp}", scheme_base(SCHEME), &BACKGROUND_PATH[1..])
}

/// Whether `name` is a file name v1 writes: `background-`, digits, a dot
/// and one of the extensions, lower case.
pub fn is_background_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(NAME_PREFIX) else {
        return false;
    };
    let Some((stamp, extension)) = rest.split_once('.') else {
        return false;
    };
    !stamp.is_empty()
        && stamp.chars().all(|c| c.is_ascii_digit())
        && EXTENSIONS.contains(&extension)
}

/// The image type a file extension names (any case).
pub(crate) fn extension_kind(extension: &str) -> Option<ImageKind> {
    match extension.to_ascii_lowercase().as_str() {
        "jpeg" | "jpg" => Some(ImageKind::Jpeg),
        "png" => Some(ImageKind::Png),
        "webp" => Some(ImageKind::Webp),
        "gif" => Some(ImageKind::Gif),
        _ => None,
    }
}

/// The image type a file's first bytes show.
fn sniffed_kind(head: &[u8]) -> Option<ImageKind> {
    if head.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(ImageKind::Jpeg)
    } else if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageKind::Png)
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        Some(ImageKind::Gif)
    } else if head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        Some(ImageKind::Webp)
    } else {
        None
    }
}

/// A plain file (not a link, not a folder).
pub(crate) fn is_plain_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file())
}

/// The background file `name` in `folder`, when the name is v1's and it is
/// a plain file there (not a link, not a folder).
pub fn background_file(folder: &Path, name: Option<&str>) -> Option<PathBuf> {
    let name = name.filter(|name| is_background_name(name))?;
    let file = folder.join(name);
    is_plain_file(&file).then_some(file)
}

/// Check `source` and copy it into `folder` as `background-{now_ms}.{ext}`.
/// Returns the new name. Deletes nothing.
pub fn store_background(
    folder: &Path,
    source: &Path,
    now_ms: i64,
) -> Result<String, BackgroundError> {
    store_image(folder, source, &format!("{NAME_PREFIX}{now_ms}"))
}

/// Check `source` (a jpeg, png, webp or gif of at most 5 MiB, by extension
/// and first bytes) and copy it into `folder` as `{stem}.{ext}`. Returns the
/// new name. Deletes nothing; shared with the deck covers.
pub(crate) fn store_image(
    folder: &Path,
    source: &Path,
    stem: &str,
) -> Result<String, BackgroundError> {
    let extension = source
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let Some(kind) = extension_kind(&extension) else {
        return Err(BackgroundError::Refused(TYPE_MESSAGE));
    };
    let size = std::fs::metadata(source)
        .map_err(|error| BackgroundError::Failed(format!("could not read the image: {error}")))?
        .len();
    // Read the head before the whole file, so a huge file is never read.
    let mut head = [0u8; 12];
    let read = {
        use std::io::Read;
        let mut file = std::fs::File::open(source).map_err(|error| {
            BackgroundError::Failed(format!("could not read the image: {error}"))
        })?;
        let mut filled = 0;
        while filled < head.len() {
            match file.read(&mut head[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(error) => {
                    return Err(BackgroundError::Failed(format!(
                        "could not read the image: {error}"
                    )));
                }
            }
        }
        filled
    };
    if sniffed_kind(&head[..read]) != Some(kind) {
        return Err(BackgroundError::Refused(TYPE_MESSAGE));
    }
    if size > MAX_BYTES {
        return Err(BackgroundError::Refused(SIZE_MESSAGE));
    }
    std::fs::create_dir_all(folder).map_err(|error| {
        BackgroundError::Failed(format!("could not create the image folder: {error}"))
    })?;
    let name = format!("{stem}.{extension}");
    copy_new(source, &folder.join(&name))
        .map_err(|error| BackgroundError::Failed(format!("could not copy the image: {error}")))?;
    Ok(name)
}

/// Copy `source` to a new file at `dest`. A file or link already at `dest`
/// is an error and is never written (`create_new`), so the copy cannot be
/// pointed through a link at another file. A copy that fails part way is
/// deleted.
fn copy_new(source: &Path, dest: &Path) -> std::io::Result<()> {
    let mut from = std::fs::File::open(source)?;
    let mut to = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)?;
    let copied = std::io::copy(&mut from, &mut to).and_then(|_| to.sync_all());
    if copied.is_err() {
        drop(to);
        let _ = std::fs::remove_file(dest);
    }
    copied
}

/// Delete the background `name` from `folder`, if it is v1's file.
pub fn delete_background(folder: &Path, name: Option<&str>) {
    if let Some(file) = background_file(folder, name)
        && let Err(error) = std::fs::remove_file(&file)
    {
        tracing::warn!(%error, "could not delete the old overlay background");
    }
}

/// Store `source` as the background: copy it, `save` the new name, then
/// delete `previous`. When the save fails the new copy is deleted and the
/// previous background stays.
pub fn replace_background(
    folder: &Path,
    source: &Path,
    previous: Option<&str>,
    now_ms: i64,
    save: &mut dyn FnMut(Option<&str>) -> Result<(), String>,
) -> Result<String, BackgroundError> {
    let name = store_background(folder, source, now_ms)?;
    if let Err(error) = save(Some(&name)) {
        if previous != Some(name.as_str()) {
            delete_background(folder, Some(&name));
        }
        return Err(BackgroundError::Failed(error));
    }
    if previous != Some(name.as_str()) {
        delete_background(folder, previous);
    }
    Ok(name)
}

/// Remove the background: `save` no name, then delete `current`.
pub fn remove_background(
    folder: &Path,
    current: Option<&str>,
    save: &mut dyn FnMut(Option<&str>) -> Result<(), String>,
) -> Result<(), String> {
    save(None)?;
    delete_background(folder, current);
    Ok(())
}

/// 0.x's custom background brought into v1 by the 0.x import (overlay spec
/// section 12): copied into `folder` with the upload's checks when v1 has
/// none of its own (`current`, the `overlay_background_file` row). Returns
/// what happened and the new file's name, which the caller stores as the
/// row (and deletes again if that fails). 0.x's file is only read.
pub fn legacy_background(
    folder: &Path,
    current: Option<&str>,
    legacy: &importer::overlay_read::LegacyBackground,
    now_ms: i64,
) -> (crate::legacy::BackgroundImport, Option<String>) {
    use crate::legacy::BackgroundImport;
    use importer::overlay_read::LegacyBackground;
    let source = match legacy {
        LegacyBackground::NotSet => return (BackgroundImport::NotSet, None),
        _ if background_file(folder, current).is_some() => return (BackgroundImport::Kept, None),
        LegacyBackground::Unusable => return (BackgroundImport::Missing, None),
        LegacyBackground::File(source) => source,
    };
    if !is_plain_file(source) {
        return (BackgroundImport::Missing, None);
    }
    match store_background(folder, source, now_ms) {
        Ok(name) => (BackgroundImport::Copied, Some(name)),
        Err(BackgroundError::Refused(_)) => (BackgroundImport::Refused, None),
        Err(BackgroundError::Failed(error)) => {
            tracing::warn!(%error, "the 0.x overlay background could not be copied");
            (BackgroundImport::Failed, None)
        }
    }
}

/// The scheme's answer for a request `path` with the stored background
/// `current` in `folder` (`None` when the app has no folder).
pub fn respond(folder: Option<&Path>, current: Option<&str>, path: &str) -> Response<Vec<u8>> {
    if path != BACKGROUND_PATH {
        return not_found();
    }
    let Some(folder) = folder else {
        return not_found();
    };
    let Some(file) = background_file(folder, current) else {
        return not_found();
    };
    let kind = file
        .extension()
        .and_then(|extension| extension.to_str())
        .and_then(extension_kind);
    let (Some(kind), Ok(bytes)) = (kind, std::fs::read(&file)) else {
        return not_found();
    };
    let mut response = Response::new(bytes);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static(kind.content_type()),
    );
    response
}

pub(crate) fn not_found() -> Response<Vec<u8>> {
    let mut response = Response::new(Vec::new());
    *response.status_mut() = StatusCode::NOT_FOUND;
    response
}

/// The scheme's answer in the running app.
pub fn respond_for_app(app: &AppHandle, path: &str) -> Response<Vec<u8>> {
    let Some(startup) = app.try_state::<Startup>() else {
        return respond(None, None, path);
    };
    let Ok(state) = ready(&startup) else {
        return respond(None, None, path);
    };
    let current = match store::read::settings(&lock(&state.reader)) {
        Ok(rows) => overlay_background_file(&rows),
        Err(_) => None,
    };
    respond(Some(&folder(&state.data_dir)), current.as_deref(), path)
}

/// Make `source` the league card's background.
pub fn set_background(state: &AppState, source: &Path, now_ms: i64) -> Result<(), CommandError> {
    let _one_save = lock(&state.settings_lock);
    let rows = store::read::settings(&lock(&state.reader)).map_err(storage)?;
    let previous = overlay_background_file(&rows);
    let mut save =
        |name: Option<&str>| state.save_setting_rows(vec![overlay_background_file_row(name)]);
    match replace_background(
        &folder(&state.data_dir),
        source,
        previous.as_deref(),
        now_ms,
        &mut save,
    ) {
        Ok(_) => Ok(()),
        Err(BackgroundError::Refused(message)) => Err(CommandError::Refused(message.to_string())),
        Err(BackgroundError::Failed(message)) => Err(CommandError::Storage(message)),
    }
}

/// Remove the league card's background.
pub fn clear_background(state: &AppState) -> Result<(), CommandError> {
    let _one_save = lock(&state.settings_lock);
    let rows = store::read::settings(&lock(&state.reader)).map_err(storage)?;
    let current = overlay_background_file(&rows);
    let mut save =
        |name: Option<&str>| state.save_setting_rows(vec![overlay_background_file_row(name)]);
    remove_background(&folder(&state.data_dir), current.as_deref(), &mut save)
        .map_err(CommandError::Tracker)
}

/// Choose the league card's background with the open dialog. `false` when
/// the dialog was cancelled; a refused file is an error with Laravel's
/// message. Async: a blocking dialog must not run on the main thread.
#[tauri::command]
#[specta::specta]
pub async fn choose_overlay_background(app: AppHandle) -> Result<bool, CommandError> {
    let _timer = Timer::start("choose_overlay_background");
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Choose a background image")
        .add_filter("Images", &EXTENSIONS);
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    let Some(picked) = dialog.blocking_pick_file() else {
        return Ok(false);
    };
    let Ok(source) = picked.into_path() else {
        return Ok(false);
    };
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        set_background(ready(&startup)?, &source, crate::wall_clock_ms())?;
        crate::overlay::signal(&app, OverlayMsg::BackgroundChanged);
        Ok(true)
    })
    .await
}

/// Remove the league card's background.
#[tauri::command]
#[specta::specta]
pub async fn remove_overlay_background(app: AppHandle) -> Result<(), CommandError> {
    let _timer = Timer::start("remove_overlay_background");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        clear_background(ready(&startup)?)?;
        crate::overlay::signal(&app, OverlayMsg::BackgroundChanged);
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use fixture_tools::TestDir;

    use crate::commands::tests::ready_state;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDRq-png";
    const JPEG: &[u8] = b"\xff\xd8\xff\xe0\0\x10JFIFq-jpeg";
    const GIF: &[u8] = b"GIF89aq-gif";
    const WEBP: &[u8] = b"RIFF\x10\0\0\0WEBPVP8 q-webp";

    fn rows_of(state: &AppState) -> Vec<(String, String)> {
        store::read::settings(&lock(&state.reader)).expect("rows")
    }

    fn stored_name(state: &AppState) -> Option<String> {
        overlay_background_file(&rows_of(state))
    }

    fn files_in(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> = match std::fs::read_dir(folder) {
            Ok(entries) => entries
                .map(|entry| {
                    entry
                        .expect("entry")
                        .file_name()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        names.sort();
        names
    }

    // 0.x UploadOverlayBackgroundControllerTest "stores an uploaded image
    // and saves the path in app settings".
    #[test]
    fn stores_a_chosen_image_and_saves_its_name() {
        let dir = TestDir::new("overlay-bg-store");
        let (state, _events) = ready_state(&dir);
        let source = dir.write("pick/Qbg.PNG", PNG);
        set_background(&state, &source, 1_700).expect("stored");
        let name = stored_name(&state).expect("a name is saved");
        assert_eq!(name, "background-1700.png");
        let stored = folder(&state.data_dir).join(&name);
        assert_eq!(std::fs::read(stored).expect("the copy"), PNG);
        assert!(source.exists(), "the chosen file is copied, never moved");
        crate::commands::stop_tracker(&state);
    }

    #[test]
    fn an_existing_file_is_never_overwritten() {
        let dir = TestDir::new("overlay-bg-existing");
        let target = dir.join("overlay");
        let existing = dir.write("overlay/background-1700.png", b"q kept");
        let source = dir.write("pick/Qbg.png", PNG);
        let result = store_background(&target, &source, 1_700);
        assert!(
            matches!(result, Err(BackgroundError::Failed(_))),
            "{result:?}"
        );
        assert_eq!(std::fs::read(&existing).expect("kept"), b"q kept");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_the_folder_is_never_written_through() {
        let dir = TestDir::new("overlay-bg-link");
        let outside = dir.write("outside/secret.txt", b"q outside");
        std::fs::create_dir_all(dir.join("overlay")).expect("folder");
        std::os::unix::fs::symlink(&outside, dir.join("overlay/background-1700.png"))
            .expect("link");
        let source = dir.write("pick/Qbg.png", PNG);
        assert!(store_background(&dir.join("overlay"), &source, 1_700).is_err());
        assert_eq!(std::fs::read(&outside).expect("kept"), b"q outside");
    }

    // 0.x "replaces an existing background and removes the old file".
    #[test]
    fn replacing_the_background_deletes_the_old_file() {
        let dir = TestDir::new("overlay-bg-replace");
        let (state, _events) = ready_state(&dir);
        set_background(&state, &dir.write("pick/old.gif", GIF), 1_000).expect("old");
        set_background(&state, &dir.write("pick/new.webp", WEBP), 2_000).expect("new");
        assert_eq!(stored_name(&state).as_deref(), Some("background-2000.webp"));
        assert_eq!(
            files_in(&folder(&state.data_dir)),
            vec!["background-2000.webp".to_string()]
        );
        crate::commands::stop_tracker(&state);
    }

    // 0.x "rejects non-image uploads".
    #[test]
    fn a_file_that_is_not_an_image_is_refused_with_laravels_message() {
        let dir = TestDir::new("overlay-bg-refuse");
        let (state, _events) = ready_state(&dir);
        let text = dir.write("pick/bad.txt", b"q plain text");
        assert_eq!(
            set_background(&state, &text, 1_000),
            Err(CommandError::Refused(TYPE_MESSAGE.to_string()))
        );
        // An image extension with other bytes, and image bytes under another
        // extension, are refused too.
        let fake = dir.write("pick/fake.png", b"q not a png");
        assert_eq!(
            set_background(&state, &fake, 1_000),
            Err(CommandError::Refused(TYPE_MESSAGE.to_string()))
        );
        let renamed = dir.write("pick/real.gif", PNG);
        assert_eq!(
            set_background(&state, &renamed, 1_000),
            Err(CommandError::Refused(TYPE_MESSAGE.to_string()))
        );
        assert_eq!(stored_name(&state), None);
        assert!(files_in(&folder(&state.data_dir)).is_empty());
        crate::commands::stop_tracker(&state);
    }

    // F3: a file that is there but cannot be copied is not "missing".
    #[test]
    fn a_legacy_file_that_cannot_be_copied_is_failed_not_missing() {
        use crate::legacy::BackgroundImport;
        use importer::overlay_read::LegacyBackground;
        let dir = TestDir::new("overlay-bg-legacy-failed");
        let source = dir.write("old/Qbg.png", PNG);
        // The overlay folder's path is a plain file, so it cannot be created.
        let blocked = dir.write("blocked", b"q");
        let (result, name) = legacy_background(
            &blocked,
            None,
            &LegacyBackground::File(source.clone()),
            1_000,
        );
        assert_eq!(result, BackgroundImport::Failed);
        assert_eq!(name, None);
        let gone = dir.path().join("old/none.png");
        let (result, _) = legacy_background(&blocked, None, &LegacyBackground::File(gone), 1_000);
        assert_eq!(result, BackgroundImport::Missing);
    }

    #[test]
    fn an_image_over_5_mib_is_refused() {
        let dir = TestDir::new("overlay-bg-size");
        let (state, _events) = ready_state(&dir);
        let mut big = JPEG.to_vec();
        big.resize(MAX_BYTES as usize + 1, 0);
        let over = dir.write("pick/big.jpg", &big);
        assert_eq!(
            set_background(&state, &over, 1_000),
            Err(CommandError::Refused(SIZE_MESSAGE.to_string()))
        );
        assert_eq!(stored_name(&state), None);
        // Exactly 5120 kilobytes passes (Laravel `max`).
        big.truncate(MAX_BYTES as usize);
        let limit = dir.write("pick/limit.jpeg", &big);
        set_background(&state, &limit, 2_000).expect("at the limit");
        assert_eq!(stored_name(&state).as_deref(), Some("background-2000.jpeg"));
        crate::commands::stop_tracker(&state);
    }

    // 0.x "deletes the background image and clears the setting".
    #[test]
    fn removing_the_background_deletes_it_and_clears_the_row() {
        let dir = TestDir::new("overlay-bg-remove");
        let (state, _events) = ready_state(&dir);
        set_background(&state, &dir.write("pick/bg.png", PNG), 1_000).expect("stored");
        clear_background(&state).expect("removed");
        assert_eq!(stored_name(&state), None);
        assert!(files_in(&folder(&state.data_dir)).is_empty());
        clear_background(&state).expect("removing nothing is fine");
        crate::commands::stop_tracker(&state);
    }

    #[test]
    fn a_failed_save_keeps_the_previous_background() {
        let dir = TestDir::new("overlay-bg-save-fails");
        let target = dir.join("overlay");
        let mut saved: Vec<Option<String>> = Vec::new();
        let mut save = |name: Option<&str>| {
            saved.push(name.map(str::to_string));
            Ok(())
        };
        let first = replace_background(&target, &dir.write("a.png", PNG), None, 1, &mut save)
            .expect("first");
        let mut failing = |_: Option<&str>| Err("the tracker is busy".to_string());
        let second = replace_background(
            &target,
            &dir.write("b.png", PNG),
            Some(&first),
            2,
            &mut failing,
        );
        assert_eq!(
            second,
            Err(BackgroundError::Failed("the tracker is busy".to_string()))
        );
        assert_eq!(files_in(&target), vec![first.clone()]);
        assert_eq!(saved, vec![Some(first)]);
    }

    #[test]
    fn only_v1s_background_names_are_taken() {
        for name in [
            "background-1.png",
            "background-1700000000000.jpeg",
            "background-2.jpg",
            "background-3.webp",
            "background-4.gif",
        ] {
            assert!(is_background_name(name), "{name}");
        }
        for name in [
            "background-.png",
            "background-1.PNG",
            "background-1.svg",
            "background-1",
            "background-1.png/..",
            "../background-1.png",
            "background-1a.png",
            "background-1.png.png",
            "Background-1.png",
            "",
        ] {
            assert!(!is_background_name(name), "{name}");
        }
    }

    #[test]
    fn the_url_changes_with_the_file_and_uses_the_platforms_form() {
        let url = background_url("background-1700.png");
        let base = if cfg!(windows) {
            "http://overlay.localhost/"
        } else {
            "overlay://localhost/"
        };
        assert_eq!(url, format!("{base}background?v=1700"));
        assert_ne!(url, background_url("background-1800.png"));
    }

    #[test]
    fn the_scheme_serves_the_stored_background_with_its_type() {
        let dir = TestDir::new("overlay-scheme-serves");
        let target = dir.join("overlay");
        std::fs::create_dir_all(&target).expect("folder");
        std::fs::write(target.join("background-5.webp"), WEBP).expect("file");
        let response = respond(Some(&target), Some("background-5.webp"), "/background");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.body(), WEBP);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .map(|value| value.as_bytes()),
            Some(&b"image/webp"[..])
        );
    }

    #[test]
    fn the_scheme_refuses_every_other_path_name_and_file() {
        let dir = TestDir::new("overlay-scheme-refuses");
        let target = dir.join("overlay");
        std::fs::create_dir_all(&target).expect("folder");
        std::fs::write(target.join("background-5.png"), PNG).expect("file");
        std::fs::write(dir.join("background-6.png"), PNG).expect("outside");
        std::fs::create_dir_all(target.join("background-7.png")).expect("a folder");
        let not_found = |current: Option<&str>, path: &str| {
            respond(Some(&target), current, path).status() == StatusCode::NOT_FOUND
        };
        for path in [
            "/background-5.png",
            "/../background-6.png",
            "/background/..",
            "/background/",
            "//background",
            "/BACKGROUND",
            "/%62ackground",
            "background",
            "/",
            "",
        ] {
            assert!(not_found(Some("background-5.png"), path), "{path}");
        }
        for current in [
            None,
            Some("../background-6.png"),
            Some("background-6.png"),
            Some("background-7.png"),
            Some("secret.png"),
        ] {
            assert!(not_found(current, "/background"), "{current:?}");
        }
        assert_eq!(
            respond(None, Some("background-5.png"), "/background").status(),
            StatusCode::NOT_FOUND
        );
    }
}
