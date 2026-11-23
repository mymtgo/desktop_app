//! The signed-in session's tokens (survey section 2, 0.x `SyncTokens`,
//! `EnsureAccessToken`, `RefreshAccessToken`, `ExchangeAuthorizationCode`
//! and the linked half of `RecoverFromUnauthorized`).
//!
//! Passport rotates the refresh token on every refresh, so two refreshes
//! with the same token would revoke it and sign the user out. Every refresh
//! therefore runs under one lock (`TokenKeeper`), and a caller that waited
//! for it uses the token the first caller stored instead of refreshing
//! again.

use std::sync::Mutex;

use mymtgo_api::{
    ApiError, ApiRequest, ApiResponse, Credentials, RefreshResult, SyncAuth, SyncAuthStep,
    TokenError, Tokens, exchange_request, needs_refresh, parse_token, refresh_request,
};

use crate::api::device::{ensure_device_key, register_device};
use crate::api::secrets::{ACCESS_TOKEN, REFRESH_TOKEN};
use crate::api::session::ApiState;
use crate::bridge::lock;
use crate::settings::API_TOKEN_EXPIRES_AT;

/// The one refresh lock, held for a whole refresh.
#[derive(Default)]
pub struct TokenKeeper {
    refreshing: Mutex<()>,
}

/// What `access_token` reads from the session in one go.
struct Snapshot {
    linked: bool,
    offline: bool,
    access: Option<String>,
    refresh: Option<String>,
    expires_at: Option<i64>,
}

fn snapshot(state: &ApiState) -> Snapshot {
    let session = state.session();
    Snapshot {
        linked: session.linked(),
        offline: session.gate.offline,
        access: session.access_token.clone(),
        refresh: session.refresh_token.clone(),
        expires_at: session.token_expires_at,
    }
}

/// A usable access token (0.x `EnsureAccessToken`): `NotLinked` when signed
/// out; offline, the stored token as it is; online, refreshed first when it
/// expires within 24 hours. A caller that waited for another caller's
/// refresh returns the token that refresh stored.
pub fn access_token(state: &ApiState, now_ms: i64) -> Result<String, ApiError> {
    let before = snapshot(state);
    let Some(access) = before.access.clone().filter(|_| before.linked) else {
        return Err(ApiError::NotLinked);
    };
    if before.offline || !needs_refresh(before.expires_at, now_ms) {
        return Ok(access);
    }
    let _one = lock(&state.tokens.refreshing);
    let now = snapshot(state);
    let Some(access) = now.access.clone().filter(|_| now.linked) else {
        return Err(ApiError::NotLinked);
    };
    // Another caller refreshed while this one waited: its tokens are new.
    if now.refresh != before.refresh || !needs_refresh(now.expires_at, now_ms) {
        return Ok(access);
    }
    refresh_locked(state, now_ms)
}

/// Refresh now, whatever the expiry (0.x `RefreshAccessToken::run`): both
/// tokens rotate. A dead refresh token (`invalid_grant`) signs out and
/// answers `NotLinked`; any other failure keeps the credentials and returns
/// the error.
pub fn refresh(state: &ApiState, now_ms: i64) -> Result<String, ApiError> {
    let _one = lock(&state.tokens.refreshing);
    refresh_locked(state, now_ms)
}

/// The refresh itself; the caller holds the refresh lock.
fn refresh_locked(state: &ApiState, now_ms: i64) -> Result<String, ApiError> {
    let (refresh_token, gate) = {
        let session = state.session();
        (session.refresh_token.clone(), session.gate)
    };
    let Some(refresh_token) = refresh_token else {
        return Err(ApiError::NotLinked);
    };
    let request = refresh_request(state.config()?, &refresh_token);
    let response = state
        .client()?
        .send(&request, &gate, &Credentials::default())?;
    match parse_token(&response, now_ms) {
        Ok(tokens) => {
            store_tokens(state, &tokens);
            Ok(tokens.access)
        }
        Err(TokenError::InvalidGrant) => {
            tracing::warn!(
                status = response.status,
                "the refresh token was revoked; signing this device out"
            );
            clear_tokens(state);
            Err(ApiError::NotLinked)
        }
        Err(TokenError::Other(message)) => {
            tracing::error!(status = response.status, %message, "the token refresh failed");
            Err(token_failure(&response, message))
        }
    }
}

/// The error for a failed token answer: the status for a refusal, else
/// `Malformed` (the message never quotes a token).
fn token_failure(response: &mymtgo_api::ApiResponse, message: String) -> ApiError {
    if response.is_success() {
        ApiError::Malformed(message)
    } else {
        response.status_error()
    }
}

/// Exchange the consent screen's code for tokens and store them (0.x
/// `ExchangeAuthorizationCode`). A refusal stores nothing.
pub fn exchange_code(
    state: &ApiState,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
    now_ms: i64,
) -> Result<(), ApiError> {
    let request: ApiRequest = exchange_request(state.config()?, code, verifier, redirect_uri);
    let gate = state.session().gate;
    let response = state
        .client()?
        .send(&request, &gate, &Credentials::default())?;
    match parse_token(&response, now_ms) {
        Ok(tokens) => {
            let _one = lock(&state.tokens.refreshing);
            store_tokens(state, &tokens);
            Ok(())
        }
        Err(TokenError::InvalidGrant) => {
            tracing::error!(status = response.status, "the sign-in code was refused");
            Err(response.status_error())
        }
        Err(TokenError::Other(message)) => {
            tracing::error!(status = response.status, %message, "the sign-in code was refused");
            Err(token_failure(&response, message))
        }
    }
}

/// Store both tokens (keychain) and the expiry (settings row).
fn store_tokens(state: &ApiState, tokens: &Tokens) {
    state.set_secret(ACCESS_TOKEN, Some(&tokens.access));
    state.set_secret(REFRESH_TOKEN, Some(&tokens.refresh));
    state.session().token_expires_at = Some(tokens.expires_at_ms);
    state.persist(vec![(
        API_TOKEN_EXPIRES_AT.to_string(),
        tokens.expires_at_ms.to_string(),
    )]);
}

/// Forget both tokens and the expiry (0.x `SyncTokens::clear`). The device
/// key stays: requests go on in device mode.
pub fn clear_tokens(state: &ApiState) {
    state.set_secret(ACCESS_TOKEN, None);
    state.set_secret(REFRESH_TOKEN, None);
    state.session().token_expires_at = None;
    state.persist(vec![(API_TOKEN_EXPIRES_AT.to_string(), "null".to_string())]);
}

/// Sign out: wait for any refresh in flight, then forget both tokens and
/// the expiry, so a refresh that already sent its request cannot store its
/// answer after the sign-out.
pub fn sign_out_tokens(state: &ApiState) {
    let _one = lock(&state.tokens.refreshing);
    clear_tokens(state);
}

/// Sign out from the Account page (0.x `SyncController::unlink`): forget
/// both tokens, then the deck slot ledger and every deck's sync flag (no
/// account, no slots), through the tracker and without touching
/// `decks.updated_at`. The device key stays (ruling 8). A tracker that is
/// restarting (no editor) or does not answer leaves the mirror for the next
/// sign-in's first run to replace.
pub fn sign_out_and_forget_slots(state: &ApiState, editor: Option<&tracker::SyncEditor>) {
    sign_out_tokens(state);
    let Some(editor) = editor else {
        tracing::warn!("tracking is not running; the deck slots are cleared at the next run");
        return;
    };
    if let Err(error) = editor.edit(store::sync::SyncEdit::SignedOut) {
        tracing::warn!(%error, "could not clear the deck slots after signing out");
    }
}

/// A request answered 401 (0.x `RecoverFromUnauthorized`): true when a retry
/// can go out. Signed in, the token is refreshed, but only if the stored one
/// is still `failed_token` (otherwise another caller already refreshed); a
/// dead refresh token signs out and makes sure the device key is fresh, so
/// the retry goes out in device mode. Signed out, the device registers
/// again.
pub fn recover_unauthorized(state: &ApiState, failed_token: Option<&str>, now_ms: i64) -> bool {
    if !state.session().linked() {
        return register_device(state, now_ms);
    }
    let _one = lock(&state.tokens.refreshing);
    let stored = state.session().access_token.clone();
    if stored.as_deref() != failed_token {
        return true;
    }
    match refresh_locked(state, now_ms) {
        Ok(_) => true,
        Err(ApiError::NotLinked) => ensure_device_key(state, now_ms).is_ok(),
        Err(error) => {
            tracing::warn!(%error, "could not refresh the access token after a 401");
            false
        }
    }
}

/// After a 401 for the bearer `used`: refresh, unless another caller
/// already replaced that token (then the resend carries the new one).
fn refresh_after_401(state: &ApiState, used: Option<&str>, now_ms: i64) -> Result<(), ApiError> {
    let _one = lock(&state.tokens.refreshing);
    if state.session().access_token.as_deref() != used {
        return Ok(());
    }
    refresh_locked(state, now_ms).map(|_| ())
}

/// Send through 0.x `SyncApi::send`'s 401 loop, the one loop the sync and
/// replay shares use (#307 F2): a 401 refreshes the token and resends
/// once; a second 401 refreshes again (a revoked refresh token is
/// `NotLinked` in one place) and then stands. Every other answer is the
/// caller's to read. A refresh that fails another way ends the send with
/// its error, the credentials kept.
pub fn send_with_refresh(
    state: &ApiState,
    request: &ApiRequest,
    now_ms: i64,
) -> Result<ApiResponse, ApiError> {
    let mut auth = SyncAuth::default();
    loop {
        let (response, used) = state.send_with_bearer(request, now_ms)?;
        let mut step = auth.after_response(&response);
        if step == SyncAuthStep::Refresh {
            let refreshed = match refresh_after_401(state, used.as_deref(), now_ms) {
                Ok(()) => RefreshResult::Refreshed,
                Err(ApiError::NotLinked) => RefreshResult::InvalidGrant,
                Err(error) => return Err(error),
            };
            step = auth.after_refresh(refreshed);
        }
        match step {
            SyncAuthStep::Done => return Ok(response),
            SyncAuthStep::Send | SyncAuthStep::Refresh => continue,
            SyncAuthStep::SignedOut => return Err(ApiError::NotLinked),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::secrets::{API_KEY, SecretStore};
    use crate::api::session::testing::{TestApi, test_api};
    use crate::api::stub::{Recorded, Reply, Stub};
    use mymtgo_api::php::urldecode;
    use mymtgo_api::{AuthMode, Method, RequestClass};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    const NOW: i64 = 1_800_000_000_000;
    const HOUR: i64 = 3_600_000;
    const MONTH_S: i64 = 2_592_000;

    fn tokens_json(access: &str, refresh: &str) -> String {
        format!(
            r#"{{"token_type":"Bearer","access_token":"{access}","refresh_token":"{refresh}","expires_in":{MONTH_S}}}"#
        )
    }

    /// The decoded form fields of a recorded request.
    fn form(request: &Recorded) -> Vec<(String, String)> {
        let text = String::from_utf8(request.body.clone()).expect("utf8");
        text.split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| {
                let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                (urldecode(key), urldecode(value))
            })
            .collect()
    }

    fn field(request: &Recorded, name: &str) -> Option<String> {
        form(request)
            .into_iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    }

    fn token_requests(stub: &Stub) -> Vec<Recorded> {
        stub.requests()
            .into_iter()
            .filter(|request| request.target == "/oauth/token")
            .collect()
    }

    /// Signed in with these tokens, the access token expiring at `expires_at`
    /// and a device key valid for a day.
    fn linked_api(base_url: &str, access: &str, refresh: &str, expires_at: i64) -> TestApi {
        let expires = expires_at.to_string();
        let key_valid = (NOW + 24 * HOUR).to_string();
        test_api(
            base_url,
            &[
                ("api_device_id", r#""test-device""#),
                ("api_key_expires_at", &key_valid),
                ("api_token_expires_at", &expires),
            ],
            &[
                (API_KEY, "old-key"),
                (ACCESS_TOKEN, access),
                (REFRESH_TOKEN, refresh),
            ],
        )
    }

    fn cards_request() -> ApiRequest {
        ApiRequest::new(
            Method::Post,
            "/api/cards",
            RequestClass::Reference,
            AuthMode::DeviceOrBearer,
        )
    }

    // The offline archetype download: offline and signed in with an
    // expired access token, the device request goes with the device key
    // and never tries the refresh (blocked offline) or the bearer.
    #[test]
    fn a_device_request_offline_and_signed_in_skips_the_token() {
        let stub = Stub::start(|_| Reply::json(200, "[]"));
        let key_valid = (NOW + 24 * HOUR).to_string();
        let expired = (NOW - HOUR).to_string();
        let api = test_api(
            &stub.base_url,
            &[
                ("api_device_id", r#""test-device""#),
                ("api_key_expires_at", &key_valid),
                ("api_token_expires_at", &expired),
                (mymtgo_api::OFFLINE_MODE, "true"),
            ],
            &[
                (API_KEY, "old-key"),
                (ACCESS_TOKEN, "at-old"),
                (REFRESH_TOKEN, "rt-old"),
            ],
        );
        let response = api
            .state
            .send(&mymtgo_api::offline_catalog_request(), NOW)
            .expect("sent");
        assert_eq!(response.status, 200);
        let requests = stub.requests();
        assert_eq!(requests.len(), 1, "{requests:?}");
        assert!(requests[0].target.starts_with("/api/archetypes"));
        assert_eq!(requests[0].header("x-device-id"), Some("test-device"));
        assert_eq!(requests[0].header("authorization"), None);
    }

    // 0.x DeviceLinkTest: "exchanges the code and stores rotated tokens".
    #[test]
    fn exchanges_the_code_and_stores_rotated_tokens() {
        let stub = Stub::start(|_| Reply::json(200, &tokens_json("at1", "rt1")));
        let api = test_api(&stub.base_url, &[], &[]);
        exchange_code(
            &api.state,
            "qcode",
            "qverifier",
            "http://127.0.0.1:1/oauth/callback",
            NOW,
        )
        .expect("exchanged");
        assert!(api.state.session().linked());
        assert_eq!(
            api.secrets.get(ACCESS_TOKEN).expect("get").as_deref(),
            Some("at1")
        );
        assert_eq!(
            api.secrets.get(REFRESH_TOKEN).expect("get").as_deref(),
            Some("rt1")
        );
        assert_eq!(
            api.saved("api_token_expires_at"),
            Some((NOW + MONTH_S * 1000).to_string())
        );
        let sent = token_requests(&stub);
        assert_eq!(sent.len(), 1);
        assert_eq!(
            field(&sent[0], "grant_type").as_deref(),
            Some("authorization_code")
        );
        assert_eq!(
            field(&sent[0], "code_verifier").as_deref(),
            Some("qverifier")
        );
        assert_eq!(field(&sent[0], "code").as_deref(), Some("qcode"));
        assert_eq!(field(&sent[0], "client_id").as_deref(), Some("qclient-id"));
        assert_eq!(sent[0].header("authorization"), None);
    }

    #[test]
    fn a_refused_exchange_stores_nothing() {
        let stub = Stub::start(|_| Reply::json(400, r#"{"error":"invalid_grant"}"#));
        let api = test_api(&stub.base_url, &[], &[]);
        assert!(exchange_code(&api.state, "qcode", "qverifier", "http://x", NOW).is_err());
        assert!(!api.state.session().linked());
        assert_eq!(api.secrets.get(ACCESS_TOKEN).expect("get"), None);
        assert_eq!(api.saved("api_token_expires_at"), None);
    }

    // 0.x DeviceLinkTest: "refreshes and rotates both tokens".
    #[test]
    fn refreshes_and_rotates_both_tokens() {
        let stub = Stub::start(|_| Reply::json(200, &tokens_json("at2", "rt2")));
        let api = linked_api(&stub.base_url, "old-at", "old-rt", NOW + 60_000);
        assert_eq!(refresh(&api.state, NOW).expect("refreshed"), "at2");
        assert_eq!(
            api.secrets.get(ACCESS_TOKEN).expect("get").as_deref(),
            Some("at2")
        );
        assert_eq!(
            api.secrets.get(REFRESH_TOKEN).expect("get").as_deref(),
            Some("rt2")
        );
        let sent = token_requests(&stub);
        assert_eq!(
            field(&sent[0], "grant_type").as_deref(),
            Some("refresh_token")
        );
        assert_eq!(field(&sent[0], "refresh_token").as_deref(), Some("old-rt"));
    }

    // 0.x DeviceLinkTest: "clears credentials and reports unlinked when the
    // refresh is refused".
    #[test]
    fn clears_credentials_and_reports_unlinked_when_the_refresh_is_refused() {
        let stub = Stub::start(|_| Reply::json(400, r#"{"error":"invalid_grant"}"#));
        let api = linked_api(&stub.base_url, "old-at", "revoked-rt", NOW + 60_000);
        assert_eq!(refresh(&api.state, NOW), Err(ApiError::NotLinked));
        assert!(!api.state.session().linked());
        assert_eq!(api.secrets.get(ACCESS_TOKEN).expect("get"), None);
        assert_eq!(api.secrets.get(REFRESH_TOKEN).expect("get"), None);
        assert_eq!(api.saved("api_token_expires_at").as_deref(), Some("null"));
    }

    // 0.x DeviceLinkTest: "leaves credentials intact and rethrows on a 400
    // that is not invalid_grant".
    #[test]
    fn leaves_credentials_intact_and_rethrows_on_a_400_that_is_not_invalid_grant() {
        let stub = Stub::start(|_| Reply::json(400, r#"{"error":"invalid_request"}"#));
        let api = linked_api(&stub.base_url, "old-at", "old-rt", NOW + 60_000);
        let error = refresh(&api.state, NOW).expect_err("refused");
        assert!(matches!(error, ApiError::Status(400, _)), "{error:?}");
        assert!(api.state.session().linked());
        assert_eq!(
            api.secrets.get(ACCESS_TOKEN).expect("get").as_deref(),
            Some("old-at")
        );
        assert_eq!(
            api.secrets.get(REFRESH_TOKEN).expect("get").as_deref(),
            Some("old-rt")
        );
    }

    // 0.x DeviceLinkTest: "ensures a token by refreshing one that is near expiry".
    #[test]
    fn ensures_a_token_by_refreshing_one_that_is_near_expiry() {
        let stub = Stub::start(|_| Reply::json(200, &tokens_json("fresh", "rt2")));
        let api = linked_api(&stub.base_url, "stale", "rt", NOW + HOUR);
        assert_eq!(access_token(&api.state, NOW).expect("token"), "fresh");
        // Fresh now: no second refresh.
        assert_eq!(access_token(&api.state, NOW).expect("token"), "fresh");
        assert_eq!(token_requests(&stub).len(), 1);
    }

    // 0.x DeviceLinkTest: "throws NotLinked when nothing is stored".
    #[test]
    fn throws_not_linked_when_nothing_is_stored() {
        let stub = Stub::start(|_| Reply::json(200, &tokens_json("x", "y")));
        let api = test_api(&stub.base_url, &[], &[]);
        assert_eq!(access_token(&api.state, NOW), Err(ApiError::NotLinked));
        assert_eq!(refresh(&api.state, NOW), Err(ApiError::NotLinked));
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(stub.connections(), 0);
    }

    // 0.x ApiMacroAuthTest: "uses the stored token without refreshing while offline".
    #[test]
    fn uses_the_stored_token_without_refreshing_while_offline() {
        let stub = Stub::start(|request| match request.target.as_str() {
            "/oauth/token" => Reply::json(200, &tokens_json("new-token", "r2")),
            _ => Reply::json(200, "[]"),
        });
        let api = linked_api(
            &stub.base_url,
            "access-token",
            "refresh-token",
            NOW + 60_000,
        );
        api.state.session().gate.offline = true;
        api.state.send(&cards_request(), NOW).expect("sent offline");
        let seen = stub.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].target, "/api/cards");
        assert_eq!(seen[0].header("authorization"), Some("Bearer access-token"));
        assert!(token_requests(&stub).is_empty());
    }

    // 0.x ApiMacroAuthTest: "refreshes an expiring token before sending when online".
    #[test]
    fn refreshes_an_expiring_token_before_sending_when_online() {
        let stub = Stub::start(|request| match request.target.as_str() {
            "/oauth/token" => Reply::json(200, &tokens_json("new-token", "r2")),
            _ => Reply::json(200, "[]"),
        });
        let api = linked_api(&stub.base_url, "old-token", "refresh-token", NOW + 60_000);
        api.state.send(&cards_request(), NOW).expect("sent");
        let seen = stub.requests();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].target, "/oauth/token");
        assert_eq!(seen[1].target, "/api/cards");
        assert_eq!(seen[1].header("authorization"), Some("Bearer new-token"));
    }

    // 0.x RecoverFromUnauthorizedTest: "refreshes the bearer and leaves the
    // device key alone when linked".
    #[test]
    fn refreshes_the_bearer_and_leaves_the_device_key_alone_when_linked() {
        let stub = Stub::start(|request| match request.target.as_str() {
            "/oauth/token" => Reply::json(200, &tokens_json("new-token", "r2")),
            _ => Reply::json(200, "[]"),
        });
        let api = linked_api(
            &stub.base_url,
            "dead-token",
            "refresh-token",
            NOW + 30 * 24 * HOUR,
        );
        assert!(recover_unauthorized(&api.state, Some("dead-token"), NOW));
        assert_eq!(
            api.state.session().access_token.as_deref(),
            Some("new-token")
        );
        assert_eq!(api.state.session().api_key.as_deref(), Some("old-key"));
        assert!(
            stub.requests()
                .iter()
                .all(|request| request.target != "/api/devices/register")
        );
    }

    // 0.x RecoverFromUnauthorizedTest: "falls back to device mode when the
    // refresh token has been revoked".
    #[test]
    fn falls_back_to_device_mode_when_the_refresh_token_has_been_revoked() {
        let stub = Stub::start(|request| match request.target.as_str() {
            "/oauth/token" => Reply::json(401, r#"{"error":"invalid_grant"}"#),
            "/api/devices/register" => Reply::json(200, r#"{"api_key":"fresh-key"}"#),
            _ => Reply::json(200, "[]"),
        });
        let api = linked_api(
            &stub.base_url,
            "dead-token",
            "refresh-token",
            NOW + 30 * 24 * HOUR,
        );
        // The device key lapsed while the client was linked.
        api.state.session().api_key_expires_at = Some(NOW - HOUR);
        assert!(recover_unauthorized(&api.state, Some("dead-token"), NOW));
        assert!(!api.state.session().linked());
        assert_eq!(api.state.session().api_key.as_deref(), Some("fresh-key"));
    }

    #[test]
    fn invalid_grant_falls_back_to_device_mode_keeping_the_key() {
        let stub = Stub::start(|request| match request.target.as_str() {
            "/oauth/token" => Reply::json(400, r#"{"error":"invalid_grant"}"#),
            "/api/devices/register" => Reply::json(200, r#"{"api_key":"other-key"}"#),
            _ => Reply::json(200, "[]"),
        });
        let api = linked_api(
            &stub.base_url,
            "dead-token",
            "revoked-rt",
            NOW + 30 * 24 * HOUR,
        );
        assert!(recover_unauthorized(&api.state, Some("dead-token"), NOW));
        assert!(!api.state.session().linked());
        assert_eq!(api.state.session().api_key.as_deref(), Some("old-key"));
        // The retry goes out in device mode with the kept key.
        api.state.send(&cards_request(), NOW).expect("sent");
        let seen = stub.requests();
        assert!(seen.iter().all(|r| r.target != "/api/devices/register"));
        let cards = seen
            .iter()
            .find(|r| r.target == "/api/cards")
            .expect("cards");
        assert_eq!(cards.header("x-api-key"), Some("old-key"));
        assert_eq!(cards.header("x-device-id"), Some("test-device"));
        assert_eq!(cards.header("authorization"), None);
    }

    #[test]
    fn a_401_after_another_caller_refreshed_does_not_refresh_again() {
        let stub = Stub::start(|_| Reply::json(200, &tokens_json("x", "y")));
        let api = linked_api(&stub.base_url, "current", "rt", NOW + 30 * 24 * HOUR);
        assert!(recover_unauthorized(&api.state, Some("older"), NOW));
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(stub.connections(), 0);
        assert_eq!(api.state.session().access_token.as_deref(), Some("current"));
    }

    // 0.x RecoverFromUnauthorizedTest: "re-registers the device when not
    // linked" (through the one entry point).
    #[test]
    fn recover_while_signed_out_registers_the_device() {
        let stub = Stub::start(|_| Reply::json(200, r#"{"api_key":"fresh-key"}"#));
        let api = test_api(&stub.base_url, &[("api_device_id", r#""qdevice""#)], &[]);
        assert!(recover_unauthorized(&api.state, None, NOW));
        assert_eq!(api.state.session().api_key.as_deref(), Some("fresh-key"));
        assert!(token_requests(&stub).is_empty());
    }

    #[test]
    fn two_threads_refresh_once_and_share_the_new_token() {
        // Passport rotates the refresh token: reusing "rt1" is refused.
        let used = Arc::new(AtomicBool::new(false));
        let stub = {
            let used = Arc::clone(&used);
            Stub::start(move |request| {
                let refresh = field(request, "refresh_token");
                if refresh.as_deref() == Some("rt1") && !used.swap(true, Ordering::SeqCst) {
                    // Hold the answer so the second thread is surely waiting.
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    Reply::json(200, &tokens_json("at2", "rt2"))
                } else {
                    Reply::json(400, r#"{"error":"invalid_grant"}"#)
                }
            })
        };
        let api = Arc::new(linked_api(&stub.base_url, "at1", "rt1", NOW + HOUR));
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let api = Arc::clone(&api);
                std::thread::spawn(move || access_token(&api.state, NOW))
            })
            .collect();
        for thread in threads {
            assert_eq!(thread.join().expect("joined"), Ok("at2".to_string()));
        }
        assert_eq!(token_requests(&stub).len(), 1, "exactly one refresh");
        assert!(api.state.session().linked());
        assert_eq!(
            api.secrets.get(REFRESH_TOKEN).expect("get").as_deref(),
            Some("rt2")
        );
    }

    fn status_request() -> ApiRequest {
        mymtgo_api::sync_status_request()
    }

    // #264 (Q5 M1): the 401 loop weighs the bearer the request carried, not
    // the one stored once the 401 is back. Another caller refreshes while
    // the request is out: the 401 then needs no refresh of its own, only a
    // resend with the new token.
    #[test]
    fn a_401_for_a_token_another_caller_replaced_meanwhile_is_resent_without_a_refresh() {
        let (arrived_tx, arrived_rx) = std::sync::mpsc::channel::<()>();
        let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
        let arrived_tx = Mutex::new(arrived_tx);
        let go_rx = Mutex::new(go_rx);
        let stub = Stub::start(move |request| {
            // The status call carries its `platforms[]` query.
            let path = request.target.split('?').next().unwrap_or_default();
            match (path, request.header("authorization")) {
                ("/api/sync/status", Some("Bearer qold")) => {
                    let _ = lock(&arrived_tx).send(());
                    let _ = lock(&go_rx).recv();
                    Reply::json(401, r#"{"message":"Unauthenticated."}"#)
                }
                ("/api/sync/status", _) => Reply::json(200, "{}"),
                _ => Reply::json(200, &tokens_json("qwrong", "qwrong")),
            }
        });
        let api = linked_api(&stub.base_url, "qold", "rt", NOW + 30 * 24 * HOUR);
        let answer = std::thread::scope(|scope| {
            let sending = scope.spawn(|| send_with_refresh(&api.state, &status_request(), NOW));
            arrived_rx.recv().expect("the request is out");
            api.state.set_secret(ACCESS_TOKEN, Some("qnew"));
            go_tx.send(()).expect("answer it");
            sending.join().expect("joined")
        });
        assert_eq!(answer.expect("sent").status, 200);
        assert!(token_requests(&stub).is_empty(), "no refresh of its own");
        let bearers: Vec<Option<String>> = stub
            .requests()
            .iter()
            .map(|request| request.header("authorization").map(str::to_string))
            .collect();
        assert_eq!(
            bearers,
            [
                Some("Bearer qold".to_string()),
                Some("Bearer qnew".to_string())
            ]
        );
    }

    // #307 F2: a refresh that fails for any reason but a revoked token ends
    // the send with that error; the credentials stay.
    #[test]
    fn a_refresh_that_fails_ends_the_send_with_its_error() {
        let stub = Stub::start(|request| match request.target.as_str() {
            "/oauth/token" => Reply::json(500, r#"{"message":"Server Error"}"#),
            _ => Reply::json(401, r#"{"message":"Unauthenticated."}"#),
        });
        let api = linked_api(&stub.base_url, "qold", "rt", NOW + 30 * 24 * HOUR);
        let error = send_with_refresh(&api.state, &status_request(), NOW).expect_err("failed");
        assert!(matches!(error, ApiError::Status(500, _)), "{error:?}");
        assert_eq!(token_requests(&stub).len(), 1);
        assert!(api.state.session().linked(), "credentials intact");
    }

    // 0.x SyncApiTest "propagates a non-invalid_grant refresh failure on a
    // second 401 with credentials intact" (review 4 finding 6): the first
    // refresh works, the second answers 500.
    #[test]
    fn a_refresh_failure_on_a_second_401_propagates_with_credentials_intact() {
        let minted = Mutex::new(0);
        let stub = Stub::start(move |request| match request.target.as_str() {
            "/oauth/token" => {
                let mut count = lock(&minted);
                *count += 1;
                if *count == 1 {
                    Reply::json(200, &tokens_json("qat1", "qrt1"))
                } else {
                    Reply::json(500, r#"{"message":"Server Error"}"#)
                }
            }
            _ => Reply::json(401, r#"{"message":"Unauthenticated."}"#),
        });
        let api = linked_api(&stub.base_url, "qold", "rt", NOW + 30 * 24 * HOUR);
        let error = send_with_refresh(&api.state, &status_request(), NOW).expect_err("failed");
        assert!(matches!(error, ApiError::Status(500, _)), "{error:?}");
        assert_eq!(token_requests(&stub).len(), 2);
        assert!(api.state.session().linked(), "credentials intact");
        assert_eq!(api.state.session().access_token.as_deref(), Some("qat1"));
    }

    // 0.x SyncApi::send: a second 401 refreshes again (a revoked token signs
    // out in one place); after a good refresh the 401 is the caller's.
    #[test]
    fn a_second_401_refreshes_again_and_is_returned() {
        let minted = Mutex::new(0);
        let stub = Stub::start(move |request| match request.target.as_str() {
            "/oauth/token" => {
                let mut count = lock(&minted);
                *count += 1;
                Reply::json(
                    200,
                    &tokens_json(&format!("qat{count}"), &format!("qrt{count}")),
                )
            }
            _ => Reply::json(401, r#"{"message":"Unauthenticated."}"#),
        });
        let api = linked_api(&stub.base_url, "qold", "rt", NOW + 30 * 24 * HOUR);
        let answer = send_with_refresh(&api.state, &status_request(), NOW).expect("answered");
        assert_eq!(answer.status, 401);
        assert_eq!(token_requests(&stub).len(), 2, "one refresh per 401");
        let revoked = Stub::start(|request| match request.target.as_str() {
            "/oauth/token" => Reply::json(400, r#"{"error":"invalid_grant"}"#),
            _ => Reply::json(401, r#"{"message":"Unauthenticated."}"#),
        });
        let api = linked_api(&revoked.base_url, "qold", "rt", NOW + 30 * 24 * HOUR);
        assert_eq!(
            send_with_refresh(&api.state, &status_request(), NOW).map(|answer| answer.status),
            Err(ApiError::NotLinked)
        );
    }
}
