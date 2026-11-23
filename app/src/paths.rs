//! The Storage page's File Paths statuses (release spec section 4, survey
//! 2.2): 0.x's `ValidatePath`. The counts walk the chosen folders, so the
//! command runs them on a blocking thread, never on the IPC thread.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

use crate::commands::{CommandError, Startup, off_async_workers, read_settings, ready};
use crate::logging::Timer;

/// `mtgo.log` files are counted this many folders deep at most (0.x
/// `depth('< 8')`: a file in the chosen folder itself is at depth 0).
pub const LOG_DEPTH_LIMIT: usize = 8;
const LOG_FILE_NAME: &str = "mtgo.log";
const GAME_LOG_NAME_PART: &str = "Match_GameLog";

/// Why a network or device path is refused (the user reads it).
pub const REMOTE_PATH_REFUSED: &str =
    "Network and device paths are not supported. Choose a folder on this computer.";

/// A path that starts with two slashes or backslashes, in any mix: a UNC
/// share (`\\host\share`, `//host/share`) or a Windows device path
/// (`\\?\C:\x`, `\\.\pipe\x`). Windows reaches a share by signing in to
/// it, which sends the user's NTLM hash to that host, so such paths are never
/// opened. Leading spaces are ignored, as the settings trim them.
pub fn is_remote_or_device_path(text: &str) -> bool {
    let mut characters = text.trim_start().chars();
    let is_separator = |character: Option<char>| matches!(character, Some('\\') | Some('/'));
    is_separator(characters.next()) && is_separator(characters.next())
}

/// Refuse a network or device path with a message for the user.
pub fn refuse_remote_path(text: &str) -> Result<(), CommandError> {
    if is_remote_or_device_path(text) {
        return Err(CommandError::Refused(REMOTE_PATH_REFUSED.to_string()));
    }
    Ok(())
}

/// One chosen folder's status line (0.x `PathStatus`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PathStatus {
    pub valid: bool,
    pub file_count: u32,
    pub message: String,
}

/// The File Paths card's statuses. `None` is a folder left on automatic:
/// the card shows v1's own status for it (folders searched, logs found).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct FolderStatuses {
    pub log: Option<PathStatus>,
    pub data: Option<PathStatus>,
}

impl FolderStatuses {
    /// Count the chosen folders (walks them: call on a blocking thread).
    pub fn for_folders(log_folder: Option<String>, data_folder: Option<String>) -> Self {
        Self {
            log: log_folder.map(|folder| log_folder_status(&folder)),
            data: data_folder.map(|folder| data_folder_status(&folder)),
        }
    }
}

/// 0.x `ValidatePath::forLogs`: `mtgo.log` files below depth 8.
pub fn log_folder_status(path: &str) -> PathStatus {
    folder_status(
        path,
        Some(LOG_DEPTH_LIMIT),
        is_log_file,
        "log",
        LOG_FILE_NAME,
    )
}

/// 0.x `ValidatePath::forData`: files whose name contains `Match_GameLog`,
/// at any depth.
pub fn data_folder_status(path: &str) -> PathStatus {
    folder_status(path, None, is_game_log_file, "game log", GAME_LOG_NAME_PART)
}

fn is_log_file(name: &str) -> bool {
    // The rule of `mtgo_log::discover`: MTGO under Wine may write `MTGO.log`.
    name.eq_ignore_ascii_case(LOG_FILE_NAME)
}

fn is_game_log_file(name: &str) -> bool {
    name.contains(GAME_LOG_NAME_PART)
}

/// The status of one folder: 0.x's messages, word for word.
fn folder_status(
    path: &str,
    depth_limit: Option<usize>,
    wanted: fn(&str) -> bool,
    noun: &str,
    none_name: &str,
) -> PathStatus {
    if is_remote_or_device_path(path) {
        return PathStatus {
            valid: false,
            file_count: 0,
            message: REMOTE_PATH_REFUSED.to_string(),
        };
    }
    if path.is_empty() || !Path::new(path).is_dir() {
        let message = if path.is_empty() {
            String::new()
        } else {
            "Directory not found.".to_string()
        };
        return PathStatus {
            valid: false,
            file_count: 0,
            message,
        };
    }
    let count = count_files(Path::new(path), depth_limit, wanted);
    let message = if count > 0 {
        let files = if count == 1 { "file" } else { "files" };
        format!("Found {count} {noun} {files}.")
    } else {
        format!("No {none_name} files found in this directory.")
    };
    PathStatus {
        valid: count > 0,
        file_count: count,
        message,
    }
}

/// Count the files below `root` whose name passes `wanted`, at most
/// `depth_limit` folders deep (exclusive). Unreadable folders are skipped
/// (0.x `ignoreUnreadableDirs`); folder links are not followed, so a loop
/// cannot run forever.
fn count_files(root: &Path, depth_limit: Option<usize>, wanted: fn(&str) -> bool) -> u32 {
    let mut count: u32 = 0;
    let mut folders: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((folder, depth)) = folders.pop() {
        if depth_limit.is_some_and(|limit| depth >= limit) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                folders.push((entry.path(), depth + 1));
                continue;
            }
            // A link to a file counts as the file (0.x's finder lists it).
            let is_file = kind.is_file() || (kind.is_symlink() && entry.path().is_file());
            if is_file && wanted(&entry.file_name().to_string_lossy()) {
                count = count.saturating_add(1);
            }
        }
    }
    count
}

/// The chosen folders' statuses. The counts walk the folders, so they run
/// on a blocking thread.
#[tauri::command]
#[specta::specta]
pub async fn folder_statuses(app: AppHandle) -> Result<FolderStatuses, CommandError> {
    let _timer = Timer::start("folder_statuses");
    let settings = {
        let startup = app.state::<Startup>();
        read_settings(ready(&startup)?)?
    };
    off_async_workers(move || {
        Ok(FolderStatuses::for_folders(
            settings.log_folder,
            settings.mtgo_data_folder,
        ))
    })
    .await
}

/// The File Paths card's "Browse" (0.x `BrowseFolderController`: a folder
/// dialog titled "Select folder", opened at `start` when it is a local
/// folder).
/// Async: a blocking dialog must not run on the main thread.
#[tauri::command]
#[specta::specta]
pub async fn choose_folder(app: AppHandle, start: Option<String>) -> Option<String> {
    let _timer = Timer::start("choose_folder");
    let mut dialog = app.dialog().file().set_title("Select folder");
    // A network start is never probed: the dialog opens at its default.
    if let Some(folder) =
        start.filter(|path| !is_remote_or_device_path(path) && Path::new(path).is_dir())
    {
        dialog = dialog.set_directory(folder);
    }
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    let picked = dialog.blocking_pick_folder()?;
    let path = picked.into_path().ok()?;
    Some(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fixture_tools::TestDir;

    fn folder(dir: &TestDir) -> String {
        dir.path().to_string_lossy().into_owned()
    }

    #[test]
    fn log_folder_status_counts_logs_below_depth_8() {
        let dir = TestDir::new("paths-log-depth");
        // Depth counts folders below the chosen one, as Symfony Finder does:
        // a file in the folder itself is at depth 0.
        dir.write("mtgo.log", b"x");
        dir.write("1/2/3/4/5/6/7/mtgo.log", b"x");
        dir.write("1/2/3/4/5/6/7/8/mtgo.log", b"x");
        dir.write("a/MTGO.log", b"x");
        dir.write("a/mtgo.log.bak", b"x");
        let status = log_folder_status(&folder(&dir));
        assert_eq!(
            status,
            PathStatus {
                valid: true,
                file_count: 3,
                message: "Found 3 log files.".to_string(),
            }
        );
    }

    #[test]
    fn log_folder_status_messages_are_0x_verbatim() {
        let one = TestDir::new("paths-log-one");
        one.write("x/mtgo.log", b"x");
        assert_eq!(
            log_folder_status(&folder(&one)).message,
            "Found 1 log file."
        );

        let none = TestDir::new("paths-log-none");
        none.write("x/other.log", b"x");
        let status = log_folder_status(&folder(&none));
        assert!(!status.valid);
        assert_eq!(status.file_count, 0);
        assert_eq!(status.message, "No mtgo.log files found in this directory.");

        let missing = log_folder_status(&none.join("nowhere").to_string_lossy());
        assert_eq!(
            missing,
            PathStatus {
                valid: false,
                file_count: 0,
                message: "Directory not found.".to_string(),
            }
        );
        // A file is not a directory.
        let file = none.join("x/other.log");
        assert_eq!(
            log_folder_status(&file.to_string_lossy()).message,
            "Directory not found."
        );
        assert_eq!(log_folder_status("").message, "", "0.x: empty says nothing");
    }

    #[test]
    fn data_folder_status_counts_match_gamelog_files() {
        let dir = TestDir::new("paths-data");
        dir.write("Match_GameLog_100.dat", b"x");
        dir.write("a/b/c/d/e/f/g/h/i/Match_GameLog_101.dat", b"x");
        dir.write("a/QMatch_GameLog", b"x");
        dir.write("a/match_gamelog_102.dat", b"x");
        dir.write("a/deck.xml", b"x");
        let status = data_folder_status(&folder(&dir));
        assert_eq!(
            status,
            PathStatus {
                valid: true,
                file_count: 3,
                message: "Found 3 game log files.".to_string(),
            }
        );
    }

    #[test]
    fn data_folder_status_messages_are_0x_verbatim() {
        let one = TestDir::new("paths-data-one");
        one.write("Match_GameLog_1.dat", b"x");
        assert_eq!(
            data_folder_status(&folder(&one)).message,
            "Found 1 game log file."
        );

        let none = TestDir::new("paths-data-none");
        none.write("deck.xml", b"x");
        let status = data_folder_status(&folder(&none));
        assert!(!status.valid);
        assert_eq!(
            status.message,
            "No Match_GameLog files found in this directory."
        );
        assert_eq!(
            data_folder_status(&none.join("nowhere").to_string_lossy()).message,
            "Directory not found."
        );
        assert_eq!(data_folder_status("").message, "");
    }

    #[test]
    fn remote_and_device_paths_are_recognised() {
        for remote in [
            r"\\host\share",
            r"\\host\share\logs",
            "//host/share",
            r"\\?\C:\x",
            r"\\.\pipe\x",
            r"\\?\UNC\host\share",
            r"/\host\share",
            r"\/host/share",
            r"  \\host\share",
        ] {
            assert!(is_remote_or_device_path(remote), "{remote}");
        }
        for local in [
            r"C:\ok",
            "C:/ok",
            "/home/ok",
            "relative/ok",
            r"\ok",
            "/ok",
            "",
        ] {
            assert!(!is_remote_or_device_path(local), "{local}");
        }
    }

    #[test]
    fn refuse_remote_path_explains_itself() {
        assert_eq!(refuse_remote_path("/home/ok"), Ok(()));
        assert_eq!(
            refuse_remote_path(r"\\host\share"),
            Err(CommandError::Refused(REMOTE_PATH_REFUSED.to_string()))
        );
    }

    #[test]
    fn a_remote_folder_is_not_walked() {
        let status = log_folder_status(r"\\host\share");
        assert_eq!(
            status,
            PathStatus {
                valid: false,
                file_count: 0,
                message: REMOTE_PATH_REFUSED.to_string(),
            }
        );
        assert_eq!(
            data_folder_status("//host/share").message,
            REMOTE_PATH_REFUSED
        );
    }

    #[test]
    fn automatic_folders_have_no_status() {
        let dir = TestDir::new("paths-statuses");
        dir.write("logs/mtgo.log", b"x");
        let chosen = FolderStatuses::for_folders(
            Some(dir.join("logs").to_string_lossy().into_owned()),
            None,
        );
        assert_eq!(chosen.log.expect("chosen").file_count, 1);
        assert_eq!(chosen.data, None, "automatic: v1's own status shows");
    }
}
