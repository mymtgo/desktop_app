//! Sign-in, sign-out and the account card's status (survey section 2,
//! 07 spec section 4 and rulings 1, 2 and 8).
//!
//! Sign-in listens on `127.0.0.1` with a random port for up to 240 seconds
//! (0.x's UI wait), opens the authorize URL in the browser and exchanges the
//! code the browser brings back. The PKCE verifier stays in memory. The
//! listener reads at most 8 KiB of request head per connection within 5
//! seconds, answers a short page and closes; only `GET /oauth/callback`
//! counts.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mymtgo_api::{Callback, Pkce, authorize_url, loopback_redirect, parse_callback};
use serde::Serialize;
use tauri::{AppHandle, Manager};
use tauri_specta::Event;

use crate::api::attest::attest_known_accounts;
use crate::api::random::random_bytes;
use crate::api::session::ApiState;
use crate::api::tokens::{exchange_code, sign_out_tokens};
use crate::api::worker::WorkerSignal;
use crate::bridge::lock;
use crate::commands::{CommandError, Startup, off_async_workers, ready};
use crate::events::ApiChanged;

/// How long sign-in waits for the browser (0.x polled for 240 s).
pub const SIGN_IN_WAIT: Duration = Duration::from_secs(240);
/// How long one connection may take to send its request head.
pub const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);
/// The most request head read from one connection.
pub const MAX_REQUEST_HEAD: usize = 8 * 1024;
/// How often the listener looks for a connection, a cancel or its deadline.
const ACCEPT_POLL: Duration = Duration::from_millis(25);

pub const OFFLINE_REFUSAL: &str = "Offline mode is on. Turn it off in Data & Privacy to sync.";
pub const NOT_CONFIGURED: &str = "Sign-in is not configured in this build.";
pub const ALREADY_WAITING: &str = "Sign-in is already waiting for approval.";
pub const NOT_STARTED: &str = "Could not start sign-in. Try again.";
pub const NO_BROWSER: &str = "The browser could not be opened.";
/// 0.x's message when the wait ran out.
pub const NOT_FINISHED: &str = "Sign-in wasn't finished. Try again.";
pub const DENIED: &str = "Sign-in was cancelled.";
pub const EXCHANGE_FAILED: &str = "Sign-in could not be finished. Try again.";

const SIGNED_IN_PAGE: &str = "Signed in. You can close this tab and return to MyMTGO.";
const CANCELLED_PAGE: &str = "Sign-in was cancelled. You can close this tab.";
const BAD_STATE_PAGE: &str = "This sign-in link is not valid. Return to MyMTGO and try again.";
const NOT_FOUND_PAGE: &str = "Not found.";
const BAD_REQUEST_PAGE: &str = "Bad request.";

/// The sign-in that is waiting for the browser, and how the last one ended.
#[derive(Default)]
pub struct SignInSlot {
    /// Set while a sign-in waits; storing `true` cancels it.
    running: Mutex<Option<Arc<AtomicBool>>>,
    /// Why the last sign-in failed, for the card.
    error: Mutex<Option<String>>,
}

impl SignInSlot {
    /// Cancel the sign-in that waits for the browser, if any.
    pub fn cancel_waiting(&self) {
        if let Some(cancel) = lock(&self.running).as_ref() {
            cancel.store(true, Ordering::SeqCst);
        }
    }
}

/// What the account card shows. Never a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    /// This build can sign in (it has an OAuth client id).
    pub configured: bool,
    /// Signed in: an access and a refresh token are stored.
    pub linked: bool,
    /// A sign-in is waiting for the browser.
    pub signing_in: bool,
    pub offline: bool,
    /// Why the last sign-in failed.
    pub error: Option<String>,
}

pub fn account_status_now(state: &ApiState) -> AccountStatus {
    let configured = state
        .config()
        .is_ok_and(|config| config.client_id.is_some());
    let (linked, offline) = {
        let session = state.session();
        (session.linked(), session.gate.offline)
    };
    AccountStatus {
        configured,
        linked,
        signing_in: lock(&state.sign_in.running).is_some(),
        offline,
        error: lock(&state.sign_in.error).clone(),
    }
}

/// A sign-in whose browser is open: the listener and the secrets to finish
/// it with.
pub struct PendingSignIn {
    listener: TcpListener,
    pkce: Pkce,
    redirect_uri: String,
}

/// `len` alphanumeric characters from the OS random source.
fn new_pkce() -> Pkce {
    loop {
        // 248 of 256 byte values are used, so twice the length is plenty;
        // a short draw simply draws again.
        let verifier = random_bytes(mymtgo_api::oauth::VERIFIER_LEN * 2);
        let state = random_bytes(mymtgo_api::oauth::STATE_LEN * 2);
        if let Some(pkce) = Pkce::from_random(&verifier, &state) {
            return pkce;
        }
    }
}

/// Start a sign-in: refused offline or without a client id; otherwise
/// listen on `127.0.0.1:0` and open the authorize URL with `open`.
pub fn begin_sign_in(
    state: &ApiState,
    open: &dyn Fn(&str) -> Result<(), String>,
) -> Result<PendingSignIn, String> {
    if state.session().gate.offline {
        return Err(OFFLINE_REFUSAL.to_string());
    }
    let config = match state.config() {
        Ok(config) if config.client_id.is_some() => config.clone(),
        _ => return Err(NOT_CONFIGURED.to_string()),
    };
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|error| {
        tracing::error!(%error, "could not listen for the sign-in callback");
        NOT_STARTED.to_string()
    })?;
    let port = listener
        .local_addr()
        .map_err(|_| NOT_STARTED.to_string())?
        .port();
    let pkce = new_pkce();
    let redirect_uri = loopback_redirect(port);
    let url = authorize_url(&config, &pkce, &redirect_uri);
    if let Err(error) = open(&url) {
        tracing::warn!(%error, "could not open the sign-in page");
        return Err(NO_BROWSER.to_string());
    }
    Ok(PendingSignIn {
        listener,
        pkce,
        redirect_uri,
    })
}

/// How a sign-in ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInEnd {
    SignedIn,
    Cancelled,
    /// The message for the card.
    Failed(String),
}

/// Wait for the browser, then exchange the code and store the tokens;
/// `on_signed_in` runs then (focus the window, tell
/// the UI), and known accounts are attested last.
pub fn finish_sign_in(
    state: &ApiState,
    pending: PendingSignIn,
    deadline: Instant,
    cancel: &AtomicBool,
    now_ms: &dyn Fn() -> i64,
    on_signed_in: &dyn Fn(),
) -> SignInEnd {
    let waited = wait_for_callback(
        &pending.listener,
        &pending.pkce.state,
        deadline,
        CONNECTION_TIMEOUT,
        cancel,
    );
    drop(pending.listener);
    let code = match waited {
        Waited::Code(code) => code,
        Waited::Denied => return SignInEnd::Failed(DENIED.to_string()),
        Waited::Cancelled => return SignInEnd::Cancelled,
        Waited::TimedOut => return SignInEnd::Failed(NOT_FINISHED.to_string()),
    };
    // A Cancel that landed while the callback was read wins. Once the
    // exchange has begun, a Cancel may still sign in.
    if cancel.load(Ordering::SeqCst) {
        return SignInEnd::Cancelled;
    }
    if let Err(error) = exchange_code(
        state,
        &code,
        &pending.pkce.verifier,
        &pending.redirect_uri,
        now_ms(),
    ) {
        tracing::error!(error = %error.kind(), "sign-in could not exchange its code");
        return SignInEnd::Failed(EXCHANGE_FAILED.to_string());
    }
    tracing::info!("signed in to MyMTGO");
    on_signed_in();
    attest_known_accounts(state, now_ms());
    SignInEnd::SignedIn
}

/// What the listener heard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Waited {
    Code(String),
    Denied,
    Cancelled,
    TimedOut,
}

/// Accept connections until a callback ends the sign-in, `cancel` is set or
/// `deadline` passes. Each connection gets `connection_timeout` (and never
/// past the deadline) to send at most `MAX_REQUEST_HEAD` bytes of head, is
/// answered with a short page and closed.
pub fn wait_for_callback(
    listener: &TcpListener,
    expected_state: &str,
    deadline: Instant,
    connection_timeout: Duration,
    cancel: &AtomicBool,
) -> Waited {
    if listener.set_nonblocking(true).is_err() {
        return Waited::TimedOut;
    }
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Waited::Cancelled;
        }
        if Instant::now() >= deadline {
            return Waited::TimedOut;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let until = deadline.min(Instant::now() + connection_timeout);
                if let Some(waited) = serve(stream, expected_state, until) {
                    return waited;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(ACCEPT_POLL);
            }
            Err(error) => {
                tracing::warn!(%error, "the sign-in listener could not accept a connection");
                std::thread::sleep(ACCEPT_POLL);
            }
        }
    }
}

/// Read one request and answer it. `Some` when it ends the wait.
fn serve(mut stream: TcpStream, expected_state: &str, until: Instant) -> Option<Waited> {
    // Accepted sockets inherit non-blocking mode on some platforms.
    if stream.set_nonblocking(false).is_err() {
        return None;
    }
    let head = read_head(&mut stream, until)?;
    let request_line = head.lines().next().unwrap_or("").trim_end_matches('\r');
    let (status, page, waited) = match parse_callback(request_line, expected_state) {
        Callback::Code(code) => (200, SIGNED_IN_PAGE, Some(Waited::Code(code))),
        Callback::Denied(error) => {
            tracing::info!(%error, "sign-in was declined in the browser");
            (200, CANCELLED_PAGE, Some(Waited::Denied))
        }
        Callback::BadState => {
            tracing::warn!("a sign-in callback without a matching state was refused");
            (400, BAD_STATE_PAGE, None)
        }
        Callback::Ignore if request_line.starts_with("GET /") => (404, NOT_FOUND_PAGE, None),
        Callback::Ignore => (400, BAD_REQUEST_PAGE, None),
    };
    answer(&mut stream, status, page);
    waited
}

/// The request head (up to the blank line), or `None` when the connection
/// closed, sent more than `MAX_REQUEST_HEAD` bytes or ran out of time. An
/// oversized head is answered 400.
fn read_head(stream: &mut TcpStream, until: Instant) -> Option<String> {
    let mut data: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        if find(&data, b"\r\n\r\n").is_some() || find(&data, b"\n\n").is_some() {
            return Some(String::from_utf8_lossy(&data).into_owned());
        }
        if data.len() >= MAX_REQUEST_HEAD {
            answer(stream, 400, BAD_REQUEST_PAGE);
            return None;
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() || stream.set_read_timeout(Some(left)).is_err() {
            return None;
        }
        let room = (MAX_REQUEST_HEAD - data.len()).min(chunk.len());
        match stream.read(&mut chunk[..room]) {
            Ok(0) | Err(_) => return None,
            Ok(read) => data.extend_from_slice(&chunk[..read]),
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// A short plain page, then close.
fn answer(stream: &mut TcpStream, status: u16, text: &str) {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        _ => "Bad Request",
    };
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>MyMTGO</title></head>\
         <body style=\"font-family:sans-serif;padding:2rem\"><p>{text}</p></body></html>"
    );
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.set_write_timeout(Some(CONNECTION_TIMEOUT));
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

/// Sign out (0.x `SyncController::unlink`): forget both tokens and the
/// expiry; no server call. The device key stays, so device mode goes on
/// (ruling 8).
pub fn sign_out_now(state: &ApiState) {
    sign_out_tokens(state);
    tracing::info!("signed out of MyMTGO");
}

fn emit_changed(app: &AppHandle) {
    if let Err(error) = (ApiChanged {}).emit(app) {
        tracing::warn!(%error, "could not tell the UI the account changed");
    }
    // The hosted overlay's heartbeat follows sign-in (overlay spec 10).
    crate::overlay::signal(app, crate::overlay::worker::OverlayMsg::GateChanged);
}

/// End a sign-in's wait: free the slot, keep the failure for the card and
/// tell the UI.
fn end_wait(app: &AppHandle, error: Option<String>) {
    let state = app.state::<ApiState>();
    *lock(&state.sign_in.running) = None;
    *lock(&state.sign_in.error) = error;
    emit_changed(app);
}

/// Claim the sign-in slot, then begin the sign-in with the slot's lock
/// released: beginning may open the keychain (an unlock prompt) and the
/// browser, and the account card and Cancel must not wait on either. A
/// sign-in that cannot begin frees the slot. The flag cancels the wait.
pub fn claim_and_begin(
    state: &ApiState,
    open: &dyn Fn(&str) -> Result<(), String>,
) -> Result<(PendingSignIn, Arc<AtomicBool>), String> {
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut running = lock(&state.sign_in.running);
        if running.is_some() {
            return Err(ALREADY_WAITING.to_string());
        }
        *running = Some(Arc::clone(&cancel));
    }
    match begin_sign_in(state, open) {
        Ok(pending) => {
            *lock(&state.sign_in.error) = None;
            Ok((pending, cancel))
        }
        Err(error) => {
            *lock(&state.sign_in.running) = None;
            Err(error)
        }
    }
}

/// Begin a sign-in and wait for it on its own thread.
fn start_sign_in(app: &AppHandle) -> Result<(), CommandError> {
    let state = app.state::<ApiState>();
    let open = |url: &str| crate::external::open_api_url(app, url);
    let (pending, cancel) = claim_and_begin(&state, &open).map_err(CommandError::Refused)?;
    let thread_app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("sign-in".to_string())
        .spawn(move || {
            let app = thread_app;
            let state = app.state::<ApiState>();
            let on_signed_in = || {
                end_wait(&app, None);
                crate::show_main(&app);
                crate::api::worker::signal(&app, WorkerSignal::SignedIn);
            };
            let end = finish_sign_in(
                &state,
                pending,
                Instant::now() + SIGN_IN_WAIT,
                &cancel,
                &crate::wall_clock_ms,
                &on_signed_in,
            );
            match end {
                SignInEnd::SignedIn => {}
                SignInEnd::Cancelled => end_wait(&app, None),
                SignInEnd::Failed(message) => end_wait(&app, Some(message)),
            }
        });
    if let Err(error) = spawned {
        tracing::error!(%error, "could not start the sign-in thread");
        *lock(&state.sign_in.running) = None;
        return Err(CommandError::Refused(NOT_STARTED.to_string()));
    }
    emit_changed(app);
    Ok(())
}

/// The account card's state.
#[tauri::command]
#[specta::specta]
pub async fn account_status(app: AppHandle) -> Result<AccountStatus, CommandError> {
    off_async_workers(move || Ok(account_status_now(&app.state::<ApiState>()))).await
}

/// "Sign in": opens MyMTGO in the browser and returns; the wait goes on in
/// the background and `ApiChanged` reports its end.
#[tauri::command]
#[specta::specta]
pub async fn sign_in(app: AppHandle) -> Result<(), CommandError> {
    off_async_workers(move || start_sign_in(&app)).await
}

/// "Cancel": stop waiting for the browser. The slot's lock is only ever
/// held for a moment (never across IO), so this never stalls the async
/// worker. A Cancel after the code exchange began may still sign in.
#[tauri::command]
#[specta::specta]
pub async fn cancel_sign_in(app: AppHandle) {
    let state = app.state::<ApiState>();
    if let Some(cancel) = lock(&state.sign_in.running).as_ref() {
        cancel.store(true, Ordering::SeqCst);
    }
}

/// "Sign out".
#[tauri::command]
#[specta::specta]
pub async fn sign_out(app: AppHandle) -> Result<(), CommandError> {
    off_async_workers(move || {
        let editor = app.try_state::<Startup>().and_then(|startup| {
            let state = ready(&startup).ok()?;
            state.tracker_part(tracker::TrackerHandle::sync_editor).ok()
        });
        crate::api::tokens::sign_out_and_forget_slots(&app.state::<ApiState>(), editor.as_ref());
        tracing::info!("signed out of MyMTGO");
        emit_changed(&app);
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::secrets::{ACCESS_TOKEN, API_KEY, REFRESH_TOKEN, SecretStore};
    use crate::api::session::testing::{TestApi, test_api};
    use crate::api::stub::{Recorded, Reply, Stub};
    use crate::bridge::lock;
    use mymtgo_api::KnownAccount;
    use mymtgo_api::php::urldecode;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::{Arc, Mutex};

    const NOW: i64 = 1_800_000_000_000;
    const TOKENS: &str = r#"{"access_token":"at1","refresh_token":"rt1","expires_in":2592000}"#;

    fn now() -> i64 {
        NOW
    }

    fn token_stub() -> Stub {
        Stub::start(|request| match request.target.as_str() {
            "/oauth/token" => Reply::json(200, TOKENS),
            "/api/account/players" => Reply::json(200, r#"{"id":1}"#),
            _ => Reply::json(404, "{}"),
        })
    }

    /// A query parameter of a URL, decoded.
    fn param(url: &str, name: &str) -> Option<String> {
        let (_, query) = url.split_once('?')?;
        query.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            (urldecode(key) == name).then(|| urldecode(value))
        })
    }

    fn form_field(request: &Recorded, name: &str) -> Option<String> {
        let text = String::from_utf8(request.body.clone()).ok()?;
        param(&format!("?{text}"), name)
    }

    /// Begin a sign-in whose browser is this test: the opened URL is kept.
    fn begin(api: &TestApi) -> (PendingSignIn, String) {
        let opened = Arc::new(Mutex::new(Vec::<String>::new()));
        let open = {
            let opened = Arc::clone(&opened);
            move |url: &str| {
                lock(&opened).push(url.to_string());
                Ok(())
            }
        };
        let pending = begin_sign_in(&api.state, &open).expect("sign-in starts");
        let url = lock(&opened).pop().expect("the browser was opened");
        (pending, url)
    }

    /// What a browser does with a redirect: one GET, the whole answer back.
    fn browser_get(port: u16, target: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        let request = format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
        stream.write_all(request.as_bytes()).expect("write");
        let mut answer = String::new();
        let _ = stream.read_to_string(&mut answer);
        answer
    }

    fn port_of(redirect_uri: &str) -> u16 {
        let rest = redirect_uri
            .strip_prefix("http://127.0.0.1:")
            .expect("a loopback redirect");
        rest.split('/')
            .next()
            .expect("port")
            .parse()
            .expect("number")
    }

    /// Run `finish_sign_in` on a thread while the test plays the browser.
    fn finish_on_thread(
        api: &Arc<TestApi>,
        pending: PendingSignIn,
        wait: Duration,
        cancel: Arc<AtomicBool>,
    ) -> std::thread::JoinHandle<SignInEnd> {
        let api = Arc::clone(api);
        std::thread::spawn(move || {
            finish_sign_in(
                &api.state,
                pending,
                Instant::now() + wait,
                &cancel,
                &now,
                &|| {},
            )
        })
    }

    // 0.x DeviceLinkTest: "links the device from the deep-link callback and
    // clears the one-time stash", as it applies to the loopback listener.
    #[test]
    fn links_the_device_from_the_callback() {
        let stub = token_stub();
        let api = Arc::new(test_api(&stub.base_url, &[], &[]));
        let (pending, url) = begin(&api);
        assert!(
            url.starts_with(&format!("{}/oauth/authorize?", stub.base_url)),
            "{url}"
        );
        let redirect = param(&url, "redirect_uri").expect("redirect");
        let state = param(&url, "state").expect("state");
        let challenge = param(&url, "code_challenge").expect("challenge");
        let finished = finish_on_thread(&api, pending, Duration::from_secs(10), Arc::default());
        let page = browser_get(
            port_of(&redirect),
            &format!("/oauth/callback?code=abc123&state={state}"),
        );
        assert!(page.starts_with("HTTP/1.1 200"), "{page}");
        assert!(
            page.contains("Signed in. You can close this tab and return to MyMTGO."),
            "{page}"
        );
        assert_eq!(finished.join().expect("joined"), SignInEnd::SignedIn);
        assert!(api.state.session().linked());
        assert_eq!(
            api.secrets.get(ACCESS_TOKEN).expect("get").as_deref(),
            Some("at1")
        );
        let exchange = stub
            .requests()
            .into_iter()
            .find(|request| request.target == "/oauth/token")
            .expect("the code was exchanged");
        assert_eq!(form_field(&exchange, "code").as_deref(), Some("abc123"));
        assert_eq!(form_field(&exchange, "redirect_uri"), Some(redirect));
        let verifier = form_field(&exchange, "code_verifier").expect("verifier");
        assert_eq!(verifier.len(), 128);
        assert_eq!(mymtgo_api::oauth::pkce_challenge(&verifier), challenge);
        // The listener is gone with the sign-in.
        assert!(
            TcpStream::connect((
                "127.0.0.1",
                port_of(&param(&url, "redirect_uri").expect("r"))
            ))
            .is_err()
        );
    }

    // 0.x DeviceLinkTest: "rejects a state mismatch before the code is ever exchanged".
    #[test]
    fn rejects_a_state_mismatch_before_the_code_is_ever_exchanged() {
        let stub = token_stub();
        let api = Arc::new(test_api(&stub.base_url, &[], &[]));
        let (pending, url) = begin(&api);
        let redirect = param(&url, "redirect_uri").expect("redirect");
        let finished = finish_on_thread(&api, pending, Duration::from_millis(600), Arc::default());
        let page = browser_get(
            port_of(&redirect),
            "/oauth/callback?code=abc123&state=evilstate",
        );
        assert!(page.starts_with("HTTP/1.1 400"), "{page}");
        assert_eq!(
            finished.join().expect("joined"),
            SignInEnd::Failed(NOT_FINISHED.to_string())
        );
        assert!(!api.state.session().linked());
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(stub.connections(), 0, "nothing was exchanged");
    }

    // 0.x DeviceLinkTest: "ignores deep links that are not the oauth callback".
    #[test]
    fn ignores_requests_that_are_not_the_oauth_callback() {
        let stub = token_stub();
        let api = Arc::new(test_api(&stub.base_url, &[], &[]));
        let (pending, url) = begin(&api);
        let redirect = param(&url, "redirect_uri").expect("redirect");
        let state = param(&url, "state").expect("state");
        let finished = finish_on_thread(&api, pending, Duration::from_secs(10), Arc::default());
        let port = port_of(&redirect);
        let page = browser_get(port, &format!("/somewhere/else?code=abc123&state={state}"));
        assert!(page.starts_with("HTTP/1.1 404"), "{page}");
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(stub.connections(), 0);
        // Still listening: the real callback completes.
        browser_get(port, &format!("/oauth/callback?code=abc123&state={state}"));
        assert_eq!(finished.join().expect("joined"), SignInEnd::SignedIn);
    }

    // 0.x DeviceLinkTest: "drops the stash without exchanging when the
    // consent screen returns an error".
    #[test]
    fn stops_without_exchanging_when_the_consent_screen_returns_an_error() {
        let stub = token_stub();
        let api = Arc::new(test_api(&stub.base_url, &[], &[]));
        let (pending, url) = begin(&api);
        let redirect = param(&url, "redirect_uri").expect("redirect");
        let state = param(&url, "state").expect("state");
        let finished = finish_on_thread(&api, pending, Duration::from_secs(10), Arc::default());
        let page = browser_get(
            port_of(&redirect),
            &format!("/oauth/callback?error=access_denied&state={state}"),
        );
        assert!(
            page.contains("Sign-in was cancelled. You can close this tab."),
            "{page}"
        );
        assert_eq!(
            finished.join().expect("joined"),
            SignInEnd::Failed(DENIED.to_string())
        );
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(stub.connections(), 0);
        assert!(!api.state.session().linked());
    }

    // 0.x DeviceLinkTest: "attests every account with a login id once the
    // device is linked".
    #[test]
    fn attests_every_account_with_a_login_id_once_the_device_is_linked() {
        let stub = token_stub();
        let api = Arc::new(test_api(&stub.base_url, &[], &[]));
        *lock(&api.accounts) = vec![
            KnownAccount {
                username: "Qkeyed".to_string(),
                login_id: Some(6_031_972),
            },
            KnownAccount {
                username: "Qunkeyed".to_string(),
                login_id: None,
            },
        ];
        let (pending, url) = begin(&api);
        let redirect = param(&url, "redirect_uri").expect("redirect");
        let state = param(&url, "state").expect("state");
        let finished = finish_on_thread(&api, pending, Duration::from_secs(10), Arc::default());
        browser_get(
            port_of(&redirect),
            &format!("/oauth/callback?code=abc123&state={state}"),
        );
        assert_eq!(finished.join().expect("joined"), SignInEnd::SignedIn);
        let attests: Vec<Recorded> = stub
            .requests()
            .into_iter()
            .filter(|request| request.target == "/api/account/players")
            .collect();
        assert_eq!(attests.len(), 1);
        assert_eq!(attests[0].header("authorization"), Some("Bearer at1"));
        assert_eq!(
            attests[0].body,
            br#"{"login_id":6031972,"username":"Qkeyed"}"#.to_vec()
        );
    }

    #[test]
    fn the_loopback_listener_survives_junk_and_stops_at_its_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("address").port();
        let cancel = Arc::new(AtomicBool::new(false));
        let limit = Duration::from_millis(300);
        let waiting = {
            let cancel = Arc::clone(&cancel);
            std::thread::spawn(move || {
                wait_for_callback(
                    &listener,
                    "goodstate",
                    Instant::now() + Duration::from_secs(20),
                    limit,
                    &cancel,
                )
            })
        };
        // Junk is answered and the listener keeps going.
        let mut junk = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        junk.write_all(b"\x16\x03\x01 not http at all\r\n\r\n")
            .expect("write");
        let mut answer = String::new();
        let _ = junk.read_to_string(&mut answer);
        assert!(answer.starts_with("HTTP/1.1 400"), "{answer}");
        // A request with no newline is dropped once its time is up.
        let started = Instant::now();
        let mut open_ended = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        open_ended
            .write_all(b"GET /oauth/callback?code=x&state=goodstate")
            .expect("write");
        let mut rest = Vec::new();
        let _ = open_ended.read_to_end(&mut rest);
        assert!(started.elapsed() < Duration::from_secs(5));
        // A slow drip is dropped too.
        let started = Instant::now();
        let mut drip = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        drip.set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        for byte in b"GET /oauth/callback?code=x&state=goodstate HTTP/1.1\r\n\r\n" {
            if drip.write_all(&[*byte]).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(40));
            if started.elapsed() > Duration::from_secs(2) {
                break;
            }
        }
        let mut rest = Vec::new();
        let _ = drip.read_to_end(&mut rest);
        assert!(
            !String::from_utf8_lossy(&rest).contains("Signed in"),
            "the drip never counts"
        );
        // An oversized head is refused.
        let mut huge = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        let mut big = b"GET /oauth/callback?pad=".to_vec();
        big.extend(std::iter::repeat_n(b'a', 9_000));
        let _ = huge.write_all(&big);
        let mut answer = String::new();
        let _ = huge.read_to_string(&mut answer);
        assert!(!answer.contains("Signed in"), "{answer}");
        // Then a real callback completes.
        let page = browser_get(port, "/oauth/callback?code=qcode&state=goodstate");
        assert!(page.contains("Signed in."), "{page}");
        assert_eq!(
            waiting.join().expect("joined"),
            Waited::Code("qcode".to_string())
        );

        // With a short deadline and no callback, it stops.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let started = Instant::now();
        let waited = wait_for_callback(
            &listener,
            "goodstate",
            Instant::now() + Duration::from_millis(200),
            limit,
            &AtomicBool::new(false),
        );
        assert_eq!(waited, Waited::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn cancel_stops_the_wait() {
        let stub = token_stub();
        let api = Arc::new(test_api(&stub.base_url, &[], &[]));
        let (pending, _) = begin(&api);
        let cancel = Arc::new(AtomicBool::new(false));
        let finished =
            finish_on_thread(&api, pending, Duration::from_secs(20), Arc::clone(&cancel));
        cancel.store(true, Ordering::SeqCst);
        let started = Instant::now();
        assert_eq!(finished.join().expect("joined"), SignInEnd::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn sign_in_is_refused_offline_and_without_a_client_id() {
        let stub = token_stub();
        let api = test_api(&stub.base_url, &[], &[]);
        api.state.session().gate.offline = true;
        let opened = AtomicBool::new(false);
        let open = |_: &str| {
            opened.store(true, Ordering::SeqCst);
            Ok(())
        };
        assert_eq!(
            begin_sign_in(&api.state, &open).err().as_deref(),
            Some("Offline mode is on. Turn it off in Data & Privacy to sync.")
        );
        let unconfigured = crate::api::session::testing::test_api_with_client(&stub.base_url, None);
        assert_eq!(
            begin_sign_in(&unconfigured.state, &open).err().as_deref(),
            Some("Sign-in is not configured in this build.")
        );
        assert!(!opened.load(Ordering::SeqCst), "no browser was opened");
        assert!(!account_status_now(&unconfigured.state).configured);
        assert!(account_status_now(&api.state).configured);
    }

    #[test]
    fn a_browser_that_cannot_open_ends_the_sign_in() {
        let stub = token_stub();
        let api = test_api(&stub.base_url, &[], &[]);
        let open = |_: &str| Err("no browser".to_string());
        assert_eq!(
            begin_sign_in(&api.state, &open).err().as_deref(),
            Some("The browser could not be opened.")
        );
    }

    // 0.x SyncSettingsTest: "clears sync tokens on unlink".
    #[test]
    fn clears_sync_tokens_on_unlink() {
        let api = test_api(
            "http://127.0.0.1:1",
            &[("api_token_expires_at", "1")],
            &[
                (API_KEY, "qdevice-key"),
                (ACCESS_TOKEN, "access-token"),
                (REFRESH_TOKEN, "refresh-token"),
            ],
        );
        assert!(account_status_now(&api.state).linked);
        sign_out_now(&api.state);
        let status = account_status_now(&api.state);
        assert!(!status.linked);
        assert_eq!(api.secrets.get(ACCESS_TOKEN).expect("get"), None);
        assert_eq!(api.secrets.get(REFRESH_TOKEN).expect("get"), None);
        assert_eq!(api.saved("api_token_expires_at").as_deref(), Some("null"));
        // Device mode continues with the kept key (ruling 8).
        assert_eq!(
            api.secrets.get(API_KEY).expect("get").as_deref(),
            Some("qdevice-key")
        );
    }

    #[test]
    fn the_status_never_carries_a_secret() {
        let api = test_api(
            "http://127.0.0.1:1",
            &[],
            &[
                (API_KEY, "qsecret-key"),
                (ACCESS_TOKEN, "qsecret-access"),
                (REFRESH_TOKEN, "qsecret-refresh"),
            ],
        );
        let json = serde_json::to_string(&account_status_now(&api.state)).expect("json");
        assert!(!json.contains("qsecret"), "{json}");
        assert_eq!(
            json,
            r#"{"configured":true,"linked":true,"signingIn":false,"offline":false,"error":null}"#
        );
    }

    // Q3 review Important 1: a refresh already in flight cannot write its
    // tokens back after a sign-out.
    #[test]
    fn a_refresh_in_flight_cannot_undo_a_sign_out() {
        let stub = Stub::start(|request| {
            if request.target == "/oauth/token" {
                std::thread::sleep(Duration::from_millis(300));
            }
            Reply::json(
                200,
                r#"{"access_token":"at2","refresh_token":"rt2","expires_in":2592000}"#,
            )
        });
        let soon = (NOW + 60_000).to_string();
        let api = Arc::new(test_api(
            &stub.base_url,
            &[("api_token_expires_at", &soon)],
            &[(ACCESS_TOKEN, "at1"), (REFRESH_TOKEN, "rt1")],
        ));
        let refreshing = {
            let api = Arc::clone(&api);
            std::thread::spawn(move || crate::api::tokens::refresh(&api.state, NOW))
        };
        let started = Instant::now();
        while stub.requests().is_empty() {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "the refresh began"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        sign_out_now(&api.state);
        let _ = refreshing.join().expect("joined");
        assert!(!api.state.session().linked());
        assert_eq!(api.secrets.get(ACCESS_TOKEN).expect("get"), None);
        assert_eq!(api.secrets.get(REFRESH_TOKEN).expect("get"), None);
        assert_eq!(api.saved("api_token_expires_at").as_deref(), Some("null"));
    }

    // Q3 review Minor 2: an error with another state is refused, and the
    // sign-in keeps waiting for its own callback.
    #[test]
    fn an_error_with_another_state_does_not_end_the_wait() {
        let stub = token_stub();
        let api = Arc::new(test_api(&stub.base_url, &[], &[]));
        let (pending, url) = begin(&api);
        let redirect = param(&url, "redirect_uri").expect("redirect");
        let state = param(&url, "state").expect("state");
        let finished = finish_on_thread(&api, pending, Duration::from_secs(10), Arc::default());
        let port = port_of(&redirect);
        for target in [
            "/oauth/callback?error=x",
            "/oauth/callback?error=access_denied&state=qnotthestate",
        ] {
            let page = browser_get(port, target);
            assert!(page.starts_with("HTTP/1.1 400"), "{page}");
        }
        browser_get(port, &format!("/oauth/callback?code=abc123&state={state}"));
        assert_eq!(finished.join().expect("joined"), SignInEnd::SignedIn);
    }

    // Q3 review Minor 3: the slot is claimed first, and its lock is not
    // held while the keychain opens or the browser starts.
    #[test]
    fn the_slot_lock_is_free_while_the_browser_opens() {
        let stub = token_stub();
        let api = test_api(&stub.base_url, &[], &[]);
        let checked = AtomicBool::new(false);
        let open = |_: &str| -> Result<(), String> {
            assert!(
                api.state.sign_in.running.try_lock().is_ok(),
                "the lock is free"
            );
            assert!(
                lock(&api.state.sign_in.running).is_some(),
                "the slot is claimed"
            );
            assert_eq!(
                claim_and_begin(&api.state, &|_: &str| Ok(())).err(),
                Some(ALREADY_WAITING.to_string())
            );
            checked.store(true, Ordering::SeqCst);
            Ok(())
        };
        let (_pending, cancel) = claim_and_begin(&api.state, &open).expect("begun");
        assert!(checked.load(Ordering::SeqCst));
        assert!(!cancel.load(Ordering::SeqCst));
        assert!(lock(&api.state.sign_in.running).is_some());
    }

    #[test]
    fn a_sign_in_that_cannot_begin_frees_the_slot() {
        let api = test_api("http://127.0.0.1:1", &[], &[]);
        api.state.session().gate.offline = true;
        assert_eq!(
            claim_and_begin(&api.state, &|_: &str| Ok(())).err(),
            Some(OFFLINE_REFUSAL.to_string())
        );
        assert!(lock(&api.state.sign_in.running).is_none());
    }

    // Q3 review Minor 4: a Cancel that lands while the callback is being
    // read stops before the code is exchanged.
    #[test]
    fn a_cancel_before_the_exchange_stops_the_sign_in() {
        let stub = token_stub();
        let api = Arc::new(test_api(&stub.base_url, &[], &[]));
        let (pending, url) = begin(&api);
        let redirect = param(&url, "redirect_uri").expect("redirect");
        let state = param(&url, "state").expect("state");
        let cancel = Arc::new(AtomicBool::new(false));
        let finished =
            finish_on_thread(&api, pending, Duration::from_secs(10), Arc::clone(&cancel));
        let mut stream = TcpStream::connect(("127.0.0.1", port_of(&redirect))).expect("connect");
        stream
            .write_all(
                format!("GET /oauth/callback?code=abc123&state={state} HTTP/1.1\r\n").as_bytes(),
            )
            .expect("write");
        // The listener has accepted and is reading the head.
        std::thread::sleep(Duration::from_millis(200));
        cancel.store(true, Ordering::SeqCst);
        stream.write_all(b"\r\n").expect("write");
        let mut answer = String::new();
        let _ = stream.read_to_string(&mut answer);
        assert_eq!(finished.join().expect("joined"), SignInEnd::Cancelled);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(stub.connections(), 0, "nothing was exchanged");
        assert!(!api.state.session().linked());
    }
}
