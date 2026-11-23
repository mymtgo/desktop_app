//! The device key (survey section 1, 0.x `RegisterDevice`): mint the device
//! id at first need, register for a key when there is none or it expired,
//! and register again on a 401 while signed out. A signed-in session sends
//! its bearer and never registers or rotates the key.

use mymtgo_api::{
    ApiError, KEY_LIFETIME_MS, key_needs_refresh, parse_register, register_device_request, uuid_v4,
};

use crate::api::random::random_bytes;
use crate::api::secrets::API_KEY;
use crate::api::session::ApiState;
use crate::bridge::lock;
use crate::settings::{API_DEVICE_ID, API_KEY_EXPIRES_AT};

/// Make sure a signed-out device has a fresh key: register when it has none
/// or it expired. Signed in, nothing happens.
pub fn ensure_device_key(state: &ApiState, now_ms: i64) -> Result<(), ApiError> {
    if !needs_key(state, now_ms) {
        return Ok(());
    }
    let _one = lock(&state.registering);
    // Another caller may have registered while this one waited.
    if !needs_key(state, now_ms) {
        return Ok(());
    }
    register(state, now_ms)
}

/// `ensure_device_key` for a request signed with the device key only
/// (`AuthMode::Device`): renewed when missing or expired even while signed
/// in. Registering only rotates the key on the server.
pub fn ensure_device_key_signed_in_too(state: &ApiState, now_ms: i64) -> Result<(), ApiError> {
    if !key_missing_or_expired(state, now_ms) {
        return Ok(());
    }
    let _one = lock(&state.registering);
    if !key_missing_or_expired(state, now_ms) {
        return Ok(());
    }
    register(state, now_ms)
}

fn needs_key(state: &ApiState, now_ms: i64) -> bool {
    !state.session().linked() && key_missing_or_expired(state, now_ms)
}

fn key_missing_or_expired(state: &ApiState, now_ms: i64) -> bool {
    let session = state.session();
    session.api_key.is_none() || key_needs_refresh(session.api_key_expires_at, now_ms)
}

/// Register now, whatever the stored key's age (0.x `RegisterDevice::run`):
/// true when a key was stored. Signed in, nothing is sent and the answer is
/// false.
pub fn register_device(state: &ApiState, now_ms: i64) -> bool {
    if state.session().linked() {
        return false;
    }
    let _one = lock(&state.registering);
    register(state, now_ms).is_ok()
}

/// A 401 while signed out: the key was refused, so register again (0.x
/// `RecoverFromUnauthorized`, unlinked branch). Signed in is Q3's refresh.
pub fn recover_unauthorized_unlinked(state: &ApiState, now_ms: i64) -> bool {
    register_device(state, now_ms)
}

/// The device id, minted (UUID v4) and saved when there is none.
pub fn ensure_device_id(state: &ApiState) -> String {
    let mut session = state.session();
    if let Some(id) = &session.device_id {
        return id.clone();
    }
    let bytes: [u8; 16] = random_bytes(16)
        .try_into()
        .expect("random_bytes gives the asked length");
    let id = uuid_v4(bytes);
    session.device_id = Some(id.clone());
    drop(session);
    state.persist(vec![(API_DEVICE_ID.to_string(), json_string(&id))]);
    id
}

/// One registration. Failures are logged at error with the status and body
/// (at warning when MyMTGO could not be reached) and returned; there is no
/// retry.
fn register(state: &ApiState, now_ms: i64) -> Result<(), ApiError> {
    let device_id = ensure_device_id(state);
    let request = register_device_request(&device_id);
    let gate = state.session().gate;
    let result = state
        .client()
        .and_then(|client| client.send(&request, &gate, &Default::default()));
    let response = match result {
        Ok(response) => response,
        // No network or a timeout: the next request registers again.
        Err(error) if error.is_transient() => {
            tracing::warn!(%error, "device registration could not reach MyMTGO");
            return Err(error);
        }
        Err(error) => {
            tracing::error!(%error, "device registration failed");
            return Err(error);
        }
    };
    let Some(key) = parse_register(&response) else {
        let error = if response.is_success() {
            ApiError::Malformed("the registration answer has no api_key".to_string())
        } else {
            response.status_error()
        };
        tracing::error!(status = response.status, %error, "device registration failed");
        return Err(error);
    };
    state.set_secret(API_KEY, Some(&key));
    let expires_at = now_ms + KEY_LIFETIME_MS;
    state.session().api_key_expires_at = Some(expires_at);
    state.persist(vec![(
        API_KEY_EXPIRES_AT.to_string(),
        expires_at.to_string(),
    )]);
    Ok(())
}

fn json_string(value: &str) -> String {
    serde_json::Value::String(value.to_string()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::secrets::{ACCESS_TOKEN, REFRESH_TOKEN, SecretStore};
    use crate::api::session::testing::test_api;
    use crate::api::stub::{Reply, Stub};
    use mymtgo_api::{ApiRequest, AuthMode, Method, RequestClass};

    const NOW: i64 = 1_800_000_000_000;
    const HOUR: i64 = 3_600_000;

    fn register_stub(key: &str) -> Stub {
        let body = format!(r#"{{"api_key":"{key}"}}"#);
        Stub::start(move |request| {
            if request.target == "/api/devices/register" {
                Reply::json(200, &body)
            } else {
                Reply::json(200, "[]")
            }
        })
    }

    // The offline archetype download signs with the device key even when
    // signed in, so an expired key is renewed first, signed in or not.
    #[test]
    fn a_device_request_renews_an_expired_key_while_signed_in() {
        let stub = register_stub("key-new");
        let expired = (NOW - HOUR).to_string();
        let token_valid = (NOW + 48 * HOUR).to_string();
        let api = test_api(
            &stub.base_url,
            &[
                ("api_device_id", r#""test-device""#),
                ("api_key_expires_at", &expired),
                ("api_token_expires_at", &token_valid),
                (mymtgo_api::OFFLINE_MODE, "true"),
            ],
            &[
                (API_KEY, "key-old"),
                (ACCESS_TOKEN, "at"),
                (REFRESH_TOKEN, "rt"),
            ],
        );
        api.state
            .send(&mymtgo_api::offline_catalog_request(), NOW)
            .expect("sent");
        let requests = stub.requests();
        assert_eq!(requests.len(), 2, "{requests:?}");
        assert_eq!(requests[0].target, "/api/devices/register");
        assert_eq!(requests[1].header("x-api-key"), Some("key-new"));
        assert_eq!(requests[1].header("authorization"), None);
    }

    // 0.x ApiKeyFreshnessTest: "re-registers when the key has expired".
    #[test]
    fn re_registers_when_the_key_has_expired() {
        let stub = register_stub("fresh-key");
        let expired = (NOW - HOUR).to_string();
        let api = test_api(
            &stub.base_url,
            &[
                ("api_device_id", r#""device-freshness-test""#),
                ("api_key_expires_at", &expired),
            ],
            &[("api_key", "stale-key")],
        );
        api.state.session().gate.offline = true;
        ensure_device_key(&api.state, NOW).expect("registered");
        assert_eq!(
            api.secrets.get("api_key").expect("get").as_deref(),
            Some("fresh-key")
        );
        assert_eq!(api.state.session().api_key.as_deref(), Some("fresh-key"));
        assert_eq!(
            api.saved("api_key_expires_at"),
            Some((NOW + 47 * HOUR).to_string())
        );
        let seen = stub.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].method, "POST");
        assert_eq!(seen[0].body, br#"{"device_id":"device-freshness-test"}"#);
        assert_eq!(
            seen[0].header("x-api-key"),
            None,
            "registration sends no auth"
        );
    }

    // 0.x ApiKeyFreshnessTest: "does not re-register while the key is still valid".
    #[test]
    fn does_not_re_register_while_the_key_is_still_valid() {
        let stub = register_stub("fresh-key");
        let valid = (NOW + 24 * HOUR).to_string();
        let api = test_api(
            &stub.base_url,
            &[
                ("api_device_id", r#""device-freshness-test""#),
                ("api_key_expires_at", &valid),
            ],
            &[("api_key", "good-key")],
        );
        api.state.session().gate.offline = true;
        ensure_device_key(&api.state, NOW).expect("fresh");
        assert_eq!(api.state.session().api_key.as_deref(), Some("good-key"));
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(stub.connections(), 0);
    }

    // 0.x ApiKeyFreshnessTest: "keeps card identity requests working past
    // the key lifetime while offline".
    #[test]
    fn keeps_card_identity_requests_working_past_the_key_lifetime_while_offline() {
        let stub = register_stub("fresh-key");
        let expired = (NOW - HOUR).to_string();
        let api = test_api(
            &stub.base_url,
            &[
                ("api_device_id", r#""device-freshness-test""#),
                ("api_key_expires_at", &expired),
            ],
            &[("api_key", "stale-key")],
        );
        api.state.session().gate.offline = true;
        let cards = ApiRequest::new(
            Method::Post,
            "/api/cards",
            RequestClass::Reference,
            AuthMode::DeviceOrBearer,
        );
        let response = api.state.send(&cards, NOW).expect("sent offline");
        assert_eq!(response.status, 200);
        let seen = stub.requests();
        let card_request = seen
            .iter()
            .find(|request| request.target == "/api/cards")
            .expect("the card request went out");
        assert_eq!(card_request.header("x-api-key"), Some("fresh-key"));
        assert_eq!(
            card_request.header("x-device-id"),
            Some("device-freshness-test")
        );
    }

    // 0.x RecoverFromUnauthorizedTest: "re-registers the device when not linked".
    #[test]
    fn re_registers_the_device_when_not_linked() {
        let stub = register_stub("fresh-key");
        let valid = (NOW + 24 * HOUR).to_string();
        let api = test_api(
            &stub.base_url,
            &[
                ("api_device_id", r#""qdevice""#),
                ("api_key_expires_at", &valid),
            ],
            &[("api_key", "refused-key")],
        );
        assert!(recover_unauthorized_unlinked(&api.state, NOW));
        assert_eq!(api.state.session().api_key.as_deref(), Some("fresh-key"));
        let seen = stub.requests();
        assert!(
            seen.iter()
                .all(|request| !request.target.ends_with("/oauth/token"))
        );
        assert_eq!(seen.len(), 1);
    }

    #[test]
    fn a_linked_session_never_registers_or_rotates_the_key() {
        let stub = register_stub("fresh-key");
        let expired = (NOW - HOUR).to_string();
        let api = test_api(
            &stub.base_url,
            &[
                ("api_device_id", r#""qdevice""#),
                ("api_key_expires_at", &expired),
            ],
            &[
                ("api_key", "expired-key"),
                (ACCESS_TOKEN, "qaccess"),
                (REFRESH_TOKEN, "qrefresh"),
            ],
        );
        ensure_device_key(&api.state, NOW).expect("nothing to do");
        assert!(!register_device(&api.state, NOW));
        assert!(!recover_unauthorized_unlinked(&api.state, NOW));
        assert_eq!(api.state.session().api_key.as_deref(), Some("expired-key"));
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(stub.connections(), 0);
    }

    #[test]
    fn the_device_id_is_minted_once_and_saved() {
        let stub = register_stub("fresh-key");
        let api = test_api(&stub.base_url, &[], &[]);
        ensure_device_key(&api.state, NOW).expect("registered");
        let saved = api.saved("api_device_id").expect("the id is saved");
        let id: String = serde_json::from_str(&saved).expect("a JSON string");
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
        assert_eq!(ensure_device_id(&api.state), id, "minted once");
        let body = String::from_utf8(stub.requests()[0].body.clone()).expect("utf8");
        assert_eq!(body, format!(r#"{{"device_id":"{id}"}}"#));
    }

    #[test]
    fn a_failed_registration_stores_nothing_and_is_not_retried() {
        let stub = Stub::start(|_| Reply::json(500, r#"{"error":"down"}"#));
        let api = test_api(&stub.base_url, &[("api_device_id", r#""qdevice""#)], &[]);
        let error = ensure_device_key(&api.state, NOW).expect_err("failed");
        assert_eq!(
            error,
            ApiError::Status(500, r#"{"error":"down"}"#.to_string())
        );
        assert_eq!(api.state.session().api_key, None);
        assert_eq!(api.saved("api_key_expires_at"), None);
        assert_eq!(stub.requests().len(), 1);
    }
}
