//! Local card image file names (cards spec section 7). v1 owns only the
//! files named `{catalog_id}.jpg` (the card) and `{catalog_id}_art.jpg` (the
//! art crop) in its images folder, the names 0.x used too; anything else in
//! the folder is never touched.

/// The names v1 owns in its images folder, as a regular expression for
/// docs and messages (`is_owned_file` checks it without a regex).
pub const IMAGE_FILE_PATTERN: &str = r"^[0-9]+(_art)?\.jpg$";

/// Scryfall's image host (cards spec section 7).
pub const IMAGE_ORIGIN: &str = "https://cards.scryfall.io";

/// The MyMTGO image host: the API serves self-hosted image URLs on it.
pub const MYMTGO_IMAGE_ORIGIN: &str = "https://cards.mymtgo.com";

/// The only places images are downloaded from.
pub const IMAGE_ORIGINS: [&str; 2] = [IMAGE_ORIGIN, MYMTGO_IMAGE_ORIGIN];

/// The subfolder v1 keeps its images in inside a chosen folder (0.x used
/// `mymtgo-card-images`, so v1 never clears or moves 0.x's files).
pub const V1_SUBFOLDER: &str = "mymtgo-desktop-card-images";

/// The move's validation messages (0.x `QueueCardImagesMove`), in 0.x's order.
pub const ALREADY_MOVING: &str = "Card images are already being moved.";
pub const NOT_ABSOLUTE: &str = "Choose a full folder path.";
pub const OVERLAPS: &str = "Choose a folder outside the current image folder.";
pub const NOT_CREATED: &str = "That folder could not be created.";
pub const NOT_WRITABLE: &str = "That folder cannot be written to.";

/// A POSIX path, a Windows drive path (`X:\` or `X:/`) or a UNC share.
pub fn is_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/');
    path.starts_with('/') || drive || path.starts_with("\\\\")
}

/// The same folder, or one inside the other (lowercased, `/` separators,
/// no trailing slash): a move would copy files onto themselves or delete
/// what it just copied.
pub fn overlaps(from: &str, to: &str) -> bool {
    let from = normalise(from);
    let to = normalise(to);
    from == to || to.starts_with(&format!("{from}/")) || from.starts_with(&format!("{to}/"))
}

/// 0.x `normalise`: lowercased, `/` separators, no trailing slash.
fn normalise(path: &str) -> String {
    path.replace('\\', "/").trim_end_matches('/').to_lowercase()
}

/// Validate a move from `from` to `to` in 0.x's order. `exists_or_created`
/// and `writable` touch the disk, so they are asked only when the checks
/// before them pass.
pub fn validate_move(
    from: &str,
    to: &str,
    moving: bool,
    exists_or_created: &dyn Fn() -> bool,
    writable: &dyn Fn() -> bool,
) -> Result<(), String> {
    if moving {
        return Err(ALREADY_MOVING.to_string());
    }
    if !is_absolute(to) {
        return Err(NOT_ABSOLUTE.to_string());
    }
    if overlaps(from, to) {
        return Err(OVERLAPS.to_string());
    }
    if !exists_or_created() {
        return Err(NOT_CREATED.to_string());
    }
    if !writable() {
        return Err(NOT_WRITABLE.to_string());
    }
    Ok(())
}

/// The card image's file name.
pub fn image_file(catalog_id: u32) -> String {
    format!("{catalog_id}.jpg")
}

/// The art crop's file name.
pub fn art_file(catalog_id: u32) -> String {
    format!("{catalog_id}_art.jpg")
}

/// Whether `name` is one of v1's image files (`^[0-9]+(_art)?\.jpg$`).
pub fn is_owned_file(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".jpg") else {
        return false;
    };
    let digits = stem.strip_suffix("_art").unwrap_or(stem);
    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
}

/// Whether `url` may be fetched: it starts with `origin` (scheme and host,
/// no trailing slash) followed by a path. Anything else (another scheme,
/// host, port or user info) is never fetched.
pub fn is_allowed_image_url(url: &str, origin: &str) -> bool {
    // The authority ends at the first `/` after `scheme://`, so a prefix of
    // `origin/` pins scheme, host and port exactly (no user info either).
    url.strip_prefix(origin)
        .is_some_and(|rest| rest.starts_with('/'))
}

/// Whether `url` is on any of the `origins` (each checked exactly, as
/// `is_allowed_image_url` does).
pub fn is_allowed_image_url_in(url: &str, origins: &[String]) -> bool {
    origins
        .iter()
        .any(|origin| is_allowed_image_url(url, origin))
}

/// The images folder's size as 0.x's `MeasureLocalImagesSize` shows it:
/// GB and MB with one decimal, KB with none, else bytes, PHP
/// `number_format` style (`1,024.0 GB`).
pub fn size_text(bytes: u64) -> String {
    const KB: u64 = 1_024;
    const MB: u64 = 1_048_576;
    const GB: u64 = 1_073_741_824;
    if bytes >= GB {
        format!("{} GB", number_format(bytes as f64 / GB as f64, 1))
    } else if bytes >= MB {
        format!("{} MB", number_format(bytes as f64 / MB as f64, 1))
    } else if bytes >= KB {
        format!("{} KB", number_format(bytes as f64 / KB as f64, 0))
    } else {
        format!("{bytes} B")
    }
}

/// PHP `number_format($value, $decimals)` for a value of zero or more:
/// rounded half away from zero, `,` between thousands, `.` before the
/// decimals.
fn number_format(value: f64, decimals: u32) -> String {
    let scale = 10u64.pow(decimals);
    let scaled = (value * scale as f64).round() as u64;
    let whole = (scaled / scale).to_string();
    let mut grouped = String::new();
    for (index, digit) in whole.chars().enumerate() {
        if index > 0 && (whole.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    if decimals == 0 {
        return grouped;
    }
    let fraction = scaled % scale;
    format!("{grouped}.{fraction:0width$}", width = decimals as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_names_are_0xs() {
        assert_eq!(image_file(12345), "12345.jpg");
        assert_eq!(art_file(12345), "12345_art.jpg");
        assert!(is_owned_file(&image_file(1)));
        assert!(is_owned_file(&art_file(u32::MAX)));
    }

    #[test]
    fn only_digits_then_jpg_or_art_jpg_are_owned() {
        for name in ["0.jpg", "12345.jpg", "12345_art.jpg", "007_art.jpg"] {
            assert!(is_owned_file(name), "{name}");
        }
        for name in [
            "",
            ".jpg",
            "_art.jpg",
            "12345",
            "12345.JPG",
            "12345.jpeg",
            "12345.png",
            "12345_ART.jpg",
            "12345_art_art.jpg",
            "12a45.jpg",
            "-1.jpg",
            "+1.jpg",
            " 1.jpg",
            "1.jpg ",
            "1.jpg.part",
            "../1.jpg",
            "sub/1.jpg",
            "sub\\1.jpg",
            "１.jpg",
            "notes.txt",
        ] {
            assert!(!is_owned_file(name), "{name}");
        }
    }

    #[test]
    fn size_text_matches_0x() {
        assert_eq!(size_text(0), "0 B");
        assert_eq!(size_text(1_023), "1023 B");
        assert_eq!(size_text(1_024), "1 KB");
        assert_eq!(size_text(1_536), "2 KB");
        assert_eq!(size_text(1_048_575), "1,024 KB");
        assert_eq!(size_text(1_048_576), "1.0 MB");
        assert_eq!(size_text(1_572_864), "1.5 MB");
        assert_eq!(size_text(1_073_741_823), "1,024.0 MB");
        assert_eq!(size_text(1_073_741_824), "1.0 GB");
        assert_eq!(size_text(1_610_612_736), "1.5 GB");
        assert_eq!(size_text(1_099_511_627_776), "1,024.0 GB");
    }

    #[test]
    fn the_pattern_is_0xs() {
        assert_eq!(IMAGE_FILE_PATTERN, r"^[0-9]+(_art)?\.jpg$");
    }

    #[test]
    fn only_the_scryfall_image_host_is_allowed() {
        assert!(is_allowed_image_url(
            "https://cards.scryfall.io/large/front/a/b.jpg?1",
            IMAGE_ORIGIN
        ));
        for url in [
            "http://cards.scryfall.io/large/a.jpg",
            "https://cards.scryfall.io.evil.test/a.jpg",
            "https://cards.scryfall.io@evil.test/a.jpg",
            "https://cards.scryfall.io:8443/a.jpg",
            "https://evil.test/https://cards.scryfall.io/a.jpg",
            "https://CARDS.scryfall.io/a.jpg",
            "https://cards.scryfall.io",
            "ftp://cards.scryfall.io/a.jpg",
            "",
        ] {
            assert!(!is_allowed_image_url(url, IMAGE_ORIGIN), "{url}");
        }
        assert!(is_allowed_image_url(
            "http://127.0.0.1:4100/q.jpg",
            "http://127.0.0.1:4100"
        ));
    }

    #[test]
    fn the_mymtgo_image_host_is_allowed_and_others_still_are_not() {
        let origins: Vec<String> = IMAGE_ORIGINS.iter().map(|o| o.to_string()).collect();
        assert!(is_allowed_image_url_in(
            "https://cards.mymtgo.com/cards/12.jpg",
            &origins
        ));
        assert!(is_allowed_image_url_in(
            "https://cards.scryfall.io/large/a.jpg",
            &origins
        ));
        for url in [
            "https://cards.mymtgo.com.evil.test/a.jpg",
            "https://cards.mymtgo.com@evil.test/a.jpg",
            "http://cards.mymtgo.com/a.jpg",
            "https://mymtgo.com/a.jpg",
            "https://evil.test/a.jpg",
            "https://cards.mymtgo.com",
        ] {
            assert!(!is_allowed_image_url_in(url, &origins), "{url}");
        }
    }

    #[test]
    fn absolute_paths_are_posix_drive_or_unc() {
        for path in [
            "/",
            "/home/q/images",
            "C:\\images",
            "d:/images",
            "\\\\server\\share",
        ] {
            assert!(is_absolute(path), "{path}");
        }
        for path in [
            "",
            "relative/folder",
            "images",
            "C:images",
            "\\single",
            ":/x",
            "1:/x",
            "~/q",
        ] {
            assert!(!is_absolute(path), "{path}");
        }
    }

    #[test]
    fn overlapping_folders_are_the_same_or_nested() {
        assert!(overlaps("/q/custom/", "/q/custom"));
        assert!(overlaps("C:\\Q\\Images", "c:/q/images/"));
        assert!(overlaps("/q/custom", "/q/custom/deeper"));
        assert!(overlaps(
            "/q/custom/mymtgo-desktop-card-images",
            "/q/custom"
        ));
        assert!(!overlaps("/q/custom", "/q/custom-two"));
        assert!(!overlaps("/q/a", "/q/b"));
    }

    #[test]
    fn a_move_is_validated_in_0xs_order() {
        let yes = || true;
        let no = || false;
        let unreachable = || panic!("asked too early");
        let check =
            |to: &str, moving: bool, made: &dyn Fn() -> bool, writable: &dyn Fn() -> bool| {
                validate_move("/q/default", to, moving, made, writable)
            };
        assert_eq!(
            check("relative", true, &unreachable, &unreachable),
            Err(ALREADY_MOVING.to_string())
        );
        assert_eq!(
            check("relative", false, &unreachable, &unreachable),
            Err(NOT_ABSOLUTE.to_string())
        );
        assert_eq!(
            check("/q/default/x", false, &unreachable, &unreachable),
            Err(OVERLAPS.to_string())
        );
        assert_eq!(
            check("/q/other", false, &no, &unreachable),
            Err(NOT_CREATED.to_string())
        );
        assert_eq!(
            check("/q/other", false, &yes, &no),
            Err(NOT_WRITABLE.to_string())
        );
        assert_eq!(check("/q/other", false, &yes, &yes), Ok(()));
        assert_eq!(V1_SUBFOLDER, "mymtgo-desktop-card-images");
    }
}
