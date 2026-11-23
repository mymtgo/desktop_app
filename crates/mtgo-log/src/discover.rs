//! Where MTGO keeps `mtgo.log` (spec 5.1). MTGO is a ClickOnce app: its log sits
//! somewhere under `AppData\Local\Apps\2.0` in the Windows user's home, a few
//! random folder names deep. On Linux and macOS that home is inside a Wine
//! prefix.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::walk::walk_files;

/// Depth limit when searching a root (0.x `FindMtgoLogPath` uses `depth('< 8')`).
pub const MAX_DEPTH: usize = 8;

/// The log player's folder (development), searched on macOS before the Wine
/// prefixes.
pub const MAC_DEV_FOLDER: &str = "mymtgo-log-player";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Linux,
    MacOs,
}

/// What detection needs to know about the machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Env {
    pub platform: Platform,
    pub home: Option<PathBuf>,
    /// The OS user name. Wine names the prefix's user folder after it.
    pub user: Option<String>,
}

impl Env {
    pub fn current() -> Self {
        let platform = if cfg!(windows) {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Linux
        };
        let (home, user_var) = if cfg!(windows) {
            let home = windows_home(
                std::env::var_os("USERPROFILE"),
                std::env::var_os("HOMEDRIVE"),
                std::env::var_os("HOMEPATH"),
            );
            (home, "USERNAME")
        } else {
            (std::env::var_os("HOME").map(PathBuf::from), "USER")
        };
        Self {
            platform,
            home,
            user: std::env::var(user_var).ok(),
        }
    }
}

/// The Windows home folder: `USERPROFILE`, or `HOMEDRIVE` + `HOMEPATH` when it
/// is unset (as 0.x does). Takes the variable values so tests need no real env.
fn windows_home(
    userprofile: Option<OsString>,
    homedrive: Option<OsString>,
    homepath: Option<OsString>,
) -> Option<PathBuf> {
    if let Some(profile) = userprofile.filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(profile));
    }
    let drive = homedrive.filter(|value| !value.is_empty())?;
    let path = homepath.filter(|value| !value.is_empty())?;
    let mut joined = drive;
    joined.push(path);
    Some(PathBuf::from(joined))
}

/// The roots searched and the logs found, for the status bar and support bundles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub roots: Vec<PathBuf>,
    pub logs: Vec<PathBuf>,
}

pub fn detect(env: &Env, overrides: &[PathBuf]) -> Detection {
    let roots = candidate_roots(env, overrides);
    let logs = find_logs(&roots);
    Detection { roots, logs }
}

/// Folders (or files) to search. Manual overrides replace auto-detection.
pub fn candidate_roots(env: &Env, overrides: &[PathBuf]) -> Vec<PathBuf> {
    if !overrides.is_empty() {
        return overrides.to_vec();
    }
    let Some(home) = &env.home else {
        return Vec::new();
    };
    match env.platform {
        Platform::Windows => vec![clickonce_root(home)],
        Platform::MacOs => mac_roots(home, env.user.as_deref()),
        Platform::Linux => linux_roots(home, env.user.as_deref()),
    }
}

/// Every `mtgo.log` (any letter case) under the roots, at most `MAX_DEPTH` deep.
pub fn find_logs(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for root in roots {
        if root.is_file() {
            if is_log_name(root) {
                found.push(root.clone());
            }
        } else {
            // Linked folders are never entered (see `walk_files`), but a root that
            // is itself a link is still searched, and so are linked prefixes
            // found by `children`.
            walk_files(root, MAX_DEPTH, &is_log_name, &mut |path, _| {
                found.push(path)
            });
        }
    }
    unique_by_real_path(found)
}

/// Drops logs that are the same file reached through aliased roots (a symlinked
/// Steam home, say) or symlinked `mtgo.log` files. The shortest path is kept,
/// the lexically smaller one on a tie so the choice never depends on folder
/// listing order. It is kept as written: canonical paths gain a `\\?\` prefix
/// on Windows, and the store keys files by path string.
fn unique_by_real_path(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut slot_of_real: std::collections::HashMap<PathBuf, usize> =
        std::collections::HashMap::new();
    let mut unique: Vec<PathBuf> = Vec::new();
    for path in paths {
        let real = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        match slot_of_real.get(&real) {
            Some(&slot) => {
                if is_preferred(&path, &unique[slot]) {
                    unique[slot] = path;
                }
            }
            None => {
                slot_of_real.insert(real, unique.len());
                unique.push(path);
            }
        }
    }
    unique.sort();
    unique
}

/// Shorter wins; on equal length the lexically smaller path wins.
fn is_preferred(path: &Path, kept: &Path) -> bool {
    let (length, kept_length) = (path.components().count(), kept.components().count());
    length < kept_length || (length == kept_length && path < kept)
}

fn is_log_name(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("mtgo.log"))
}

fn under(base: &Path, parts: &[&str]) -> PathBuf {
    let mut path = base.to_path_buf();
    for part in parts {
        path.push(part);
    }
    path
}

fn clickonce_root(windows_home: &Path) -> PathBuf {
    under(windows_home, &["AppData", "Local", "Apps", "2.0"])
}

/// Old Wine versions use XP-style folders inside the prefix.
fn legacy_clickonce_root(windows_home: &Path) -> PathBuf {
    under(
        windows_home,
        &["Local Settings", "Application Data", "Apps", "2.0"],
    )
}

/// The ClickOnce roots in every Wine user home of every prefix.
fn wine_roots(user_homes: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for user_home in user_homes {
        roots.push(clickonce_root(&user_home));
        roots.push(legacy_clickonce_root(&user_home));
    }
    roots
}

/// The log player's folder, then the Wine prefixes of the Mac wrappers. The
/// user folder inside a prefix is the OS user, or `crossover` in CrossOver.
fn mac_roots(home: &Path, user: Option<&str>) -> Vec<PathBuf> {
    let mut roots = vec![home.join(MAC_DEV_FOLDER)];
    let mut users: Vec<String> = user.map(str::to_string).into_iter().collect();
    push_unique(&mut users, "crossover");
    roots.extend(wine_roots(prefix_user_homes(
        &mac_wine_prefixes(home),
        &users,
    )));
    roots
}

/// Plain Wine (and the Game Porting Toolkit), CrossOver bottles, Whisky
/// bottles, MTGO-OSX prefixes (one per architecture), and the prefixes inside
/// Sikarugir and Kegworks wrapper apps.
fn mac_wine_prefixes(home: &Path) -> Vec<PathBuf> {
    let mut prefixes = vec![home.join(".wine")];
    prefixes.extend(children(&under(
        home,
        &["Library", "Application Support", "CrossOver", "Bottles"],
    )));
    prefixes.extend(children(&under(
        home,
        &[
            "Library",
            "Containers",
            "com.isaacmarovitz.Whisky",
            "Bottles",
        ],
    )));
    prefixes.extend(children(&under(
        home,
        &["Library", "Application Support", "MTGO-OSX", "Prefixes"],
    )));
    for wrappers in ["Sikarugir", "Kegworks"] {
        for app in children(&under(home, &["Applications", wrappers])) {
            prefixes.push(under(&app, &["Contents", "SharedSupport", "prefix"]));
        }
    }
    prefixes
}

/// The Wine prefix a file sits in: the folder above its last `drive_c`.
pub fn prefix_of(path: &Path) -> Option<PathBuf> {
    let mut prefix = None;
    let mut so_far = PathBuf::new();
    for part in path.components() {
        if part.as_os_str() == "drive_c" {
            prefix = Some(so_far.clone());
        }
        so_far.push(part);
    }
    prefix
}

/// Wine, Bottles (native and Flatpak), Lutris and Steam Proton (native and
/// Flatpak) prefixes; inside each, the OS user's home and `steamuser`.
fn linux_roots(home: &Path, user: Option<&str>) -> Vec<PathBuf> {
    wine_roots(wine_user_homes(home, user))
}

/// The Windows user homes (`drive_c/users/<user>`) inside every Wine,
/// Bottles (native and Flatpak), Lutris and Steam Proton (native and Flatpak)
/// prefix under the Linux `home`, for the OS user and `steamuser`. Arena's
/// log discovery searches the same prefixes.
pub fn wine_user_homes(home: &Path, user: Option<&str>) -> Vec<PathBuf> {
    let mut users: Vec<String> = user.map(str::to_string).into_iter().collect();
    push_unique(&mut users, "steamuser");
    prefix_user_homes(&linux_wine_prefixes(home), &users)
}

fn linux_wine_prefixes(home: &Path) -> Vec<PathBuf> {
    let mut prefixes = vec![home.join(".wine")];
    prefixes.extend(children(&under(
        home,
        &[".local", "share", "bottles", "bottles"],
    )));
    prefixes.extend(children(&under(
        home,
        &[
            ".var",
            "app",
            "com.usebottles.bottles",
            "data",
            "bottles",
            "bottles",
        ],
    )));
    prefixes.extend(children(&home.join("Games")));
    let steam_homes = [
        under(home, &[".steam", "steam"]),
        under(home, &[".local", "share", "Steam"]),
        under(
            home,
            &[
                ".var",
                "app",
                "com.valvesoftware.Steam",
                ".local",
                "share",
                "Steam",
            ],
        ),
    ];
    for steam in steam_homes {
        for game in children(&under(&steam, &["steamapps", "compatdata"])) {
            prefixes.push(game.join("pfx"));
        }
    }
    prefixes
}

fn push_unique(users: &mut Vec<String>, name: &str) {
    if !users.iter().any(|user| user == name) {
        users.push(name.to_string());
    }
}

/// `drive_c/users/<user>` in every prefix, for every user.
fn prefix_user_homes(prefixes: &[PathBuf], users: &[String]) -> Vec<PathBuf> {
    let mut homes = Vec::new();
    for prefix in prefixes {
        for user in users {
            homes.push(under(prefix, &["drive_c", "users", user]));
        }
    }
    homes
}

/// Sub-folders of `dir` (symlinked ones included), sorted; empty when it does
/// not exist.
fn children(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| fs::metadata(path).is_ok_and(|meta| meta.is_dir()))
        .collect();
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use fixture_tools::TestDir;

    const DEEP: &str = "AppData/Local/Apps/2.0/Q1W2E3R4.T5Y/A1B2C3D4.E5F/mtgo..tion_0000000000000000_0003.0004_0000000000000000/mtgo.log";

    fn env(platform: Platform, home: &std::path::Path) -> Env {
        Env {
            platform,
            home: Some(home.to_path_buf()),
            user: Some("someone".to_string()),
        }
    }

    #[test]
    fn finds_the_windows_clickonce_log() {
        let dir = TestDir::new("discover-windows");
        let log = dir.write(DEEP, b"x");
        let found = detect(&env(Platform::Windows, dir.path()), &[]);
        assert_eq!(found.logs, vec![log]);
    }

    #[test]
    fn finds_logs_in_wine_bottles_and_steam_prefixes() {
        let dir = TestDir::new("discover-linux");
        let wine = dir.write(&format!(".wine/drive_c/users/someone/{DEEP}"), b"x");
        let bottle = dir.write(
            &format!(".var/app/com.usebottles.bottles/data/bottles/bottles/MTGO/drive_c/users/someone/{DEEP}"),
            b"x",
        );
        let steam = dir.write(
            &format!(
                ".local/share/Steam/steamapps/compatdata/123/pfx/drive_c/users/steamuser/{DEEP}"
            ),
            b"x",
        );
        let mut expected = vec![wine, bottle, steam];
        expected.sort();
        let found = detect(&env(Platform::Linux, dir.path()), &[]);
        assert_eq!(found.logs, expected);
    }

    #[test]
    fn overrides_replace_detection_and_may_name_the_file() {
        let dir = TestDir::new("discover-override");
        dir.write(DEEP, b"x");
        let chosen = dir.write("elsewhere/MTGO.LOG", b"x");
        let found = detect(
            &env(Platform::Windows, dir.path()),
            std::slice::from_ref(&chosen),
        );
        assert_eq!(found.roots, vec![chosen.clone()]);
        assert_eq!(found.logs, vec![chosen]);
    }

    #[test]
    fn the_search_stops_at_the_depth_limit() {
        let dir = TestDir::new("discover-depth");
        let too_deep = "a/b/c/d/e/f/g/h/mtgo.log";
        dir.write(too_deep, b"x");
        assert!(find_logs(&[dir.path().to_path_buf()]).is_empty());
        let shallow = dir.write("a/b/c/d/e/f/g/mtgo.log", b"x");
        assert_eq!(find_logs(&[dir.path().to_path_buf()]), vec![shallow]);
    }

    #[test]
    fn macos_searches_the_log_player_folder_first() {
        let dir = TestDir::new("discover-mac");
        let roots = candidate_roots(&env(Platform::MacOs, dir.path()), &[]);
        assert_eq!(roots.first(), Some(&dir.path().join(MAC_DEV_FOLDER)));
    }

    #[test]
    fn finds_logs_in_mac_wine_prefixes() {
        let dir = TestDir::new("discover-mac-wine");
        let player = dir.write(&format!("{MAC_DEV_FOLDER}/mtgo.log"), b"x");
        let wine = dir.write(&format!(".wine/drive_c/users/someone/{DEEP}"), b"x");
        let crossover = dir.write(
            &format!(
                "Library/Application Support/CrossOver/Bottles/MTGO/drive_c/users/crossover/{DEEP}"
            ),
            b"x",
        );
        let whisky = dir.write(
            &format!(
                "Library/Containers/com.isaacmarovitz.Whisky/Bottles/ABC-123/drive_c/users/someone/{DEEP}"
            ),
            b"x",
        );
        let sikarugir = dir.write(
            &format!(
                "Applications/Sikarugir/MTGO.app/Contents/SharedSupport/prefix/drive_c/users/someone/{DEEP}"
            ),
            b"x",
        );
        let kegworks = dir.write(
            &format!(
                "Applications/Kegworks/Magic.app/Contents/SharedSupport/prefix/drive_c/users/someone/{DEEP}"
            ),
            b"x",
        );
        // MTGO-OSX writes its log in a Logs folder beside mtgo.exe.
        let mtgo_osx = dir.write(
            "Library/Application Support/MTGO-OSX/Prefixes/arm64/drive_c/users/someone/AppData/Local/Apps/2.0/51OP3ZAG.L94/HR8B38VY.LVG/mtgo..tion_a2d3b1cd60f98833_0003.0004_bf4bfe5c26d706f6/Logs/mtgo.log",
            b"x",
        );
        let mut expected = vec![
            player, wine, crossover, whisky, sikarugir, kegworks, mtgo_osx,
        ];
        expected.sort();
        let mut found = detect(&env(Platform::MacOs, dir.path()), &[]).logs;
        found.sort();
        assert_eq!(found, expected);
    }

    #[test]
    fn linux_does_not_search_mac_wrappers() {
        let dir = TestDir::new("discover-linux-no-mac");
        dir.write(
            &format!(
                "Library/Application Support/CrossOver/Bottles/MTGO/drive_c/users/crossover/{DEEP}"
            ),
            b"x",
        );
        assert!(
            detect(&env(Platform::Linux, dir.path()), &[])
                .logs
                .is_empty()
        );
    }

    #[test]
    fn prefix_of_is_the_folder_above_drive_c() {
        let log = PathBuf::from(format!(
            "/Users/someone/Library/Application Support/CrossOver/Bottles/MTGO/drive_c/users/crossover/{DEEP}"
        ));
        assert_eq!(
            prefix_of(&log),
            Some(PathBuf::from(
                "/Users/someone/Library/Application Support/CrossOver/Bottles/MTGO"
            ))
        );
        assert_eq!(prefix_of(&PathBuf::from("/Users/someone/mtgo.log")), None);
    }

    #[test]
    fn missing_folders_find_nothing() {
        let dir = TestDir::new("discover-empty");
        let found = detect(&env(Platform::Linux, dir.path()), &[]);
        assert!(found.logs.is_empty());
        assert!(
            !found.roots.is_empty(),
            "the roots tried are still reported"
        );
    }

    #[test]
    fn windows_home_prefers_userprofile() {
        let home = windows_home(
            Some("C:\\Users\\someone".into()),
            Some("D:".into()),
            Some("\\Other".into()),
        );
        assert_eq!(home, Some(PathBuf::from("C:\\Users\\someone")));
    }

    #[test]
    fn windows_home_falls_back_to_homedrive_and_homepath() {
        let home = windows_home(None, Some("D:".into()), Some("\\Users\\someone".into()));
        assert_eq!(home, Some(PathBuf::from("D:\\Users\\someone")));
    }

    #[test]
    fn windows_home_is_unknown_without_any_variable() {
        assert_eq!(windows_home(None, Some("D:".into()), None), None);
        assert_eq!(windows_home(None, None, Some("\\Users".into())), None);
        assert_eq!(windows_home(None, None, None), None);
    }

    #[test]
    fn a_steamuser_os_user_is_searched_once_per_prefix() {
        let dir = TestDir::new("discover-steamuser");
        let linux = Env {
            platform: Platform::Linux,
            home: Some(dir.path().to_path_buf()),
            user: Some("steamuser".to_string()),
        };
        let roots = candidate_roots(&linux, &[]);
        let mut unique = roots.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(roots.len(), unique.len());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_prefix_folder_is_searched() {
        let dir = TestDir::new("discover-symlinked-prefix");
        dir.write(
            &format!("elsewhere/MTGO/drive_c/users/someone/{DEEP}"),
            b"x",
        );
        std::fs::create_dir_all(dir.join(".local/share/bottles/bottles")).unwrap();
        std::os::unix::fs::symlink(
            dir.join("elsewhere/MTGO"),
            dir.join(".local/share/bottles/bottles/MTGO"),
        )
        .unwrap();
        let found = detect(&env(Platform::Linux, dir.path()), &[]);
        let expected = dir.join(&format!(
            ".local/share/bottles/bottles/MTGO/drive_c/users/someone/{DEEP}"
        ));
        assert_eq!(found.logs, vec![expected]);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_log_file_is_found() {
        let dir = TestDir::new("discover-symlinked-log");
        let real = dir.write("elsewhere/real.log", b"x");
        std::fs::create_dir_all(dir.join("root/deeper")).unwrap();
        std::os::unix::fs::symlink(&real, dir.join("root/deeper/mtgo.log")).unwrap();
        assert_eq!(
            find_logs(&[dir.join("root")]),
            vec![dir.join("root/deeper/mtgo.log")]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_loop_terminates() {
        let dir = TestDir::new("discover-loop");
        let log = dir.write("root/mtgo.log", b"x");
        // Linked folders are never entered by the walk, so a loop cannot start.
        std::os::unix::fs::symlink(dir.join("root"), dir.join("root/again")).unwrap();
        std::os::unix::fs::symlink(dir.join("root"), dir.join("root/and-again")).unwrap();
        assert_eq!(find_logs(&[dir.join("root")]), vec![log]);
    }

    #[cfg(unix)]
    #[test]
    fn a_log_reached_through_two_aliased_roots_is_reported_once() {
        let dir = TestDir::new("discover-alias");
        let real_log = dir.write("real/mtgo.log", b"x");
        std::os::unix::fs::symlink(dir.join("real"), dir.join("alias")).unwrap();
        // Equally short paths: the lexically smaller one is kept, whatever the order.
        let alias_log = dir.join("alias/mtgo.log");
        let roots = [dir.join("real"), dir.join("alias")];
        assert_eq!(find_logs(&roots), vec![alias_log.clone()]);
        let reversed = [dir.join("alias"), dir.join("real")];
        assert_eq!(find_logs(&reversed), vec![alias_log]);
        let same_root_twice = [dir.join("real"), dir.join("real")];
        assert_eq!(find_logs(&same_root_twice), vec![real_log]);
    }

    #[cfg(unix)]
    #[test]
    fn equally_short_aliases_keep_the_lexically_smaller_path() {
        let dir = TestDir::new("discover-tie");
        let real = dir.write("elsewhere/real.log", b"x");
        std::fs::create_dir_all(dir.join("root/a")).unwrap();
        std::fs::create_dir_all(dir.join("root/b")).unwrap();
        let a = dir.join("root/a/mtgo.log");
        let b = dir.join("root/b/mtgo.log");
        std::os::unix::fs::symlink(&real, &a).unwrap();
        std::os::unix::fs::symlink(&real, &b).unwrap();
        assert_eq!(
            unique_by_real_path(vec![b.clone(), a.clone()]),
            vec![a.clone()]
        );
        assert_eq!(unique_by_real_path(vec![a.clone(), b]), vec![a]);
    }

    #[cfg(unix)]
    #[test]
    fn the_walk_does_not_follow_a_symlinked_folder_out_of_the_root() {
        let dir = TestDir::new("discover-dosdevices");
        let inside = dir.write("prefix/drive_c/users/someone/mtgo.log", b"x");
        dir.write("host/home/someone/mtgo.log", b"x");
        std::fs::create_dir_all(dir.join("prefix/dosdevices")).unwrap();
        // Every Wine prefix has `dosdevices/z:` pointing at the host's `/`.
        std::os::unix::fs::symlink(dir.join("host"), dir.join("prefix/dosdevices/z:")).unwrap();
        assert_eq!(find_logs(&[dir.join("prefix")]), vec![inside]);
    }

    #[test]
    fn no_home_gives_no_roots_and_no_logs() {
        for platform in [Platform::Windows, Platform::Linux, Platform::MacOs] {
            let env = Env {
                platform,
                home: None,
                user: Some("someone".to_string()),
            };
            assert!(candidate_roots(&env, &[]).is_empty(), "{platform:?}");
            assert_eq!(
                detect(&env, &[]),
                Detection {
                    roots: Vec::new(),
                    logs: Vec::new()
                }
            );
        }
    }

    /// Puts a folder's permissions back when dropped, so `TestDir` can delete it
    /// even when an assertion fails first.
    #[cfg(unix)]
    struct RestorePermissions(PathBuf);

    #[cfg(unix)]
    impl Drop for RestorePermissions {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_folder_does_not_abort_discovery() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TestDir::new("discover-unreadable");
        dir.write("a/locked/mtgo.log", b"x");
        let kept = dir.write("a/open/mtgo.log", b"x");
        let locked = dir.join("a/locked");
        let _restore = RestorePermissions(locked.clone());
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::read_dir(&locked).is_ok() {
            // Running as root: permissions are not enforced, nothing to test.
            return;
        }
        assert_eq!(find_logs(&[dir.path().to_path_buf()]), vec![kept]);
    }
}
