//! A request as data: what to send, how it is gated and authenticated, and
//! its limits. The app's client turns it into bytes on a socket; every
//! builder in this crate returns one, so tests check method, path, headers
//! and body without a network.

use std::fmt;
use std::io::Write;

use flate2::Compression;
use flate2::write::GzEncoder;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::ApiError;
use crate::gate::RequestClass;
use crate::php::http_build_query;

pub const KIB: u64 = 1024;
pub const MIB: u64 = 1024 * KIB;
/// The body cap of most answers.
pub const DEFAULT_BODY_LIMIT: u64 = MIB;
/// Card and archetype answers can be large.
pub const REFERENCE_BODY_LIMIT: u64 = 16 * MIB;
/// `/api/sync/blobs/fetch` returns up to 20 bundles at once.
pub const BLOB_FETCH_BODY_LIMIT: u64 = 64 * MIB;

/// 0.x: registration, status and tournament metadata 5 s total, 5 s connect.
pub const SHORT_TIMEOUT_MS: u64 = 5_000;
/// 0.x: card lookups 15 s.
pub const CARD_TIMEOUT_MS: u64 = 15_000;
/// 0.x: shipping and sync 30 s; Laravel's default for everything else.
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
/// Connect timeout unless a call states one.
pub const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 10_000;

/// The gzip level of every compressed body: 0.x `gzencode` at its default
/// (observations, card stats) or with an explicit 6 (replays, sync blobs).
pub const GZIP_LEVEL: u32 = 6;

/// Headers whose values never appear in `Debug` output.
const SECRET_HEADERS: [&str; 2] = ["authorization", "x-api-key"];
/// Query keys whose values never appear in `Debug` output.
const SECRET_QUERY_PARTS: [&str; 6] = ["token", "key", "secret", "code", "verifier", "password"];
const REDACTED: &str = "<redacted>";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
        }
    }
}

/// The request body, already encoded. The variant decides the content headers.
#[derive(Clone, PartialEq, Eq)]
pub enum Body {
    Empty,
    /// `application/json`.
    Json(Vec<u8>),
    /// `application/x-www-form-urlencoded` (built with `http_build_query`).
    Form(Vec<u8>),
    /// JSON compressed with gzip: `application/json` plus
    /// `Content-Encoding: gzip`.
    GzipJson(Vec<u8>),
    /// `multipart/form-data`, built by hand around `boundary`.
    Multipart {
        boundary: String,
        bytes: Vec<u8>,
    },
}

impl Body {
    /// `value` as compact JSON (empty if it cannot be written as JSON).
    pub fn json<T: Serialize + ?Sized>(value: &T) -> Body {
        Body::Json(serde_json::to_vec(value).unwrap_or_default())
    }

    /// The bytes on the wire.
    pub fn bytes(&self) -> &[u8] {
        match self {
            Body::Empty => &[],
            Body::Json(bytes) | Body::Form(bytes) | Body::GzipJson(bytes) => bytes,
            Body::Multipart { bytes, .. } => bytes,
        }
    }

    /// `Content-Type` and, for gzip, `Content-Encoding`.
    pub fn content_headers(&self) -> Vec<(String, String)> {
        let content_type = |value: &str| ("Content-Type".to_string(), value.to_string());
        match self {
            Body::Empty => Vec::new(),
            Body::Json(_) => vec![content_type("application/json")],
            Body::Form(_) => vec![content_type("application/x-www-form-urlencoded")],
            Body::GzipJson(_) => vec![
                content_type("application/json"),
                ("Content-Encoding".to_string(), "gzip".to_string()),
            ],
            Body::Multipart { boundary, .. } => vec![content_type(&format!(
                "multipart/form-data; boundary={boundary}"
            ))],
        }
    }
}

/// Only the kind and size: bodies carry codes, verifiers and tokens.
impl fmt::Debug for Body {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let len = self.bytes().len();
        match self {
            Body::Empty => write!(f, "Empty"),
            Body::Json(_) => write!(f, "Json({len} bytes)"),
            Body::Form(_) => write!(f, "Form({len} bytes)"),
            Body::GzipJson(_) => write!(f, "GzipJson({len} bytes)"),
            Body::Multipart { boundary, .. } => {
                write!(f, "Multipart {{ boundary: {boundary:?}, {len} bytes }}")
            }
        }
    }
}

/// `bytes` compressed as 0.x's `gzencode` at `GZIP_LEVEL`.
pub fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::new(GZIP_LEVEL));
    // Writing to a Vec cannot fail.
    let _ = encoder.write_all(bytes);
    encoder.finish().unwrap_or_default()
}

/// Add one multipart file part to `body`: field `name`, file `file_name`
/// of type `content_type`, then `contents` and its closing line break.
pub fn file_part(
    body: &mut Vec<u8>,
    boundary: &str,
    name: &str,
    file_name: &str,
    content_type: &str,
    contents: &[u8],
) {
    let head = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{file_name}\"\r\nContent-Type: {content_type}\r\n\r\n"
    );
    body.extend_from_slice(head.as_bytes());
    body.extend_from_slice(contents);
    body.extend_from_slice(b"\r\n");
}

/// How a request authenticates (0.x's macros).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    /// No auth headers (registration, `/oauth/token`).
    None,
    /// The bearer when signed in, else the device id and key
    /// (`mymtgoReference`, `mymtgoApi`).
    DeviceOrBearer,
    /// The bearer, or `NotLinked` (`mymtgoSync()->withToken`).
    Bearer,
    /// The device id and key only, even when signed in (the offline
    /// archetype download: no token refresh offline, no account named).
    Device,
}

/// What auth headers are made from. `Debug` shows only the device id.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Credentials {
    pub device_id: String,
    pub api_key: Option<String>,
    pub access_token: Option<String>,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let shown = |value: &Option<String>| value.as_ref().map(|_| REDACTED);
        f.debug_struct("Credentials")
            .field("device_id", &self.device_id)
            .field("api_key", &shown(&self.api_key))
            .field("access_token", &shown(&self.access_token))
            .finish()
    }
}

/// One request. `path` starts with `/` and is joined to the base URL;
/// `query` is encoded with PHP's `http_build_query`; `headers` are extra
/// headers (the client adds `Accept`, the content headers and auth).
#[derive(Clone, PartialEq, Eq)]
pub struct ApiRequest {
    pub method: Method,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: Body,
    pub class: RequestClass,
    pub auth: AuthMode,
    pub timeout_ms: u64,
    pub connect_timeout_ms: u64,
    pub body_limit: u64,
}

impl ApiRequest {
    /// A request with no query, headers or body, and the default timeouts
    /// for its class and body limit for its path.
    pub fn new(method: Method, path: &str, class: RequestClass, auth: AuthMode) -> ApiRequest {
        let (timeout_ms, connect_timeout_ms) = default_timeouts(class);
        ApiRequest {
            method,
            path: path.to_string(),
            query: Vec::new(),
            headers: Vec::new(),
            body: Body::Empty,
            class,
            auth,
            timeout_ms,
            connect_timeout_ms,
            body_limit: body_limit_for(path),
        }
    }
}

/// The timeouts (total, connect) a class gets unless its builder says
/// otherwise (tournament metadata is `Community` but 5 s, set by its builder).
pub fn default_timeouts(class: RequestClass) -> (u64, u64) {
    match class {
        RequestClass::Registration | RequestClass::Status => (SHORT_TIMEOUT_MS, SHORT_TIMEOUT_MS),
        RequestClass::Reference => (CARD_TIMEOUT_MS, DEFAULT_CONNECT_TIMEOUT_MS),
        RequestClass::Lookup
        | RequestClass::Community
        | RequestClass::Sync
        | RequestClass::Token
        | RequestClass::OfflineDownload => (DEFAULT_TIMEOUT_MS, DEFAULT_CONNECT_TIMEOUT_MS),
    }
}

/// The body cap for a path: 16 MiB for `/api/cards`, `/api/cards/resolve`
/// and `/api/archetypes*`, 64 MiB for `/api/sync/blobs/fetch`, else 1 MiB.
pub fn body_limit_for(path: &str) -> u64 {
    if path == "/api/sync/blobs/fetch" {
        BLOB_FETCH_BODY_LIMIT
    } else if path == "/api/cards"
        || path == "/api/cards/resolve"
        || path == "/api/archetypes"
        || path.starts_with("/api/archetypes/")
    {
        REFERENCE_BODY_LIMIT
    } else {
        DEFAULT_BODY_LIMIT
    }
}

/// Secret headers and query values are redacted; the body shows only its
/// kind and size.
impl fmt::Debug for ApiRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let headers: Vec<(String, String)> = self
            .headers
            .iter()
            .map(|(name, value)| {
                let secret = SECRET_HEADERS.contains(&name.to_ascii_lowercase().as_str());
                let shown = if secret { REDACTED } else { value.as_str() };
                (name.clone(), shown.to_string())
            })
            .collect();
        let query: Vec<(String, String)> = self
            .query
            .iter()
            .map(|(key, value)| {
                let lower = key.to_ascii_lowercase();
                let secret = SECRET_QUERY_PARTS.iter().any(|part| lower.contains(part));
                let shown = if secret { REDACTED } else { value.as_str() };
                (key.clone(), shown.to_string())
            })
            .collect();
        f.debug_struct("ApiRequest")
            .field("method", &self.method)
            .field("path", &self.path)
            .field("query", &query)
            .field("headers", &headers)
            .field("body", &self.body)
            .field("class", &self.class)
            .field("auth", &self.auth)
            .field("timeout_ms", &self.timeout_ms)
            .field("connect_timeout_ms", &self.connect_timeout_ms)
            .field("body_limit", &self.body_limit)
            .finish()
    }
}

/// The auth headers for a request (0.x `ApiMacroAuthTest`): signed in gives
/// only `Authorization: Bearer <access>`; signed out, `DeviceOrBearer` gives
/// `X-Device-Id` and `X-Api-Key`, and `Bearer` is `NotLinked`. A signed-out
/// device request without a key is a `Config` error: the caller registers
/// first (Q2's `ensure_device_key`).
pub fn auth_headers(
    mode: AuthMode,
    credentials: &Credentials,
) -> Result<Vec<(String, String)>, ApiError> {
    let bearer = |token: &str| vec![("Authorization".to_string(), format!("Bearer {token}"))];
    match mode {
        AuthMode::None => Ok(Vec::new()),
        AuthMode::Bearer => match &credentials.access_token {
            Some(token) => Ok(bearer(token)),
            None => Err(ApiError::NotLinked),
        },
        AuthMode::DeviceOrBearer => {
            if let Some(token) = &credentials.access_token {
                return Ok(bearer(token));
            }
            device_headers(credentials)
        }
        AuthMode::Device => device_headers(credentials),
    }
}

/// The device id and key headers, or `Config` before the device has a key.
fn device_headers(credentials: &Credentials) -> Result<Vec<(String, String)>, ApiError> {
    let Some(key) = &credentials.api_key else {
        return Err(ApiError::Config(
            "the device has no API key yet".to_string(),
        ));
    };
    Ok(vec![
        ("X-Device-Id".to_string(), credentials.device_id.clone()),
        ("X-Api-Key".to_string(), key.clone()),
    ])
}

/// Every header the request goes out with, in order: `Accept:
/// application/json`, the body's content headers, the request's own headers,
/// then auth.
pub fn wire_headers(
    request: &ApiRequest,
    credentials: &Credentials,
) -> Result<Vec<(String, String)>, ApiError> {
    let mut headers = vec![("Accept".to_string(), "application/json".to_string())];
    headers.extend(request.body.content_headers());
    headers.extend(request.headers.iter().cloned());
    headers.extend(auth_headers(request.auth, credentials)?);
    Ok(headers)
}

/// The full URL: base, path, then `?` and the PHP-encoded query when there
/// is one.
pub fn request_url(base_url: &str, request: &ApiRequest) -> String {
    let mut url = format!("{base_url}{}", request.path);
    if !request.query.is_empty() {
        url.push('?');
        url.push_str(&http_build_query(&request.query));
    }
    url
}

/// An answer, whatever its status: a non-2xx is a response, not an error.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiResponse {
    pub status: u16,
    /// Names lowercase, in the order received.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl ApiResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The first header with this name (any case).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The body as JSON of type `T`, or `Malformed` naming where it broke
    /// (never quoting the body, which may hold tokens).
    pub fn json<T: DeserializeOwned>(&self) -> Result<T, ApiError> {
        serde_json::from_slice(&self.body).map_err(|error| {
            ApiError::Malformed(format!(
                "{:?} error in the JSON answer at line {} column {}",
                error.classify(),
                error.line(),
                error.column()
            ))
        })
    }

    /// This response as `ApiError::Status` (the body cut to 500 characters).
    pub fn status_error(&self) -> ApiError {
        ApiError::status(self.status, &self.body)
    }
}

/// Response headers whose values never appear in `Debug` output.
const SECRET_RESPONSE_HEADERS: [&str; 2] = ["set-cookie", "authorization"];

/// Only the status, headers (cookies and auth redacted) and body size: token
/// answers carry tokens.
impl fmt::Debug for ApiResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let headers: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(name, value)| {
                let lower = name.to_ascii_lowercase();
                let secret = SECRET_RESPONSE_HEADERS.contains(&lower.as_str());
                (
                    name.as_str(),
                    if secret { REDACTED } else { value.as_str() },
                )
            })
            .collect();
        f.debug_struct("ApiResponse")
            .field("status", &self.status)
            .field("headers", &headers)
            .field("body", &format_args!("{} bytes", self.body.len()))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The offline archetype download never sends the bearer: the token
    // refresh is blocked offline, and the request names no account.
    #[test]
    fn device_auth_never_sends_the_bearer() {
        let mut credentials = Credentials {
            device_id: "qdevice".to_string(),
            api_key: Some("key".to_string()),
            access_token: Some("tok".to_string()),
        };
        assert_eq!(
            auth_headers(AuthMode::Device, &credentials).expect("headers"),
            headers(&[("X-Device-Id", "qdevice"), ("X-Api-Key", "key")])
        );
        credentials.api_key = None;
        assert!(matches!(
            auth_headers(AuthMode::Device, &credentials),
            Err(ApiError::Config(_))
        ));
    }

    fn headers(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    fn device_credentials() -> Credentials {
        Credentials {
            device_id: "test-device".to_string(),
            api_key: Some("test-key".to_string()),
            access_token: None,
        }
    }

    fn linked_credentials() -> Credentials {
        Credentials {
            access_token: Some("access-token".to_string()),
            ..device_credentials()
        }
    }

    fn has(list: &[(String, String)], name: &str) -> bool {
        list.iter().any(|(key, _)| key.eq_ignore_ascii_case(name))
    }

    // 0.x ApiMacroAuthTest: "sends the bearer and no device headers once linked".
    #[test]
    fn sends_the_bearer_and_no_device_headers_once_linked() {
        let sent = auth_headers(AuthMode::DeviceOrBearer, &linked_credentials()).expect("ok");
        assert_eq!(sent, headers(&[("Authorization", "Bearer access-token")]));
    }

    // 0.x ApiMacroAuthTest: "does not touch the device key while linked, even
    // when it has expired". Credentials carry no expiry: whatever the key's
    // age, a linked request never reads it.
    #[test]
    fn does_not_touch_the_device_key_while_linked_even_when_it_has_expired() {
        let expired = Credentials {
            api_key: Some("expired-key".to_string()),
            ..linked_credentials()
        };
        let without_key = Credentials {
            api_key: None,
            ..linked_credentials()
        };
        for credentials in [expired, without_key] {
            let sent = auth_headers(AuthMode::DeviceOrBearer, &credentials).expect("ok");
            assert!(!has(&sent, "X-Api-Key"), "{sent:?}");
            assert!(!has(&sent, "X-Device-Id"), "{sent:?}");
            assert_eq!(sent, headers(&[("Authorization", "Bearer access-token")]));
        }
    }

    // 0.x ApiMacroAuthTest: "sends device headers as before when not linked".
    #[test]
    fn sends_device_headers_as_before_when_not_linked() {
        let sent = auth_headers(AuthMode::DeviceOrBearer, &device_credentials()).expect("ok");
        assert_eq!(
            sent,
            headers(&[("X-Device-Id", "test-device"), ("X-Api-Key", "test-key")])
        );
        assert!(!has(&sent, "Authorization"));
    }

    #[test]
    fn bearer_mode_needs_a_session_and_none_sends_nothing() {
        assert_eq!(
            auth_headers(AuthMode::Bearer, &device_credentials()),
            Err(ApiError::NotLinked)
        );
        assert_eq!(
            auth_headers(AuthMode::Bearer, &linked_credentials()).expect("ok"),
            headers(&[("Authorization", "Bearer access-token")])
        );
        assert_eq!(
            auth_headers(AuthMode::None, &linked_credentials()).expect("ok"),
            Vec::new()
        );
        let no_key = Credentials {
            api_key: None,
            ..device_credentials()
        };
        assert!(matches!(
            auth_headers(AuthMode::DeviceOrBearer, &no_key),
            Err(ApiError::Config(_))
        ));
    }

    #[test]
    fn every_request_accepts_json_and_carries_its_content_headers() {
        let mut request = ApiRequest::new(
            Method::Post,
            "/api/tournaments/observations",
            RequestClass::Community,
            AuthMode::DeviceOrBearer,
        );
        request.body = Body::GzipJson(vec![1, 2, 3]);
        request.headers = headers(&[("X-Extra", "1")]);
        let sent = wire_headers(&request, &device_credentials()).expect("ok");
        assert_eq!(
            sent,
            headers(&[
                ("Accept", "application/json"),
                ("Content-Type", "application/json"),
                ("Content-Encoding", "gzip"),
                ("X-Extra", "1"),
                ("X-Device-Id", "test-device"),
                ("X-Api-Key", "test-key"),
            ])
        );
        let get = ApiRequest::new(
            Method::Get,
            "/api/status",
            RequestClass::Status,
            AuthMode::None,
        );
        assert_eq!(
            wire_headers(&get, &device_credentials()).expect("ok"),
            headers(&[("Accept", "application/json")])
        );
    }

    // The builders used both `serde_json::to_vec` and `Value::to_string`;
    // `Body::json` must give the same bytes as either.
    #[test]
    fn a_json_body_is_compact_json() {
        let value = serde_json::json!({
            "login_id": 6031972,
            "username": "Q\u{e9}\"player\\</x>\n",
            "mtgo_ids": [1, 2, u32::MAX],
            "state": {"nested": [null, true, 1.5]},
        });
        let Body::Json(bytes) = Body::json(&value) else {
            panic!("a json body");
        };
        assert_eq!(bytes, value.to_string().into_bytes());
        assert_eq!(bytes, serde_json::to_vec(&value).expect("json"));
        assert_eq!(Body::json(&[1u32, 2]).bytes(), b"[1,2]");
    }

    #[test]
    fn gzip_is_level_six_as_0x() {
        use std::io::Read;
        assert_eq!(GZIP_LEVEL, 6);
        let json = br#"{"games":[]}"#;
        let mut php_default = GzEncoder::new(Vec::new(), Compression::default());
        php_default.write_all(json).expect("write");
        assert_eq!(gzip(json), php_default.finish().expect("gzip"));
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(gzip(json).as_slice())
            .read_to_end(&mut out)
            .expect("gunzip");
        assert_eq!(out, json);
    }

    #[test]
    fn a_file_part_is_its_head_contents_and_line_break() {
        let mut body = Vec::new();
        file_part(&mut body, "qb", "image", "a.png", "image/png", b"\x00\xff");
        assert_eq!(
            body,
            b"--qb\r\nContent-Disposition: form-data; name=\"image\"; filename=\"a.png\"\r\nContent-Type: image/png\r\n\r\n\x00\xff\r\n"
        );
    }

    #[test]
    fn content_headers_follow_the_body() {
        assert_eq!(Body::Empty.content_headers(), Vec::new());
        assert_eq!(
            Body::Json(Vec::new()).content_headers(),
            headers(&[("Content-Type", "application/json")])
        );
        assert_eq!(
            Body::Form(Vec::new()).content_headers(),
            headers(&[("Content-Type", "application/x-www-form-urlencoded")])
        );
        assert_eq!(
            Body::Multipart {
                boundary: "qboundary".to_string(),
                bytes: Vec::new()
            }
            .content_headers(),
            headers(&[("Content-Type", "multipart/form-data; boundary=qboundary")])
        );
    }

    #[test]
    fn timeouts_follow_the_class() {
        assert_eq!(default_timeouts(RequestClass::Registration), (5_000, 5_000));
        assert_eq!(default_timeouts(RequestClass::Status), (5_000, 5_000));
        assert_eq!(default_timeouts(RequestClass::Reference), (15_000, 10_000));
        for class in [
            RequestClass::Lookup,
            RequestClass::Community,
            RequestClass::Sync,
            RequestClass::Token,
        ] {
            assert_eq!(default_timeouts(class), (30_000, 10_000), "{class:?}");
        }
        let request = ApiRequest::new(
            Method::Get,
            "/api/status",
            RequestClass::Status,
            AuthMode::DeviceOrBearer,
        );
        assert_eq!(
            (request.timeout_ms, request.connect_timeout_ms),
            (5_000, 5_000)
        );
        assert_eq!(request.body_limit, MIB);
        assert!(request.query.is_empty() && request.headers.is_empty());
        assert_eq!(request.body, Body::Empty);
    }

    #[test]
    fn body_limits_follow_the_path() {
        assert_eq!(body_limit_for("/api/cards"), 16 * MIB);
        assert_eq!(body_limit_for("/api/cards/resolve"), 16 * MIB);
        assert_eq!(body_limit_for("/api/archetypes/estimate"), 16 * MIB);
        assert_eq!(body_limit_for("/api/archetypes/qid/decklists"), 16 * MIB);
        assert_eq!(body_limit_for("/api/sync/blobs/fetch"), 64 * MIB);
        assert_eq!(body_limit_for("/api/cards/token-catalog"), MIB);
        assert_eq!(body_limit_for("/api/cardsx"), MIB);
        assert_eq!(body_limit_for("/api/status"), MIB);
        assert_eq!(body_limit_for("/api/sync/manifest"), MIB);
    }

    #[test]
    fn the_url_joins_base_path_and_php_query() {
        let mut request = ApiRequest::new(
            Method::Get,
            "/api/metagame/modern",
            RequestClass::Lookup,
            AuthMode::DeviceOrBearer,
        );
        assert_eq!(
            request_url("https://mymtgo.test", &request),
            "https://mymtgo.test/api/metagame/modern"
        );
        request.query = headers(&[("period", "last week"), ("ids[]", "1")]);
        assert_eq!(
            request_url("https://mymtgo.test", &request),
            "https://mymtgo.test/api/metagame/modern?period=last+week&ids%5B%5D=1"
        );
    }

    #[test]
    fn debug_never_shows_a_token_key_or_refresh_token() {
        let mut request = ApiRequest::new(
            Method::Post,
            "/oauth/token",
            RequestClass::Token,
            AuthMode::None,
        );
        request.headers = headers(&[
            ("Authorization", "Bearer qaccess-secret"),
            ("x-api-key", "qkey-secret"),
            ("X-Device-Id", "qdevice"),
        ]);
        request.query = headers(&[("refresh_token", "qrefresh-in-query"), ("page", "2")]);
        request.body = Body::Form(
            b"grant_type=refresh_token&refresh_token=qrefresh-secret&client_id=qclient".to_vec(),
        );
        let shown = format!("{request:?}");
        for secret in [
            "qaccess-secret",
            "qkey-secret",
            "qrefresh-in-query",
            "qrefresh-secret",
        ] {
            assert!(!shown.contains(secret), "{secret} in {shown}");
        }
        assert!(
            shown.contains("qdevice") && shown.contains("/oauth/token"),
            "{shown}"
        );
        let credentials = Credentials {
            device_id: "qdevice".to_string(),
            api_key: Some("qkey-secret".to_string()),
            access_token: Some("qaccess-secret".to_string()),
        };
        let shown = format!("{credentials:?}");
        assert!(!shown.contains("qkey-secret") && !shown.contains("qaccess-secret"));
        let response = ApiResponse {
            status: 200,
            headers: headers(&[
                ("set-cookie", "laravel_session=qcookie-secret"),
                ("authorization", "Bearer qaccess-secret"),
                ("content-type", "application/json"),
            ]),
            body: br#"{"access_token":"qaccess-secret"}"#.to_vec(),
        };
        let shown = format!("{response:?}");
        assert!(!shown.contains("qaccess-secret"), "{shown}");
        assert!(!shown.contains("qcookie-secret"), "{shown}");
        assert!(shown.contains("application/json"), "{shown}");
    }

    #[test]
    fn malformed_json_is_a_typed_error_that_quotes_nothing() {
        let response = ApiResponse {
            status: 200,
            headers: Vec::new(),
            body: br#"{"access_token": "qsecret-token", "#.to_vec(),
        };
        let error = response.json::<serde_json::Value>().expect_err("malformed");
        let ApiError::Malformed(message) = &error else {
            panic!("{error:?}");
        };
        assert!(!message.contains("qsecret-token"), "{message}");
        let wrong_type = ApiResponse {
            status: 200,
            headers: Vec::new(),
            body: br#""qsecret-token""#.to_vec(),
        };
        let error = wrong_type.json::<Vec<u32>>().expect_err("wrong type");
        assert!(matches!(&error, ApiError::Malformed(m) if !m.contains("qsecret-token")));
        let good = ApiResponse {
            status: 200,
            headers: Vec::new(),
            body: b"[1,2]".to_vec(),
        };
        assert_eq!(good.json::<Vec<u32>>().expect("valid"), vec![1, 2]);
    }

    #[test]
    fn a_response_reads_headers_in_any_case() {
        let response = ApiResponse {
            status: 404,
            headers: headers(&[("content-type", "application/json")]),
            body: Vec::new(),
        };
        assert_eq!(response.header("Content-Type"), Some("application/json"));
        assert!(!response.is_success());
        assert_eq!(
            response.status_error(),
            ApiError::Status(404, String::new())
        );
    }
}
