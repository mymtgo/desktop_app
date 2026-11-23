//! Where MTGO keeps its match history (0.x `ParseGameHistory::findFiles`):
//! every file named `mtgo_game_history` under MTGO's data folders. MTGO's
//! ClickOnce install keeps a copy per installation, so there can be several;
//! the caller reads and merges them all. Searched under the same roots as the
//! deck and game log files, without entering linked folders.

use std::path::{Path, PathBuf};

use crate::deck_files::deck_roots;
use crate::discover::Env;
use crate::walk::walk_files;

/// The history file's exact name.
pub const HISTORY_FILE: &str = "mtgo_game_history";

/// 0.x searched at a depth below 10 (Symfony Finder `depth('< 10')`): a
/// file at most nine folders below a root.
pub const HISTORY_MAX_DEPTH: usize = 10;

/// Every history file under the roots (the chosen data folder, else the
/// detected ones), sorted, so a merge never depends on folder listing order.
/// Linked folders are not entered; a linked file counts.
pub fn find_history_files(env: &Env, data_folder: Option<&Path>) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for root in deck_roots(env, data_folder) {
        walk_files(
            &root,
            HISTORY_MAX_DEPTH,
            &|path| path.file_name() == Some(HISTORY_FILE.as_ref()),
            &mut |path, _| found.push(path),
        );
    }
    found.sort();
    found.dedup();
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::Platform;
    use fixture_tools::TestDir;

    const DATA: &str = "AppData/Local/Apps/2.0/Data";

    fn env(platform: Platform, home: &Path) -> Env {
        Env {
            platform,
            home: Some(home.to_path_buf()),
            user: Some("someone".to_string()),
        }
    }

    #[test]
    fn finds_every_copy_under_the_windows_data_folder_sorted() {
        let dir = TestDir::new("history-files-windows");
        let second = dir.write(&format!("{DATA}/Q2/B2/{HISTORY_FILE}"), b"x");
        let first = dir.write(&format!("{DATA}/Q1/A1/acct/{HISTORY_FILE}"), b"x");
        dir.write(&format!("{DATA}/Q1/A1/acct/{HISTORY_FILE}.bak"), b"x");
        dir.write(&format!("{DATA}/Q1/A1/acct/MTGO_GAME_HISTORY_old"), b"x");

        let found = find_history_files(&env(Platform::Windows, dir.path()), None);
        assert_eq!(found, vec![first, second]);
    }

    #[test]
    fn a_wine_prefix_data_folder_is_searched_on_linux() {
        let dir = TestDir::new("history-files-linux");
        let wanted = dir.write(
            &format!(".wine/drive_c/users/someone/{DATA}/Q1/acct/{HISTORY_FILE}"),
            b"x",
        );
        let found = find_history_files(&env(Platform::Linux, dir.path()), None);
        assert_eq!(found, vec![wanted]);
    }

    #[test]
    fn a_chosen_data_folder_replaces_the_detected_ones() {
        let dir = TestDir::new("history-files-override");
        dir.write(&format!("{DATA}/Q1/acct/{HISTORY_FILE}"), b"x");
        let chosen = dir.write(&format!("elsewhere/acct/{HISTORY_FILE}"), b"x");
        let found = find_history_files(
            &env(Platform::Windows, dir.path()),
            Some(&dir.join("elsewhere")),
        );
        assert_eq!(found, vec![chosen]);
    }

    #[test]
    fn a_file_nine_folders_down_is_found_and_ten_is_too_deep() {
        let dir = TestDir::new("history-files-depth");
        let mut nine = String::from("root");
        for level in 0..9 {
            nine.push_str(&format!("/d{level}"));
        }
        let within = dir.write(&format!("{nine}/{HISTORY_FILE}"), b"x");
        dir.write(&format!("{nine}/d9/{HISTORY_FILE}"), b"x");
        let found =
            find_history_files(&env(Platform::Windows, dir.path()), Some(&dir.join("root")));
        assert_eq!(found, vec![within]);
    }

    #[test]
    fn a_folder_with_the_name_is_not_a_file() {
        let dir = TestDir::new("history-files-folder");
        dir.write(&format!("root/{HISTORY_FILE}/inside"), b"x");
        let found =
            find_history_files(&env(Platform::Windows, dir.path()), Some(&dir.join("root")));
        assert!(found.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn linked_folders_are_not_entered_but_linked_files_count() {
        let dir = TestDir::new("history-files-links");
        let outside = dir.write(&format!("outside/{HISTORY_FILE}"), b"x");
        std::fs::create_dir_all(dir.join("root/acct")).expect("root");
        std::os::unix::fs::symlink(dir.join("outside"), dir.join("root/linked")).expect("link");
        let env = env(Platform::Windows, dir.path());
        let chosen = Some(dir.join("root"));
        assert!(find_history_files(&env, chosen.as_deref()).is_empty());

        let link = dir.join(&format!("root/acct/{HISTORY_FILE}"));
        std::os::unix::fs::symlink(&outside, &link).expect("file link");
        assert_eq!(find_history_files(&env, chosen.as_deref()), vec![link]);
    }
}
