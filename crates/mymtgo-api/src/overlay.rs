//! The hosted OBS overlay (overlay spec section 10, 0.x `SyncApi`'s overlay
//! calls): publish the league card, clear it, and upload or delete the
//! custom background. Class `Sync`: bearer only, blocked offline and while
//! signed out. Each goes through the app's sync send
//! (0.x `SyncApi::send`, a 401 refreshes and retries once).

use serde_json::Value;

use crate::gate::RequestClass;
use crate::loose;
use crate::request::{ApiRequest, ApiResponse, AuthMode, Body, Method, file_part};

pub const OVERLAY_PATH: &str = "/api/overlay";
pub const OVERLAY_BACKGROUND_PATH: &str = "/api/overlay/background";

/// Statuses the clear call reads as done: a 404 means nothing was published
/// (0.x `passthroughStatuses: [404]`).
pub const CLEAR_OVERLAY_PASSTHROUGH: [u16; 1] = [404];

/// `PUT /api/overlay` with `{"login_id", "state"}`.
pub fn publish_overlay_request(login_id: i64, state: &Value) -> ApiRequest {
    let mut request = overlay_request(Method::Put, OVERLAY_PATH);
    let body = serde_json::json!({ "login_id": login_id, "state": state });
    request.body = Body::json(&body);
    request
}

/// `DELETE /api/overlay/{login_id}`. Its caller passes
/// `CLEAR_OVERLAY_PASSTHROUGH`.
pub fn clear_overlay_request(login_id: i64) -> ApiRequest {
    overlay_request(Method::Delete, &format!("{OVERLAY_PATH}/{login_id}"))
}

/// `POST /api/overlay/background`, multipart around `boundary` (random,
/// from the app) with one file part `image` named `file_name` (0.x
/// `attach('image', $contents, $filename)`; the part's type comes from the
/// extension, as Guzzle sets it).
pub fn upload_overlay_background_request(
    bytes: &[u8],
    file_name: &str,
    boundary: &str,
) -> ApiRequest {
    let mut body = Vec::new();
    file_part(
        &mut body,
        boundary,
        "image",
        file_name,
        image_type(file_name),
        bytes,
    );
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    let mut request = overlay_request(Method::Post, OVERLAY_BACKGROUND_PATH);
    request.body = Body::Multipart {
        boundary: boundary.to_string(),
        bytes: body,
    };
    request
}

/// The part's content type from the file name's extension (any case).
fn image_type(file_name: &str) -> &'static str {
    let extension = match file_name.rsplit_once('.') {
        Some((_, extension)) => extension.to_ascii_lowercase(),
        None => String::new(),
    };
    match extension.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "application/octet-stream",
    }
}

/// The uploaded copy's public URL from a 2xx answer (`{"url"}`); `None` for
/// any other status or an answer without a URL.
pub fn parse_background_url(response: &ApiResponse) -> Option<String> {
    if !response.is_success() {
        return None;
    }
    let body: Value = response.json().ok()?;
    loose::text(&body, "url").filter(|url| !url.is_empty())
}

/// `DELETE /api/overlay/background`.
pub fn delete_overlay_background_request() -> ApiRequest {
    overlay_request(Method::Delete, OVERLAY_BACKGROUND_PATH)
}

fn overlay_request(method: Method, path: &str) -> ApiRequest {
    ApiRequest::new(method, path, RequestClass::Sync, AuthMode::Bearer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::response;
    use serde_json::json;

    fn assert_sync(request: &ApiRequest, method: Method, path: &str) {
        assert_eq!(request.method, method);
        assert_eq!(request.path, path);
        assert_eq!(request.class, RequestClass::Sync);
        assert_eq!(request.auth, AuthMode::Bearer);
    }

    // 0.x SyncApiOverlayTest "puts the overlay state with the login id".
    #[test]
    fn puts_the_overlay_state_with_the_login_id() {
        let request = publish_overlay_request(4242, &json!({"status": "idle", "art": null}));
        assert_sync(&request, Method::Put, "/api/overlay");
        let Body::Json(bytes) = &request.body else {
            panic!("json: {:?}", request.body);
        };
        let sent: Value = serde_json::from_slice(bytes).expect("json");
        assert_eq!(
            sent,
            json!({"login_id": 4242, "state": {"status": "idle", "art": null}})
        );
    }

    // 0.x "clears the overlay and treats a 404 as done".
    #[test]
    fn clears_the_overlay_and_treats_a_404_as_done() {
        let request = clear_overlay_request(4242);
        assert_sync(&request, Method::Delete, "/api/overlay/4242");
        assert_eq!(request.body, Body::Empty);
        assert_eq!(CLEAR_OVERLAY_PASSTHROUGH, [404]);
    }

    // 0.x "uploads the background and returns its url".
    #[test]
    fn uploads_the_background_as_a_multipart_image() {
        let request =
            upload_overlay_background_request(b"png-bytes", "background-1.png", "qboundary");
        assert_sync(&request, Method::Post, "/api/overlay/background");
        let Body::Multipart { boundary, bytes } = &request.body else {
            panic!("multipart: {:?}", request.body);
        };
        assert_eq!(boundary, "qboundary");
        assert_eq!(
            bytes.as_slice(),
            b"--qboundary\r\nContent-Disposition: form-data; name=\"image\"; filename=\"background-1.png\"\r\nContent-Type: image/png\r\n\r\npng-bytes\r\n--qboundary--\r\n"
                .as_slice()
        );
    }

    #[test]
    fn the_image_part_type_follows_the_extension() {
        for (name, kind) in [
            ("background-1.jpg", "image/jpeg"),
            ("background-1.JPEG", "image/jpeg"),
            ("background-1.png", "image/png"),
            ("background-1.webp", "image/webp"),
            ("background-1.gif", "image/gif"),
            ("background-1", "application/octet-stream"),
        ] {
            let request = upload_overlay_background_request(&[0, 255], name, "b");
            let text = String::from_utf8_lossy(request.body.bytes()).into_owned();
            assert!(
                text.contains(&format!("Content-Type: {kind}\r\n")),
                "{name}: {text}"
            );
        }
    }

    // 0.x "uploads the background and returns its url": the answer.
    #[test]
    fn the_background_url_is_read_from_a_2xx() {
        let body = r#"{"url":"https://cdn.example/bg.png"}"#;
        assert_eq!(
            parse_background_url(&response(201, body)).as_deref(),
            Some("https://cdn.example/bg.png")
        );
        assert_eq!(parse_background_url(&response(422, body)), None);
        assert_eq!(parse_background_url(&response(201, "{}")), None);
        assert_eq!(parse_background_url(&response(201, r#"{"url":""}"#)), None);
        assert_eq!(parse_background_url(&response(201, "nope")), None);
    }

    // 0.x "deletes the remote background".
    #[test]
    fn deletes_the_remote_background() {
        let request = delete_overlay_background_request();
        assert_sync(&request, Method::Delete, "/api/overlay/background");
        assert_eq!(request.body, Body::Empty);
    }
}
