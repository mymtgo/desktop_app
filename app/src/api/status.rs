//! The API Status card's commands (survey section 4): `api_status` asks
//! `GET /api/status` (0.x `CheckApiStatusController`), `reauthenticate`
//! registers the device again and asks once more (0.x
//! `ReauthenticateController`).

use mymtgo_api::{ApiStatus, parse_status, status_request};
use tauri::{AppHandle, Manager};

use crate::api::device::register_device;
use crate::api::session::ApiState;

/// What the card shows when registering again failed.
pub const REGISTRATION_FAILED: &str = "Device registration failed. Check connection or try again.";

/// The API's state. Offline mode answers `Offline` without a request.
pub fn check_status(state: &ApiState, now_ms: i64) -> ApiStatus {
    let (offline, linked) = {
        let session = state.session();
        (session.gate.offline, session.linked())
    };
    if offline {
        return ApiStatus::Offline;
    }
    parse_status(state.send(&status_request(), now_ms), linked)
}

/// Register the device again, then check. Signed in, the bearer is what the
/// API checks, so nothing is registered and only the status is asked.
pub fn reauthenticate_now(state: &ApiState, now_ms: i64) -> ApiStatus {
    let linked = state.session().linked();
    if !linked && !register_device(state, now_ms) {
        return ApiStatus::Unreachable {
            error: REGISTRATION_FAILED.to_string(),
        };
    }
    check_status(state, now_ms)
}

/// Run `work` with the API state on a blocking thread (the client and the
/// keychain block).
async fn on_blocking_thread(app: AppHandle, work: fn(&ApiState, i64) -> ApiStatus) -> ApiStatus {
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<ApiState>();
        work(&state, crate::wall_clock_ms())
    })
    .await;
    joined.unwrap_or_else(|error| ApiStatus::Unreachable {
        error: format!("the status check stopped unexpectedly: {error}"),
    })
}

/// The API Status card's check (checked on mount and by "Recheck").
#[tauri::command]
#[specta::specta]
pub async fn api_status(app: AppHandle) -> ApiStatus {
    on_blocking_thread(app, check_status).await
}

/// The card's "Reauthenticate" button.
#[tauri::command]
#[specta::specta]
pub async fn reauthenticate(app: AppHandle) -> ApiStatus {
    on_blocking_thread(app, reauthenticate_now).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::secrets::{ACCESS_TOKEN, REFRESH_TOKEN, SecretStore};
    use crate::api::session::testing::test_api;
    use crate::api::stub::{Reply, Stub};
    use serde_json::json;

    const NOW: i64 = 1_800_000_000_000;
    const VALID: &str = "1800003600000";

    fn device_rows() -> Vec<(&'static str, &'static str)> {
        vec![
            ("api_device_id", r#""device-123""#),
            ("api_key_expires_at", VALID),
        ]
    }

    // 0.x CheckApiStatusTest: "sends device id and api key headers".
    #[test]
    fn sends_device_id_and_api_key_headers() {
        let stub = Stub::start(|_| Reply::json(200, r#"{"status":"ok"}"#));
        let api = test_api(&stub.base_url, &device_rows(), &[("api_key", "key-abc")]);
        assert_eq!(check_status(&api.state, NOW), ApiStatus::Ok);
        let seen = stub.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].method, "GET");
        assert_eq!(seen[0].target, "/api/status");
        assert_eq!(seen[0].header("x-device-id"), Some("device-123"));
        assert_eq!(seen[0].header("x-api-key"), Some("key-abc"));
    }

    // 0.x CheckApiStatusTest: "flags a noauth answer for a signed-in client
    // so the card offers sign-in, not device re-registration", end to end.
    #[test]
    fn a_signed_in_client_sends_its_bearer_and_is_flagged_signed_in() {
        let stub = Stub::start(|_| {
            Reply::json(
                200,
                r#"{"status":"noauth","message":"Your sign-in has expired. Please sign in again."}"#,
            )
        });
        // A token valid for a month: nothing to refresh first.
        let mut rows = device_rows();
        rows.push(("api_token_expires_at", "1802592000000"));
        let api = test_api(
            &stub.base_url,
            &rows,
            &[
                ("api_key", "key-abc"),
                (ACCESS_TOKEN, "access-token"),
                (REFRESH_TOKEN, "refresh-token"),
            ],
        );
        assert_eq!(
            check_status(&api.state, NOW),
            ApiStatus::Noauth {
                message: "Your sign-in has expired. Please sign in again.".to_string(),
                signed_in: true
            }
        );
        let seen = stub.requests();
        assert_eq!(seen[0].header("authorization"), Some("Bearer access-token"));
        assert_eq!(seen[0].header("x-api-key"), None);
    }

    // 0.x CheckApiStatusTest: "returns unreachable when connection fails".
    #[test]
    fn returns_unreachable_when_connection_fails() {
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.local_addr().expect("address").port()
        };
        let api = test_api(
            &format!("http://127.0.0.1:{port}"),
            &device_rows(),
            &[("api_key", "key-abc")],
        );
        let ApiStatus::Unreachable { error } = check_status(&api.state, NOW) else {
            panic!("unreachable");
        };
        assert!(error.starts_with("Connect: "), "{error}");
    }

    // 0.x CheckApiStatusTest: "returns unreachable for non-2xx response".
    #[test]
    fn returns_unreachable_for_non_2xx_response() {
        let stub = Stub::start(|_| Reply::Respond {
            status: 502,
            headers: Vec::new(),
            body: b"Bad gateway".to_vec(),
        });
        let api = test_api(&stub.base_url, &device_rows(), &[("api_key", "key-abc")]);
        assert_eq!(
            check_status(&api.state, NOW),
            ApiStatus::Unreachable {
                error: "HTTP 502: Bad gateway".to_string()
            }
        );
    }

    // 0.x DegradedRoutesTest: offline reports offline rather than pinging.
    #[test]
    fn offline_answers_offline_without_a_request() {
        let stub = Stub::start(|_| Reply::json(200, r#"{"status":"ok"}"#));
        let api = test_api(&stub.base_url, &device_rows(), &[("api_key", "key-abc")]);
        api.state.session().gate.offline = true;
        assert_eq!(check_status(&api.state, NOW), ApiStatus::Offline);
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(stub.connections(), 0);
    }

    // 0.x CheckApiStatusControllerTest: "returns status JSON from action".
    #[test]
    fn returns_status_json_from_action() {
        let stub = Stub::start(|_| Reply::json(200, r#"{"status":"ok"}"#));
        let api = test_api(&stub.base_url, &device_rows(), &[("api_key", "key-abc")]);
        let answer = serde_json::to_value(check_status(&api.state, NOW)).expect("json");
        assert_eq!(answer, json!({ "state": "ok" }));
    }

    // 0.x CheckApiStatusControllerTest: "passes through noauth result".
    #[test]
    fn passes_through_noauth_result() {
        let stub = Stub::start(|_| {
            Reply::json(
                200,
                r#"{"status":"noauth","message":"API key has expired. Please re-authenticate."}"#,
            )
        });
        let api = test_api(&stub.base_url, &device_rows(), &[("api_key", "key-abc")]);
        let answer = serde_json::to_value(check_status(&api.state, NOW)).expect("json");
        assert_eq!(
            answer,
            json!({
                "state": "noauth",
                "message": "API key has expired. Please re-authenticate.",
                "signedIn": false,
            })
        );
    }

    // 0.x ReauthenticateControllerTest: "registers device then returns fresh status".
    #[test]
    fn registers_device_then_returns_fresh_status() {
        let stub = Stub::start(|request| match request.target.as_str() {
            "/api/devices/register" => Reply::json(200, r#"{"api_key":"fresh-key"}"#),
            _ => Reply::json(200, r#"{"status":"ok"}"#),
        });
        let api = test_api(&stub.base_url, &[("api_device_id", r#""device-123""#)], &[]);
        let answer = serde_json::to_value(reauthenticate_now(&api.state, NOW)).expect("json");
        assert_eq!(answer, json!({ "state": "ok" }));
        assert_eq!(
            api.secrets.get("api_key").expect("get").as_deref(),
            Some("fresh-key")
        );
        let seen = stub.requests();
        assert_eq!(seen.len(), 2, "one registration, one status check");
        assert_eq!(seen[1].header("x-api-key"), Some("fresh-key"));
    }

    // 0.x ReauthenticateControllerTest: "returns unreachable when registration fails".
    #[test]
    fn returns_unreachable_when_registration_fails() {
        let stub = Stub::start(|_| Reply::json(500, r#"{"error":"down"}"#));
        let api = test_api(&stub.base_url, &[("api_device_id", r#""device-123""#)], &[]);
        let ApiStatus::Unreachable { error } = reauthenticate_now(&api.state, NOW) else {
            panic!("unreachable");
        };
        assert!(error.contains("Device registration failed"), "{error}");
        assert_eq!(error, REGISTRATION_FAILED);
        assert_eq!(
            stub.requests().len(),
            1,
            "no status check after a failed registration"
        );
    }
}
