//! Sharing a replay and switching its link off (replays spec section 8;
//! 0.x `SyncApi::shareReplay` and `revokeReplay`).
//!
//! The share request is only ever built from a `replay::PreparedSnapshot`:
//! redacted for every opponent name and validated. There is no size check
//! here: the server's limit is the server's, and its 413 decides (owner,
//! 2026-10-09).

use replay::{PreparedSnapshot, ShareFailure};
use serde::Deserialize;
use serde_json::Value;

use crate::error::ApiError;
use crate::gate::RequestClass;
use crate::request::{ApiRequest, AuthMode, Body, DEFAULT_TIMEOUT_MS, Method, gzip};

/// The 422 `error` of a free account.
const SUPPORTER_ERROR: &str = "replay_share_requires_supporter";

/// A shared replay's link, as the server answers a share.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SharedReplay {
    pub uuid: String,
    pub url: String,
}

/// `POST /api/replays`: gzip (level 6) of `{"client_match_key",
/// "login_id", "snapshot"}`, `Content-Encoding: gzip`, class `Sync`, bearer
/// auth, 30 s. The snapshot must be a prepared one; a raw snapshot does not
/// compile:
///
/// ```compile_fail
/// // the_request_builder_only_takes_a_prepared_snapshot
/// fn upload(raw: &replay::Snapshot) -> mymtgo_api::ApiRequest {
///     mymtgo_api::share_replay_request("q-match", 1, raw)
/// }
/// ```
pub fn share_replay_request(
    match_key: &str,
    login_id: i64,
    prepared: &PreparedSnapshot,
) -> ApiRequest {
    // The prepared JSON goes in as its bytes, so what was redacted and
    // validated is exactly what is sent. A `String` always writes as JSON.
    let key = serde_json::to_string(match_key).unwrap_or_default();
    let mut json =
        format!(r#"{{"client_match_key":{key},"login_id":{login_id},"snapshot":"#).into_bytes();
    json.extend_from_slice(prepared.json());
    json.push(b'}');

    let mut request = ApiRequest::new(
        Method::Post,
        "/api/replays",
        RequestClass::Sync,
        AuthMode::Bearer,
    );
    request.body = Body::GzipJson(gzip(&json));
    request.timeout_ms = DEFAULT_TIMEOUT_MS;
    request
}

/// `DELETE /api/replays/{uuid}`: class `Sync`, bearer auth, 30 s.
pub fn revoke_replay_request(uuid: &str) -> ApiRequest {
    let mut request = ApiRequest::new(
        Method::Delete,
        &format!("/api/replays/{uuid}"),
        RequestClass::Sync,
        AuthMode::Bearer,
    );
    request.timeout_ms = DEFAULT_TIMEOUT_MS;
    request
}

/// A share's 200 or 201 answer, `{uuid, url}`. A missing or empty uuid or
/// url is `Malformed` (0.x stored them as empty strings; v1 never stores a
/// link it cannot show).
pub fn parse_share_response(body: &[u8]) -> Result<SharedReplay, ApiError> {
    let shared: SharedReplay = serde_json::from_slice(body).map_err(|error| {
        ApiError::Malformed(format!(
            "{:?} error in the share answer at line {} column {}",
            error.classify(),
            error.line(),
            error.column()
        ))
    })?;
    if shared.uuid.is_empty() || shared.url.is_empty() {
        return Err(ApiError::Malformed(
            "the share answer has no uuid or url".to_string(),
        ));
    }
    Ok(shared)
}

/// Why a share answered with a failed status (0.x `shareReplay`'s match):
/// 422 `replay_share_requires_supporter` is `Supporter`; any 409
/// `NotClaimed`; any 413 `TooLarge` (the server sends
/// `{"error":"replay_too_large"}`, a proxy a bare page); any other 422
/// `Invalid`; anything else `Server` (429 included).
pub fn share_status(status: u16, body: &[u8]) -> ShareFailure {
    match status {
        422 => {
            let answer: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
            if answer["error"].as_str() == Some(SUPPORTER_ERROR) {
                ShareFailure::Supporter
            } else {
                ShareFailure::Invalid
            }
        }
        409 => ShareFailure::NotClaimed,
        413 => ShareFailure::TooLarge,
        _ => ShareFailure::Server,
    }
}

/// Whether a switch off answer means the link is off: 2xx, or 404 (already
/// gone, which is the goal).
pub fn revoke_ok(status: u16) -> bool {
    (200..300).contains(&status) || status == 404
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::{Credentials, wire_headers};
    use crate::test_support::gunzip;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use replay::{Snapshot, prepare_shared_snapshot};
    use std::io::Write;

    /// The package's API fixture, prepared with its opponent `Opp_Name`.
    fn prepared() -> PreparedSnapshot {
        let snapshot: Snapshot = serde_json::from_str(include_str!(
            "../../replay/tests/fixtures/api_snapshot.json"
        ))
        .expect("the fixture is a snapshot");
        prepare_shared_snapshot(snapshot, &["Opp_Name".to_string()]).expect("prepared")
    }

    fn header(list: &[(String, String)], name: &str) -> Option<String> {
        list.iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    }

    #[test]
    fn the_share_request_is_a_gzip_post_with_the_match_key_and_login_id() {
        let prepared = prepared();
        let request =
            share_replay_request("a1b2c3d4-0000-4000-8000-000000000001", 3_000_001, &prepared);
        assert_eq!(request.method, Method::Post);
        assert_eq!(request.path, "/api/replays");
        assert!(request.query.is_empty());
        assert_eq!(request.class, RequestClass::Sync);
        assert_eq!(request.auth, AuthMode::Bearer);
        assert_eq!(request.timeout_ms, 30_000);
        let Body::GzipJson(bytes) = &request.body else {
            panic!("{:?}", request.body);
        };

        let mut expected = br#"{"client_match_key":"a1b2c3d4-0000-4000-8000-000000000001","login_id":3000001,"snapshot":"#.to_vec();
        expected.extend_from_slice(prepared.json());
        expected.push(b'}');
        assert_eq!(gunzip(bytes), expected);

        // Level 6, as 0.x `gzencode($json, 6)`.
        let mut level_six = GzEncoder::new(Vec::new(), Compression::new(6));
        level_six.write_all(&expected).expect("write");
        assert_eq!(bytes, &level_six.finish().expect("gzip"));

        let credentials = Credentials {
            device_id: "qdevice".to_string(),
            api_key: None,
            access_token: Some("qaccess".to_string()),
        };
        let headers = wire_headers(&request, &credentials).expect("headers");
        assert_eq!(
            header(&headers, "Accept").as_deref(),
            Some("application/json")
        );
        assert_eq!(
            header(&headers, "Content-Type").as_deref(),
            Some("application/json")
        );
        assert_eq!(
            header(&headers, "Content-Encoding").as_deref(),
            Some("gzip")
        );
        assert_eq!(
            header(&headers, "Authorization").as_deref(),
            Some("Bearer qaccess")
        );
        assert_eq!(header(&headers, "X-Api-Key"), None);
    }

    #[test]
    fn the_shared_body_holds_the_redacted_snapshot() {
        let request = share_replay_request("q-match", 7, &prepared());
        let body: Value = serde_json::from_slice(&gunzip(request.body.bytes())).expect("json");
        assert_eq!(body["client_match_key"], "q-match");
        assert_eq!(body["login_id"], 7);
        assert_eq!(
            body["snapshot"]["games"][0]["frames"][0]["content"]["Players"][1]["Name"],
            "Opponent"
        );
        assert!(!body.to_string().contains("Opp_Name"));
    }

    #[test]
    fn the_revoke_request_deletes_the_link() {
        let request = revoke_replay_request("5f0c6a8e-1111-4000-8000-000000000001");
        assert_eq!(request.method, Method::Delete);
        assert_eq!(
            request.path,
            "/api/replays/5f0c6a8e-1111-4000-8000-000000000001"
        );
        assert_eq!(request.body, Body::Empty);
        assert_eq!(request.class, RequestClass::Sync);
        assert_eq!(request.auth, AuthMode::Bearer);
        assert_eq!(request.timeout_ms, 30_000);
    }

    #[test]
    fn a_share_answer_gives_the_uuid_and_url() {
        let shared = parse_share_response(
            br#"{"message":"Replay shared.","uuid":"5f0c6a8e-1111-4000-8000-000000000001","url":"https://mymtgo.test/players/Qplayer/replays/5f0c6a8e-1111-4000-8000-000000000001"}"#,
        )
        .expect("shared");
        assert_eq!(
            shared,
            SharedReplay {
                uuid: "5f0c6a8e-1111-4000-8000-000000000001".to_string(),
                url: "https://mymtgo.test/players/Qplayer/replays/5f0c6a8e-1111-4000-8000-000000000001"
                    .to_string(),
            }
        );
        for broken in [
            &b"{}"[..],
            br#"{"uuid":"q"}"#,
            br#"{"uuid":"","url":"q"}"#,
            b"<html>",
        ] {
            assert!(
                matches!(parse_share_response(broken), Err(ApiError::Malformed(_))),
                "{}",
                String::from_utf8_lossy(broken)
            );
        }
    }

    #[test]
    fn share_statuses_follow_0x() {
        let cases: [(u16, &str, ShareFailure); 10] = [
            (
                422,
                r#"{"error":"replay_share_requires_supporter"}"#,
                ShareFailure::Supporter,
            ),
            (422, r#"{"error":"something_else"}"#, ShareFailure::Invalid),
            (
                422,
                r#"{"message":"The snapshot is invalid."}"#,
                ShareFailure::Invalid,
            ),
            (
                409,
                r#"{"error":"player_not_claimed"}"#,
                ShareFailure::NotClaimed,
            ),
            (409, "", ShareFailure::NotClaimed),
            (
                413,
                r#"{"error":"replay_too_large"}"#,
                ShareFailure::TooLarge,
            ),
            // A proxy's bare 413 maps the same, as 0.x.
            (
                413,
                "<html>Request Entity Too Large</html>",
                ShareFailure::TooLarge,
            ),
            (
                429,
                r#"{"message":"Too Many Attempts."}"#,
                ShareFailure::Server,
            ),
            (500, "", ShareFailure::Server),
            (
                403,
                r#"{"error":"replay_share_requires_supporter"}"#,
                ShareFailure::Server,
            ),
        ];
        for (status, body, expected) in cases {
            assert_eq!(
                share_status(status, body.as_bytes()),
                expected,
                "{status} {body}"
            );
        }
    }

    #[test]
    fn a_revoke_is_done_on_any_2xx_or_a_404() {
        for status in [200, 202, 204] {
            assert!(revoke_ok(status), "{status}");
        }
        assert!(revoke_ok(404));
        for status in [400, 401, 403, 409, 410, 429, 500, 503] {
            assert!(!revoke_ok(status), "{status}");
        }
    }
}
