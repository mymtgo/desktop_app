//! Downloading, verifying and installing the pinned recorder build. The
//! network is a closure (`fetch`), so tests need no network. Nothing
//! unverified ever sits in `<root>/recorder/<version>/`: files land in a
//! `.download-<version>` folder first and the folder is renamed only after
//! every hash matches.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use recorder::{Pin, PinnedFile, download_blocked, file_url};
use sha2::{Digest, Sha256};

/// Why an install did not finish.
#[derive(Debug, PartialEq, Eq)]
pub enum InstallError {
    /// The pin has no hashes yet, so nothing may be downloaded.
    Blocked,
    /// Offline mode is on (the caller decides; kept for D7).
    Offline,
    /// A download failed.
    Fetch(String),
    /// A downloaded file did not match its pin (the file name).
    HashMismatch(String),
    /// Folders could not be created, written or renamed.
    Move(String),
}

/// How many times the final rename is tried.
const RENAME_TRIES: u32 = 5;

/// The wait between rename tries.
const RENAME_WAIT: Duration = Duration::from_millis(500);

/// Download every pinned file and install them (see the module docs).
pub fn install(
    pin: &Pin,
    root: &Path,
    fetch: &dyn Fn(&str) -> Result<Vec<u8>, String>,
) -> Result<PathBuf, InstallError> {
    install_with_wait(pin, root, fetch, RENAME_WAIT)
}

fn install_with_wait(
    pin: &Pin,
    root: &Path,
    fetch: &dyn Fn(&str) -> Result<Vec<u8>, String>,
    rename_wait: Duration,
) -> Result<PathBuf, InstallError> {
    if download_blocked(pin) {
        return Err(InstallError::Blocked);
    }
    let base = root.join("recorder");
    let staging = base.join(format!(".download-{}", pin.version));
    let target = base.join(pin.version);

    // A leftover from an interrupted run starts over.
    remove_dir_if_present(&staging)?;
    fs::create_dir_all(&staging).map_err(move_error)?;

    if let Err(error) = download_all(pin, &staging, fetch) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }

    remove_dir_if_present(&target)?;
    if let Err(error) = rename_with_retries(&staging, &target, rename_wait) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    remove_other_folders(&base, pin.version);
    Ok(target)
}

fn download_all(
    pin: &Pin,
    staging: &Path,
    fetch: &dyn Fn(&str) -> Result<Vec<u8>, String>,
) -> Result<(), InstallError> {
    for file in pin.files {
        let bytes = fetch(&file_url(pin, file)).map_err(InstallError::Fetch)?;
        if sha256_hex(&bytes) != file.sha256 {
            return Err(InstallError::HashMismatch(file.name.to_string()));
        }
        fs::write(staging.join(file.name), &bytes).map_err(move_error)?;
    }
    Ok(())
}

fn rename_with_retries(from: &Path, to: &Path, wait: Duration) -> Result<(), InstallError> {
    let mut last = String::new();
    for attempt in 0..RENAME_TRIES {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error) => last = error.to_string(),
        }
        if attempt + 1 < RENAME_TRIES {
            std::thread::sleep(wait);
        }
    }
    Err(InstallError::Move(last))
}

/// Delete every other version folder and stale `.download-*` folder.
/// Best effort: a folder in use (Windows) stays for the next install.
fn remove_other_folders(base: &Path, keep: &str) {
    let Ok(entries) = fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name != keep && entry.path().is_dir() {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

fn remove_dir_if_present(path: &Path) -> Result<(), InstallError> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(move_error(error)),
    }
}

fn move_error(error: std::io::Error) -> InstallError {
    InstallError::Move(error.to_string())
}

/// The installed exe, only when it exists and still matches its pin. Hashes
/// the exe on every call (antivirus or tampering may change it).
pub fn verified_exe(pin: &Pin, root: &Path) -> Option<PathBuf> {
    let file: &PinnedFile = pin.files.iter().find(|file| file.name == pin.exe)?;
    let path = root.join("recorder").join(pin.version).join(pin.exe);
    // Streamed through the hasher: the exe is too large to read whole.
    let mut opened = fs::File::open(&path).ok()?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut opened, &mut hasher).ok()?;
    (hex(&hasher.finalize()) == file.sha256).then_some(path)
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(digest: &[u8]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    const EXE_BYTES: &[u8] = b"pretend exe";
    const NOTICE_BYTES: &[u8] = b"pretend notice";

    fn pin_with(version: &'static str, hashes: [String; 2]) -> Pin {
        let [exe_hash, notice_hash] = hashes;
        let files: Vec<PinnedFile> = vec![
            PinnedFile {
                name: "mtgr-record.exe",
                sha256: Box::leak(exe_hash.into_boxed_str()),
            },
            PinnedFile {
                name: "NOTICE",
                sha256: Box::leak(notice_hash.into_boxed_str()),
            },
        ];
        Pin {
            version,
            consent_version: 1,
            exe: "mtgr-record.exe",
            files: Box::leak(files.into_boxed_slice()),
        }
    }

    fn good_pin(version: &'static str) -> Pin {
        pin_with(version, [sha256_hex(EXE_BYTES), sha256_hex(NOTICE_BYTES)])
    }

    fn serve(url: &str) -> Result<Vec<u8>, String> {
        if url.ends_with("/mtgr-record.exe") {
            Ok(EXE_BYTES.to_vec())
        } else if url.ends_with("/NOTICE") {
            Ok(NOTICE_BYTES.to_vec())
        } else {
            Err(format!("no such file: {url}"))
        }
    }

    fn temp_root() -> PathBuf {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "mymtgo-install-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn fast_install(pin: &Pin, root: &Path) -> Result<PathBuf, InstallError> {
        install_with_wait(pin, root, &serve, Duration::from_millis(1))
    }

    #[test]
    fn an_empty_pin_hash_blocks_install() {
        let root = temp_root();
        let pin = pin_with("9.0.0", [String::new(), sha256_hex(NOTICE_BYTES)]);
        let result = fast_install(&pin, &root);
        assert_eq!(result, Err(InstallError::Blocked));
        assert!(!root.join("recorder").exists());
    }

    #[test]
    fn a_wrong_hash_installs_nothing() {
        let root = temp_root();
        let pin = pin_with("9.0.0", [sha256_hex(EXE_BYTES), sha256_hex(b"other")]);
        let result = fast_install(&pin, &root);
        assert_eq!(result, Err(InstallError::HashMismatch("NOTICE".into())));
        assert!(!root.join("recorder/9.0.0").exists());
        assert!(!root.join("recorder/.download-9.0.0").exists());
    }

    #[test]
    fn a_failed_fetch_installs_nothing() {
        let root = temp_root();
        let pin = good_pin("9.0.0");
        let result = install_with_wait(
            &pin,
            &root,
            &|_url| Err("connection reset".to_string()),
            Duration::from_millis(1),
        );
        assert_eq!(result, Err(InstallError::Fetch("connection reset".into())));
        assert!(!root.join("recorder/9.0.0").exists());
        assert!(!root.join("recorder/.download-9.0.0").exists());
    }

    #[test]
    fn a_good_download_lands_in_the_version_folder() {
        let root = temp_root();
        let pin = good_pin("9.0.0");
        let folder = fast_install(&pin, &root).unwrap();
        assert_eq!(folder, root.join("recorder/9.0.0"));
        assert_eq!(fs::read(folder.join("mtgr-record.exe")).unwrap(), EXE_BYTES);
        assert_eq!(fs::read(folder.join("NOTICE")).unwrap(), NOTICE_BYTES);
        assert!(!root.join("recorder/.download-9.0.0").exists());
        assert_eq!(
            verified_exe(&pin, &root),
            Some(folder.join("mtgr-record.exe"))
        );
    }

    #[test]
    fn older_versions_are_removed() {
        let root = temp_root();
        fs::create_dir_all(root.join("recorder/8.0.0")).unwrap();
        fs::write(root.join("recorder/8.0.0/mtgr-record.exe"), b"old").unwrap();
        fs::create_dir_all(root.join("recorder/.download-7.0.0")).unwrap();
        let pin = good_pin("9.0.0");
        fast_install(&pin, &root).unwrap();
        assert!(!root.join("recorder/8.0.0").exists());
        assert!(!root.join("recorder/.download-7.0.0").exists());
        assert!(root.join("recorder/9.0.0/mtgr-record.exe").exists());
    }

    #[test]
    fn an_exe_that_no_longer_matches_its_pin_is_not_started() {
        let root = temp_root();
        let pin = good_pin("9.0.0");
        let folder = fast_install(&pin, &root).unwrap();
        let mut changed = EXE_BYTES.to_vec();
        changed[0] ^= 1;
        fs::write(folder.join("mtgr-record.exe"), changed).unwrap();
        assert_eq!(verified_exe(&pin, &root), None);
    }

    #[test]
    fn a_missing_exe_is_not_verified() {
        let root = temp_root();
        let pin = good_pin("9.0.0");
        assert_eq!(verified_exe(&pin, &root), None);
    }
}
