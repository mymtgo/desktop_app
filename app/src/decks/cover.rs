//! A deck's own cover image (0.x has only cover cards).
//!
//! - **Set** (`set_deck_cover_image`): the page picks the file and, when it
//!   is larger than the recommended cover size, crops it first; it sends the
//!   image's bytes and extension. They are written to a scratch file and the
//!   overlay background's checks apply (type by extension and first bytes,
//!   at most 5 MiB). The file is copied into `<app data>/deck-covers/` as
//!   `{deck id}-{ms}.{ext}`, the edit `SetCoverFile` names it, and the
//!   previous file is deleted once the edit is stored.
//! - **Remove** (`remove_deck_cover`): the edit clears the name, then the
//!   file is deleted. The cover card stays as it was.
//! - **Serve**: the `deckcover` URI scheme (`deckcover://localhost/{name}`,
//!   `http://deckcover.localhost/{name}` on Windows) answers only a name v1
//!   writes that is a plain file in the folder itself, so no path traversal.
//!
//! Local only: the name is never synced, and the hosted overlay and the API
//! keep using the cover card. A missing file shows the cover card (the page
//! falls back when the image fails to load; the league overlay card checks
//! the file when it is built).

use std::path::{Path, PathBuf};

use store::decks::{DeckEdit, deck_by_id};
use tauri::http::{Response, header};
use tauri::{AppHandle, Manager};

use super::{edit_result, refused, send_edit};
use crate::bridge::lock;
use crate::cards::data_url::{data_url_of, mime_for};
use crate::commands::{AppState, CommandError, Startup, off_async_workers, ready, storage};
use crate::logging::Timer;
use crate::overlay::background::{
    BackgroundError, EXTENSIONS, MAX_BYTES, SIZE_MESSAGE, TYPE_MESSAGE, extension_kind,
    is_plain_file, not_found, store_image,
};

/// The scheme's name.
pub const SCHEME: &str = "deckcover";
/// The folder in the app data folder that holds the covers.
pub const FOLDER: &str = "deck-covers";

/// `<app data>/deck-covers/`.
pub fn folder(data_dir: &Path) -> PathBuf {
    data_dir.join(FOLDER)
}

/// Whether `name` is a file name v1 writes: a deck id, `-`, digits, a dot
/// and one of the extensions, lower case.
pub fn is_cover_name(name: &str) -> bool {
    let Some((deck, rest)) = name.split_once('-') else {
        return false;
    };
    let Some((stamp, extension)) = rest.split_once('.') else {
        return false;
    };
    let digits = |text: &str| !text.is_empty() && text.chars().all(|c| c.is_ascii_digit());
    digits(deck) && digits(stamp) && EXTENSIONS.contains(&extension)
}

/// The cover file `name` in `folder`, when the name is v1's and it is a
/// plain file there (not a link, not a folder).
pub fn cover_file(folder: &Path, name: Option<&str>) -> Option<PathBuf> {
    let name = name.filter(|name| is_cover_name(name))?;
    let file = folder.join(name);
    is_plain_file(&file).then_some(file)
}

/// The cover `name` as a `data:` URL, for the screenshots; `None` when the
/// file is not there.
pub fn data_url(folder: &Path, name: &str) -> Option<String> {
    let bytes = std::fs::read(cover_file(folder, Some(name))?).ok()?;
    if bytes.is_empty() {
        return None;
    }
    Some(data_url_of(mime_for(&bytes, name), &bytes))
}

fn delete_cover(folder: &Path, name: Option<&str>) {
    if let Some(file) = cover_file(folder, name)
        && let Err(error) = std::fs::remove_file(&file)
    {
        tracing::warn!(%error, "could not delete the old deck cover");
    }
}

/// Check `source` and copy it into `folder` as `{deck_id}-{now_ms}.{ext}`.
/// Returns the new name. Deletes nothing.
pub fn store_cover(
    folder: &Path,
    source: &Path,
    deck_id: i64,
    now_ms: i64,
) -> Result<String, BackgroundError> {
    store_image(folder, source, &format!("{deck_id}-{now_ms}"))
}

/// Store `source` as the deck's cover: copy it, `save` the new name, then
/// delete `previous`. When the save fails the new copy is deleted and the
/// previous cover stays.
pub fn replace_cover(
    folder: &Path,
    source: &Path,
    deck_id: i64,
    previous: Option<&str>,
    now_ms: i64,
    save: &mut dyn FnMut(Option<&str>) -> Result<(), CommandError>,
) -> Result<String, CommandError> {
    let name = store_cover(folder, source, deck_id, now_ms).map_err(|error| match error {
        BackgroundError::Refused(message) => CommandError::Refused(message.to_string()),
        BackgroundError::Failed(message) => CommandError::Storage(message),
    })?;
    if let Err(error) = save(Some(&name)) {
        delete_cover(folder, Some(&name));
        return Err(error);
    }
    delete_cover(folder, previous);
    Ok(name)
}

/// Make `source` the deck's own cover image.
pub fn set_cover_file(
    state: &AppState,
    deck_id: i64,
    source: &Path,
    now_ms: i64,
) -> Result<(), CommandError> {
    let deck = deck_by_id(&lock(&state.reader), deck_id)
        .map_err(storage)?
        .ok_or_else(|| refused(store::decks::EditOutcome::NotFound))?;
    let mut save = |name: Option<&str>| -> Result<(), CommandError> {
        let file = name.map(str::to_string);
        edit_result(send_edit(state, DeckEdit::SetCoverFile { deck_id, file })?)
    };
    replace_cover(
        &folder(&state.data_dir),
        source,
        deck_id,
        deck.cover_file.as_deref(),
        now_ms,
        &mut save,
    )
    .map(|_name| ())
}

/// Make `bytes`, an image of type `extension`, the deck's own cover image.
/// The bytes go through a scratch file in `scratch` so the checks and the
/// copy are `set_cover_file`'s; the scratch file is deleted either way.
pub fn set_cover_bytes(
    state: &AppState,
    deck_id: i64,
    bytes: &[u8],
    extension: &str,
    scratch: &Path,
    now_ms: i64,
) -> Result<(), CommandError> {
    let extension = extension.to_ascii_lowercase();
    if extension_kind(&extension).is_none() {
        return Err(CommandError::Refused(TYPE_MESSAGE.to_string()));
    }
    if bytes.len() as u64 > MAX_BYTES {
        return Err(CommandError::Refused(SIZE_MESSAGE.to_string()));
    }
    std::fs::create_dir_all(scratch)
        .map_err(|error| CommandError::Storage(format!("could not stage the image: {error}")))?;
    let source = scratch.join(format!("{deck_id}-{now_ms}.{extension}"));
    std::fs::write(&source, bytes)
        .map_err(|error| CommandError::Storage(format!("could not stage the image: {error}")))?;
    let result = set_cover_file(state, deck_id, &source, now_ms);
    if let Err(error) = std::fs::remove_file(&source) {
        tracing::warn!(%error, "could not delete the staged deck cover");
    }
    result
}

/// Remove the deck's own cover image: the edit first, then the file.
pub fn clear_cover_file(state: &AppState, deck_id: i64) -> Result<(), CommandError> {
    let deck = deck_by_id(&lock(&state.reader), deck_id)
        .map_err(storage)?
        .ok_or_else(|| refused(store::decks::EditOutcome::NotFound))?;
    edit_result(send_edit(
        state,
        DeckEdit::SetCoverFile {
            deck_id,
            file: None,
        },
    )?)?;
    delete_cover(&folder(&state.data_dir), deck.cover_file.as_deref());
    Ok(())
}

/// The scheme's answer for a request `path` (`None` folder when the app has
/// none).
pub fn respond(folder: Option<&Path>, path: &str) -> Response<Vec<u8>> {
    let (Some(folder), Some(name)) = (folder, path.strip_prefix('/')) else {
        return not_found();
    };
    let Some(file) = cover_file(folder, Some(name)) else {
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

/// The scheme's answer in the running app.
pub fn respond_for_app(app: &AppHandle, path: &str) -> Response<Vec<u8>> {
    let Some(startup) = app.try_state::<Startup>() else {
        return respond(None, path);
    };
    let Ok(state) = ready(&startup) else {
        return respond(None, path);
    };
    respond(Some(&folder(&state.data_dir)), path)
}

/// Make `bytes` (a `jpeg`, `png`, `webp` or `gif` image, cropped by the page
/// when it was larger than the recommended cover size) the deck's own cover
/// image. A refused image is an error with Laravel's message (the overlay
/// background's).
#[tauri::command]
#[specta::specta]
pub async fn set_deck_cover_image(
    app: AppHandle,
    id: u32,
    bytes: Vec<u8>,
    extension: String,
) -> Result<(), CommandError> {
    let _timer = Timer::start("set_deck_cover_image");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        set_cover_bytes(
            ready(&startup)?,
            i64::from(id),
            &bytes,
            &extension,
            &std::env::temp_dir().join("mymtgo-deck-covers"),
            crate::wall_clock_ms(),
        )
    })
    .await
}

/// Remove the deck's own cover image.
#[tauri::command]
#[specta::specta]
pub async fn remove_deck_cover(app: AppHandle, id: u32) -> Result<(), CommandError> {
    let _timer = Timer::start("remove_deck_cover");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        clear_cover_file(ready(&startup)?, i64::from(id))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use fixture_tools::TestDir;
    use tauri::http::StatusCode;

    use crate::commands::tests::ready_state;
    use crate::overlay::background::{SIZE_MESSAGE, TYPE_MESSAGE};

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDRq-png";
    const WEBP: &[u8] = b"RIFF\x10\0\0\0WEBPVP8 q-webp";

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

    fn with_deck(dir: &TestDir) -> (AppState, std::sync::mpsc::Receiver<tracker::TrackerEvent>) {
        let (state, events) = ready_state(dir);
        let conn = store::open_writer(&state.db_path).expect("open");
        conn.execute_batch(
            "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source)
             VALUES (5, 'q-cover', 'Qcover', 'CMODERN', 1, 1, 'file');",
        )
        .expect("deck");
        (state, events)
    }

    fn stored_file(state: &AppState) -> Option<String> {
        deck_by_id(&lock(&state.reader), 5)
            .expect("read")
            .expect("deck")
            .cover_file
    }

    #[test]
    fn only_v1s_cover_names_are_taken() {
        for name in [
            "5-1700.png",
            "12-1700000000000.jpeg",
            "5-2.jpg",
            "5-3.webp",
            "5-4.gif",
        ] {
            assert!(is_cover_name(name), "{name}");
        }
        for name in [
            "-1.png",
            "5-.png",
            "5-1.PNG",
            "5-1.svg",
            "5-1",
            "5-1.png.png",
            "5a-1.png",
            "5-1a.png",
            "5-1-2.png",
            "../5-1.png",
            "5-1.png/..",
            "background-1.png",
            "",
        ] {
            assert!(!is_cover_name(name), "{name}");
        }
    }

    #[test]
    fn a_chosen_image_is_copied_checked_and_named_for_the_deck() {
        let dir = TestDir::new("deck-cover-store");
        let target = dir.join("deck-covers");
        let source = dir.write("pick/Qart.PNG", PNG);
        assert_eq!(
            store_cover(&target, &source, 5, 1_700),
            Ok("5-1700.png".to_string())
        );
        assert_eq!(std::fs::read(target.join("5-1700.png")).expect("copy"), PNG);
        assert!(source.exists(), "the chosen file is copied, never moved");

        let text = dir.write("pick/bad.txt", b"q plain text");
        assert_eq!(
            store_cover(&target, &text, 5, 1_701),
            Err(BackgroundError::Refused(TYPE_MESSAGE))
        );
        let fake = dir.write("pick/fake.png", b"q not a png");
        assert_eq!(
            store_cover(&target, &fake, 5, 1_701),
            Err(BackgroundError::Refused(TYPE_MESSAGE))
        );
        let mut big = PNG.to_vec();
        big.resize(crate::overlay::background::MAX_BYTES as usize + 1, 0);
        let over = dir.write("pick/big.png", &big);
        assert_eq!(
            store_cover(&target, &over, 5, 1_701),
            Err(BackgroundError::Refused(SIZE_MESSAGE))
        );
        assert_eq!(files_in(&target), vec!["5-1700.png".to_string()]);
    }

    #[test]
    fn replacing_deletes_the_old_file_and_a_failed_save_keeps_it() {
        let dir = TestDir::new("deck-cover-replace");
        let target = dir.join("deck-covers");
        let mut save = |_: Option<&str>| Ok(());
        let first =
            replace_cover(&target, &dir.write("a.png", PNG), 5, None, 1, &mut save).expect("first");
        let second = replace_cover(
            &target,
            &dir.write("b.webp", WEBP),
            5,
            Some(&first),
            2,
            &mut save,
        )
        .expect("second");
        assert_eq!(second, "5-2.webp");
        assert_eq!(files_in(&target), vec![second.clone()]);

        let mut failing = |_: Option<&str>| Err(CommandError::Tracker("busy".to_string()));
        let third = replace_cover(
            &target,
            &dir.write("c.png", PNG),
            5,
            Some(&second),
            3,
            &mut failing,
        );
        assert_eq!(third, Err(CommandError::Tracker("busy".to_string())));
        assert_eq!(files_in(&target), vec![second], "the previous cover stays");
    }

    #[test]
    fn choosing_and_removing_a_cover_go_through_the_stored_deck() {
        let dir = TestDir::new("deck-cover-edit");
        let (state, _events) = with_deck(&dir);
        let covers = folder(&state.data_dir);

        set_cover_file(&state, 5, &dir.write("pick/one.png", PNG), 1_000).expect("one");
        assert_eq!(stored_file(&state).as_deref(), Some("5-1000.png"));
        set_cover_file(&state, 5, &dir.write("pick/two.webp", WEBP), 2_000).expect("two");
        assert_eq!(stored_file(&state).as_deref(), Some("5-2000.webp"));
        assert_eq!(files_in(&covers), vec!["5-2000.webp".to_string()]);

        assert_eq!(
            set_cover_file(&state, 5, &dir.write("pick/bad.txt", b"q"), 3_000),
            Err(CommandError::Refused(TYPE_MESSAGE.to_string()))
        );
        assert_eq!(
            set_cover_file(&state, 99, &dir.write("pick/three.png", PNG), 3_000),
            Err(CommandError::Refused(
                "This deck no longer exists.".to_string()
            ))
        );
        assert_eq!(stored_file(&state).as_deref(), Some("5-2000.webp"));

        clear_cover_file(&state, 5).expect("removed");
        assert_eq!(stored_file(&state), None);
        assert!(files_in(&covers).is_empty());
        clear_cover_file(&state, 5).expect("removing nothing is fine");
        crate::commands::stop_tracker(&state);
    }

    #[test]
    fn an_image_sent_as_bytes_is_checked_stored_and_unstaged() {
        let dir = TestDir::new("deck-cover-bytes");
        let (state, _events) = with_deck(&dir);
        let scratch = dir.join("scratch");

        set_cover_bytes(&state, 5, PNG, "PNG", &scratch, 1_000).expect("stored");
        assert_eq!(stored_file(&state).as_deref(), Some("5-1000.png"));
        assert!(files_in(&scratch).is_empty(), "the scratch file is deleted");

        assert_eq!(
            set_cover_bytes(&state, 5, WEBP, "png", &scratch, 2_000),
            Err(CommandError::Refused(TYPE_MESSAGE.to_string())),
            "the bytes must be the type the extension names"
        );
        assert_eq!(
            set_cover_bytes(&state, 5, PNG, "../png", &scratch, 3_000),
            Err(CommandError::Refused(TYPE_MESSAGE.to_string()))
        );
        let huge = vec![0u8; MAX_BYTES as usize + 1];
        assert_eq!(
            set_cover_bytes(&state, 5, &huge, "png", &scratch, 4_000),
            Err(CommandError::Refused(SIZE_MESSAGE.to_string()))
        );
        assert!(files_in(&scratch).is_empty());
        assert_eq!(stored_file(&state).as_deref(), Some("5-1000.png"));
        crate::commands::stop_tracker(&state);
    }

    #[test]
    fn an_archived_deck_refuses_a_cover_and_keeps_no_copy() {
        let dir = TestDir::new("deck-cover-archived");
        let (state, _events) = with_deck(&dir);
        crate::decks::archive(&state, 5, "DELETE", 10).expect("archived");
        assert_eq!(
            set_cover_file(&state, 5, &dir.write("pick/one.png", PNG), 1_000),
            Err(CommandError::Refused(
                store::decks::ARCHIVED_DECK.to_string()
            ))
        );
        assert!(files_in(&folder(&state.data_dir)).is_empty());
        crate::commands::stop_tracker(&state);
    }

    #[test]
    fn the_screenshot_reads_a_cover_as_a_data_url() {
        let dir = TestDir::new("deck-cover-data-url");
        let target = dir.join("deck-covers");
        std::fs::create_dir_all(&target).expect("folder");
        std::fs::write(target.join("5-1.png"), PNG).expect("file");
        let url = data_url(&target, "5-1.png").expect("url");
        assert!(url.starts_with("data:image/png;base64,"), "{url}");
        assert_eq!(data_url(&target, "5-2.png"), None, "a missing file");
        assert_eq!(data_url(&target, "../5-1.png"), None);
    }

    #[test]
    fn the_scheme_serves_a_stored_cover_with_its_type() {
        let dir = TestDir::new("deck-cover-scheme-serves");
        let target = dir.join("deck-covers");
        std::fs::create_dir_all(&target).expect("folder");
        std::fs::write(target.join("5-7.webp"), WEBP).expect("file");
        let response = respond(Some(&target), "/5-7.webp");
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
        let dir = TestDir::new("deck-cover-scheme-refuses");
        let target = dir.join("deck-covers");
        std::fs::create_dir_all(&target).expect("folder");
        std::fs::write(target.join("5-7.png"), PNG).expect("file");
        std::fs::write(target.join("notes.png"), PNG).expect("not v1's name");
        std::fs::write(dir.join("6-1.png"), PNG).expect("outside");
        std::fs::create_dir_all(target.join("5-8.png")).expect("a folder");
        let not_found = |path: &str| respond(Some(&target), path).status() == StatusCode::NOT_FOUND;
        for path in [
            "/5-9.png",
            "/notes.png",
            "/../6-1.png",
            "/5-8.png",
            "//5-7.png",
            "/5-7.png/..",
            "/%35-7.png",
            "5-7.png",
            "/",
            "",
        ] {
            assert!(not_found(path), "{path}");
        }
        assert_eq!(respond(None, "/5-7.png").status(), StatusCode::NOT_FOUND);
    }
}
