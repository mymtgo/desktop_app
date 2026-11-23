//! Sign-in with PKCE through a loopback redirect, and the token endpoint
//! (survey section 2, 07 spec section 4 and ruling 1). 0.x sent the browser
//! back through a website page and a `mymtgo://` deep link; v1 listens on
//! `http://127.0.0.1:<port>/oauth/callback` for the length of the sign-in,
//! so the verifier never leaves memory.
//!
//! Like 0.x's `BuildAuthorizationRequest`, `HandleSyncOauthCallback`,
//! `ExchangeAuthorizationCode` and `RefreshAccessToken`, minus the IO.

use std::fmt;

use sha2::{Digest, Sha256};

use crate::config::ApiConfig;
use crate::gate::RequestClass;
use crate::php::{http_build_query, urldecode};
use crate::request::{ApiRequest, ApiResponse, AuthMode, Body, Method};

/// 0.x `Str::random(128)`.
pub const VERIFIER_LEN: usize = 128;
/// 0.x `Str::random(40)`.
pub const STATE_LEN: usize = 40;
pub const AUTHORIZE_PATH: &str = "/oauth/authorize";
pub const TOKEN_PATH: &str = "/oauth/token";
/// The loopback listener's only path.
pub const LOOPBACK_PATH: &str = "/oauth/callback";
/// 0.x `EnsureAccessToken`: refresh when less than 24 hours are left.
pub const REFRESH_AHEAD_MS: i64 = 24 * 3_600_000;

const ALPHANUMERIC: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
/// The largest multiple of 62 that fits a byte: bytes from here up would
/// make some characters likelier than others.
const UNBIASED_LIMIT: u8 = 248;

/// `len` characters of `[A-Za-z0-9]` from random bytes: each byte below 248
/// picks `ALPHANUMERIC[byte % 62]`, the rest are skipped (so every character
/// is equally likely). `None` when the bytes run out first.
pub fn alphanumeric(bytes: &[u8], len: usize) -> Option<String> {
    let mut out = String::with_capacity(len);
    for &byte in bytes {
        if out.len() == len {
            break;
        }
        if byte < UNBIASED_LIMIT {
            out.push(ALPHANUMERIC[(byte % 62) as usize] as char);
        }
    }
    if out.len() == len { Some(out) } else { None }
}

/// Base64url without padding (RFC 4648 section 5), as 0.x's
/// `rtrim(strtr(base64_encode(..), '+/', '-_'), '=')`.
pub fn base64url_no_pad(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0] as u32;
        let second = chunk.get(1).copied().unwrap_or(0) as u32;
        let third = chunk.get(2).copied().unwrap_or(0) as u32;
        let joined = (first << 16) | (second << 8) | third;
        let characters = chunk.len() + 1;
        for index in 0..characters {
            let six_bits = (joined >> (18 - 6 * index)) & 0x3f;
            out.push(ALPHABET[six_bits as usize] as char);
        }
    }
    out
}

/// The S256 challenge of a verifier: base64url (no padding) of its sha256.
pub fn pkce_challenge(verifier: &str) -> String {
    base64url_no_pad(&Sha256::digest(verifier.as_bytes()))
}

/// One sign-in's secrets. `Debug` shows only the challenge.
#[derive(Clone, PartialEq, Eq)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

impl Pkce {
    pub fn new(verifier: String, state: String) -> Pkce {
        let challenge = pkce_challenge(&verifier);
        Pkce {
            verifier,
            challenge,
            state,
        }
    }

    /// A verifier of 128 and a state of 40 alphanumeric characters from
    /// random bytes; `None` when either has too few usable bytes.
    pub fn from_random(verifier_bytes: &[u8], state_bytes: &[u8]) -> Option<Pkce> {
        let verifier = alphanumeric(verifier_bytes, VERIFIER_LEN)?;
        let state = alphanumeric(state_bytes, STATE_LEN)?;
        Some(Pkce::new(verifier, state))
    }
}

impl fmt::Debug for Pkce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pkce")
            .field("challenge", &self.challenge)
            .finish_non_exhaustive()
    }
}

/// The client id, empty when the build has none (sign-in refuses to start
/// without one, so an empty id never reaches the server).
fn client_id(config: &ApiConfig) -> String {
    config.client_id.clone().unwrap_or_default()
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// `{api}/oauth/authorize?client_id=..&redirect_uri=..&response_type=code&
/// state=..&code_challenge=..&code_challenge_method=S256`, encoded as PHP's
/// `http_build_query` (no scope: the API no longer checks one).
pub fn authorize_url(config: &ApiConfig, pkce: &Pkce, redirect_uri: &str) -> String {
    let client_id = client_id(config);
    let query = pairs(&[
        ("client_id", &client_id),
        ("redirect_uri", redirect_uri),
        ("response_type", "code"),
        ("state", &pkce.state),
        ("code_challenge", &pkce.challenge),
        ("code_challenge_method", "S256"),
    ]);
    format!(
        "{}{AUTHORIZE_PATH}?{}",
        config.base_url,
        http_build_query(&query)
    )
}

/// The redirect URI for a listener on `port`.
pub fn loopback_redirect(port: u16) -> String {
    format!("http://127.0.0.1:{port}{LOOPBACK_PATH}")
}

/// What one request to the loopback listener means.
#[derive(Clone, PartialEq, Eq)]
pub enum Callback {
    /// The consent screen approved: exchange this code.
    Code(String),
    /// The consent screen sent an `error` (the user declined) with this
    /// sign-in's state: stop.
    Denied(String),
    /// Not `GET /oauth/callback` (a favicon, a probe): answer and keep
    /// listening.
    Ignore,
    /// The callback without a code or state, or with someone else's state
    /// (an `error` included): refuse and keep listening.
    BadState,
}

/// The code is a secret; `Debug` shows only the variant.
impl fmt::Debug for Callback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Callback::Code(_) => write!(f, "Code(<redacted>)"),
            Callback::Denied(error) => write!(f, "Denied({error:?})"),
            Callback::Ignore => write!(f, "Ignore"),
            Callback::BadState => write!(f, "BadState"),
        }
    }
}

/// Read a request line (`GET /oauth/callback?code=..&state=.. HTTP/1.1`).
/// Only `GET` of exactly `/oauth/callback` counts. An `error` with the
/// expected state wins (0.x checked it first; the API sends the state back
/// with `access_denied`), and an `error` with any other state, or none, is
/// refused. Otherwise the code and state must both be present and the state
/// must equal `expected_state` (compared without stopping early).
pub fn parse_callback(request_line: &str, expected_state: &str) -> Callback {
    let mut parts = request_line.split(' ');
    let (Some("GET"), Some(target)) = (parts.next(), parts.next()) else {
        return Callback::Ignore;
    };
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != LOOPBACK_PATH {
        return Callback::Ignore;
    }
    let mut code = None;
    let mut state = None;
    let mut error = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = urldecode(value);
        match urldecode(key).as_str() {
            "code" => code = Some(value),
            "state" => state = Some(value),
            "error" => error = Some(value),
            _ => {}
        }
    }
    let state_matches = state
        .as_deref()
        .is_some_and(|state| same_text(state, expected_state));
    if let Some(error) = error {
        // Only this sign-in's own state may end it: anything else could be
        // a page probing loopback ports.
        return if state_matches {
            Callback::Denied(error)
        } else {
            Callback::BadState
        };
    }
    match (code, state) {
        (Some(code), Some(state)) if !code.is_empty() && same_text(&state, expected_state) => {
            Callback::Code(code)
        }
        _ => Callback::BadState,
    }
}

/// Equal without stopping at the first difference (PHP's `hash_equals`).
pub fn same_text(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

/// A form `POST /oauth/token` (class `Token`: blocked offline, no auth).
fn token_request(form: &[(&str, &str)]) -> ApiRequest {
    let mut request = ApiRequest::new(
        Method::Post,
        TOKEN_PATH,
        RequestClass::Token,
        AuthMode::None,
    );
    request.body = Body::Form(http_build_query(&pairs(form)).into_bytes());
    request
}

/// Exchange the consent screen's code (0.x `ExchangeAuthorizationCode`).
pub fn exchange_request(
    config: &ApiConfig,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> ApiRequest {
    let client_id = client_id(config);
    token_request(&[
        ("grant_type", "authorization_code"),
        ("client_id", &client_id),
        ("redirect_uri", redirect_uri),
        ("code_verifier", verifier),
        ("code", code),
    ])
}

/// Swap the refresh token for new tokens (0.x `RefreshAccessToken`). Both
/// rotate: the old refresh token is dead once this succeeds.
pub fn refresh_request(config: &ApiConfig, refresh_token: &str) -> ApiRequest {
    let client_id = client_id(config);
    token_request(&[
        ("grant_type", "refresh_token"),
        ("client_id", &client_id),
        ("refresh_token", refresh_token),
    ])
}

/// A token answer. `Debug` shows only the expiry.
#[derive(Clone, PartialEq, Eq)]
pub struct Tokens {
    pub access: String,
    pub refresh: String,
    /// `now + expires_in` seconds, in ms.
    pub expires_at_ms: i64,
}

impl fmt::Debug for Tokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tokens")
            .field("expires_at_ms", &self.expires_at_ms)
            .finish_non_exhaustive()
    }
}

/// Why a token answer gave no tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenError {
    /// Passport's "this refresh token is dead": the device is signed out.
    InvalidGrant,
    /// Anything else (credentials are kept). The message never quotes a
    /// token.
    Other(String),
}

/// The tokens of a `/oauth/token` answer. `InvalidGrant` only for a 400 or
/// 401 whose `error` is `invalid_grant` (0.x: any other failure must not
/// sign the device out). A success needs both tokens; a missing
/// `expires_in` counts as 0 (0.x cast it with `(int)`), so it refreshes at
/// next use.
pub fn parse_token(response: &ApiResponse, now_ms: i64) -> Result<Tokens, TokenError> {
    let body: Option<serde_json::Value> = response.json().ok();
    let text = |name: &str| -> Option<String> {
        body.as_ref()?
            .get(name)?
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    if !response.is_success() {
        let invalid_grant = text("error").as_deref() == Some("invalid_grant");
        if matches!(response.status, 400 | 401) && invalid_grant {
            return Err(TokenError::InvalidGrant);
        }
        return Err(TokenError::Other(format!(
            "the token request was refused (HTTP {})",
            response.status
        )));
    }
    let (Some(access), Some(refresh)) = (text("access_token"), text("refresh_token")) else {
        return Err(TokenError::Other(
            "the token answer has no access or refresh token".to_string(),
        ));
    };
    let expires_in_s = body
        .as_ref()
        .and_then(|body| body.get("expires_in"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    Ok(Tokens {
        access,
        refresh,
        expires_at_ms: now_ms.saturating_add(expires_in_s.saturating_mul(1000)),
    })
}

/// Whether the access token must be refreshed before use: its expiry is
/// unknown or less than 24 hours away.
pub fn needs_refresh(expires_at: Option<i64>, now_ms: i64) -> bool {
    match expires_at {
        Some(expires_at) => expires_at < now_ms.saturating_add(REFRESH_AHEAD_MS),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ApiConfig;
    use crate::gate::RequestClass;
    use crate::php::urldecode;
    use crate::request::{AuthMode, Body, Method};
    use crate::test_support::response;

    const NOW: i64 = 1_800_000_000_000;
    const HOUR: i64 = 3_600_000;

    fn config() -> ApiConfig {
        ApiConfig {
            base_url: "https://mymtgo.test".to_string(),
            host: "mymtgo.test".to_string(),
            client_id: Some("qclient-id".to_string()),
            ca_pem_path: None,
        }
    }

    /// The query of a URL as decoded pairs.
    fn query_pairs(url: &str) -> Vec<(String, String)> {
        let (_, query) = url.split_once('?').expect("a query");
        query
            .split('&')
            .map(|pair| {
                let (key, value) = pair.split_once('=').expect("key=value");
                (urldecode(key), urldecode(value))
            })
            .collect()
    }

    fn form_pairs(body: &Body) -> Vec<(String, String)> {
        let Body::Form(bytes) = body else {
            panic!("a form body: {body:?}");
        };
        let text = String::from_utf8(bytes.clone()).expect("utf8");
        query_pairs(&format!("?{text}"))
    }

    #[test]
    fn alphanumeric_maps_bytes_below_248_and_skips_the_rest() {
        assert_eq!(
            alphanumeric(&[0, 25, 26, 51, 52, 61], 6).as_deref(),
            Some("AZaz09")
        );
        // 62 wraps to 'A'; 247 is 247 % 62 = 61 ('9'); 248 and above are skipped.
        assert_eq!(alphanumeric(&[62, 248, 255, 247], 2).as_deref(), Some("A9"));
        assert_eq!(alphanumeric(&[1, 2, 3], 2).as_deref(), Some("BC"));
        assert_eq!(alphanumeric(&[1, 250], 2), None, "too few usable bytes");
        assert_eq!(alphanumeric(&[], 0).as_deref(), Some(""));
    }

    #[test]
    fn the_challenge_is_base64url_sha256_of_the_verifier() {
        // RFC 7636 appendix B.
        let pkce = Pkce::new(
            "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".to_string(),
            "qstate".to_string(),
        );
        assert_eq!(
            pkce.challenge,
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert_eq!(base64url_no_pad(b""), "");
        assert_eq!(base64url_no_pad(b"f"), "Zg");
        assert_eq!(base64url_no_pad(b"fo"), "Zm8");
        assert_eq!(base64url_no_pad(b"foo"), "Zm9v");
        assert_eq!(base64url_no_pad(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn pkce_from_random_bytes_has_the_0x_lengths() {
        let verifier_bytes: Vec<u8> = (0..=255u8).cycle().take(400).collect();
        let pkce = Pkce::from_random(&verifier_bytes, &verifier_bytes).expect("enough bytes");
        assert_eq!(pkce.verifier.len(), VERIFIER_LEN);
        assert_eq!(pkce.state.len(), STATE_LEN);
        assert!(pkce.verifier.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_eq!(VERIFIER_LEN, 128);
        assert_eq!(STATE_LEN, 40);
        assert!(Pkce::from_random(&[1; 10], &[1; 100]).is_none());
        let shown = format!("{pkce:?}");
        assert!(!shown.contains(&pkce.verifier), "{shown}");
        assert!(!shown.contains(&pkce.state), "{shown}");
    }

    // 0.x DeviceLinkTest: "builds an authorize url whose challenge is derived
    // from the stashed verifier".
    #[test]
    fn builds_an_authorize_url_whose_challenge_is_derived_from_the_verifier() {
        let pkce = Pkce::new("qverifier".to_string(), "qstate".to_string());
        let redirect = loopback_redirect(49152);
        let url = authorize_url(&config(), &pkce, &redirect);
        assert!(
            url.starts_with("https://mymtgo.test/oauth/authorize?"),
            "{url}"
        );
        let pairs = query_pairs(&url);
        let keys: Vec<&str> = pairs.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "client_id",
                "redirect_uri",
                "response_type",
                "state",
                "code_challenge",
                "code_challenge_method"
            ],
            "http_build_query order, no scope"
        );
        let value = |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        assert_eq!(value("client_id").as_deref(), Some("qclient-id"));
        assert_eq!(
            value("redirect_uri").as_deref(),
            Some("http://127.0.0.1:49152/oauth/callback")
        );
        assert_eq!(value("response_type").as_deref(), Some("code"));
        assert_eq!(value("state").as_deref(), Some("qstate"));
        assert_eq!(value("code_challenge"), Some(pkce_challenge("qverifier")));
        assert_eq!(value("code_challenge_method").as_deref(), Some("S256"));
        // RFC 1738 encoding as PHP: the redirect's `:` and `/` are escaped.
        assert!(
            url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A49152%2Foauth%2Fcallback"),
            "{url}"
        );
    }

    // 0.x DeviceLinkTest: "registers the API callback page as the redirect,
    // not the deep link", as it applies to the loopback redirect.
    #[test]
    fn registers_the_loopback_address_as_the_redirect() {
        assert_eq!(LOOPBACK_PATH, "/oauth/callback");
        assert_eq!(
            loopback_redirect(8123),
            "http://127.0.0.1:8123/oauth/callback"
        );
    }

    #[test]
    fn a_callback_with_the_right_state_gives_its_code() {
        assert_eq!(
            parse_callback(
                "GET /oauth/callback?code=abc%2B123&state=goodstate HTTP/1.1",
                "goodstate"
            ),
            Callback::Code("abc+123".to_string())
        );
        assert_eq!(
            parse_callback(
                "GET /oauth/callback?state=goodstate&code=c HTTP/1.0",
                "goodstate"
            ),
            Callback::Code("c".to_string())
        );
    }

    // 0.x DeviceLinkTest: "rejects a state mismatch before the code is ever exchanged".
    #[test]
    fn rejects_a_state_mismatch() {
        for line in [
            "GET /oauth/callback?code=abc123&state=evilstate HTTP/1.1",
            "GET /oauth/callback?code=abc123&state=goodstat HTTP/1.1",
            "GET /oauth/callback?code=abc123 HTTP/1.1",
            "GET /oauth/callback?state=goodstate HTTP/1.1",
            "GET /oauth/callback?code=&state=goodstate HTTP/1.1",
            "GET /oauth/callback HTTP/1.1",
        ] {
            assert_eq!(
                parse_callback(line, "goodstate"),
                Callback::BadState,
                "{line}"
            );
        }
    }

    // 0.x DeviceLinkTest: "ignores deep links that are not the oauth callback".
    #[test]
    fn ignores_requests_that_are_not_the_oauth_callback() {
        for line in [
            "GET /favicon.ico HTTP/1.1",
            "GET /somewhere/else?code=abc123&state=goodstate HTTP/1.1",
            "GET /oauth/callbackx?code=abc123&state=goodstate HTTP/1.1",
            "POST /oauth/callback?code=abc123&state=goodstate HTTP/1.1",
            "GET http://127.0.0.1/oauth/callback?code=abc123&state=goodstate HTTP/1.1",
            "garbage",
            "",
        ] {
            assert_eq!(
                parse_callback(line, "goodstate"),
                Callback::Ignore,
                "{line}"
            );
        }
    }

    // 0.x DeviceLinkTest: "drops the stash without exchanging when the
    // consent screen returns an error".
    #[test]
    fn an_error_from_the_consent_screen_is_a_denial() {
        assert_eq!(
            parse_callback(
                "GET /oauth/callback?error=access_denied&state=goodstate HTTP/1.1",
                "goodstate"
            ),
            Callback::Denied("access_denied".to_string())
        );
    }

    // Q3 review Minor 2: an error ends the sign-in only with this sign-in's
    // state, so a page probing loopback ports cannot cancel it.
    #[test]
    fn an_error_without_the_matching_state_is_refused() {
        for line in [
            "GET /oauth/callback?error=access_denied&state=otherstate HTTP/1.1",
            "GET /oauth/callback?error=access_denied HTTP/1.1",
            "GET /oauth/callback?error=x&code=qcode&state=goodstat HTTP/1.1",
        ] {
            assert_eq!(
                parse_callback(line, "goodstate"),
                Callback::BadState,
                "{line}"
            );
        }
    }

    // 0.x DeviceLinkTest: "exchanges the code and stores rotated tokens" (the request).
    #[test]
    fn the_exchange_request_matches_0x() {
        let request = exchange_request(
            &config(),
            "qcode",
            "qverifier",
            "http://127.0.0.1:1/oauth/callback",
        );
        assert_eq!(request.method, Method::Post);
        assert_eq!(request.path, "/oauth/token");
        assert_eq!(request.class, RequestClass::Token);
        assert_eq!(request.auth, AuthMode::None);
        assert_eq!(
            form_pairs(&request.body),
            vec![
                ("grant_type".to_string(), "authorization_code".to_string()),
                ("client_id".to_string(), "qclient-id".to_string()),
                (
                    "redirect_uri".to_string(),
                    "http://127.0.0.1:1/oauth/callback".to_string()
                ),
                ("code_verifier".to_string(), "qverifier".to_string()),
                ("code".to_string(), "qcode".to_string()),
            ]
        );
        assert!(!format!("{request:?}").contains("qverifier"));
    }

    #[test]
    fn the_refresh_request_matches_0x() {
        let request = refresh_request(&config(), "qrefresh");
        assert_eq!(request.method, Method::Post);
        assert_eq!(request.path, "/oauth/token");
        assert_eq!(request.class, RequestClass::Token);
        assert_eq!(request.auth, AuthMode::None);
        assert_eq!(
            form_pairs(&request.body),
            vec![
                ("grant_type".to_string(), "refresh_token".to_string()),
                ("client_id".to_string(), "qclient-id".to_string()),
                ("refresh_token".to_string(), "qrefresh".to_string()),
            ]
        );
    }

    #[test]
    fn a_token_answer_gives_both_tokens_and_the_expiry() {
        let tokens = parse_token(
            &response(
                200,
                r#"{"token_type":"Bearer","access_token":"at1","refresh_token":"rt1","expires_in":2592000}"#,
            ),
            NOW,
        )
        .expect("tokens");
        assert_eq!(tokens.access, "at1");
        assert_eq!(tokens.refresh, "rt1");
        assert_eq!(tokens.expires_at_ms, NOW + 2_592_000_000);
        let shown = format!("{tokens:?}");
        assert!(!shown.contains("at1") && !shown.contains("rt1"), "{shown}");
    }

    // 0.x DeviceLinkTest: "clears credentials and reports unlinked when the
    // refresh is refused" and "leaves credentials intact and rethrows on a
    // 400 that is not invalid_grant" (the classification).
    #[test]
    fn only_a_400_or_401_invalid_grant_is_invalid_grant() {
        for status in [400, 401] {
            assert_eq!(
                parse_token(&response(status, r#"{"error":"invalid_grant"}"#), NOW),
                Err(TokenError::InvalidGrant),
                "{status}"
            );
        }
        for (status, body) in [
            (400, r#"{"error":"invalid_request"}"#),
            (403, r#"{"error":"invalid_grant"}"#),
            (500, r#"{"error":"invalid_grant"}"#),
            (400, "not json"),
            (200, r#"{"access_token":"at1"}"#),
            (
                200,
                r#"{"access_token":"","refresh_token":"rt1","expires_in":5}"#,
            ),
            (200, "not json"),
        ] {
            assert!(
                matches!(
                    parse_token(&response(status, body), NOW),
                    Err(TokenError::Other(_))
                ),
                "{status} {body}"
            );
        }
        let Err(TokenError::Other(message)) = parse_token(
            &response(200, r#"{"access_token":"qsecret-at","refresh_token":""}"#),
            NOW,
        ) else {
            panic!("other");
        };
        assert!(!message.contains("qsecret-at"), "{message}");
    }

    #[test]
    fn a_missing_expiry_counts_as_expiring_now() {
        let tokens = parse_token(
            &response(200, r#"{"access_token":"at1","refresh_token":"rt1"}"#),
            NOW,
        )
        .expect("tokens");
        assert_eq!(tokens.expires_at_ms, NOW);
    }

    // 0.x EnsureAccessToken: refresh when the expiry is unknown or less than
    // 24 hours away.
    #[test]
    fn a_token_needs_refresh_within_24_hours_of_expiry() {
        assert!(needs_refresh(None, NOW));
        assert!(needs_refresh(Some(NOW + HOUR), NOW));
        assert!(needs_refresh(Some(NOW + 24 * HOUR - 1), NOW));
        assert!(!needs_refresh(Some(NOW + 24 * HOUR), NOW));
        assert!(!needs_refresh(Some(NOW + 30 * 24 * HOUR), NOW));
        assert!(needs_refresh(Some(NOW - HOUR), NOW));
    }

    #[test]
    fn states_are_compared_whole() {
        assert!(same_text("goodstate", "goodstate"));
        assert!(!same_text("goodstate", "goodstatx"));
        assert!(!same_text("goodstate", "goodstat"));
        assert!(!same_text("", "goodstate"));
    }
}
