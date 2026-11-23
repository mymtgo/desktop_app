//! Where MTGO keeps a match's game log file (0.x `EnsureGameLogForMatch`):
//! a file whose name contains `Match_GameLog`, somewhere under MTGO's data
//! folders, whose stem's last `_` part is the match token. Searched under the
//! same roots as the deck files, with the same depth limit, without entering
//! linked folders.

use std::path::{Path, PathBuf};

use crate::deck_files::{DECK_MAX_DEPTH, deck_roots};
use crate::discover::Env;
use crate::walk::walk_files;

/// The marker in every game log file name.
pub const GAME_LOG_MARKER: &str = "Match_GameLog";

/// The match token a game log file belongs to: the last `_` part of its
/// stem. `None` when the name is not a game log's.
pub fn game_log_token(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    if !name.contains(GAME_LOG_MARKER) {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    stem.rsplit('_').next().map(str::to_string)
}

/// The game log file of the match `token`, or `None` when there is none.
/// When several match (copies under two roots), the lexically smallest path
/// wins, so the choice never depends on folder listing order.
pub fn find_game_log(env: &Env, data_folder: Option<&Path>, token: &str) -> Option<PathBuf> {
    let mut found: Option<PathBuf> = None;
    for root in deck_roots(env, data_folder) {
        walk_files(
            &root,
            DECK_MAX_DEPTH,
            &|path| game_log_token(path).as_deref() == Some(token),
            &mut |path, _| {
                if found.as_ref().is_none_or(|kept| path < *kept) {
                    found = Some(path);
                }
            },
        );
    }
    found
}

/// Every game log file under the roots (the chosen data folder, else the
/// detected ones), sorted (0.x `DiscoverGameLogsJob`). Linked folders are
/// not entered; a linked file counts.
pub fn list_game_logs(env: &Env, data_folder: Option<&Path>) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for root in deck_roots(env, data_folder) {
        walk_files(
            &root,
            DECK_MAX_DEPTH,
            &|path| game_log_token(path).is_some(),
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
    const TOKEN: &str = "6a0f564b-f27a-42c0-acea-464f7929342b";
    const OTHER: &str = "e0ecf08e-9ab2-4551-9cfd-06207c0c69c1";

    fn env(platform: Platform, home: &Path) -> Env {
        Env {
            platform,
            home: Some(home.to_path_buf()),
            user: Some("someone".to_string()),
        }
    }

    fn file_name(token: &str) -> String {
        format!("Match_GameLog_{token}.dat")
    }

    #[test]
    fn the_token_is_the_last_underscore_part_of_the_stem() {
        assert_eq!(
            game_log_token(Path::new(&format!("x/{}", file_name(TOKEN)))).as_deref(),
            Some(TOKEN)
        );
        assert_eq!(
            game_log_token(Path::new("x/12345_Match_GameLog_abc_def.dat")).as_deref(),
            Some("def")
        );
        assert_eq!(
            game_log_token(Path::new("x/Match_GameLog")).as_deref(),
            Some("GameLog")
        );
        assert_eq!(
            game_log_token(Path::new(&format!("x/Deck_{TOKEN}.dat"))),
            None
        );
        assert_eq!(game_log_token(Path::new("x/match_gamelog_abc.dat")), None);
    }

    #[test]
    fn finds_the_matchs_game_log_under_the_windows_data_folder() {
        let dir = TestDir::new("game-log-windows");
        dir.write(&format!("{DATA}/Q1/A1/acct/{}", file_name(OTHER)), b"x");
        let wanted = dir.write(
            &format!("{DATA}/Q1/A1/acct/logs/{}", file_name(TOKEN)),
            b"x",
        );
        dir.write(&format!("{DATA}/Q1/A1/acct/{TOKEN}.dat"), b"x");

        let found = find_game_log(&env(Platform::Windows, dir.path()), None, TOKEN);
        assert_eq!(found, Some(wanted));
        assert_eq!(
            find_game_log(&env(Platform::Windows, dir.path()), None, "missing"),
            None
        );
    }

    #[test]
    fn a_wine_prefix_data_folder_is_searched_on_linux() {
        let dir = TestDir::new("game-log-linux");
        let wanted = dir.write(
            &format!(
                ".wine/drive_c/users/someone/{DATA}/Q1/acct/{}",
                file_name(TOKEN)
            ),
            b"x",
        );
        let found = find_game_log(&env(Platform::Linux, dir.path()), None, TOKEN);
        assert_eq!(found, Some(wanted));
    }

    #[test]
    fn a_chosen_data_folder_replaces_the_detected_ones() {
        let dir = TestDir::new("game-log-override");
        dir.write(&format!("{DATA}/Q1/acct/{}", file_name(TOKEN)), b"x");
        let chosen = dir.write(&format!("elsewhere/acct/{}", file_name(TOKEN)), b"x");
        let found = find_game_log(
            &env(Platform::Windows, dir.path()),
            Some(&dir.join("elsewhere")),
            TOKEN,
        );
        assert_eq!(found, Some(chosen));
    }

    #[test]
    fn of_two_copies_the_smallest_path_wins() {
        let dir = TestDir::new("game-log-two");
        dir.write(&format!("root/b/{}", file_name(TOKEN)), b"x");
        let first = dir.write(&format!("root/a/{}", file_name(TOKEN)), b"x");
        let found = find_game_log(
            &env(Platform::Windows, dir.path()),
            Some(&dir.join("root")),
            TOKEN,
        );
        assert_eq!(found, Some(first));
    }

    #[test]
    fn a_game_log_deeper_than_the_limit_is_not_found() {
        let dir = TestDir::new("game-log-depth");
        let mut deep = String::from("root");
        for level in 0..DECK_MAX_DEPTH {
            deep.push_str(&format!("/d{level}"));
        }
        dir.write(&format!("{deep}/{}", file_name(TOKEN)), b"x");
        let chosen = Some(dir.join("root"));
        let env = env(Platform::Windows, dir.path());
        assert_eq!(find_game_log(&env, chosen.as_deref(), TOKEN), None);

        let within = dir.write(&format!("root/d0/d1/d2/{}", file_name(TOKEN)), b"x");
        assert_eq!(find_game_log(&env, chosen.as_deref(), TOKEN), Some(within));
    }

    #[test]
    fn a_folder_named_like_a_game_log_is_not_a_file() {
        let dir = TestDir::new("game-log-folder");
        dir.write(&format!("root/{}/inside", file_name(TOKEN)), b"x");
        assert_eq!(
            find_game_log(
                &env(Platform::Windows, dir.path()),
                Some(&dir.join("root")),
                TOKEN
            ),
            None
        );
    }

    #[test]
    fn lists_every_game_log_under_the_roots_sorted() {
        let dir = TestDir::new("game-log-list");
        let second = dir.write(&format!("{DATA}/Q2/{}", file_name(OTHER)), b"x");
        let first = dir.write(&format!("{DATA}/Q1/A1/acct/{}", file_name(TOKEN)), b"x");
        let prefixed = dir.write(
            &format!("{DATA}/Q1/A1/acct/12_Match_GameLog_{OTHER}.dat"),
            b"x",
        );
        dir.write(&format!("{DATA}/Q1/A1/acct/{TOKEN}.dat"), b"x");
        dir.write(
            &format!("{DATA}/Q1/A1/acct/match_gamelog_{TOKEN}.dat"),
            b"x",
        );
        dir.write(&format!("{DATA}/Q1/{}/inside", file_name(TOKEN)), b"x");

        let found = list_game_logs(&env(Platform::Windows, dir.path()), None);
        let mut expected = vec![first, prefixed, second];
        expected.sort();
        assert_eq!(found, expected);
    }

    #[test]
    fn listing_stops_at_the_depth_limit_and_uses_the_chosen_folder() {
        let dir = TestDir::new("game-log-list-depth");
        dir.write(&format!("{DATA}/Q1/{}", file_name(OTHER)), b"x");
        let mut deep = String::from("root");
        for level in 0..DECK_MAX_DEPTH {
            deep.push_str(&format!("/d{level}"));
        }
        dir.write(&format!("{deep}/{}", file_name(TOKEN)), b"x");
        let within = dir.write(&format!("root/d0/{}", file_name(TOKEN)), b"x");
        let found = list_game_logs(&env(Platform::Windows, dir.path()), Some(&dir.join("root")));
        assert_eq!(found, vec![within]);
    }

    #[cfg(unix)]
    #[test]
    fn listing_does_not_enter_linked_folders_but_counts_linked_files() {
        let dir = TestDir::new("game-log-list-links");
        let outside = dir.write(&format!("outside/{}", file_name(TOKEN)), b"x");
        std::fs::create_dir_all(dir.join("root")).expect("root");
        std::os::unix::fs::symlink(dir.join("outside"), dir.join("root/linked")).expect("link");
        let env = env(Platform::Windows, dir.path());
        let chosen = Some(dir.join("root"));
        assert!(list_game_logs(&env, chosen.as_deref()).is_empty());

        let link = dir.join(&format!("root/{}", file_name(TOKEN)));
        std::os::unix::fs::symlink(&outside, &link).expect("file link");
        assert_eq!(list_game_logs(&env, chosen.as_deref()), vec![link]);
    }

    #[cfg(unix)]
    #[test]
    fn linked_folders_are_not_entered_but_linked_files_count() {
        let dir = TestDir::new("game-log-links");
        let outside = dir.write(&format!("outside/{}", file_name(TOKEN)), b"x");
        std::fs::create_dir_all(dir.join("root")).expect("root");
        std::os::unix::fs::symlink(dir.join("outside"), dir.join("root/linked")).expect("link");
        let env = env(Platform::Windows, dir.path());
        let chosen = Some(dir.join("root"));
        assert_eq!(find_game_log(&env, chosen.as_deref(), TOKEN), None);

        std::os::unix::fs::symlink(&outside, dir.join(&format!("root/{}", file_name(TOKEN))))
            .expect("file link");
        assert_eq!(
            find_game_log(&env, chosen.as_deref(), TOKEN),
            Some(dir.join(&format!("root/{}", file_name(TOKEN))))
        );
    }
}
