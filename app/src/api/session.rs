//! `ApiState`: who the app is to the API and how it reaches it. Managed by
//! Tauri; every method blocks (the client, the keychain), so callers run on
//! the `api` worker thread or in `spawn_blocking`, never on the IPC thread
//! or inside an async task.
//!
//! Like 0.x's `AppSettings` (`device_id`, `api_key`, `api_key_expires_at`)
//! plus `SyncTokens`, with the secrets in the keychain and the rest in
//! settings rows written through the tracker.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use mymtgo_api::{
    ApiConfig, ApiError, ApiRequest, ApiResponse, Attested, AuthMode, Credentials, GateState,
    KnownAccount, RateLimits, parse_attested, read_locked_until, read_offline,
};
use tauri::{AppHandle, Manager};

use crate::api::client::ApiClient;
use crate::api::secrets::{
    ACCESS_TOKEN, API_KEY, FORCE_FILE_ENV, REFRESH_TOKEN, SecretStore, force_file_from_env,
    open_secrets,
};
use crate::api::signin::SignInSlot;
use crate::api::sync_runner::SyncRunState;
use crate::api::tokens::TokenKeeper;
use crate::bridge::lock;
use crate::commands::{Startup, ready};
use crate::settings::{API_ATTESTED, API_DEVICE_ID, API_KEY_EXPIRES_AT, API_TOKEN_EXPIRES_AT};

/// Stores settings rows (through the tracker's writer in the app).
pub type Persist = Box<dyn Fn(Vec<(String, String)>) -> Result<(), String> + Send + Sync>;
/// Opens the secret store, once, at first need.
pub type OpenSecrets = Box<dyn Fn() -> Box<dyn SecretStore + Send + Sync> + Send + Sync>;
/// Reads the local accounts (username and login id) for attestation.
pub type ReadAccounts = Box<dyn Fn() -> Result<Vec<KnownAccount>, String> + Send + Sync>;

/// What the API layer knows about this device. No `Debug`: it holds secrets.
#[derive(Clone, Default)]
pub struct Session {
    /// Settings row `api_device_id`, minted at first API use.
    pub device_id: Option<String>,
    /// Settings row `api_key_expires_at` (ms).
    pub api_key_expires_at: Option<i64>,
    /// Cached from the secret store.
    pub api_key: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    /// Settings row `api_token_expires_at` (ms).
    pub token_expires_at: Option<i64>,
    /// Settings row `api_attested`: the confirmed MTGO logins.
    pub attested: Attested,
    /// Settings row `offline_mode_locked_until` (ms): offline mode cannot be
    /// switched back on before it.
    pub offline_locked_until: Option<i64>,
    /// The gate's flags; `linked` follows the two tokens.
    pub gate: GateState,
    /// The secrets were read from the store.
    loaded: bool,
}

impl Session {
    /// What the auth headers are made from. The bearer is offered only
    /// while signed in (both tokens): a lone access token goes nowhere, so
    /// the request uses the device key (0.x `mymtgoReference` checked
    /// `SyncTokens::linked`).
    pub fn credentials(&self) -> Credentials {
        let access_token = if self.linked() {
            self.access_token.clone()
        } else {
            None
        };
        Credentials {
            device_id: self.device_id.clone().unwrap_or_default(),
            api_key: self.api_key.clone(),
            access_token,
        }
    }

    /// Signed in: an access and a refresh token are both stored.
    pub fn linked(&self) -> bool {
        self.access_token.is_some() && self.refresh_token.is_some()
    }
}

pub struct ApiState {
    config: Result<ApiConfig, String>,
    client: Result<ApiClient, String>,
    open_secrets: OpenSecrets,
    secrets: OnceLock<Box<dyn SecretStore + Send + Sync>>,
    session: Mutex<Session>,
    /// Held for a whole registration, so two callers never register twice.
    pub(crate) registering: Mutex<()>,
    /// The one token refresh lock.
    pub(crate) tokens: TokenKeeper,
    /// Login ids with an attestation in flight (one per account).
    pub(crate) attesting: Mutex<BTreeSet<i64>>,
    /// The sign-in that is waiting for the browser, if any.
    pub(crate) sign_in: SignInSlot,
    /// Held for a whole offline mode switch, so two never interleave.
    pub(crate) offline_switch: Mutex<()>,
    /// Held while a confirmed login is recorded and the map stored, so an
    /// older map is never stored last.
    pub(crate) attested_save: Mutex<()>,
    /// The cross-device sync run: one at a time, and its activity feed.
    pub(crate) sync: SyncRunState,
    /// The paths a 429 paused, and until when (#499).
    rate_limits: Mutex<RateLimits>,
    persist: Persist,
    read_accounts: ReadAccounts,
}

impl ApiState {
    /// The state from the configuration, the stored settings rows (`Err`
    /// when they could not be read: offline, as offline mode fails closed),
    /// a way to open the secret store, a way to store rows and a way to read
    /// the local accounts.
    pub fn new(
        config: Result<ApiConfig, String>,
        rows: Result<&[(String, String)], ()>,
        open_secrets: OpenSecrets,
        persist: Persist,
        read_accounts: ReadAccounts,
    ) -> ApiState {
        let client = config.clone().and_then(|config| ApiClient::new(&config));
        if let Err(error) = &client {
            tracing::error!(%error, "the MyMTGO API is not configured");
        }
        let mut session = Session::default();
        session.gate.offline = read_offline(rows);
        let rows = rows.unwrap_or_default();
        session.offline_locked_until = read_locked_until(rows);
        for (key, value) in rows {
            match key.as_str() {
                API_DEVICE_ID => session.device_id = serde_json::from_str(value).ok(),
                API_KEY_EXPIRES_AT => session.api_key_expires_at = serde_json::from_str(value).ok(),
                API_TOKEN_EXPIRES_AT => {
                    session.token_expires_at = serde_json::from_str(value).ok().flatten()
                }
                API_ATTESTED => session.attested = parse_attested(value),
                _ => {}
            }
        }
        ApiState {
            config,
            client,
            open_secrets,
            secrets: OnceLock::new(),
            session: Mutex::new(session),
            registering: Mutex::new(()),
            tokens: TokenKeeper::default(),
            attesting: Mutex::new(BTreeSet::new()),
            sign_in: SignInSlot::default(),
            offline_switch: Mutex::new(()),
            attested_save: Mutex::new(()),
            sync: SyncRunState::default(),
            rate_limits: Mutex::new(RateLimits::default()),
            persist,
            read_accounts,
        }
    }

    /// The app's state: configuration from the build and the process,
    /// secrets in the keychain (or the data folder, when there is one), rows
    /// through the tracker, accounts from the database.
    pub fn for_app(
        app: AppHandle,
        data_dir: Option<PathBuf>,
        rows: Result<&[(String, String)], ()>,
    ) -> ApiState {
        let config = crate::api::load_config().map_err(|error| error.to_string());
        // macOS is for development only, and a rebuilt binary asks for
        // keychain access again on every start, so it always uses the file.
        let force_file = cfg!(target_os = "macos")
            || force_file_from_env(
                std::env::var(FORCE_FILE_ENV).ok().as_deref(),
                cfg!(debug_assertions),
            );
        let open: OpenSecrets = Box::new(move || open_secrets(data_dir.as_deref(), force_file));
        let reader_app = app.clone();
        let read_accounts: ReadAccounts = Box::new(move || {
            let startup = reader_app.state::<Startup>();
            let state = ready(&startup).map_err(|error| format!("{error:?}"))?;
            let rows = store::read::account_logins(&lock(&state.reader))
                .map_err(|error| error.to_string())?;
            Ok(rows
                .into_iter()
                .map(|(username, login_id)| KnownAccount { username, login_id })
                .collect())
        });
        let persist: Persist = Box::new(move |rows| {
            let startup = app.state::<Startup>();
            let state = ready(&startup).map_err(|error| format!("{error:?}"))?;
            state.save_setting_rows(rows)
        });
        ApiState::new(config, rows, open, persist, read_accounts)
    }

    pub fn config(&self) -> Result<&ApiConfig, ApiError> {
        self.config
            .as_ref()
            .map_err(|error| ApiError::Config(error.clone()))
    }

    pub fn client(&self) -> Result<&ApiClient, ApiError> {
        self.client
            .as_ref()
            .map_err(|error| ApiError::Config(error.clone()))
    }

    fn store(&self) -> &(dyn SecretStore + Send + Sync) {
        self.secrets.get_or_init(|| (self.open_secrets)()).as_ref()
    }

    /// The session, its secrets read from the store on first use.
    pub fn session(&self) -> MutexGuard<'_, Session> {
        let store = self.store();
        let mut session = lock(&self.session);
        if !session.loaded {
            let read = |name: &str| match store.get(name) {
                Ok(value) => value,
                Err(error) => {
                    tracing::warn!(%error, name, "could not read a secret");
                    None
                }
            };
            session.api_key = read(API_KEY);
            session.access_token = read(ACCESS_TOKEN);
            session.refresh_token = read(REFRESH_TOKEN);
            session.gate.linked = session.linked();
            session.loaded = true;
        }
        session
    }

    /// Store (or, with `None`, delete) a secret and keep the cache in step.
    /// The cache changes even when the store fails, so this run goes on with
    /// it; the failure is logged without the value.
    pub fn set_secret(&self, name: &str, value: Option<&str>) {
        let result = match value {
            Some(value) => self.store().set(name, value),
            None => self.store().delete(name),
        };
        if let Err(error) = result {
            tracing::error!(%error, name, "could not store a secret");
        }
        let mut session = self.session();
        let value = value.map(str::to_string);
        match name {
            API_KEY => session.api_key = value,
            ACCESS_TOKEN => session.access_token = value,
            REFRESH_TOKEN => session.refresh_token = value,
            _ => {}
        }
        session.gate.linked = session.linked();
    }

    /// The local accounts; empty (logged) when they cannot be read.
    pub fn accounts(&self) -> Vec<KnownAccount> {
        (self.read_accounts)().unwrap_or_else(|error| {
            tracing::warn!(%error, "could not read the local accounts");
            Vec::new()
        })
    }

    /// Store settings rows; a failure is logged (the value stays in memory
    /// for this run).
    pub fn persist(&self, rows: Vec<(String, String)>) {
        if let Err(error) = self.try_persist(rows) {
            tracing::warn!(%error, "could not store API settings");
        }
    }

    /// Store settings rows, saying why that failed.
    pub fn try_persist(&self, rows: Vec<(String, String)>) -> Result<(), String> {
        if rows.is_empty() {
            return Ok(());
        }
        (self.persist)(rows)
    }

    /// Offline mode and its cooldown end, read without opening the secret
    /// store. The session lock is shared with the first secret read, so a
    /// keychain prompt on first use can briefly hold up the switch (an edge
    /// case the owner accepted, Q4 review I1).
    pub fn offline_mode(&self) -> (bool, Option<i64>) {
        let session = lock(&self.session);
        (session.gate.offline, session.offline_locked_until)
    }

    /// The gate's flags for the worker's schedule. The secret store (and a
    /// keychain prompt) is opened only when the flags it adds could matter:
    /// online. Offline, `linked` stays as cached, and every job that reads
    /// it is gated off anyway.
    pub fn gate_now(&self) -> GateState {
        let cached = lock(&self.session).gate;
        if cached.offline {
            return cached;
        }
        self.session().gate
    }

    /// Switch the gate's offline flag (and, when given, the cooldown end) in
    /// memory. Every request checks the flag before a socket opens, so once
    /// this returns, no new request goes out offline.
    pub fn set_offline_mode(&self, offline: bool, locked_until: Option<i64>) {
        let mut session = lock(&self.session);
        session.gate.offline = offline;
        if locked_until.is_some() {
            session.offline_locked_until = locked_until;
        }
    }

    /// Send a request. Signed in, an authenticated request first makes
    /// sure the access token is fresh (0.x `EnsureAccessToken` in the
    /// macros: refreshed online when it expires within 24 hours, used as
    /// stored offline); a refresh that fails for any reason but a dead
    /// refresh token fails the request. A device-key request while signed
    /// out (or just signed out by that refresh) first makes sure the key is
    /// fresh (0.x `RegisterDevice::ensureFresh`); if that fails the request
    /// goes out with what is stored.
    pub fn send(&self, request: &ApiRequest, now_ms: i64) -> Result<ApiResponse, ApiError> {
        self.send_with_bearer(request, now_ms)
            .map(|(response, _)| response)
    }

    /// `send`, with the access token the request carried (`None` without
    /// one), read before the request goes out: a 401 weighs that token, not
    /// whatever is stored once the answer is back (#264).
    pub fn send_with_bearer(
        &self,
        request: &ApiRequest,
        now_ms: i64,
    ) -> Result<(ApiResponse, Option<String>), ApiError> {
        // A 429 paused this path (#499): nothing goes until its pause ends.
        if let Some(until) = lock(&self.rate_limits).paused_until(&request.path, now_ms) {
            return Err(ApiError::RateLimited(until));
        }
        let uses_bearer = matches!(request.auth, AuthMode::DeviceOrBearer | AuthMode::Bearer);
        if uses_bearer && self.session().linked() {
            match crate::api::tokens::access_token(self, now_ms) {
                Ok(_) | Err(ApiError::NotLinked) => {}
                Err(error) => return Err(error),
            }
        }
        if request.auth == AuthMode::DeviceOrBearer {
            let _ = crate::api::device::ensure_device_key(self, now_ms);
        }
        if request.auth == AuthMode::Device {
            let _ = crate::api::device::ensure_device_key_signed_in_too(self, now_ms);
        }
        let (gate, credentials) = {
            let session = self.session();
            (session.gate, session.credentials())
        };
        let bearer = credentials.access_token.clone();
        let response = self.client()?.send(request, &gate, &credentials)?;
        if response.status == 429 {
            let retry_after = response.header("retry-after");
            tracing::warn!(
                path = request.path.as_str(),
                retry_after,
                "rate limited by the API; the path waits"
            );
            lock(&self.rate_limits).record(&request.path, retry_after, now_ms);
        }
        Ok((response, bearer))
    }
}

#[cfg(test)]
pub mod testing {
    //! An `ApiState` against a stub server, with secrets in memory and saved
    //! rows recorded.

    use std::sync::{Arc, Mutex};

    use mymtgo_api::{ApiConfig, KnownAccount};

    use super::{ApiState, OpenSecrets, Persist, ReadAccounts};
    use crate::api::secrets::{MemoryStore, SecretStore};
    use crate::bridge::lock;

    pub struct TestApi {
        pub state: ApiState,
        pub secrets: Arc<MemoryStore>,
        pub saved: Arc<Mutex<Vec<(String, String)>>>,
        /// What `accounts()` reads; tests change it freely.
        pub accounts: Arc<Mutex<Vec<KnownAccount>>>,
        /// Runs before each store of rows; an error fails that store.
        pub persist_hook: Arc<Mutex<Option<PersistHook>>>,
    }

    /// A test's hook into storing rows.
    pub type PersistHook = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

    impl TestApi {
        /// The last saved value of a settings row.
        pub fn saved(&self, key: &str) -> Option<String> {
            lock(&self.saved)
                .iter()
                .rev()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
        }
    }

    /// A state for `base_url` with these rows and secrets already stored,
    /// and the OAuth client id `qclient-id`.
    pub fn test_api(base_url: &str, rows: &[(&str, &str)], secrets: &[(&str, &str)]) -> TestApi {
        build(base_url, Some("qclient-id"), rows, secrets)
    }

    /// A registered device (`qdevice`) with key `key-one`, valid a day after
    /// 1_800_000_000_000 (the tests' `NOW`); offline when asked; never signed in.
    pub fn device_api(base_url: &str, offline: bool) -> TestApi {
        let key_valid = (1_800_000_000_000_i64 + 24 * 3_600_000).to_string();
        let offline_row = offline.to_string();
        let rows = [
            ("api_device_id", r#""qdevice""#),
            ("api_key_expires_at", key_valid.as_str()),
            (mymtgo_api::OFFLINE_MODE, offline_row.as_str()),
        ];
        test_api(base_url, &rows, &[("api_key", "key-one")])
    }

    /// A state for `base_url` with this client id and nothing stored.
    pub fn test_api_with_client(base_url: &str, client_id: Option<&str>) -> TestApi {
        build(base_url, client_id, &[], &[])
    }

    fn build(
        base_url: &str,
        client_id: Option<&str>,
        rows: &[(&str, &str)],
        secrets: &[(&str, &str)],
    ) -> TestApi {
        let config = ApiConfig {
            base_url: base_url.to_string(),
            host: "127.0.0.1".to_string(),
            client_id: client_id.map(str::to_string),
            ca_pem_path: None,
        };
        let store = Arc::new(MemoryStore::default());
        for (name, value) in secrets {
            store.set(name, value).expect("memory store");
        }
        let saved = Arc::new(Mutex::new(Vec::new()));
        let open: OpenSecrets = {
            let store = Arc::clone(&store);
            Box::new(move || Box::new(Arc::clone(&store)))
        };
        let persist_hook: Arc<Mutex<Option<PersistHook>>> = Arc::new(Mutex::new(None));
        let persist: Persist = {
            let saved = Arc::clone(&saved);
            let persist_hook = Arc::clone(&persist_hook);
            Box::new(move |rows| {
                let hook = lock(&persist_hook).clone();
                if let Some(hook) = hook {
                    hook()?;
                }
                lock(&saved).extend(rows);
                Ok(())
            })
        };
        let accounts = Arc::new(Mutex::new(Vec::new()));
        let read_accounts: ReadAccounts = {
            let accounts = Arc::clone(&accounts);
            Box::new(move || Ok(lock(&accounts).clone()))
        };
        let rows: Vec<(String, String)> = rows
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        TestApi {
            state: ApiState::new(Ok(config), Ok(&rows), open, persist, read_accounts),
            secrets: store,
            saved,
            accounts,
            persist_hook,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::test_api;
    use crate::api::secrets::ACCESS_TOKEN;
    use crate::api::stub::{Reply, Stub};
    use mymtgo_api::{AuthMode, Method, RequestClass};

    const NOW: i64 = 1_800_000_000_000;

    // Q2 review Minor 1: a lone access token (no refresh token) is not
    // signed in, so requests carry the device key, never the stale bearer.
    #[test]
    fn a_lone_access_token_sends_the_device_key_not_the_bearer() {
        let stub = Stub::start(|_| Reply::json(200, "[]"));
        let valid = (NOW + 3_600_000).to_string();
        let api = test_api(
            &stub.base_url,
            &[
                ("api_device_id", r#""qdevice""#),
                ("api_key_expires_at", &valid),
            ],
            &[("api_key", "qdevice-key"), (ACCESS_TOKEN, "qstale-access")],
        );
        assert!(!api.state.session().linked());
        assert_eq!(api.state.session().credentials().access_token, None);
        let request = mymtgo_api::ApiRequest::new(
            Method::Post,
            "/api/cards",
            RequestClass::Reference,
            AuthMode::DeviceOrBearer,
        );
        api.state.send(&request, NOW).expect("sent");
        let seen = stub.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].header("x-api-key"), Some("qdevice-key"));
        assert_eq!(seen[0].header("x-device-id"), Some("qdevice"));
        assert_eq!(seen[0].header("authorization"), None);
    }

    #[test]
    fn the_stored_rows_set_expiry_and_attested_logins() {
        let api = test_api(
            "http://127.0.0.1:1",
            &[
                ("api_token_expires_at", "1800000000000"),
                ("api_attested", r#"{"6031972":"Qplayer"}"#),
            ],
            &[],
        );
        let session = api.state.session();
        assert_eq!(session.token_expires_at, Some(1_800_000_000_000));
        assert_eq!(
            session.attested.get("6031972").map(String::as_str),
            Some("Qplayer")
        );
        drop(session);
        let cleared = test_api(
            "http://127.0.0.1:1",
            &[("api_token_expires_at", "null")],
            &[],
        );
        assert_eq!(cleared.state.session().token_expires_at, None);
    }
}
