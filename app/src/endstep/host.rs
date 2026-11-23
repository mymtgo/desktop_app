//! Registering the Endstep helper (endstep support spec) at every start: the
//! helper beside MyMTGO's exe (the installers ship it as a Tauri sidecar,
//! `app/tauri.bundle.conf.json`) is copied next to MyMTGO's data, so its inbox
//! is `<data_dir>/endstep/inbox` (`import::inbox_folder`), and the browsers
//! are told where it is: manifest files on macOS and Linux, `HKCU` registry
//! keys on Windows. The places and manifests are `endstep_capture::registration`.
//! Runs on its own thread; failures are logged, never fatal. Switching
//! Endstep off removes the manifests and keys again (`uninstall`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use endstep_capture::registration::{
    Family, Os, Place, Target, chrome_manifest, firefox_manifest, helper_file_name, targets,
};
use sha2::{Digest, Sha256};

use crate::settings::Settings;

/// What one install did.
#[derive(Debug, PartialEq, Eq)]
pub enum Installed {
    /// No helper beside MyMTGO (installers ship it as a sidecar; a dev build
    /// needs `cargo build -p endstep-host`): nothing was registered.
    HelperMissing,
    /// The helper is in place; `copied` is false when the copy was already
    /// the same file. `targets` names each browser registered.
    Registered { copied: bool, targets: Vec<String> },
}

/// The operating system this build runs on.
fn current_os() -> Os {
    if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::MacOs
    } else {
        Os::Linux
    }
}

/// The helper's folder inside the app's data folder; the helper finds its
/// inbox beside its own exe.
fn helper_folder(data_dir: &Path) -> PathBuf {
    data_dir.join("endstep")
}

/// Install and uninstall never run at once: a quick on then off must not let
/// the install finish last and leave the browsers registered.
static HOST_LOCK: Mutex<()> = Mutex::new(());

/// What the registration should be for a switch state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostAction {
    Install,
    Uninstall,
    /// Off, and nothing was ever installed: no file or registry work.
    Nothing,
}

/// The action that matches the Endstep switch: on installs, off uninstalls
/// (also at startup, so a stale registration, including the unconditional one
/// earlier builds made, is removed). Every install creates the helper folder
/// first, so off without it has nothing to remove: no `reg.exe` call per
/// browser at every start for a Windows user who never used Endstep.
pub fn action_for(endstep_enabled: bool, installed_before: bool) -> HostAction {
    if endstep_enabled {
        HostAction::Install
    } else if installed_before {
        HostAction::Uninstall
    } else {
        HostAction::Nothing
    }
}

/// Whether the helper folder (`<data dir>/endstep`) exists: an install, by
/// this build or an earlier one, has run.
pub fn helper_installed_before(data_dir: &Path) -> bool {
    helper_folder(data_dir).is_dir()
}

/// The stored `endstep_enabled` setting, read from the database file (off
/// when it cannot be read).
pub fn stored_enabled(db_path: &Path) -> bool {
    let Ok(conn) = store::open_reader(db_path) else {
        return false;
    };
    match store::read::settings(&conn) {
        Ok(rows) => Settings::from_rows(&rows).endstep_enabled,
        Err(_) => false,
    }
}

/// Registers or unregisters the helper on its own thread, so neither startup
/// nor the switch waits for it. Under `HOST_LOCK` the thread re-reads the
/// stored switch and does what matches it now, so the last switch wins
/// whatever order threads run in. `helper_missing` is left for the
/// Integrations page: true only when an install found no helper. `done` runs
/// on the thread once the registration has finished.
pub fn spawn_sync(
    data_dir: PathBuf,
    db_path: PathBuf,
    helper_missing: Arc<AtomicBool>,
    done: impl FnOnce() + Send + 'static,
) {
    let spawned = std::thread::Builder::new()
        .name("endstep-host".to_string())
        .spawn(move || {
            let one_at_a_time = HOST_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match action_for(stored_enabled(&db_path), helper_installed_before(&data_dir)) {
                HostAction::Install => match install(&data_dir) {
                    Ok(installed) => {
                        helper_missing
                            .store(installed == Installed::HelperMissing, Ordering::SeqCst);
                    }
                    Err(error) => {
                        helper_missing.store(false, Ordering::SeqCst);
                        tracing::warn!(%error, "Endstep helper not installed");
                    }
                },
                HostAction::Uninstall => {
                    helper_missing.store(false, Ordering::SeqCst);
                    match uninstall(&data_dir) {
                        Ok(removed) if removed.failed.is_empty() => {}
                        Ok(removed) => tracing::warn!(
                            failed = ?removed.failed,
                            "Endstep helper not fully unregistered"
                        ),
                        Err(error) => tracing::warn!(%error, "Endstep helper not unregistered"),
                    }
                }
                HostAction::Nothing => helper_missing.store(false, Ordering::SeqCst),
            }
            drop(one_at_a_time);
            done();
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "Endstep helper thread did not start");
    }
}

/// Copies the helper from beside MyMTGO's exe and registers it with every
/// browser for the current user.
pub fn install(data_dir: &Path) -> Result<Installed, String> {
    let exe = std::env::current_exe().map_err(|error| format!("MyMTGO's exe: {error}"))?;
    let exe_folder = exe
        .parent()
        .ok_or_else(|| format!("MyMTGO's exe has no folder: {}", exe.display()))?;
    let source = exe_folder.join(helper_file_name(current_os()));
    let home = std::env::home_dir().ok_or_else(|| "no home folder".to_string())?;
    install_from(&source, data_dir, &targets(current_os(), &home))
}

/// The log line for a missing helper. Installers ship it, so only a
/// developer's debug build gets the hint to build it.
fn missing_helper_message(debug: bool) -> &'static str {
    if debug {
        "Endstep helper not found beside MyMTGO (build it with `cargo build -p endstep-host`)"
    } else {
        "Endstep helper not found beside MyMTGO"
    }
}

/// `install` with the helper at `source` and the browser places `targets`.
pub fn install_from(
    source: &Path,
    data_dir: &Path,
    targets: &[Target],
) -> Result<Installed, String> {
    if !source.is_file() {
        tracing::warn!(
            source = %source.display(),
            "{}",
            missing_helper_message(cfg!(debug_assertions))
        );
        return Ok(Installed::HelperMissing);
    }
    let folder = helper_folder(data_dir);
    let file_name = source
        .file_name()
        .ok_or_else(|| format!("the helper has no file name: {}", source.display()))?;
    let helper = folder.join(file_name);
    let copied = copy_helper(source, &helper)?;

    // The manifests the registry keys point at, kept beside the helper.
    let hosts = folder.join("hosts");
    std::fs::create_dir_all(&hosts).map_err(|error| format!("{}: {error}", hosts.display()))?;
    let helper_text = helper.to_string_lossy().into_owned();
    let chromium_path = hosts.join("chromium.json");
    let firefox_path = hosts.join("firefox.json");
    write_file(&chromium_path, &chrome_manifest(&helper_text))?;
    write_file(&firefox_path, &firefox_manifest(&helper_text))?;

    let mut written = Vec::new();
    for target in targets {
        // A browser that is not installed has no config folder: do not
        // create one for it.
        if let Some(folder) = target.browser_folder()
            && !folder.is_dir()
        {
            tracing::debug!(
                browser = target.browser,
                "browser not installed; not registered"
            );
            continue;
        }
        let result = match &target.place {
            Place::File(path) => {
                let manifest = match target.family {
                    Family::Chromium => chrome_manifest(&helper_text),
                    Family::Firefox => firefox_manifest(&helper_text),
                };
                write_manifest(path, &manifest)
            }
            Place::Registry(key) => {
                let manifest_path = match target.family {
                    Family::Chromium => &chromium_path,
                    Family::Firefox => &firefox_path,
                };
                write_registry(key, manifest_path)
            }
        };
        match result {
            Ok(()) => written.push(target.browser.to_string()),
            Err(error) => {
                tracing::warn!(browser = target.browser, %error, "Endstep helper not registered");
            }
        }
    }
    tracing::info!(
        helper = %helper.display(),
        copied,
        targets = %written.join(", "),
        "Endstep helper registered"
    );
    Ok(Installed::Registered {
        copied,
        targets: written,
    })
}

/// What one uninstall did: the browsers whose registration is gone, and
/// those that could not be cleared, with why.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Removed {
    pub targets: Vec<&'static str>,
    pub failed: Vec<(&'static str, String)>,
}

/// Unregisters the helper from every browser for the current user. The
/// copied helper and its `hosts/` folder stay (they are inert once nothing
/// points at them, and a later switch on reuses them).
pub fn uninstall(data_dir: &Path) -> Result<Removed, String> {
    let home = std::env::home_dir().ok_or_else(|| "no home folder".to_string())?;
    let removed = uninstall_from(&targets(current_os(), &home));
    tracing::info!(
        data_dir = %data_dir.display(),
        targets = %removed.targets.join(", "),
        "Endstep helper unregistered"
    );
    Ok(removed)
}

/// `uninstall` for the browser places `targets`: each manifest file is
/// deleted and each registry key removed. A file or key that is already gone
/// counts as done.
pub fn uninstall_from(targets: &[Target]) -> Removed {
    let mut removed = Removed::default();
    for target in targets {
        let result = match &target.place {
            Place::File(path) => remove_manifest(path),
            Place::Registry(key) => delete_registry(key),
        };
        match result {
            Ok(()) => removed.targets.push(target.browser),
            Err(error) => removed.failed.push((target.browser, error)),
        }
    }
    removed
}

/// Deletes a manifest file; one that is not there is done.
fn remove_manifest(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

/// Copies `source` to `destination` unless it is already the same file.
/// Written beside it first and renamed into place, so a browser never starts
/// a half-written helper. Returns whether it copied.
fn copy_helper(source: &Path, destination: &Path) -> Result<bool, String> {
    let wanted = file_hash(source)?;
    if destination.is_file() && file_hash(destination).ok() == Some(wanted) {
        return Ok(false);
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    let mut new_name = destination.as_os_str().to_owned();
    new_name.push(".new");
    let new_path = PathBuf::from(new_name);
    std::fs::copy(source, &new_path).map_err(|error| format!("{}: {error}", new_path.display()))?;
    make_executable(&new_path)?;
    if let Err(error) = std::fs::rename(&new_path, destination) {
        let _ = std::fs::remove_file(&new_path);
        // On Windows a helper a browser is running cannot be replaced: the
        // old copy stays until the next start.
        if destination.is_file() {
            tracing::warn!(
                helper = %destination.display(),
                %error,
                "Endstep helper not updated; the previous copy stays"
            );
            return Ok(false);
        }
        return Err(format!("{}: {error}", destination.display()));
    }
    Ok(true)
}

/// The lowercase hex sha256 of a file's bytes.
fn file_hash(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let digest = Sha256::digest(&bytes);
    let mut hex = String::new();
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    Ok(hex)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn write_file(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|error| format!("{}: {error}", path.display()))
}

/// A browser's manifest file, its folder created first.
fn write_manifest(path: &Path, manifest: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    write_file(path, manifest)
}

/// A drive path (`C:\...`) or a UNC path (`\\server\...`), checked by text so it
/// behaves the same on every OS.
#[cfg_attr(not(windows), allow(dead_code))]
fn is_absolute_windows_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/');
    drive || path.starts_with("\\\\")
}

/// `reg.exe` by full path, so a planted `reg.exe` earlier on the search path
/// is never run: `<SystemRoot>\System32\reg.exe`, with `C:\Windows` when
/// `SystemRoot` is not set (or empty).
#[cfg_attr(not(windows), allow(dead_code))]
fn reg_exe_path(system_root: Option<&str>) -> PathBuf {
    let root = match system_root {
        // A relative root would resolve against the working directory.
        Some(root) if is_absolute_windows_path(root) => root.trim_end_matches(['\\', '/']),
        _ => "C:\\Windows",
    };
    PathBuf::from(format!("{root}\\System32\\reg.exe"))
}

/// Sets `key`'s default value to the manifest's path through `reg.exe`,
/// without a console window.
#[cfg(windows)]
fn write_registry(key: &str, manifest_path: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output =
        std::process::Command::new(reg_exe_path(std::env::var("SystemRoot").ok().as_deref()))
            .arg("add")
            .arg(key)
            .arg("/ve")
            .arg("/t")
            .arg("REG_SZ")
            .arg("/d")
            .arg(manifest_path)
            .arg("/f")
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|error| format!("reg.exe: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "reg.exe add {key}: {} {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Runs `reg.exe` by full path with `args`, without a console window, and
/// says whether it succeeded.
#[cfg(windows)]
fn run_reg(args: &[&str]) -> Result<bool, String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output =
        std::process::Command::new(reg_exe_path(std::env::var("SystemRoot").ok().as_deref()))
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|error| format!("reg.exe: {error}"))?;
    Ok(output.status.success())
}

/// Removes `key` through `reg.exe`. A key that is already gone is done:
/// when the delete fails, a query that finds nothing says so (the failure
/// text is localised, so it is not read).
#[cfg(windows)]
fn delete_registry(key: &str) -> Result<(), String> {
    if run_reg(&["delete", key, "/f"])? {
        return Ok(());
    }
    if run_reg(&["query", key])? {
        Err(format!("reg.exe delete {key} failed"))
    } else {
        Ok(())
    }
}

/// The registry exists only on Windows: no key to remove.
#[cfg(not(windows))]
fn delete_registry(_key: &str) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
fn write_registry(key: &str, _manifest_path: &Path) -> Result<(), String> {
    Err(format!("{key}: the registry exists only on Windows"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fixture_tools::TestDir;

    fn file_targets(dir: &TestDir) -> Vec<Target> {
        // The browsers' config folders exist: they are installed.
        std::fs::create_dir_all(dir.join("chrome")).unwrap();
        std::fs::create_dir_all(dir.join("mozilla")).unwrap();
        vec![
            Target {
                browser: "Chrome",
                family: Family::Chromium,
                place: Place::File(dir.join("chrome/NativeMessagingHosts/com.mymtgo.endstep.json")),
            },
            Target {
                browser: "Firefox",
                family: Family::Firefox,
                place: Place::File(
                    dir.join("mozilla/native-messaging-hosts/com.mymtgo.endstep.json"),
                ),
            },
        ]
    }

    fn json(path: &Path) -> serde_json::Value {
        let text = std::fs::read_to_string(path).expect("manifest written");
        serde_json::from_str(&text).expect("manifest is JSON")
    }

    #[test]
    fn uninstall_removes_manifests_and_keeps_the_helper() {
        let dir = TestDir::new("endstep-host-uninstall");
        let source = dir.write("app/mymtgo-endstep-host", b"helper v1");
        let data_dir = dir.join("data");
        let targets = file_targets(&dir);
        install_from(&source, &data_dir, &targets).expect("install");
        let chrome = dir.join("chrome/NativeMessagingHosts/com.mymtgo.endstep.json");
        let firefox = dir.join("mozilla/native-messaging-hosts/com.mymtgo.endstep.json");
        assert!(chrome.is_file() && firefox.is_file());

        let removed = uninstall_from(&targets);

        assert_eq!(
            removed,
            Removed {
                targets: vec!["Chrome", "Firefox"],
                failed: Vec::new(),
            }
        );
        assert!(!chrome.exists() && !firefox.exists());
        assert!(data_dir.join("endstep/mymtgo-endstep-host").is_file());
        assert!(data_dir.join("endstep/hosts/chromium.json").is_file());
        assert!(data_dir.join("endstep/hosts/firefox.json").is_file());

        // Nothing left to delete is still done.
        let again = uninstall_from(&targets);
        assert_eq!(again, removed);
    }

    #[test]
    fn the_action_follows_the_switch_and_startup_off_uninstalls() {
        assert_eq!(action_for(true, false), HostAction::Install);
        assert_eq!(action_for(true, true), HostAction::Install);
        assert_eq!(action_for(false, true), HostAction::Uninstall);
        // Off with no helper folder: nothing was ever installed, so nothing
        // runs (no reg.exe call per browser at every start on Windows).
        assert_eq!(action_for(false, false), HostAction::Nothing);
        // A fresh database (switch never turned on) reads off, so startup
        // removes any stale registration, when one can exist.
        let dir = TestDir::new("endstep-host-stored-switch");
        let (state, _events) = crate::commands::tests::ready_state(&dir);
        assert!(!stored_enabled(&state.db_path));
        assert!(!helper_installed_before(&state.data_dir));
        assert_eq!(
            action_for(
                stored_enabled(&state.db_path),
                helper_installed_before(&state.data_dir)
            ),
            HostAction::Nothing
        );
        std::fs::create_dir_all(state.data_dir.join("endstep")).expect("helper folder");
        assert!(helper_installed_before(&state.data_dir));
        assert_eq!(
            action_for(
                stored_enabled(&state.db_path),
                helper_installed_before(&state.data_dir)
            ),
            HostAction::Uninstall
        );
        state
            .save_setting_rows(vec![("endstep_enabled".to_string(), "true".to_string())])
            .expect("saved");
        assert!(stored_enabled(&state.db_path));
        assert_eq!(
            action_for(stored_enabled(&state.db_path), true),
            HostAction::Install
        );
        assert!(!stored_enabled(&dir.join("nope/missing.sqlite")));
        crate::platform::tests::stop(&state);
    }

    #[test]
    fn reg_exe_is_run_by_full_path() {
        assert_eq!(
            reg_exe_path(Some("D:\\WINNT")),
            PathBuf::from("D:\\WINNT\\System32\\reg.exe")
        );
        assert_eq!(
            reg_exe_path(Some("C:\\Windows\\")),
            PathBuf::from("C:\\Windows\\System32\\reg.exe")
        );
        assert_eq!(
            reg_exe_path(None),
            PathBuf::from("C:\\Windows\\System32\\reg.exe")
        );
        for relative in ["Windows", "System32\\x", "C:", "\\Windows"] {
            assert_eq!(
                reg_exe_path(Some(relative)),
                PathBuf::from("C:\\Windows\\System32\\reg.exe"),
                "{relative}"
            );
        }
        assert_eq!(
            reg_exe_path(Some("")),
            PathBuf::from("C:\\Windows\\System32\\reg.exe")
        );
    }

    #[test]
    fn install_copies_the_helper_and_writes_manifests() {
        let dir = TestDir::new("endstep-host-install");
        let source = dir.write("app/mymtgo-endstep-host", b"helper v1");
        let data_dir = dir.join("data");
        let targets = file_targets(&dir);

        let installed = install_from(&source, &data_dir, &targets).expect("install");

        assert_eq!(
            installed,
            Installed::Registered {
                copied: true,
                targets: vec!["Chrome".to_string(), "Firefox".to_string()],
            }
        );
        let helper = data_dir.join("endstep/mymtgo-endstep-host");
        assert_eq!(std::fs::read(&helper).expect("helper copied"), b"helper v1");
        assert!(!data_dir.join("endstep/mymtgo-endstep-host.new").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&helper)
                .expect("helper")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755);
        }
        let helper_text = helper.to_string_lossy().into_owned();

        let chromium = json(&data_dir.join("endstep/hosts/chromium.json"));
        assert_eq!(chromium["path"], helper_text.as_str());
        assert!(chromium["allowed_origins"].is_array());
        let firefox = json(&data_dir.join("endstep/hosts/firefox.json"));
        assert_eq!(firefox["path"], helper_text.as_str());
        assert!(firefox["allowed_extensions"].is_array());

        let chrome_target = json(&dir.join("chrome/NativeMessagingHosts/com.mymtgo.endstep.json"));
        assert_eq!(chrome_target, chromium);
        let firefox_target =
            json(&dir.join("mozilla/native-messaging-hosts/com.mymtgo.endstep.json"));
        assert_eq!(firefox_target, firefox);
    }

    #[test]
    fn an_unchanged_helper_is_not_copied_again() {
        let dir = TestDir::new("endstep-host-unchanged");
        let source = dir.write("app/mymtgo-endstep-host", b"helper v1");
        let data_dir = dir.join("data");
        let targets = file_targets(&dir);
        install_from(&source, &data_dir, &targets).expect("first install");
        let helper = data_dir.join("endstep/mymtgo-endstep-host");
        let modified = std::fs::metadata(&helper)
            .and_then(|meta| meta.modified())
            .expect("modified time");
        std::thread::sleep(std::time::Duration::from_millis(20));

        let installed = install_from(&source, &data_dir, &targets).expect("second install");

        assert_eq!(
            installed,
            Installed::Registered {
                copied: false,
                targets: vec!["Chrome".to_string(), "Firefox".to_string()],
            }
        );
        let again = std::fs::metadata(&helper)
            .and_then(|meta| meta.modified())
            .expect("modified time");
        assert_eq!(again, modified);
    }

    #[test]
    fn a_changed_helper_is_copied_again() {
        let dir = TestDir::new("endstep-host-changed");
        let source = dir.write("app/mymtgo-endstep-host", b"helper v1");
        let data_dir = dir.join("data");
        let targets = file_targets(&dir);
        install_from(&source, &data_dir, &targets).expect("first install");
        dir.write("app/mymtgo-endstep-host", b"helper v2");

        let installed = install_from(&source, &data_dir, &targets).expect("second install");

        assert!(matches!(
            installed,
            Installed::Registered { copied: true, .. }
        ));
        let helper = data_dir.join("endstep/mymtgo-endstep-host");
        assert_eq!(std::fs::read(&helper).expect("helper"), b"helper v2");
    }

    #[test]
    fn a_missing_helper_registers_nothing() {
        let dir = TestDir::new("endstep-host-missing");
        let data_dir = dir.join("data");
        let targets = file_targets(&dir);

        let installed =
            install_from(&dir.join("app/mymtgo-endstep-host"), &data_dir, &targets).expect("ok");

        assert_eq!(installed, Installed::HelperMissing);
        assert!(!data_dir.join("endstep").exists());
        assert!(!dir.join("chrome/NativeMessagingHosts").exists());
    }

    #[test]
    fn a_missing_helper_says_so_without_a_build_hint() {
        assert_eq!(
            missing_helper_message(false),
            "Endstep helper not found beside MyMTGO"
        );
        assert!(missing_helper_message(true).contains("cargo build -p endstep-host"));
    }

    fn config(file: &str) -> serde_json::Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(file);
        let text = std::fs::read_to_string(&path).expect("config file");
        serde_json::from_str(&text).expect("config is JSON")
    }

    /// Only installers carry the helper: Tauri's build script copies every
    /// declared sidecar and fails when one is missing, so the base config
    /// must not declare it (local builds and `tauri dev` never build it).
    #[test]
    fn the_base_config_has_no_sidecar() {
        assert!(config("tauri.conf.json")["bundle"]["externalBin"].is_null());
        assert_eq!(
            config("tauri.bundle.conf.json"),
            serde_json::json!({ "bundle": { "externalBin": ["binaries/mymtgo-endstep-host"] } })
        );
    }

    #[test]
    fn a_failed_target_does_not_stop_the_others() {
        let dir = TestDir::new("endstep-host-failed-target");
        let source = dir.write("app/mymtgo-endstep-host", b"helper v1");
        let data_dir = dir.join("data");
        // A file where the first target's folder should be.
        dir.write("blocked", b"not a folder");
        let mut targets = vec![Target {
            browser: "Blocked",
            family: Family::Chromium,
            place: Place::File(dir.join("blocked/NativeMessagingHosts/com.mymtgo.endstep.json")),
        }];
        targets.extend(file_targets(&dir));

        let installed = install_from(&source, &data_dir, &targets).expect("install");

        assert_eq!(
            installed,
            Installed::Registered {
                copied: true,
                targets: vec!["Chrome".to_string(), "Firefox".to_string()],
            }
        );
    }

    #[test]
    fn a_browser_that_is_not_installed_gets_no_folder() {
        let dir = TestDir::new("endstep-host-not-installed");
        let source = dir.write("app/mymtgo-endstep-host", b"helper v1");
        let data_dir = dir.join("data");
        let targets = file_targets(&dir);
        std::fs::remove_dir_all(dir.join("mozilla")).unwrap();

        let installed = install_from(&source, &data_dir, &targets).expect("install");

        assert_eq!(
            installed,
            Installed::Registered {
                copied: true,
                targets: vec!["Chrome".to_string()],
            }
        );
        assert!(!dir.join("mozilla").exists());
    }
}
