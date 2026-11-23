//! The app's identity by version (release spec 13.1): the header's "Beta"
//! tag, the Sentry environment and the update channel's default follow the
//! version (prerelease or not), never the product name, which the app reads
//! at runtime (`package_info().name`).
//!
//! Also the Linux half of leaving the "MyMTGO Beta" name: the autostart
//! plugin names its desktop entry after the product, so after the rename the
//! old entry would start nothing that exists. At start, once, the old entry
//! becomes launch at login under the new name. The data folder is named by
//! the identifier (`com.mymtgo.desktop`), which never changes, so no data
//! moves. Inert while the product is still "MyMTGO Beta".

use std::path::{Path, PathBuf};

/// The product name of every beta up to the rename. Written only here and in
/// the installer hook (`app/installer/hooks.nsh`).
pub const BETA_PRODUCT_NAME: &str = "MyMTGO Beta";

/// A version with a prerelease part (`1.0.0-beta.3`). Build metadata after
/// `+` does not count.
pub fn is_prerelease(version: &str) -> bool {
    let without_build = version.split('+').next().unwrap_or(version);
    without_build.contains('-')
}

/// Sentry's environment: `beta` for a prerelease build, else `production`.
pub fn sentry_environment(version: &str) -> &'static str {
    if is_prerelease(version) {
        "beta"
    } else {
        "production"
    }
}

/// Whether the header shows the "Beta" tag: on prerelease builds only.
pub fn show_beta_tag(version: &str) -> bool {
    is_prerelease(version)
}

/// Where the autostart plugin wrote the beta's launch at login entry on
/// Linux (auto-launch's XDG mode: `~/.config/autostart/<name>.desktop`).
pub fn legacy_autostart_entry(home: &Path) -> PathBuf {
    home.join(".config")
        .join("autostart")
        .join(format!("{BETA_PRODUCT_NAME}.desktop"))
}

/// What the Linux autostart fix did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutostartMove {
    /// Still the beta name: the entry is this install's own.
    Inert,
    /// No old entry (never on, or already moved).
    NothingToMove,
    /// Launch at login is on under the new name and the old entry is gone.
    Moved,
    /// The desktop had switched the old entry off: it is removed and launch
    /// at login stays off.
    RemovedSwitchedOff,
}

/// Move the beta's launch at login entry to `product_name`. `enable` turns
/// launch at login on under the new name (the autostart plugin). The old
/// entry is removed only after that succeeded, so a failure keeps it for the
/// next start; once removed, every later start finds nothing to move.
pub fn move_legacy_autostart(
    product_name: &str,
    legacy_entry: &Path,
    enable: &mut dyn FnMut() -> Result<(), String>,
) -> Result<AutostartMove, String> {
    if product_name == BETA_PRODUCT_NAME {
        return Ok(AutostartMove::Inert);
    }
    let text = match std::fs::read_to_string(legacy_entry) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(AutostartMove::NothingToMove);
        }
        Err(error) => return Err(error.to_string()),
    };
    let outcome = if is_switched_off(&text) {
        AutostartMove::RemovedSwitchedOff
    } else {
        enable()?;
        AutostartMove::Moved
    };
    std::fs::remove_file(legacy_entry).map_err(|error| error.to_string())?;
    Ok(outcome)
}

/// An XDG autostart entry the desktop switched off.
fn is_switched_off(text: &str) -> bool {
    text.lines().map(str::trim).any(|line| {
        line.eq_ignore_ascii_case("Hidden=true")
            || line.eq_ignore_ascii_case("X-GNOME-Autostart-enabled=false")
    })
}

/// The Linux autostart fix at start (release spec 13.1). Logged, never
/// fatal: launch at login is a convenience.
pub fn fix_linux_autostart(app: &tauri::AppHandle) {
    if !cfg!(target_os = "linux") {
        return;
    }
    let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) else {
        return;
    };
    let entry = legacy_autostart_entry(Path::new(&home));
    let product_name = app.package_info().name.clone();
    let mut enable = || {
        use tauri_plugin_autostart::ManagerExt;
        app.autolaunch().enable().map_err(|error| error.to_string())
    };
    match move_legacy_autostart(&product_name, &entry, &mut enable) {
        Ok(AutostartMove::Moved) => {
            tracing::info!("launch at login moved from the beta name")
        }
        Ok(AutostartMove::RemovedSwitchedOff) => {
            tracing::info!("the beta's switched off launch at login entry was removed")
        }
        Ok(AutostartMove::Inert | AutostartMove::NothingToMove) => {}
        Err(error) => {
            tracing::warn!(%error, "launch at login could not be moved from the beta name")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn prerelease_builds_report_as_beta_and_stable_as_production() {
        assert!(is_prerelease("1.0.0-beta.3"));
        assert!(is_prerelease("1.1.0-rc.1+build.7"));
        assert!(!is_prerelease("1.0.0"));
        assert!(!is_prerelease("1.0.0+build.7"));
        assert_eq!(sentry_environment("1.0.0-beta.3"), "beta");
        assert_eq!(sentry_environment("1.0.0"), "production");
        assert_eq!(sentry_environment("1.2.3+build.1"), "production");
    }

    #[test]
    fn the_beta_tag_follows_the_version() {
        // The version decides, not the name: the last beta is still
        // "MyMTGO Beta" and 1.0.0 is "MyMTGO" (release spec 13.1).
        assert!(show_beta_tag("1.0.0-beta.2"));
        assert!(show_beta_tag("1.1.0-beta.1"));
        assert!(!show_beta_tag("1.0.0"));
        assert!(!show_beta_tag("1.0.1"));
    }

    #[test]
    fn the_legacy_autostart_entry_is_the_beta_names_desktop_file() {
        let home = Path::new("/home/qhome");
        assert_eq!(
            legacy_autostart_entry(home),
            home.join(".config")
                .join("autostart")
                .join("MyMTGO Beta.desktop")
        );
    }

    fn write_entry(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("autostart folder");
        std::fs::write(path, text).expect("entry");
    }

    const ENABLED_ENTRY: &str = "[Desktop Entry]\nType=Application\nVersion=1.0\nName=MyMTGO Beta\nComment=MyMTGO Beta startup script\nExec=/opt/q/MyMTGO_Beta.AppImage \nStartupNotify=false\nTerminal=false\n";

    #[test]
    fn the_linux_migration_moves_the_autostart_entry_once() {
        let dir = fixture_tools::TestDir::new("identity-autostart-moves");
        let entry = legacy_autostart_entry(&dir.join("home"));
        write_entry(&entry, ENABLED_ENTRY);
        let enabled = Cell::new(0);
        let mut enable = || {
            // Launch at login under the new name exists before the old
            // entry goes, so a failure never leaves the user without it.
            assert!(entry.exists(), "the old entry is removed only afterwards");
            enabled.set(enabled.get() + 1);
            Ok(())
        };

        let first = move_legacy_autostart("MyMTGO", &entry, &mut enable);
        assert_eq!(first, Ok(AutostartMove::Moved));
        assert_eq!(enabled.get(), 1);
        assert!(!entry.exists());

        // Every later start finds nothing to move.
        let second = move_legacy_autostart("MyMTGO", &entry, &mut enable);
        assert_eq!(second, Ok(AutostartMove::NothingToMove));
        assert_eq!(enabled.get(), 1);
    }

    #[test]
    fn the_linux_migration_is_inert_under_the_beta_name() {
        let dir = fixture_tools::TestDir::new("identity-autostart-inert");
        let entry = legacy_autostart_entry(&dir.join("home"));
        write_entry(&entry, ENABLED_ENTRY);
        let mut enable = || -> Result<(), String> { panic!("never enabled under the beta name") };

        let outcome = move_legacy_autostart(BETA_PRODUCT_NAME, &entry, &mut enable);
        assert_eq!(outcome, Ok(AutostartMove::Inert));
        assert_eq!(
            std::fs::read_to_string(&entry).expect("kept"),
            ENABLED_ENTRY
        );
    }

    #[test]
    fn a_failed_enable_keeps_the_old_entry_for_the_next_start() {
        let dir = fixture_tools::TestDir::new("identity-autostart-failed");
        let entry = legacy_autostart_entry(&dir.join("home"));
        write_entry(&entry, ENABLED_ENTRY);
        let mut enable = || Err("no autostart folder".to_string());

        let outcome = move_legacy_autostart("MyMTGO", &entry, &mut enable);
        assert_eq!(outcome, Err("no autostart folder".to_string()));
        assert!(entry.exists(), "kept, so the next start tries again");
    }

    #[test]
    fn a_switched_off_entry_is_removed_without_enabling() {
        // A desktop that switched the entry off (Hidden, or GNOME's flag)
        // keeps launch at login off under the new name.
        for switched_off in [
            "[Desktop Entry]\nName=MyMTGO Beta\nHidden=true\n",
            "[Desktop Entry]\nName=MyMTGO Beta\nX-GNOME-Autostart-enabled=false\n",
        ] {
            let dir = fixture_tools::TestDir::new("identity-autostart-off");
            let entry = legacy_autostart_entry(&dir.join("home"));
            write_entry(&entry, switched_off);
            let mut enable = || -> Result<(), String> { panic!("a switched off entry stays off") };

            let outcome = move_legacy_autostart("MyMTGO", &entry, &mut enable);
            assert_eq!(outcome, Ok(AutostartMove::RemovedSwitchedOff));
            assert!(!entry.exists());
        }
    }

    #[test]
    fn no_entry_means_nothing_to_move() {
        let dir = fixture_tools::TestDir::new("identity-autostart-none");
        let entry = legacy_autostart_entry(&dir.join("home"));
        let mut enable = || -> Result<(), String> { panic!("nothing to move") };
        assert_eq!(
            move_legacy_autostart("MyMTGO", &entry, &mut enable),
            Ok(AutostartMove::NothingToMove)
        );
    }

    /// Every file under `folder` with one of `extensions`.
    fn source_files(folder: &Path, extensions: &[&str], found: &mut Vec<std::path::PathBuf>) {
        for item in std::fs::read_dir(folder).expect("read folder") {
            let path = item.expect("entry").path();
            if path.is_dir() {
                source_files(&path, extensions, found);
            } else if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extensions.contains(&extension))
            {
                found.push(path);
            }
        }
    }

    #[test]
    fn the_beta_name_appears_only_in_the_migration() {
        // Release spec 13.1: identity follows the version and the product
        // name is read at runtime, so the old name is written only here
        // (the migration's constant). The installer hook and tauri.conf.json
        // are not app source.
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        source_files(&root.join("src"), &["rs"], &mut files);
        source_files(&root.join("../ui/src"), &["ts", "vue"], &mut files);
        let this_file = root.join("src").join("identity.rs");
        let mut offenders = Vec::new();
        for file in files {
            if file == this_file {
                continue;
            }
            let text = std::fs::read_to_string(&file).expect("read source");
            if text.contains(BETA_PRODUCT_NAME) {
                offenders.push(file.display().to_string());
            }
        }
        assert!(offenders.is_empty(), "the beta name in {offenders:?}");
    }
}
