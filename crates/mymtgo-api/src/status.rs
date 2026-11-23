//! `GET /api/status` and what the API Status card shows (survey section 4,
//! 0.x `CheckApiStatus`).

use serde::Serialize;

use crate::error::{ApiError, STATUS_BODY_CHARS};
use crate::gate::RequestClass;
use crate::request::{ApiRequest, ApiResponse, AuthMode, Method};

pub const STATUS_PATH: &str = "/api/status";
/// The noauth message when the API sends none.
pub const DEFAULT_NOAUTH_MESSAGE: &str = "Authentication required.";

/// The API's state as the card shows it, serialised as 0.x's JSON
/// (`{"state":"noauth","message":"..","signedIn":false}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum ApiStatus {
    Ok,
    /// Offline mode is on: nothing was asked.
    Offline,
    /// The API refused the credentials. `signed_in` says the client sent its
    /// account token, so signing in again (not re-registering) is the fix.
    Noauth {
        message: String,
        #[serde(rename = "signedIn")]
        signed_in: bool,
    },
    Unreachable {
        error: String,
    },
}

/// `GET /api/status`: blocked offline (`Status`), device key or bearer,
/// 5 s total and 5 s connect.
pub fn status_request() -> ApiRequest {
    ApiRequest::new(
        Method::Get,
        STATUS_PATH,
        RequestClass::Status,
        AuthMode::DeviceOrBearer,
    )
}

/// The card's state from the call's outcome. `linked`: the client is signed
/// in (it sent a bearer).
pub fn parse_status(result: Result<ApiResponse, ApiError>, linked: bool) -> ApiStatus {
    let response = match result {
        Ok(response) => response,
        Err(ApiError::Offline) => return ApiStatus::Offline,
        Err(error) => {
            return ApiStatus::Unreachable {
                error: format!("{}: {error}", error.kind()),
            };
        }
    };
    if !response.is_success() {
        return ApiStatus::Unreachable {
            error: response.status_error().to_string(),
        };
    }
    let payload: Option<serde_json::Value> = response.json().ok();
    let field = |name: &str| -> Option<String> {
        payload
            .as_ref()
            .and_then(|payload| payload.get(name))
            .and_then(|value| value.as_str())
            .map(str::to_string)
    };
    match field("status").as_deref() {
        Some("ok") => ApiStatus::Ok,
        Some("noauth") => ApiStatus::Noauth {
            message: field("message").unwrap_or_else(|| DEFAULT_NOAUTH_MESSAGE.to_string()),
            signed_in: linked,
        },
        _ => {
            let body: String = String::from_utf8_lossy(&response.body)
                .chars()
                .take(STATUS_BODY_CHARS)
                .collect();
            ApiStatus::Unreachable {
                error: format!("Unexpected response: {body}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(status: u16, body: &str) -> Result<ApiResponse, ApiError> {
        Ok(ApiResponse {
            status,
            headers: Vec::new(),
            body: body.as_bytes().to_vec(),
        })
    }

    #[test]
    fn the_status_request_matches_0x() {
        let request = status_request();
        assert_eq!(request.method, Method::Get);
        assert_eq!(request.path, "/api/status");
        assert_eq!(request.class, RequestClass::Status);
        assert_eq!(request.auth, AuthMode::DeviceOrBearer);
        assert_eq!(
            (request.timeout_ms, request.connect_timeout_ms),
            (5_000, 5_000)
        );
    }

    // 0.x CheckApiStatusTest: "returns ok when API responds with status ok".
    #[test]
    fn returns_ok_when_api_responds_with_status_ok() {
        assert_eq!(
            parse_status(answer(200, r#"{"status":"ok"}"#), false),
            ApiStatus::Ok
        );
    }

    // 0.x CheckApiStatusTest: "returns noauth with message when API responds
    // with noauth".
    #[test]
    fn returns_noauth_with_message_when_api_responds_with_noauth() {
        let body = r#"{"status":"noauth","message":"Device not recognized."}"#;
        assert_eq!(
            parse_status(answer(200, body), false),
            ApiStatus::Noauth {
                message: "Device not recognized.".to_string(),
                signed_in: false
            }
        );
        assert_eq!(
            parse_status(answer(200, r#"{"status":"noauth"}"#), false),
            ApiStatus::Noauth {
                message: "Authentication required.".to_string(),
                signed_in: false
            }
        );
    }

    // 0.x CheckApiStatusTest: "flags a noauth answer for a signed-in client so
    // the card offers sign-in, not device re-registration".
    #[test]
    fn flags_a_noauth_answer_for_a_signed_in_client() {
        let body =
            r#"{"status":"noauth","message":"Your sign-in has expired. Please sign in again."}"#;
        assert_eq!(
            parse_status(answer(200, body), true),
            ApiStatus::Noauth {
                message: "Your sign-in has expired. Please sign in again.".to_string(),
                signed_in: true
            }
        );
    }

    // 0.x CheckApiStatusTest: "returns unreachable when connection fails".
    #[test]
    fn returns_unreachable_when_connection_fails() {
        let error = ApiError::Connect(
            crate::ConnectKind::Refused,
            "io: Connection refused (os error 61)".to_string(),
        );
        let ApiStatus::Unreachable { error } = parse_status(Err(error), false) else {
            panic!("unreachable");
        };
        assert_eq!(
            error,
            "Connect: connection failed (refused): io: Connection refused (os error 61)"
        );
        assert_eq!(
            parse_status(Err(ApiError::Timeout), false),
            ApiStatus::Unreachable {
                error: "Timeout: the request timed out".to_string()
            }
        );
    }

    // 0.x CheckApiStatusTest: "returns unreachable for non-2xx response".
    #[test]
    fn returns_unreachable_for_non_2xx_response() {
        assert_eq!(
            parse_status(answer(502, "Bad gateway"), false),
            ApiStatus::Unreachable {
                error: "HTTP 502: Bad gateway".to_string()
            }
        );
    }

    #[test]
    fn anything_else_is_an_unexpected_response_and_offline_is_offline() {
        assert_eq!(
            parse_status(answer(200, r#"{"status":"maintenance"}"#), false),
            ApiStatus::Unreachable {
                error: r#"Unexpected response: {"status":"maintenance"}"#.to_string()
            }
        );
        assert_eq!(
            parse_status(answer(200, "<html>"), false),
            ApiStatus::Unreachable {
                error: "Unexpected response: <html>".to_string()
            }
        );
        assert_eq!(
            parse_status(Err(ApiError::Offline), true),
            ApiStatus::Offline
        );
    }

    #[test]
    fn the_status_serialises_as_0x_json() {
        let json = |status: &ApiStatus| serde_json::to_string(status).expect("json");
        assert_eq!(json(&ApiStatus::Ok), r#"{"state":"ok"}"#);
        assert_eq!(json(&ApiStatus::Offline), r#"{"state":"offline"}"#);
        assert_eq!(
            json(&ApiStatus::Noauth {
                message: "API key has expired. Please re-authenticate.".to_string(),
                signed_in: false
            }),
            r#"{"state":"noauth","message":"API key has expired. Please re-authenticate.","signedIn":false}"#
        );
        assert_eq!(
            json(&ApiStatus::Unreachable {
                error: "x".to_string()
            }),
            r#"{"state":"unreachable","error":"x"}"#
        );
    }
}
