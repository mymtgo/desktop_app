//! Card images as `data:` URLs for screenshots (release spec section 11,
//! 0.x `EncodeImageAsDataUrl` and `ImageBase64Controller`).
//!
//! `html-to-image` copies the screenshot element into an SVG, and the
//! webview's CSP keeps script from fetching remote images, so every image in
//! a screenshot is inlined before the capture. The local file in v1's images
//! folder comes first (only one of v1's own file names, read here, never a
//! path from the webview); else the remote URL is fetched with a 5 s limit
//! through `images::Fetcher`, which only reaches the card image hosts
//! (Scryfall and cards.mymtgo.com).
//! Anything that fails gives `None`, as 0.x's `null`. Blocking: callers run
//! it in `spawn_blocking`, and never while holding the reader's lock.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cards::images::is_owned_file;
use store::Connection;
use store::cards::ImageSources;
use tauri::{AppHandle, Manager};

use crate::bridge::lock;
use crate::cards::images::{Fetcher, ImagesState};
use crate::commands::{CommandError, Startup, ready, storage};
use crate::logging::Timer;

/// A screenshot's remote image waits this long (0.x `'timeout' => 5`).
pub const DATA_URL_TIMEOUT: Duration = Duration::from_secs(5);

/// A screenshot that fetches several images one after another (the deck's)
/// stops fetching after this long in total; later cards use only local
/// files. 0.x had no total, so a slow Scryfall could hold a deck screenshot
/// for minutes.
pub const SCREENSHOT_DEADLINE: Duration = Duration::from_secs(20);

/// The time left for a screenshot's remote images.
#[derive(Debug, Clone, Copy)]
pub struct FetchBudget {
    deadline: Instant,
}

impl FetchBudget {
    pub fn starting(now: Instant, total: Duration) -> Self {
        Self {
            deadline: now + total,
        }
    }

    /// How long the next fetch may wait: 5 s, or less near the deadline;
    /// `None` once it has passed.
    pub fn next_timeout(&self, now: Instant) -> Option<Duration> {
        let left = self.deadline.checked_duration_since(now)?;
        if left.is_zero() {
            return None;
        }
        Some(left.min(DATA_URL_TIMEOUT))
    }
}

/// The image type from the first bytes: MyMTGO's host serves WebP that is
/// saved as `.jpg`, so a name cannot be trusted. `None` when the bytes are
/// none of WebP, PNG or JPEG.
pub fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(b"\x89PNG") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        Some("image/jpeg")
    } else {
        None
    }
}

/// The image type: the bytes' own when they say (`sniff_mime`), else 0.x's
/// rule from the source's name: `.png`, `.webp`, else JPEG.
pub fn mime_for(bytes: &[u8], source: &str) -> &'static str {
    if let Some(mime) = sniff_mime(bytes) {
        mime
    } else if source.contains(".png") {
        "image/png"
    } else if source.contains(".webp") {
        "image/webp"
    } else {
        "image/jpeg"
    }
}

/// A `data:` URL of `bytes`.
pub fn data_url_of(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", decks::base64_encode(bytes))
}

/// The bytes of one of v1's image files in `folder`: a plain file (not a
/// link or a folder) whose name is `{id}.jpg` or `{id}_art.jpg`.
pub fn read_local(folder: &Path, name: &str) -> Option<Vec<u8>> {
    if !is_owned_file(name) {
        return None;
    }
    let file = folder.join(name);
    let is_file = std::fs::symlink_metadata(&file).is_ok_and(|meta| meta.is_file());
    if !is_file {
        return None;
    }
    std::fs::read(&file).ok()
}

/// The image as a `data:` URL (0.x `EncodeImageAsDataUrl::run`): the local
/// file when it is in the folder, else the remote URL; `None` when neither
/// gives bytes. The type follows the local name when there is one, else the
/// URL, as 0.x.
pub fn image_data_url(
    sources: &ImageSources,
    folder: Option<&Path>,
    fetcher: &Fetcher,
) -> Option<String> {
    image_data_url_within(sources, folder, fetcher, Some(DATA_URL_TIMEOUT))
}

/// `image_data_url` with the remote fetch waiting at most `timeout`; with
/// `None`, only the local file is read.
pub fn image_data_url_within(
    sources: &ImageSources,
    folder: Option<&Path>,
    fetcher: &Fetcher,
    timeout: Option<Duration>,
) -> Option<String> {
    let local = sources
        .local_image
        .as_deref()
        .filter(|name| !name.is_empty());
    let remote = sources.image.as_deref().filter(|url| !url.is_empty());
    let from_disk = match (folder, local) {
        (Some(folder), Some(name)) => read_local(folder, name),
        _ => None,
    };
    let bytes = match from_disk {
        Some(bytes) => bytes,
        None => fetcher.fetch_within(remote?, timeout?).ok()?,
    };
    if bytes.is_empty() {
        return None;
    }
    let source = local.or(remote).unwrap_or("");
    Some(data_url_of(mime_for(&bytes, source), &bytes))
}

/// A card's image by oracle id as a `data:` URL; `None` when no card has
/// that oracle id or it has no image (0.x `ImageBase64Controller`).
/// `read` gives the card's sources (it holds the reader only while it runs).
pub fn card_image_data_url_with(
    read: impl FnOnce() -> Result<Option<ImageSources>, CommandError>,
    folder: Option<&Path>,
    fetcher: &Fetcher,
) -> Result<Option<String>, CommandError> {
    let Some(sources) = read()? else {
        return Ok(None);
    };
    Ok(image_data_url(&sources, folder, fetcher))
}

/// The card sources of an oracle id.
pub fn oracle_sources(
    conn: &Connection,
    oracle_id: &str,
) -> Result<Option<ImageSources>, CommandError> {
    store::cards::oracle_image(conn, oracle_id).map_err(storage)
}

/// The images folder the app uses now (`None` before the app is ready).
pub fn current_folder(app: &AppHandle) -> Option<PathBuf> {
    app.try_state::<ImagesState>()
        .map(|images| images.folder().path)
}

/// The Card Stats screenshot's image (0.x `cards/{oracleId}/image-base64`).
/// Off the IPC thread: a remote image can take up to 5 s.
#[tauri::command]
#[specta::specta]
pub async fn card_image_data_url(
    app: AppHandle,
    oracle_id: String,
) -> Result<Option<String>, CommandError> {
    let _timer = Timer::start("card_image_data_url");
    crate::commands::off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let folder = current_folder(&app);
        card_image_data_url_with(
            || oracle_sources(&lock(&state.reader), &oracle_id),
            folder.as_deref(),
            &Fetcher::standard(),
        )
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::stub::{Reply, Stub};
    use fixture_tools::TestDir;

    /// A database with the given cards rows: `(catalog_id, oracle_id, image,
    /// local_image)`.
    fn database(
        label: &str,
        rows: &[(u32, &str, Option<&str>, Option<&str>)],
    ) -> (TestDir, Connection) {
        let dir = TestDir::new(label);
        let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        for (catalog_id, oracle_id, image, local) in rows {
            conn.execute(
                "INSERT INTO cards (catalog_id, oracle_id, name, image, local_image)
                 VALUES (?1, ?2, 'Qcard', ?3, ?4)",
                (*catalog_id, *oracle_id, *image, *local),
            )
            .expect("card");
        }
        (dir, conn)
    }

    fn image_stub() -> Stub {
        Stub::start(|request| Reply::Respond {
            status: 200,
            headers: vec![("Content-Type".to_string(), "image/jpeg".to_string())],
            body: format!("remote:{}", request.target).into_bytes(),
        })
    }

    fn url_for(conn: &Connection, folder: &Path, stub: &Stub, oracle_id: &str) -> Option<String> {
        card_image_data_url_with(
            || oracle_sources(conn, oracle_id),
            Some(folder),
            &Fetcher::new(&stub.base_url),
        )
        .expect("read")
    }

    fn jpeg(bytes: &[u8]) -> String {
        format!("data:image/jpeg;base64,{}", decks::base64_encode(bytes))
    }

    // 0.x ImageBase64ControllerTest: "returns base64 data url from local
    // image when present".
    #[test]
    fn card_image_data_url_prefers_the_local_file() {
        let stub = image_stub();
        let remote = format!("{}/n/12.jpg", stub.base_url);
        let (dir, conn) = database(
            "data-url-local",
            &[(12, "q-oracle-local", Some(&remote), Some("12.jpg"))],
        );
        let folder = dir.join("card-images");
        std::fs::create_dir_all(&folder).expect("folder");
        std::fs::write(folder.join("12.jpg"), b"q-local-bytes").expect("image");

        let url = url_for(&conn, &folder, &stub, "q-oracle-local");
        assert_eq!(url, Some(jpeg(b"q-local-bytes")));
        assert!(stub.requests().is_empty(), "nothing fetched");
    }

    // 0.x "returns null dataUrl when oracle id has no card" and "returns null
    // dataUrl when card has no image source".
    #[test]
    fn a_missing_card_gives_null() {
        let stub = image_stub();
        let (dir, conn) = database("data-url-missing", &[(13, "q-oracle-bare", None, None)]);
        let folder = dir.join("card-images");
        assert_eq!(url_for(&conn, &folder, &stub, "q-oracle-missing"), None);
        assert_eq!(url_for(&conn, &folder, &stub, "q-oracle-bare"), None);
        assert!(stub.requests().is_empty());
    }

    // The remote case: no local file, so the image is fetched (0.x
    // `file_get_contents` with a 5 s timeout).
    #[test]
    fn a_card_without_its_local_file_is_fetched_within_5_seconds() {
        let stub = image_stub();
        let remote = format!("{}/n/14.jpg?1700", stub.base_url);
        let (dir, conn) = database(
            "data-url-remote",
            &[(14, "q-oracle-remote", Some(&remote), Some("14.jpg"))],
        );
        // The row names a file the folder does not have.
        let folder = dir.join("card-images");
        std::fs::create_dir_all(&folder).expect("folder");

        let url = url_for(&conn, &folder, &stub, "q-oracle-remote");
        assert_eq!(url, Some(jpeg(b"remote:/n/14.jpg?1700")));
        assert_eq!(stub.requests().len(), 1);
        assert_eq!(DATA_URL_TIMEOUT, Duration::from_secs(5));
    }

    #[test]
    fn a_failed_or_foreign_fetch_gives_null() {
        let failing = Stub::start(|_| Reply::Respond {
            status: 404,
            headers: Vec::new(),
            body: Vec::new(),
        });
        let missing = format!("{}/n/15.jpg", failing.base_url);
        let (dir, conn) = database(
            "data-url-failing",
            &[
                (15, "q-oracle-404", Some(&missing), None),
                (
                    16,
                    "q-oracle-foreign",
                    Some("https://example.test/16.jpg"),
                    None,
                ),
            ],
        );
        let folder = dir.join("card-images");
        assert_eq!(url_for(&conn, &folder, &failing, "q-oracle-404"), None);
        assert_eq!(url_for(&conn, &folder, &failing, "q-oracle-foreign"), None);
        assert_eq!(
            failing.requests().len(),
            1,
            "the foreign host is never asked"
        );
    }

    #[test]
    fn a_printing_with_a_local_file_is_chosen_first() {
        let stub = image_stub();
        let first = format!("{}/n/20.jpg", stub.base_url);
        let second = format!("{}/n/21.jpg", stub.base_url);
        let (dir, conn) = database(
            "data-url-printings",
            &[
                (20, "q-oracle-two", Some(&first), None),
                (21, "q-oracle-two", Some(&second), Some("21.jpg")),
            ],
        );
        let folder = dir.join("card-images");
        std::fs::create_dir_all(&folder).expect("folder");
        std::fs::write(folder.join("21.jpg"), b"q-printing-21").expect("image");
        assert_eq!(
            url_for(&conn, &folder, &stub, "q-oracle-two"),
            Some(jpeg(b"q-printing-21"))
        );
    }

    #[test]
    fn only_v1s_own_file_names_are_read_from_the_folder() {
        let dir = TestDir::new("data-url-names");
        let folder = dir.join("card-images");
        std::fs::create_dir_all(&folder).expect("folder");
        std::fs::write(dir.join("1.jpg"), b"outside").expect("outside");
        std::fs::write(folder.join("2_art.jpg"), b"art").expect("art");
        std::fs::create_dir_all(folder.join("3.jpg")).expect("folder named like an image");
        assert_eq!(read_local(&folder, "2_art.jpg"), Some(b"art".to_vec()));
        for name in ["../1.jpg", "/1.jpg", "3.jpg", "4.jpg", "a.png", ""] {
            assert_eq!(read_local(&folder, name), None, "{name}");
        }
    }

    /// A screenshot with many remote images stops fetching once its overall
    /// deadline passes; each fetch waits at most 5 s and never past it.
    #[test]
    fn the_fetch_budget_caps_each_wait_and_the_total() {
        let start = Instant::now();
        let budget = FetchBudget::starting(start, Duration::from_secs(12));
        assert_eq!(budget.next_timeout(start), Some(DATA_URL_TIMEOUT));
        let late = start + Duration::from_secs(9);
        assert_eq!(budget.next_timeout(late), Some(Duration::from_secs(3)));
        assert_eq!(budget.next_timeout(start + Duration::from_secs(12)), None);
        assert_eq!(budget.next_timeout(start + Duration::from_secs(30)), None);
        assert_eq!(SCREENSHOT_DEADLINE, Duration::from_secs(20));
    }

    #[test]
    fn past_the_deadline_only_local_files_are_read() {
        let stub = image_stub();
        let remote = format!("{}/n/30.jpg", stub.base_url);
        let dir = TestDir::new("data-url-deadline");
        let folder = dir.join("card-images");
        std::fs::create_dir_all(&folder).expect("folder");
        std::fs::write(folder.join("31.jpg"), b"q-local-31").expect("image");
        let remote_only = ImageSources {
            image: Some(remote.clone()),
            local_image: None,
        };
        let local = ImageSources {
            image: Some(remote),
            local_image: Some("31.jpg".to_string()),
        };
        let fetcher = Fetcher::new(&stub.base_url);
        assert_eq!(
            image_data_url_within(&remote_only, Some(&folder), &fetcher, None),
            None
        );
        assert_eq!(
            image_data_url_within(&local, Some(&folder), &fetcher, None),
            Some(jpeg(b"q-local-31"))
        );
        assert!(stub.requests().is_empty(), "nothing fetched");
    }

    #[test]
    fn the_type_follows_the_name_as_in_0x() {
        assert_eq!(mime_for(b"q", "12.jpg"), "image/jpeg");
        assert_eq!(
            mime_for(b"q", "https://cards.scryfall.io/a/1.png?1"),
            "image/png"
        );
        assert_eq!(
            mime_for(b"q", "https://cards.scryfall.io/a/1.webp"),
            "image/webp"
        );
    }

    #[test]
    fn the_type_follows_the_bytes_not_the_name() {
        let webp = b"RIFF\x10\x00\x00\x00WEBPVP8 ";
        let png = b"\x89PNG\r\n\x1a\n";
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0];
        assert_eq!(mime_for(webp, "12.jpg"), "image/webp");
        assert_eq!(mime_for(png, "12.jpg"), "image/png");
        assert_eq!(mime_for(&jpeg, "1.png"), "image/jpeg");
        assert_eq!(mime_for(b"RIFFxxxxWAVE", "12.jpg"), "image/jpeg");
        assert_eq!(mime_for(b"", "1.webp"), "image/webp");
        assert_eq!(data_url_of("image/png", b"a"), "data:image/png;base64,YQ==");
    }
}
