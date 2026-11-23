//! Moving the card images folder (cards spec section 7, survey section 4;
//! 0.x `QueueCardImagesMove` and `MoveCardImages`).
//!
//! A chosen folder gets v1's own subfolder (`mymtgo-desktop-card-images`),
//! so v1 never clears or moves anything else in it. The move runs on the
//! cards worker (no download lands mid-move): copy v1's own files (a target
//! of the same size is kept), switch the setting, delete the sources, remove
//! the old custom folder when empty (never the default); on failure the
//! copies made are deleted and "Moving card images failed: {message}" is
//! shown. Only v1's own files (`is_owned_file`) are ever copied or deleted.
//! A copy goes through a `.part` temporary and a rename, so a file already
//! at the target is replaced, never written into; folders that cannot be
//! told apart by a probe file are copied only, never deleted from.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use cards::images::{NOT_ABSOLUTE, OVERLAPS, V1_SUBFOLDER, is_absolute, validate_move};

use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

use crate::bridge::lock;
use crate::cards::images::{ImagesState, listed_files, listed_names};
use crate::cards::worker::{CardsSignal, CardsWorker, STOPPED};
use crate::commands::{AppState, CommandError, off_async_workers};
use crate::settings::CARD_IMAGES_PATH;

/// The prefix of a failed move's message (0.x).
pub const MOVE_FAILED: &str = "Moving card images failed";

/// A move the worker runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveRequest {
    pub from: PathBuf,
    pub to: PathBuf,
    /// The folders the queue made for `to` (deepest first): a move that
    /// fails removes them again when they are empty.
    pub made: Vec<PathBuf>,
}

/// The folder a chosen folder moves to: v1's subfolder inside it (trailing
/// separators trimmed first, as 0.x).
pub fn chosen_target(chosen: &str) -> PathBuf {
    PathBuf::from(trimmed_choice(chosen)).join(V1_SUBFOLDER)
}

/// `chosen` without trailing separators, except that a root stays as given:
/// trimming `E:\` would leave the drive-relative `E:`, and trimming `/` would
/// leave nothing.
pub fn trimmed_choice(chosen: &str) -> &str {
    let trimmed = chosen.trim_end_matches(['/', '\\']);
    let bare_drive = trimmed.len() == 2
        && trimmed.ends_with(':')
        && trimmed
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic());
    if trimmed.is_empty() || bare_drive {
        chosen
    } else {
        trimmed
    }
}

/// Validate a move to `to` (0.x's checks and order) and mark it running.
/// Creates `to` when it does not exist; a refused move removes what it made.
/// A path with `.` or `..` in it is not a full folder path: under a missing
/// parent it could not be resolved for the overlap check.
pub fn queue_move(images: &ImagesState, to: &Path) -> Result<MoveRequest, String> {
    let made: std::cell::RefCell<Vec<PathBuf>> = std::cell::RefCell::new(Vec::new());
    let from = images.begin_move(|from, moving| {
        if !moving && has_dot_components(to) {
            return Err(NOT_ABSOLUTE.to_string());
        }
        // The overlap check reads resolved paths, so the same folder under
        // another spelling (`x/.`, a link, a junction, a mapped drive) is
        // seen as the same folder. A relative path stays as typed, so it is
        // refused as one.
        let to_text = to.to_string_lossy();
        let to_checked = if is_absolute(&to_text) {
            resolved(to).to_string_lossy().into_owned()
        } else {
            to_text.into_owned()
        };
        let checked = validate_move(
            &resolved(from).to_string_lossy(),
            &to_checked,
            moving,
            &|| {
                if to.is_dir() {
                    return true;
                }
                *made.borrow_mut() = missing_folders(to);
                fs::create_dir_all(to).is_ok()
            },
            &|| writable(to),
        );
        if checked.is_err() {
            remove_empty_folders(&made.borrow());
        }
        checked
    })?;
    Ok(MoveRequest {
        from,
        to: to.to_path_buf(),
        made: made.into_inner(),
    })
}

/// Whether `path` has a `.` or `..` component.
fn has_dot_components(path: &Path) -> bool {
    path.components()
        .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
}

/// `path` and each parent of it that does not exist, deepest first.
fn missing_folders(path: &Path) -> Vec<PathBuf> {
    let mut missing = Vec::new();
    let mut current = Some(path);
    while let Some(folder) = current {
        if folder.as_os_str().is_empty() || fs::symlink_metadata(folder).is_ok() {
            break;
        }
        missing.push(folder.to_path_buf());
        current = folder.parent();
    }
    missing
}

/// Remove each folder (deepest first) when empty; one with anything in it
/// stays, and nothing inside a folder is ever deleted.
fn remove_empty_folders(folders: &[PathBuf]) {
    for folder in folders {
        let _ = fs::remove_dir(folder);
    }
}

/// `path` with links, `.` and `..` resolved: canonical when it exists, else
/// its nearest existing parent canonical with the rest appended.
pub fn resolved(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path.to_path_buf();
    loop {
        if let Ok(base) = fs::canonicalize(&current) {
            let mut full = base;
            for part in rest.iter().rev() {
                if part == ".." {
                    full.pop();
                } else if part != "." {
                    full.push(part);
                }
            }
            return full;
        }
        let Some(name) = current.file_name().map(|name| name.to_os_string()) else {
            return path.to_path_buf();
        };
        rest.push(name);
        if !current.pop() {
            return path.to_path_buf();
        }
    }
}

/// Whether a file can be written in `folder` (a probe file, removed at once).
fn writable(folder: &Path) -> bool {
    let probe = folder.join(".mymtgo-write-check");
    let written = fs::write(&probe, b"").is_ok();
    let _ = fs::remove_file(&probe);
    written
}

/// Copy, switch, delete, tidy (see the module docs). `switch` saves the
/// setting and points the app at `to`. The folders are probed before the
/// copy and again right before the deletes: one folder under two names
/// refuses the move (before the copy) or deletes nothing (after it), and a
/// folder that cannot be probed makes the move copy only.
pub fn move_images(
    request: &MoveRequest,
    default: &Path,
    switch: &dyn Fn(&Path) -> Result<(), String>,
) -> Result<(), String> {
    move_images_with(request, default, switch, &probe_folders)
}

/// `move_images` with the probe given (`probe_folders` in the app; tests
/// give their own, so the copy-only and re-probe paths run on every OS).
pub fn move_images_with(
    request: &MoveRequest,
    default: &Path,
    switch: &dyn Fn(&Path) -> Result<(), String>,
    probe: &dyn Fn(&Path, &Path) -> Probe,
) -> Result<(), String> {
    let mut created = Vec::new();
    let copied = fs::create_dir_all(&request.to)
        .map_err(|error| error.to_string())
        .and_then(|()| match probe(&request.from, &request.to) {
            Probe::Alias => Err(OVERLAPS.to_string()),
            Probe::Separate | Probe::Unknown => Ok(()),
        })
        .and_then(|()| copy_all(&request.from, &request.to, &mut created))
        .and_then(|sources| switch(&request.to).map(|()| sources));
    let sources = match copied {
        Ok(sources) => sources,
        Err(message) => {
            for path in &created {
                let _ = fs::remove_file(path);
            }
            remove_empty_folders(&request.made);
            return Err(message);
        }
    };
    let again = probe(&request.from, &request.to);
    if again != Probe::Separate {
        tracing::warn!(
            probe = ?again,
            from = %request.from.display(),
            "kept the old card images: the folders could not be told apart"
        );
        return Ok(());
    }
    for source in &sources {
        if !safe_to_delete(source, &request.to) {
            tracing::warn!(path = %source.display(), "kept a card image whose copy is not verified");
            continue;
        }
        if let Err(error) = fs::remove_file(source) {
            tracing::warn!(%error, path = %source.display(), "could not delete a moved card image");
        }
    }
    if resolved(&request.from) != resolved(&request.to) {
        remove_old_folder(&request.from, default);
    }
    Ok(())
}

/// What probing two folders found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// Two folders: the sources may be deleted (each still checked).
    Separate,
    /// One folder under two names: nothing is deleted.
    Alias,
    /// A probe file could not be written or looked for in one of the
    /// folders: the move copies only and deletes nothing.
    Unknown,
}

/// Whether `to` is the folder `from` under another name (a bind mount, two
/// mounts of one share, one share by two host names: spellings a canonical
/// path cannot join). A uniquely named probe file is written into one
/// folder and looked for in the other, both ways (a network share's cache
/// can hide one direction). A `from` that is gone has nothing to move.
pub fn probe_folders(from: &Path, to: &Path) -> Probe {
    if !from.is_dir() {
        return Probe::Separate;
    }
    let mut found = Probe::Separate;
    for (write_in, look_in) in [(from, to), (to, from)] {
        match probe_once(write_in, look_in) {
            Probe::Separate => {}
            Probe::Alias => return Probe::Alias,
            Probe::Unknown => found = Probe::Unknown,
        }
    }
    found
}

/// One probe: written in `write_in`, looked for in `look_in`, then removed.
fn probe_once(write_in: &Path, look_in: &Path) -> Probe {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let name = format!(".mymtgo-move-probe-{}-{nanos}", std::process::id());
    let probe = write_in.join(&name);
    if fs::write(&probe, b"").is_err() {
        return Probe::Unknown;
    }
    let seen = look_in.join(&name).try_exists();
    let _ = fs::remove_file(&probe);
    match seen {
        Ok(true) => Probe::Alias,
        Ok(false) => Probe::Separate,
        Err(_) => Probe::Unknown,
    }
}

/// A source is deleted only when its copy exists in `to`, has the same
/// size and is another file (not the source under another name).
fn safe_to_delete(source: &Path, to: &Path) -> bool {
    let Some(name) = source.file_name() else {
        return false;
    };
    let target = to.join(name);
    let (Ok(source_meta), Ok(target_meta)) = (fs::metadata(source), fs::metadata(&target)) else {
        return false;
    };
    if !target_meta.is_file() || target_meta.len() != source_meta.len() {
        return false;
    }
    if same_file(&source_meta, &target_meta) {
        return false;
    }
    match (fs::canonicalize(source), fs::canonicalize(&target)) {
        (Ok(source), Ok(target)) => source != target,
        _ => false,
    }
}

/// Whether two files are one file on disk (device and inode), which no path
/// spelling can hide. Other platforms rely on `probe_folders`.
#[cfg(unix)]
fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

#[cfg(not(unix))]
fn same_file(_a: &fs::Metadata, _b: &fs::Metadata) -> bool {
    false
}

/// Copy v1's own files from `from` to `to`, keeping a target of the same
/// size. Returns the sources that now exist at the target; `created` gets
/// each file this run wrote where none was, for cleanup on failure. A
/// `from` that is gone has nothing to move; one that cannot be read fails
/// the move (it would otherwise "move" nothing and forget every file). A target entry with an
/// owned name that is not a plain file (a link) fails the move: it is never
/// written through or replaced.
fn copy_all(from: &Path, to: &Path, created: &mut Vec<PathBuf>) -> Result<Vec<PathBuf>, String> {
    let listed = match from.try_exists() {
        Ok(false) => Vec::new(),
        Ok(true) => listed_files(from)?,
        Err(error) => return Err(format!("could not read {}: {error}", from.display())),
    };
    let mut sources = Vec::new();
    for (source, size) in listed {
        let Some(name) = source.file_name() else {
            continue;
        };
        let target = to.join(name);
        let shown = name.to_string_lossy();
        let (kept, existed) = match fs::symlink_metadata(&target) {
            Ok(meta) if !meta.is_file() => return Err(format!("could not copy {shown}")),
            Ok(meta) => (meta.len() == size, true),
            Err(_) => (false, false),
        };
        if !kept {
            let copied = copy_through_temporary(&source, &target)
                .map_err(|_| format!("could not copy {shown}"))?;
            // A file that was there is replaced, never removed on failure.
            if !existed {
                created.push(target);
            }
            if copied != size {
                return Err(format!("could not copy {shown}"));
            }
        }
        sources.push(source);
    }
    Ok(sources)
}

/// Copy `source` to `<target>.part`, flush it and rename it over `target`,
/// so a file already at `target` (perhaps a hard link of a file elsewhere)
/// is replaced, never written into. The bytes copied.
fn copy_through_temporary(source: &Path, target: &Path) -> Result<u64, String> {
    let mut name = target
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".part");
    let temporary = target.with_file_name(name);
    let mut reader = fs::File::open(source).map_err(|error| error.to_string())?;
    let mut writer = crate::cards::images::create_temporary(&temporary)?;
    let written = std::io::copy(&mut reader, &mut writer).and_then(|copied| {
        writer.sync_all()?;
        Ok(copied)
    });
    drop(writer);
    let outcome = written
        .map_err(|error| error.to_string())
        .and_then(|copied| {
            fs::rename(&temporary, target)
                .map(|()| copied)
                .map_err(|error| error.to_string())
        });
    if outcome.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    outcome
}

/// A custom folder is v1's to remove once empty; the default folder stays.
fn remove_old_folder(from: &Path, default: &Path) {
    if from == default || !from.is_dir() {
        return;
    }
    let empty = fs::read_dir(from).is_ok_and(|mut entries| entries.next().is_none());
    if empty && let Err(error) = fs::remove_dir(from) {
        tracing::warn!(%error, "could not remove the old card images folder");
    }
}

/// The app's switch: save `card_images_path` (none for the default folder)
/// through the tracker, then point the scheme and the worker at `to`.
pub fn switch_folder(state: &AppState, images: &ImagesState, to: &Path) -> Result<(), String> {
    let stored = (to != images.default_folder()).then(|| to.to_string_lossy().into_owned());
    let value = serde_json::to_string(&stored).map_err(|error| error.to_string())?;
    {
        let _one_save = lock(&state.settings_lock);
        state.save_setting_rows(vec![(CARD_IMAGES_PATH.to_string(), value)])?;
    }
    images.switch_to(to);
    Ok(())
}

/// Run a queued move and record how it ended. After a move, `present` is
/// given the files now in the new folder (`CardImagesPresent`).
pub fn run_move(
    images: &ImagesState,
    request: &MoveRequest,
    switch: &dyn Fn(&Path) -> Result<(), String>,
    present: &dyn Fn(BTreeSet<String>) -> Result<u32, String>,
) {
    let result = move_images(request, &images.default_folder(), switch);
    let error = match result {
        Ok(()) => {
            tracing::info!(to = %request.to.display(), "card images moved");
            // A row naming a file the move did not carry (a gone folder, a
            // missing file) is downloaded again.
            // A folder that cannot be listed forgets nothing; the next
            // download run does the check again.
            match listed_names(&request.to) {
                Ok(files) => {
                    if let Err(error) = present(files) {
                        tracing::warn!(%error, "could not forget the card images the move did not carry");
                    }
                }
                Err(error) => tracing::warn!(%error, "skipped the card images presence update"),
            }
            None
        }
        Err(message) => {
            tracing::warn!(
                from = %request.from.display(),
                to = %request.to.display(),
                %message,
                "the card images move failed; images stay in the old folder"
            );
            Some(format!("{MOVE_FAILED}: {message}"))
        }
    };
    images.end_move(error);
}

/// Queue a move to `to` and hand it to the worker. A worker that is not
/// running ends the move at once with its error.
fn start_move(app: &AppHandle, to: &Path) -> Result<(), CommandError> {
    let images = app
        .try_state::<ImagesState>()
        .ok_or_else(|| CommandError::Refused("the images folder is not set".to_string()))?;
    let request = queue_move(&images, to).map_err(CommandError::Refused)?;
    let sent = app
        .try_state::<CardsWorker>()
        .is_some_and(|worker| worker.signal(CardsSignal::Move(request)));
    if !sent {
        images.end_move(Some(format!("{MOVE_FAILED}: {STOPPED}")));
    }
    Ok(())
}

/// "Change": move the images into v1's subfolder of the chosen folder.
#[tauri::command]
#[specta::specta]
pub async fn move_card_images(app: AppHandle, chosen: String) -> Result<(), CommandError> {
    crate::paths::refuse_remote_path(&chosen)?;
    off_async_workers(move || start_move(&app, &chosen_target(&chosen))).await
}

/// "Reset to default": move the images back to the default folder.
#[tauri::command]
#[specta::specta]
pub async fn reset_card_images_path(app: AppHandle) -> Result<(), CommandError> {
    off_async_workers(move || {
        let default = app
            .try_state::<ImagesState>()
            .map(|images| images.default_folder())
            .ok_or_else(|| CommandError::Refused("the images folder is not set".to_string()))?;
        start_move(&app, &default)
    })
    .await
}

/// The folder picker for "Change". Async: a blocking dialog must not run on
/// the main thread.
#[tauri::command]
#[specta::specta]
pub async fn choose_card_images_folder(app: AppHandle) -> Option<String> {
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Choose a folder for card images");
    if let Some(images) = app.try_state::<ImagesState>() {
        dialog = dialog.set_directory(images.folder().path);
    }
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    let picked = dialog.blocking_pick_folder()?;
    let path = picked.into_path().ok()?;
    Some(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests;
