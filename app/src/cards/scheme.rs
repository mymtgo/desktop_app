//! The `card` URI scheme (cards spec section 7): `card://localhost/{file}`
//! (`http://card.localhost/{file}` on Windows) serves `{images folder}/{file}`
//! (typed by its first bytes, JPEG when unknown) when `file` is one of v1's image names
//! (`cards::images::is_owned_file`), else 404. Nothing outside the images
//! folder can be named: the name is digits, an optional `_art` and `.jpg`,
//! never decoded, so no separator, `..` or encoded character gets through.

use std::path::Path;

use crate::cards::data_url::mime_for;
use cards::images::is_owned_file;
use tauri::http::{Response, StatusCode, header};

/// The scheme's name, as `convertFileSrc(name, 'card')` uses it.
pub const SCHEME: &str = "card";

/// The answer for a request path (`/12345.jpg`) with the images folder as
/// it is now (`None` when the app has none).
pub fn respond(folder: Option<&Path>, path: &str) -> Response<Vec<u8>> {
    let Some(name) = path.strip_prefix('/') else {
        return not_found();
    };
    let Some(folder) = folder else {
        return not_found();
    };
    if !is_owned_file(name) {
        return not_found();
    }
    let file = folder.join(name);
    // Not a link and not a folder: only a plain file in the folder itself.
    let is_file = std::fs::symlink_metadata(&file).is_ok_and(|meta| meta.is_file());
    if !is_file {
        return not_found();
    }
    match std::fs::read(&file) {
        Ok(bytes) => {
            // MyMTGO's images are WebP saved as `.jpg`: the type is the bytes'.
            let mime = mime_for(&bytes, name);
            let mut response = Response::new(bytes);
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, header::HeaderValue::from_static(mime));
            response
        }
        Err(_) => not_found(),
    }
}

fn not_found() -> Response<Vec<u8>> {
    let mut response = Response::new(Vec::new());
    *response.status_mut() = StatusCode::NOT_FOUND;
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use fixture_tools::TestDir;

    fn folder(label: &str) -> (TestDir, std::path::PathBuf) {
        let dir = TestDir::new(label);
        let folder = dir.join("card-images");
        std::fs::create_dir_all(&folder).expect("folder");
        std::fs::write(folder.join("12.jpg"), b"q-image").expect("image");
        std::fs::write(folder.join("12_art.jpg"), b"q-art").expect("art");
        std::fs::write(dir.join("1.jpg"), b"outside").expect("outside");
        std::fs::write(folder.join("a.png"), b"png").expect("png");
        (dir, folder)
    }

    // 0.x ImageBase64ControllerTest: "returns base64 data url from local
    // image when present" (the scheme serves the file itself).
    #[test]
    fn an_owned_file_is_served_as_a_jpeg() {
        let (_dir, folder) = folder("scheme-serves");
        let response = respond(Some(&folder), "/12.jpg");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .map(|v| v.as_bytes()),
            Some(&b"image/jpeg"[..])
        );
        assert_eq!(response.body(), b"q-image");
        assert_eq!(respond(Some(&folder), "/12_art.jpg").body(), b"q-art");
    }

    #[test]
    fn a_webp_saved_as_jpg_is_served_as_webp() {
        let (_dir, folder) = folder("scheme-webp");
        std::fs::write(folder.join("15.jpg"), b"RIFF\x04\x00\x00\x00WEBPVP8 ").expect("webp");
        let response = respond(Some(&folder), "/15.jpg");
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .map(|v| v.as_bytes()),
            Some(&b"image/webp"[..])
        );
    }

    // 0.x "returns null dataUrl when oracle id has no card" and "when card
    // has no image source": nothing to serve is a 404 (the UI falls back to
    // the remote URL).
    #[test]
    fn a_missing_file_or_folder_is_not_found() {
        let (dir, folder) = folder("scheme-missing");
        assert_eq!(
            respond(Some(&folder), "/13.jpg").status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(respond(None, "/12.jpg").status(), StatusCode::NOT_FOUND);
        assert_eq!(
            respond(Some(&dir.join("gone")), "/12.jpg").status(),
            StatusCode::NOT_FOUND
        );
        std::fs::create_dir_all(folder.join("14.jpg")).expect("folder named like an image");
        assert_eq!(
            respond(Some(&folder), "/14.jpg").status(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn the_card_scheme_refuses_traversal_and_other_names() {
        let (_dir, folder) = folder("scheme-refuses");
        for path in [
            "/../1.jpg",
            "../1.jpg",
            "/..%2F1.jpg",
            "/%2E%2E%2F1.jpg",
            "/12.jpg/..",
            "/1.jpg/../12.jpg",
            "/sub/12.jpg",
            "//12.jpg",
            "/a.png",
            "/12.jpg%00",
            "/12%2Ejpg",
            "/%31%32.jpg",
            "/12.JPG",
            "/",
            "",
            "/..\\1.jpg",
            "12.jpg",
        ] {
            assert_eq!(
                respond(Some(&folder), path).status(),
                StatusCode::NOT_FOUND,
                "{path}"
            );
        }
    }
}
