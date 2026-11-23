//! Where 0.x keeps its database (importer spec section 3). 0.x is a NativePHP
//! (Electron) app: its files live in Electron's `userData` folder, named after
//! the app (`mymtgo`, or `com.mymtgo.app` on some installs).

use std::path::{Path, PathBuf};

/// The `userData` folder names 0.x may use, most likely first.
const APP_NAMES: [&str; 2] = ["mymtgo", "com.mymtgo.app"];

/// The database file names, packaged install first: an installed 0.x keeps
/// `database.sqlite`; `nativephp.sqlite` is the development copy.
const DATABASE_FILES: [&str; 2] = ["database.sqlite", "nativephp.sqlite"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Linux,
    MacOs,
}

/// Candidate 0.x database paths, most likely first (spec section 3).
///
/// `appdata` is the platform's config folder when the environment names one:
/// `%APPDATA%` on Windows, `$XDG_CONFIG_HOME` on Linux (unused on macOS).
/// Without it the usual folder under `home` is used; with neither there is
/// no candidate.
pub fn candidate_paths(home: Option<&Path>, appdata: Option<&Path>, os: Os) -> Vec<PathBuf> {
    let config = match (os, appdata, home) {
        (Os::Windows, Some(folder), _) | (Os::Linux, Some(folder), _) => folder.to_path_buf(),
        (Os::Windows, None, Some(home)) => home.join("AppData").join("Roaming"),
        (Os::Linux, None, Some(home)) => home.join(".config"),
        (Os::MacOs, _, Some(home)) => home.join("Library").join("Application Support"),
        (_, _, None) => return Vec::new(),
    };
    let mut paths = Vec::new();
    for name in APP_NAMES {
        for file in DATABASE_FILES {
            paths.push(config.join(name).join("database").join(file));
        }
    }
    paths
}

pub fn current_os() -> Os {
    if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::MacOs
    } else {
        Os::Linux
    }
}

/// The first candidate that exists, for this machine.
pub fn detect() -> Option<PathBuf> {
    // `%APPDATA%` alone is enough on Windows, so a missing home is no stop.
    let home = mtgo_log::Env::current().home;
    let os = current_os();
    let variable = match os {
        Os::Windows => Some("APPDATA"),
        Os::Linux => Some("XDG_CONFIG_HOME"),
        Os::MacOs => None,
    };
    let appdata = variable.and_then(absolute_env_path);
    candidate_paths(home.as_deref(), appdata.as_deref(), os)
        .into_iter()
        .find(|path| path.is_file())
}

/// An environment variable holding an absolute path; empty or relative values
/// are ignored (the XDG spec says to ignore relative ones).
fn absolute_env_path(name: &str) -> Option<PathBuf> {
    let value = PathBuf::from(std::env::var_os(name)?);
    if value.is_absolute() {
        Some(value)
    } else {
        None
    }
}

/// settings.json for a database path: `<userData>/storage/app/private/settings.json`
/// where the database is `<userData>/database/database.sqlite` (or `nativephp.sqlite`).
pub fn settings_path(db_path: &Path) -> Option<PathBuf> {
    let database_folder = db_path.parent()?;
    let user_data = database_folder.parent()?;
    if user_data.as_os_str().is_empty() {
        return None;
    }
    Some(
        user_data
            .join("storage")
            .join("app")
            .join("private")
            .join("settings.json"),
    )
}
