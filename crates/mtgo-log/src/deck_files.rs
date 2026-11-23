//! Where MTGO keeps the active account's deck files (decks spec 3; 0.x
//! `GetDeckFiles`). MTGO's per-user data sits under `AppData\Local\Apps\2.0\Data`
//! beside the ClickOnce folder that holds the log, one folder per account; the
//! account that logged in last has the newest `user_settings` file. Its deck
//! files are the `grouping <uuid>.xml` files directly in that folder.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::discover::{Env, candidate_roots};
use crate::walk::walk_files;

/// The file whose folder is an account's data folder.
pub const SETTINGS_FILE: &str = "user_settings";

/// Depth limit when searching a data folder for `SETTINGS_FILE`. 0.x searches
/// without a limit; MTGO's account folders sit about seven folders down, so
/// this leaves room while never wandering far.
pub const DECK_MAX_DEPTH: usize = 12;

/// The folders searched for account folders: the chosen data folder, or the
/// `Data` folder beside each ClickOnce folder the log search uses (the Windows
/// home, or the Wine and Proton prefixes on Linux).
pub fn deck_roots(env: &Env, data_folder: Option<&Path>) -> Vec<PathBuf> {
    if let Some(folder) = data_folder {
        return vec![folder.to_path_buf()];
    }
    candidate_roots(env, &[])
        .into_iter()
        .map(|root| root.join("Data"))
        .collect()
}

/// The active account's folder: where the most recently written
/// `user_settings` file under the roots sits. `None` when there is none.
pub fn deck_folder(env: &Env, data_folder: Option<&Path>) -> Option<PathBuf> {
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    for root in deck_roots(env, data_folder) {
        walk_files(
            &root,
            DECK_MAX_DEPTH,
            &|path| path.file_name() == Some(SETTINGS_FILE.as_ref()),
            &mut |path, meta| {
                // A settings file without a modified time is skipped.
                let Ok(modified) = meta.modified() else {
                    return;
                };
                if is_newer(modified, &path, newest.as_ref()) {
                    newest = Some((modified, path));
                }
            },
        );
    }
    let (_, settings) = newest?;
    settings.parent().map(Path::to_path_buf)
}

/// The deck files directly in `folder` (`grouping <uuid>.xml`, any case),
/// sorted. Empty when the folder cannot be read.
pub fn deck_files(folder: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_deck_name = entry
            .file_name()
            .to_str()
            .is_some_and(::decks::is_deck_file_name);
        if is_deck_name && fs::metadata(&path).is_ok_and(|meta| meta.is_file()) {
            found.push(path);
        }
    }
    found.sort();
    found
}

/// Newer wins; on the same time the lexically smaller path wins, so the choice
/// never depends on folder listing order.
fn is_newer(modified: SystemTime, path: &Path, kept: Option<&(SystemTime, PathBuf)>) -> bool {
    match kept {
        None => true,
        Some((kept_time, kept_path)) => {
            modified > *kept_time || (modified == *kept_time && path < kept_path.as_path())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::{Env, Platform};
    use fixture_tools::TestDir;
    use std::fs::File;
    use std::time::{Duration, SystemTime};

    const DATA: &str = "AppData/Local/Apps/2.0/Data";
    const DECK_A: &str = "grouping 0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d.xml";
    const DECK_B: &str = "Grouping 1A2B3C4D-5E6F-4A7B-8C9D-0E1F2A3B4C5D.XML";

    fn env(platform: Platform, home: &Path) -> Env {
        Env {
            platform,
            home: Some(home.to_path_buf()),
            user: Some("someone".to_string()),
        }
    }

    /// Write `relative` and date its last write `minutes_ago` minutes back.
    fn write_dated(dir: &TestDir, relative: &str, minutes_ago: u64) -> PathBuf {
        let path = dir.write(relative, b"x");
        let at = SystemTime::now() - Duration::from_secs(minutes_ago * 60);
        File::options()
            .write(true)
            .open(&path)
            .expect("open")
            .set_modified(at)
            .expect("set modified");
        path
    }

    #[test]
    fn the_newest_user_settings_picks_the_windows_account_folder() {
        let dir = TestDir::new("deck-folder-windows");
        write_dated(&dir, &format!("{DATA}/Q1/A1/old/user_settings"), 120);
        let newest = write_dated(&dir, &format!("{DATA}/Q2/B2/new/user_settings"), 10);
        write_dated(&dir, &format!("{DATA}/Q3/C3/user_settings.bak"), 1);

        let folder = deck_folder(&env(Platform::Windows, dir.path()), None);
        assert_eq!(folder.as_deref(), newest.parent());
    }

    #[test]
    fn a_wine_prefix_data_folder_is_searched_on_linux() {
        let dir = TestDir::new("deck-folder-linux");
        let settings = write_dated(
            &dir,
            &format!(".wine/drive_c/users/someone/{DATA}/Q1/A1/acct/user_settings"),
            5,
        );
        let steam = write_dated(
            &dir,
            &format!(
                ".steam/steam/steamapps/compatdata/123/pfx/drive_c/users/steamuser/{DATA}/Q2/acct/user_settings"
            ),
            60,
        );
        let roots = deck_roots(&env(Platform::Linux, dir.path()), None);
        assert!(roots.contains(&dir.join(&format!(".wine/drive_c/users/someone/{DATA}"))));
        assert!(roots.contains(&dir.join(&format!(
            ".steam/steam/steamapps/compatdata/123/pfx/drive_c/users/steamuser/{DATA}"
        ))));

        let folder = deck_folder(&env(Platform::Linux, dir.path()), None);
        assert_eq!(folder.as_deref(), settings.parent());
        assert_ne!(folder.as_deref(), steam.parent());
    }

    #[test]
    fn a_chosen_data_folder_replaces_the_detected_ones() {
        let dir = TestDir::new("deck-folder-override");
        write_dated(&dir, &format!("{DATA}/Q1/acct/user_settings"), 1);
        let chosen = write_dated(&dir, "elsewhere/Q2/acct/user_settings", 30);

        let folder = deck_folder(
            &env(Platform::Windows, dir.path()),
            Some(&dir.join("elsewhere")),
        );
        assert_eq!(folder.as_deref(), chosen.parent());
        assert_eq!(
            deck_roots(
                &env(Platform::Windows, dir.path()),
                Some(&dir.join("elsewhere"))
            ),
            vec![dir.join("elsewhere")]
        );
    }

    #[test]
    fn no_user_settings_means_no_deck_folder() {
        let dir = TestDir::new("deck-folder-none");
        dir.write(&format!("{DATA}/Q1/acct/{DECK_A}"), b"x");
        assert_eq!(deck_folder(&env(Platform::Windows, dir.path()), None), None);
        let nobody = Env {
            platform: Platform::Windows,
            home: None,
            user: None,
        };
        assert_eq!(deck_folder(&nobody, None), None);
        assert_eq!(
            deck_folder(&nobody, Some(&dir.join("missing"))),
            None,
            "a chosen folder that does not exist"
        );
    }

    #[test]
    fn a_user_settings_deeper_than_the_limit_is_not_found() {
        let dir = TestDir::new("deck-folder-depth");
        let mut deep = String::from("root");
        for level in 0..DECK_MAX_DEPTH {
            deep.push_str(&format!("/d{level}"));
        }
        write_dated(&dir, &format!("{deep}/user_settings"), 1);
        assert_eq!(
            deck_folder(&env(Platform::Windows, dir.path()), Some(&dir.join("root"))),
            None
        );

        let within = write_dated(&dir, "root/d0/d1/d2/d3/d4/d5/d6/user_settings", 2);
        assert_eq!(
            deck_folder(&env(Platform::Windows, dir.path()), Some(&dir.join("root"))).as_deref(),
            within.parent()
        );
    }

    #[test]
    fn deck_files_are_the_grouping_files_directly_in_the_folder() {
        let dir = TestDir::new("deck-files");
        let a = dir.write(&format!("acct/{DECK_A}"), b"x");
        let b = dir.write(&format!("acct/{DECK_B}"), b"x");
        dir.write("acct/user_settings", b"x");
        dir.write("acct/grouping 1234.xml", b"x");
        dir.write(&format!("acct/nested/{DECK_A}"), b"x");
        // A folder with a deck file's name is not a file.
        dir.write(
            "acct/grouping 2a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d.xml/inside",
            b"x",
        );

        let mut expected = vec![a, b];
        expected.sort();
        assert_eq!(deck_files(&dir.join("acct")), expected);
        assert!(deck_files(&dir.join("missing")).is_empty());
    }
}
