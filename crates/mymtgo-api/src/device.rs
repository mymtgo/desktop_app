//! The device key (survey section 1): a UUID v4 device id minted at first
//! API use and an API key from `POST /api/devices/register`, assumed to live
//! 47 hours (0.x's client-side guess; the response's own expiry is not read).

use crate::gate::RequestClass;
use crate::request::{ApiRequest, ApiResponse, AuthMode, Body, Method};

/// How long a registered key is trusted (0.x `now()->addHours(47)`).
pub const KEY_LIFETIME_MS: i64 = 47 * 3_600_000;
pub const REGISTER_PATH: &str = "/api/devices/register";

/// `POST /api/devices/register` with `{"device_id": "<uuid>"}`: no auth,
/// sent even offline (`Registration`), 5 s total and 5 s connect.
pub fn register_device_request(device_id: &str) -> ApiRequest {
    let mut request = ApiRequest::new(
        Method::Post,
        REGISTER_PATH,
        RequestClass::Registration,
        AuthMode::None,
    );
    let body = serde_json::json!({ "device_id": device_id });
    request.body = Body::json(&body);
    request
}

/// The `api_key` of a successful registration, or `None` for a non-2xx
/// answer or one without a non-empty string key.
pub fn parse_register(response: &ApiResponse) -> Option<String> {
    if !response.is_success() {
        return None;
    }
    let body: serde_json::Value = response.json().ok()?;
    let key = body.get("api_key")?.as_str()?;
    if key.is_empty() {
        return None;
    }
    Some(key.to_string())
}

/// Whether a stored key must be registered again: no expiry is known, or it
/// has passed (0.x `ensureFresh`: fresh only while `now` is before it).
pub fn key_needs_refresh(expires_at: Option<i64>, now_ms: i64) -> bool {
    match expires_at {
        Some(expires_at) => now_ms >= expires_at,
        None => true,
    }
}

/// A UUID v4 from 16 random bytes: version nibble 4, variant bits `10`,
/// lowercase `8-4-4-4-12` (what Laravel's `Str::uuid()` produced).
pub fn uuid_v4(bytes: [u8; 16]) -> String {
    let mut bytes = bytes;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::response;

    #[test]
    fn the_register_request_matches_0x() {
        let request = register_device_request("qdevice-uuid");
        assert_eq!(request.method, Method::Post);
        assert_eq!(request.path, "/api/devices/register");
        assert_eq!(request.class, RequestClass::Registration);
        assert_eq!(request.auth, AuthMode::None);
        assert_eq!(
            (request.timeout_ms, request.connect_timeout_ms),
            (5_000, 5_000)
        );
        assert_eq!(
            request.body,
            Body::Json(br#"{"device_id":"qdevice-uuid"}"#.to_vec())
        );
        assert!(request.query.is_empty() && request.headers.is_empty());
    }

    #[test]
    fn a_registration_answer_gives_its_key() {
        assert_eq!(
            parse_register(&response(200, r#"{"api_key":"qfresh-key"}"#)),
            Some("qfresh-key".to_string())
        );
        assert_eq!(
            parse_register(&response(201, r#"{"api_key":"q2"}"#)),
            Some("q2".to_string())
        );
        for (status, body) in [
            (500, r#"{"api_key":"qkey"}"#),
            (200, r#"{"error":"down"}"#),
            (200, r#"{"api_key":""}"#),
            (200, r#"{"api_key":12}"#),
            (200, "not json"),
        ] {
            assert_eq!(
                parse_register(&response(status, body)),
                None,
                "{status} {body}"
            );
        }
    }

    #[test]
    fn a_key_is_fresh_until_its_expiry() {
        assert!(key_needs_refresh(None, 0));
        assert!(!key_needs_refresh(Some(1_000), 999));
        assert!(key_needs_refresh(Some(1_000), 1_000));
        assert!(key_needs_refresh(Some(1_000), 5_000));
        assert_eq!(KEY_LIFETIME_MS, 169_200_000);
    }

    #[test]
    fn uuid_v4_sets_version_and_variant_bits() {
        assert_eq!(uuid_v4([0; 16]), "00000000-0000-4000-8000-000000000000");
        assert_eq!(uuid_v4([0xff; 16]), "ffffffff-ffff-4fff-bfff-ffffffffffff");
        let bytes: [u8; 16] = std::array::from_fn(|index| (index as u8).wrapping_mul(37));
        let id = uuid_v4(bytes);
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(
            parts.iter().map(|part| part.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert!(
            id.chars()
                .all(|c| c == '-' || c.is_ascii_digit() || ('a'..='f').contains(&c))
        );
        assert_eq!(&parts[2][0..1], "4");
        assert!(["8", "9", "a", "b"].contains(&&parts[3][0..1]), "{id}");
    }
}
