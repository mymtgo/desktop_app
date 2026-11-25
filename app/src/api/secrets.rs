//! Where the device key and the tokens live (07 spec section 4): the OS
//! keychain through `keyring`, or, when the keychain does not work (Linux
//! without a Secret Service), an obfuscated file in the data folder (spec
//! ruling 9: friction, not security, as foundation 4.5 allows).
//!
//! Like Laravel's `Crypt` over `settings.json` in 0.x, but the values sit in
//! the keychain; 0.x's encrypted values cannot be read (no `APP_KEY`), so v1
//! starts signed out.

use std::collections::BTreeMap;
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use sha2::{Digest, Sha256};

use crate::api::random::random_bytes;
use crate::bridge::lock;

/// The keychain service every entry is stored under: the app identifier, so a
/// dev build never reads or overwrites the installed app's sign-in.
pub const SERVICE: &str = crate::identity::APP_IDENTIFIER;
pub const API_KEY: &str = "api_key";
pub const ACCESS_TOKEN: &str = "access_token";
pub const REFRESH_TOKEN: &str = "refresh_token";
/// Every secret the app keeps.
pub const SECRET_NAMES: [&str; 3] = [API_KEY, ACCESS_TOKEN, REFRESH_TOKEN];
/// The entry written, read and deleted once to see whether the keychain works.
pub const PROBE: &str = "probe";
/// `MYMTGO_SECRETS_FILE=1` skips the keychain (debug builds only: dev and
/// tests; `force_file_from_env`).
pub const FORCE_FILE_ENV: &str = "MYMTGO_SECRETS_FILE";
pub const SECRETS_FILE: &str = "secrets.bin";
pub const KEY_FILE: &str = "secrets.key";

const FORMAT_VERSION: u8 = 1;
const NONCE_LEN: usize = 16;
const TAG_LEN: usize = 32;
const KEY_LEN: usize = 32;

/// Why a secret could not be read or written. Never holds a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretError(pub String);

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for SecretError {}

/// A place for named secrets. `get` of a missing name is `Ok(None)`;
/// `delete` of a missing name is `Ok(())`.
pub trait SecretStore {
    fn get(&self, name: &str) -> Result<Option<String>, SecretError>;
    fn set(&self, name: &str, value: &str) -> Result<(), SecretError>;
    fn delete(&self, name: &str) -> Result<(), SecretError>;
}

/// The OS keychain: macOS Keychain, Windows Credential Manager, the Secret
/// Service on Linux. Blocking: never call it from an async task.
pub struct KeyringStore;

impl KeyringStore {
    fn entry(name: &str) -> Result<keyring::Entry, SecretError> {
        keyring::Entry::new(SERVICE, name).map_err(keyring_error)
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, name: &str) -> Result<Option<String>, SecretError> {
        match Self::entry(name)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(keyring_error(error)),
        }
    }

    fn set(&self, name: &str, value: &str) -> Result<(), SecretError> {
        Self::entry(name)?
            .set_password(value)
            .map_err(keyring_error)
    }

    fn delete(&self, name: &str) -> Result<(), SecretError> {
        match Self::entry(name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(keyring_error(error)),
        }
    }
}

/// A keychain error without the stored bytes some variants carry.
fn keyring_error(error: keyring::Error) -> SecretError {
    let message = match error {
        keyring::Error::BadEncoding(_) | keyring::Error::BadDataFormat(_, _) => {
            "the keychain holds a value that is not text".to_string()
        }
        other => format!("keychain: {other}"),
    };
    SecretError(message)
}

/// The fallback: `secrets.bin` (byte 1, a 16-byte nonce, the JSON map XOR
/// a sha256 keystream, a 32-byte sha256 tag) and `secrets.key` (32 random
/// bytes), both `0600` on unix. A file whose tag does not match reads as
/// empty.
pub struct FileStore {
    dir: PathBuf,
    /// One read-modify-write at a time.
    writing: Mutex<()>,
}

impl FileStore {
    pub fn new(dir: &Path) -> FileStore {
        FileStore {
            dir: dir.to_path_buf(),
            writing: Mutex::new(()),
        }
    }

    fn secrets_path(&self) -> PathBuf {
        self.dir.join(SECRETS_FILE)
    }

    fn key_path(&self) -> PathBuf {
        self.dir.join(KEY_FILE)
    }

    /// Whether a secrets file is there to move into a working keychain.
    pub fn exists(&self) -> bool {
        self.secrets_path().exists()
    }

    /// Every stored secret; empty when there is no file or it was tampered
    /// with.
    pub fn read_all(&self) -> Result<BTreeMap<String, String>, SecretError> {
        let secrets_path = self.secrets_path();
        let bytes = match std::fs::read(&secrets_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(BTreeMap::new());
            }
            Err(error) => return Err(io_error("read the secrets file", &error)),
        };
        // A file opened up since it was written (a copy, a restore, a
        // chmod) is made owner-only again.
        keep_private(&secrets_path);
        keep_private(&self.key_path());
        let Some(key) = self.read_key()? else {
            tracing::warn!("the secrets file has no key file; reading it as empty");
            return Ok(BTreeMap::new());
        };
        match open_sealed(&key, &bytes) {
            Some(map) => Ok(map),
            None => {
                tracing::warn!("the secrets file does not match its key; reading it as empty");
                Ok(BTreeMap::new())
            }
        }
    }

    fn write_all(&self, map: &BTreeMap<String, String>) -> Result<(), SecretError> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|error| io_error("create the data folder", &error))?;
        let key = match self.read_key()? {
            Some(key) => key,
            None => {
                let key = random_bytes(KEY_LEN);
                write_private(&self.key_path(), &key)?;
                key
            }
        };
        let nonce = random_bytes(NONCE_LEN);
        let plain = serde_json::to_vec(map)
            .map_err(|_| SecretError("could not encode the secrets".to_string()))?;
        write_private(&self.secrets_path(), &seal(&key, &nonce, &plain))
    }

    /// The key file's 32 bytes; `None` when it is missing or the wrong size.
    fn read_key(&self) -> Result<Option<Vec<u8>>, SecretError> {
        match std::fs::read(self.key_path()) {
            Ok(key) if key.len() == KEY_LEN => Ok(Some(key)),
            Ok(_) => Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_error("read the secrets key file", &error)),
        }
    }

    /// Delete both files (after their secrets moved into the keychain).
    pub fn remove_files(&self) -> Result<(), SecretError> {
        for path in [self.secrets_path(), self.key_path()] {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(io_error("delete a secrets file", &error)),
            }
        }
        Ok(())
    }
}

impl SecretStore for FileStore {
    fn get(&self, name: &str) -> Result<Option<String>, SecretError> {
        Ok(self.read_all()?.remove(name))
    }

    fn set(&self, name: &str, value: &str) -> Result<(), SecretError> {
        let _one = lock(&self.writing);
        let mut map = self.read_all()?;
        map.insert(name.to_string(), value.to_string());
        self.write_all(&map)
    }

    fn delete(&self, name: &str) -> Result<(), SecretError> {
        let _one = lock(&self.writing);
        let mut map = self.read_all()?;
        if map.remove(name).is_some() {
            self.write_all(&map)?;
        }
        Ok(())
    }
}

/// Set a secrets file back to `0600` on unix (nothing elsewhere). A file
/// that cannot be changed is logged and still read.
fn keep_private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let private = std::fs::Permissions::from_mode(0o600);
        match std::fs::set_permissions(path, private) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(%error, "could not make a secrets file owner-only"),
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Whether `MYMTGO_SECRETS_FILE` (its value, if set) skips the keychain:
/// only `1`, and only in a debug build, so another program cannot move a
/// release build's secrets out of the keychain. `debug` is
/// `cfg!(debug_assertions)`.
pub fn force_file_from_env(value: Option<&str>, debug: bool) -> bool {
    debug && value == Some("1")
}

fn io_error(what: &str, error: &std::io::Error) -> SecretError {
    SecretError(format!("could not {what}: {error}"))
}

/// Write `bytes` to a temp file beside `path` (owner-only on unix), then
/// rename it over `path`, so a crash never leaves half a file.
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), SecretError> {
    let mut temp_name = path.file_name().unwrap_or_default().to_os_string();
    temp_name.push(".tmp");
    let temp = path.with_file_name(temp_name);
    let _ = std::fs::remove_file(&temp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temp)
        .map_err(|error| io_error("create a secrets file", &error))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| io_error("write a secrets file", &error))?;
    drop(file);
    std::fs::rename(&temp, path).map_err(|error| io_error("replace a secrets file", &error))
}

/// `sha256(key || nonce || counter as u64 big-endian)` blocks, XORed over
/// `data` (the same call seals and opens).
fn keystream_xor(key: &[u8], nonce: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for (counter, chunk) in data.chunks(32).enumerate() {
        let mut hasher = Sha256::new();
        hasher.update(key);
        hasher.update(nonce);
        hasher.update((counter as u64).to_be_bytes());
        let block = hasher.finalize();
        out.extend(
            chunk
                .iter()
                .zip(block.iter())
                .map(|(byte, mask)| byte ^ mask),
        );
    }
    out
}

/// `sha256(key || nonce || ciphertext)`.
fn tag(key: &[u8], nonce: &[u8], ciphertext: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(key);
    hasher.update(nonce);
    hasher.update(ciphertext);
    hasher.finalize().to_vec()
}

/// The file's bytes for `plain`.
fn seal(key: &[u8], nonce: &[u8], plain: &[u8]) -> Vec<u8> {
    let ciphertext = keystream_xor(key, nonce, plain);
    let mut out = vec![FORMAT_VERSION];
    out.extend_from_slice(nonce);
    out.extend_from_slice(&ciphertext);
    out.extend(tag(key, nonce, &ciphertext));
    out
}

/// The map in a sealed file, or `None` for a wrong version, a short file, a
/// bad tag or bad JSON.
fn open_sealed(key: &[u8], bytes: &[u8]) -> Option<BTreeMap<String, String>> {
    if bytes.len() < 1 + NONCE_LEN + TAG_LEN || bytes[0] != FORMAT_VERSION {
        return None;
    }
    let nonce = &bytes[1..1 + NONCE_LEN];
    let ciphertext = &bytes[1 + NONCE_LEN..bytes.len() - TAG_LEN];
    let stored_tag = &bytes[bytes.len() - TAG_LEN..];
    if !same_bytes(stored_tag, &tag(key, nonce, ciphertext)) {
        return None;
    }
    serde_json::from_slice(&keystream_xor(key, nonce, ciphertext)).ok()
}

/// Equal without stopping at the first difference.
fn same_bytes(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

/// Where secrets go when there is no data folder (the OS gave none): reads
/// find nothing and writes fail, so nothing lands in the working directory.
/// The session still keeps values in memory for this run.
pub struct NoStore;

impl SecretStore for NoStore {
    fn get(&self, _: &str) -> Result<Option<String>, SecretError> {
        Ok(None)
    }

    fn set(&self, _: &str, _: &str) -> Result<(), SecretError> {
        Err(SecretError(
            "there is no data folder to keep secrets in".to_string(),
        ))
    }

    fn delete(&self, _: &str) -> Result<(), SecretError> {
        Ok(())
    }
}

/// The file store in the data folder, or `NoStore` without one.
fn file_or_nothing(data_dir: Option<&Path>) -> Box<dyn SecretStore + Send + Sync> {
    match data_dir {
        Some(dir) => Box::new(FileStore::new(dir)),
        None => {
            tracing::warn!("no data folder; secrets are kept in memory for this run only");
            Box::new(NoStore)
        }
    }
}

/// The secret store for this run. `force_file` (`MYMTGO_SECRETS_FILE=1`)
/// uses the file without touching the keychain. Otherwise the keychain is
/// probed once (write, read, delete `probe`); any error falls back to the
/// file with one warning. With a working keychain, a secrets file left by an
/// earlier run moves into it and is deleted (also when the move fails part
/// way: a stale copy must never come back after a sign-out). `data_dir` is `None` when the
/// OS gave no data folder: then there is no file store at all.
pub fn open_secrets(
    data_dir: Option<&Path>,
    force_file: bool,
) -> Box<dyn SecretStore + Send + Sync> {
    if force_file {
        return file_or_nothing(data_dir);
    }
    // Choose the platform store once, up front.
    if let Err(error) = keyring::Entry::store_status() {
        tracing::warn!(%error, "no OS keychain; keeping secrets in the data folder");
        return file_or_nothing(data_dir);
    }
    open_secrets_with(data_dir, false, Box::new(KeyringStore))
}

/// `open_secrets` with the keychain given (tests pass a double).
pub fn open_secrets_with(
    data_dir: Option<&Path>,
    force_file: bool,
    keychain: Box<dyn SecretStore + Send + Sync>,
) -> Box<dyn SecretStore + Send + Sync> {
    if force_file {
        return file_or_nothing(data_dir);
    }
    if let Err(error) = probe(keychain.as_ref()) {
        tracing::warn!(%error, "the OS keychain does not work; keeping secrets in the data folder");
        return file_or_nothing(data_dir);
    }
    if let Some(dir) = data_dir {
        let file = FileStore::new(dir);
        if file.exists()
            && let Err(error) = move_into(&file, keychain.as_ref())
        {
            // The keychain is the store from now on, so what is left in the
            // file is stale: kept, a later start would move tokens back in
            // after a sign-out. It is deleted, at the cost of signing in
            // again for whatever did not move.
            tracing::warn!(%error, "could not move the secrets file into the keychain; deleting it");
            if let Err(error) = file.remove_files() {
                tracing::warn!(%error, "could not delete the secrets file");
            }
        }
    }
    keychain
}

/// Write, read and delete a random probe value. The delete is always tried,
/// so a failed read never leaves the probe in the user's keychain.
fn probe(keychain: &dyn SecretStore) -> Result<(), SecretError> {
    let value = format!(
        "probe-{}",
        random_bytes(4)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    keychain.set(PROBE, &value)?;
    let read = keychain.get(PROBE);
    let deleted = keychain.delete(PROBE);
    let read = read?;
    deleted?;
    if read.as_deref() != Some(value.as_str()) {
        return Err(SecretError(
            "the keychain did not return what was stored".to_string(),
        ));
    }
    Ok(())
}

/// Copy the file's secrets into the keychain, then delete the files. A name
/// the keychain already holds is skipped: after a move that failed part way,
/// this run may have stored newer values there (a rotated refresh token),
/// and the file's copy is stale.
fn move_into(file: &FileStore, keychain: &dyn SecretStore) -> Result<(), SecretError> {
    for (name, value) in file.read_all()? {
        if keychain.get(&name)?.is_some() {
            continue;
        }
        keychain.set(&name, &value)?;
    }
    file.remove_files()?;
    tracing::info!("moved the secrets file into the OS keychain");
    Ok(())
}

/// Secrets in memory, for tests of everything above this module.
#[cfg(test)]
#[derive(Default)]
pub struct MemoryStore {
    pub values: Mutex<BTreeMap<String, String>>,
    /// How many times `get` was called.
    pub gets: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
impl MemoryStore {
    /// How many secrets were read so far.
    pub fn reads(&self) -> usize {
        self.gets.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// A shared `MemoryStore`, so a test can look inside after handing it over.
#[cfg(test)]
impl SecretStore for std::sync::Arc<MemoryStore> {
    fn get(&self, name: &str) -> Result<Option<String>, SecretError> {
        self.as_ref().get(name)
    }

    fn set(&self, name: &str, value: &str) -> Result<(), SecretError> {
        self.as_ref().set(name, value)
    }

    fn delete(&self, name: &str) -> Result<(), SecretError> {
        self.as_ref().delete(name)
    }
}

#[cfg(test)]
impl SecretStore for MemoryStore {
    fn get(&self, name: &str) -> Result<Option<String>, SecretError> {
        self.gets.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(lock(&self.values).get(name).cloned())
    }

    fn set(&self, name: &str, value: &str) -> Result<(), SecretError> {
        lock(&self.values).insert(name.to_string(), value.to_string());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), SecretError> {
        lock(&self.values).remove(name);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A fresh folder under the temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let dir = std::env::temp_dir().join(format!(
                "mymtgo-secrets-{name}-{}-{}",
                std::process::id(),
                random_bytes(4)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            ));
            std::fs::create_dir_all(&dir).expect("temp dir");
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A keychain whose every call fails, counting the calls.
    struct BrokenKeychain(Arc<AtomicUsize>);

    impl SecretStore for BrokenKeychain {
        fn get(&self, _: &str) -> Result<Option<String>, SecretError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(SecretError("no Secret Service".to_string()))
        }
        fn set(&self, _: &str, _: &str) -> Result<(), SecretError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(SecretError("no Secret Service".to_string()))
        }
        fn delete(&self, _: &str) -> Result<(), SecretError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(SecretError("no Secret Service".to_string()))
        }
    }

    #[test]
    fn a_failing_keychain_falls_back_to_the_file() {
        let dir = TempDir::new("fallback");
        let calls = Arc::new(AtomicUsize::new(0));
        let store = open_secrets_with(
            Some(&dir.0),
            false,
            Box::new(BrokenKeychain(Arc::clone(&calls))),
        );
        assert!(calls.load(Ordering::SeqCst) >= 1, "the keychain was probed");
        let probed = calls.load(Ordering::SeqCst);
        store.set(API_KEY, "qdevice-key").expect("set");
        store.set(ACCESS_TOKEN, "qaccess").expect("set");
        assert_eq!(
            store.get(API_KEY).expect("get").as_deref(),
            Some("qdevice-key")
        );
        store.delete(ACCESS_TOKEN).expect("delete");
        assert_eq!(store.get(ACCESS_TOKEN).expect("get"), None);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            probed,
            "the keychain is not used again"
        );
        assert!(dir.0.join(SECRETS_FILE).exists() && dir.0.join(KEY_FILE).exists());
        let on_disk = std::fs::read(dir.0.join(SECRETS_FILE)).expect("read");
        assert!(
            !on_disk.windows(11).any(|window| window == b"qdevice-key"),
            "the file is not plain text"
        );
        // A second run reads what the first stored.
        let again = open_secrets_with(Some(&dir.0), false, Box::new(BrokenKeychain(calls)));
        assert_eq!(
            again.get(API_KEY).expect("get").as_deref(),
            Some("qdevice-key")
        );
    }

    #[test]
    fn file_secrets_move_into_a_working_keychain() {
        let dir = TempDir::new("move");
        let file = FileStore::new(&dir.0);
        file.set(API_KEY, "qdevice-key").expect("set");
        file.set(REFRESH_TOKEN, "qrefresh").expect("set");
        let keychain = Arc::new(MemoryStore::default());
        let store = open_secrets_with(Some(&dir.0), false, Box::new(Arc::clone(&keychain)));
        assert_eq!(
            store.get(API_KEY).expect("get").as_deref(),
            Some("qdevice-key")
        );
        assert_eq!(
            store.get(REFRESH_TOKEN).expect("get").as_deref(),
            Some("qrefresh")
        );
        assert_eq!(
            keychain.get(API_KEY).expect("get").as_deref(),
            Some("qdevice-key")
        );
        assert_eq!(
            keychain.get(PROBE).expect("get"),
            None,
            "the probe is deleted"
        );
        assert!(!dir.0.join(SECRETS_FILE).exists(), "secrets.bin is deleted");
        assert!(!dir.0.join(KEY_FILE).exists(), "secrets.key is deleted");
    }

    // Q2 review Minor 2: a move that failed part way must never later put
    // stale file values over newer keychain ones (a rotated refresh token).
    #[test]
    fn a_later_move_never_overwrites_a_newer_keychain_value() {
        let dir = TempDir::new("stale-move");
        let file = FileStore::new(&dir.0);
        file.set(API_KEY, "qdevice-key").expect("set");
        file.set(REFRESH_TOKEN, "qrevoked-refresh").expect("set");
        let keychain = Arc::new(MemoryStore::default());
        keychain
            .set(REFRESH_TOKEN, "qrotated-refresh")
            .expect("set");
        let store = open_secrets_with(Some(&dir.0), false, Box::new(Arc::clone(&keychain)));
        assert_eq!(
            store.get(REFRESH_TOKEN).expect("get").as_deref(),
            Some("qrotated-refresh"),
            "the keychain's newer value stays"
        );
        assert_eq!(
            store.get(API_KEY).expect("get").as_deref(),
            Some("qdevice-key"),
            "a name the keychain lacks still moves"
        );
        assert!(!dir.0.join(SECRETS_FILE).exists(), "the file is deleted");
    }

    /// A keychain that refuses to store one name, once.
    struct FlakyKeychain {
        inner: Arc<MemoryStore>,
        refuse_once: Mutex<Option<String>>,
    }

    impl SecretStore for FlakyKeychain {
        fn get(&self, name: &str) -> Result<Option<String>, SecretError> {
            self.inner.get(name)
        }
        fn set(&self, name: &str, value: &str) -> Result<(), SecretError> {
            let mut refuse = lock(&self.refuse_once);
            if refuse.as_deref() == Some(name) {
                *refuse = None;
                return Err(SecretError("the keychain is busy".to_string()));
            }
            self.inner.set(name, value)
        }
        fn delete(&self, name: &str) -> Result<(), SecretError> {
            self.inner.delete(name)
        }
    }

    // Q3 review Minor 1: after a move that failed part way, a sign-in then a
    // sign-out survives a restart: the file's old tokens never come back.
    #[test]
    fn a_sign_out_survives_a_restart_after_a_partial_move() {
        let dir = TempDir::new("partial-move");
        let file = FileStore::new(&dir.0);
        file.set(API_KEY, "qdevice-key").expect("set");
        file.set(ACCESS_TOKEN, "qaccess-0").expect("set");
        file.set(REFRESH_TOKEN, "qrefresh-0").expect("set");
        let keychain = Arc::new(MemoryStore::default());
        let flaky = FlakyKeychain {
            inner: Arc::clone(&keychain),
            refuse_once: Mutex::new(Some(REFRESH_TOKEN.to_string())),
        };
        let store = open_secrets_with(Some(&dir.0), false, Box::new(flaky));
        // Sign in, then sign out.
        store.set(ACCESS_TOKEN, "qaccess-1").expect("set");
        store.set(REFRESH_TOKEN, "qrefresh-1").expect("set");
        store.delete(ACCESS_TOKEN).expect("delete");
        store.delete(REFRESH_TOKEN).expect("delete");
        // The next start, with a keychain that works.
        let again = open_secrets_with(Some(&dir.0), false, Box::new(Arc::clone(&keychain)));
        assert_eq!(again.get(ACCESS_TOKEN).expect("get"), None);
        assert_eq!(again.get(REFRESH_TOKEN).expect("get"), None);
        assert!(!dir.0.join(SECRETS_FILE).exists(), "the stale file is gone");
    }

    /// A keychain that stores and deletes but cannot read back (a locked
    /// Secret Service collection), counting deletes.
    struct UnreadableKeychain {
        inner: MemoryStore,
        deletes: Arc<AtomicUsize>,
    }

    impl SecretStore for UnreadableKeychain {
        fn get(&self, _: &str) -> Result<Option<String>, SecretError> {
            Err(SecretError("the collection is locked".to_string()))
        }
        fn set(&self, name: &str, value: &str) -> Result<(), SecretError> {
            self.inner.set(name, value)
        }
        fn delete(&self, name: &str) -> Result<(), SecretError> {
            self.deletes.fetch_add(1, Ordering::SeqCst);
            self.inner.delete(name)
        }
    }

    // Q2 review Minor 3: the probe entry is deleted even when reading it fails.
    #[test]
    fn the_probe_is_deleted_when_reading_it_fails() {
        let dir = TempDir::new("probe-read");
        let deletes = Arc::new(AtomicUsize::new(0));
        let keychain = UnreadableKeychain {
            inner: MemoryStore::default(),
            deletes: Arc::clone(&deletes),
        };
        let probe_left = {
            let store = open_secrets_with(Some(&dir.0), false, Box::new(keychain));
            store
                .set(API_KEY, "qdevice-key")
                .expect("the file store works");
            assert!(dir.0.join(SECRETS_FILE).exists(), "fell back to the file");
            deletes.load(Ordering::SeqCst)
        };
        assert_eq!(probe_left, 1, "the probe was deleted");
    }

    // Q2 review Minor 5: without a data folder there is no file store, so
    // nothing is written to the working directory.
    #[test]
    fn without_a_data_folder_there_is_no_file_store() {
        let calls = Arc::new(AtomicUsize::new(0));
        let store = open_secrets_with(None, false, Box::new(BrokenKeychain(Arc::clone(&calls))));
        assert!(store.set(API_KEY, "qdevice-key").is_err());
        assert_eq!(store.get(API_KEY).expect("get"), None);
        assert!(store.delete(API_KEY).is_ok());
        let forced = open_secrets_with(None, true, Box::new(BrokenKeychain(calls)));
        assert!(forced.set(API_KEY, "qdevice-key").is_err());
        // A working keychain without a data folder is still used.
        let keychain = Arc::new(MemoryStore::default());
        let store = open_secrets_with(None, false, Box::new(Arc::clone(&keychain)));
        store.set(API_KEY, "qdevice-key").expect("set");
        assert_eq!(
            keychain.get(API_KEY).expect("get").as_deref(),
            Some("qdevice-key")
        );
    }

    #[test]
    fn a_tampered_secrets_file_reads_as_empty() {
        let dir = TempDir::new("tamper");
        let file = FileStore::new(&dir.0);
        file.set(API_KEY, "qdevice-key").expect("set");
        let path = dir.0.join(SECRETS_FILE);
        let mut bytes = std::fs::read(&path).expect("read");
        let middle = bytes.len() / 2;
        bytes[middle] ^= 0x01;
        std::fs::write(&path, &bytes).expect("write");
        assert_eq!(file.get(API_KEY).expect("get"), None);
        assert!(file.read_all().expect("read").is_empty());
        // A wrong key file reads as empty too, and a later write starts over.
        file.set(ACCESS_TOKEN, "qaccess").expect("set");
        assert_eq!(
            file.get(ACCESS_TOKEN).expect("get").as_deref(),
            Some("qaccess")
        );
        std::fs::write(dir.0.join(KEY_FILE), random_bytes(KEY_LEN)).expect("new key");
        assert_eq!(file.get(ACCESS_TOKEN).expect("get"), None);
        std::fs::write(&path, b"").expect("empty");
        assert_eq!(file.get(ACCESS_TOKEN).expect("get"), None);
    }

    #[cfg(unix)]
    #[test]
    fn the_secrets_files_are_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("mode");
        let file = FileStore::new(&dir.0);
        file.set(API_KEY, "qdevice-key").expect("set");
        file.set(API_KEY, "qdevice-key-2").expect("set again");
        for name in [SECRETS_FILE, KEY_FILE] {
            let mode = std::fs::metadata(dir.0.join(name))
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{name}");
        }
        let leftovers: Vec<_> = std::fs::read_dir(&dir.0)
            .expect("list")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[cfg(unix)]
    #[test]
    fn reading_the_secrets_files_makes_them_0600_again() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("mode-read");
        let file = FileStore::new(&dir.0);
        file.set(API_KEY, "qdevice-key").expect("set");
        for name in [SECRETS_FILE, KEY_FILE] {
            let opened = std::fs::Permissions::from_mode(0o644);
            std::fs::set_permissions(dir.0.join(name), opened).expect("chmod");
        }
        assert_eq!(
            file.get(API_KEY).expect("get").as_deref(),
            Some("qdevice-key")
        );
        for name in [SECRETS_FILE, KEY_FILE] {
            let mode = std::fs::metadata(dir.0.join(name))
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{name}");
        }
    }

    #[test]
    fn only_a_debug_build_takes_the_secrets_file_override() {
        assert!(force_file_from_env(Some("1"), true));
        assert!(!force_file_from_env(Some("1"), false));
        assert!(!force_file_from_env(Some("0"), true));
        assert!(!force_file_from_env(None, true));
    }

    #[test]
    fn force_file_skips_the_probe() {
        let dir = TempDir::new("force");
        let calls = Arc::new(AtomicUsize::new(0));
        let store = open_secrets_with(
            Some(&dir.0),
            true,
            Box::new(BrokenKeychain(Arc::clone(&calls))),
        );
        store.set(API_KEY, "qdevice-key").expect("set");
        assert_eq!(
            store.get(API_KEY).expect("get").as_deref(),
            Some("qdevice-key")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(dir.0.join(SECRETS_FILE).exists());
    }

    #[test]
    fn the_file_format_is_version_nonce_ciphertext_tag() {
        let key = vec![7u8; KEY_LEN];
        let nonce = vec![9u8; NONCE_LEN];
        let plain = br#"{"api_key":"qdevice-key-that-is-longer-than-one-block"}"#;
        let sealed = seal(&key, &nonce, plain);
        assert_eq!(sealed.len(), 1 + NONCE_LEN + plain.len() + TAG_LEN);
        assert_eq!(sealed[0], 1);
        assert_eq!(&sealed[1..17], nonce.as_slice());
        let ciphertext = &sealed[17..sealed.len() - TAG_LEN];
        let mut first_block = Sha256::new();
        first_block.update(&key);
        first_block.update(&nonce);
        first_block.update(0u64.to_be_bytes());
        assert_eq!(ciphertext[0], plain[0] ^ first_block.finalize()[0]);
        assert_eq!(
            &sealed[sealed.len() - TAG_LEN..],
            tag(&key, &nonce, ciphertext).as_slice()
        );
        let opened = open_sealed(&key, &sealed).expect("opens");
        assert_eq!(
            opened.get("api_key").map(String::as_str),
            Some("qdevice-key-that-is-longer-than-one-block")
        );
    }
}
