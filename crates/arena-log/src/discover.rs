//! Where Arena keeps `Player.log` (Arena spec section 3). Arena truncates it at
//! every launch and keeps the previous one beside it as `Player-prev.log`.

use std::path::{Path, PathBuf};

use mtgo_log::{Env, Platform, wine_user_homes};

pub const PLAYER_LOG: &str = "Player.log";
pub const PLAYER_PREV_LOG: &str = "Player-prev.log";

/// The `Player.log` paths to watch. Overrides (files) replace the platform
/// defaults, as MTGO's log overrides do, so tests and a hand-picked log never
/// read the machine's real log as well. The paths may not exist.
pub fn arena_log_paths(env: &Env, overrides: &[PathBuf]) -> Vec<PathBuf> {
    if !overrides.is_empty() {
        return overrides.to_vec();
    }
    let Some(home) = &env.home else {
        return Vec::new();
    };
    match env.platform {
        Platform::MacOs => vec![under(
            home,
            &[
                "Library",
                "Logs",
                "Wizards Of The Coast",
                "MTGA",
                PLAYER_LOG,
            ],
        )],
        Platform::Windows => vec![windows_log(home)],
        Platform::Linux => wine_user_homes(home, env.user.as_deref())
            .iter()
            .map(|user_home| windows_log(user_home))
            .collect(),
    }
}

/// `Player-prev.log` beside `player_log`.
pub fn prev_log_path(player_log: &Path) -> PathBuf {
    player_log.with_file_name(PLAYER_PREV_LOG)
}

/// The log under a Windows user home.
fn windows_log(windows_home: &Path) -> PathBuf {
    under(
        windows_home,
        &[
            "AppData",
            "LocalLow",
            "Wizards Of The Coast",
            "MTGA",
            PLAYER_LOG,
        ],
    )
}

fn under(base: &Path, parts: &[&str]) -> PathBuf {
    let mut path = base.to_path_buf();
    for part in parts {
        path.push(part);
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(platform: Platform) -> Env {
        Env {
            platform,
            home: Some(PathBuf::from("/home/someone")),
            user: Some("someone".to_string()),
        }
    }

    #[test]
    fn macos_path_is_under_library_logs() {
        assert_eq!(
            arena_log_paths(&env(Platform::MacOs), &[]),
            vec![PathBuf::from(
                "/home/someone/Library/Logs/Wizards Of The Coast/MTGA/Player.log"
            )]
        );
    }

    #[test]
    fn windows_path_is_under_locallow() {
        assert_eq!(
            arena_log_paths(&env(Platform::Windows), &[]),
            vec![PathBuf::from(
                "/home/someone/AppData/LocalLow/Wizards Of The Coast/MTGA/Player.log"
            )]
        );
    }

    #[test]
    fn linux_paths_are_inside_each_wine_prefix() {
        let paths = arena_log_paths(&env(Platform::Linux), &[]);
        assert!(paths.contains(&PathBuf::from(
            "/home/someone/.wine/drive_c/users/someone/AppData/LocalLow/Wizards Of The Coast/MTGA/Player.log"
        )));
        assert!(paths.contains(&PathBuf::from(
            "/home/someone/.wine/drive_c/users/steamuser/AppData/LocalLow/Wizards Of The Coast/MTGA/Player.log"
        )));
    }

    #[test]
    fn overrides_replace_the_defaults() {
        let chosen = PathBuf::from("/logs/Player.log");
        assert_eq!(
            arena_log_paths(&env(Platform::MacOs), std::slice::from_ref(&chosen)),
            vec![chosen]
        );
    }

    #[test]
    fn no_home_means_no_paths() {
        let mut no_home = env(Platform::MacOs);
        no_home.home = None;
        assert!(arena_log_paths(&no_home, &[]).is_empty());
    }

    #[test]
    fn prev_log_sits_beside_player_log() {
        assert_eq!(
            prev_log_path(Path::new("/a/MTGA/Player.log")),
            PathBuf::from("/a/MTGA/Player-prev.log")
        );
    }
}
