//! The copy of 0.x's card image files into v1's images folder (cards spec
//! section 13, ruling 3). When 0.x had local images on, the import hands
//! 0.x's image folder to the cards worker, which copies the files it can
//! before any download; whatever is not copied (missing, unreadable, empty
//! or wrongly named) is left for the download queue.
//!
//! 0.x's folder is only ever read: its files are opened read only and never
//! moved, renamed, modified or deleted. Plain `std::fs`, no dependency.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use cards::images::{art_file, image_file, is_owned_file};
use store::cards::ImageJob;

/// Why a file was not copied: the download queue fetches it.
pub const MISSING: &str = "missing";
pub const EMPTY: &str = "empty";
pub const NOT_A_FILE: &str = "not a file";
pub const WRONGLY_NAMED: &str = "wrongly named";

/// What one copy did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CopyReport {
    /// `(catalog id, file name)` of each file now in v1's folder: copied, or
    /// already there at the same size (kept, not written again). The worker
    /// records each with `CardImagesStored`.
    pub copied: Vec<(u32, String)>,
    /// `(catalog id, file name, reason)` of each file left for the download
    /// queue.
    pub skipped: Vec<(u32, String, String)>,
    /// A chosen folder went (a removed drive, an unmounted path) mid-copy:
    /// the copy stopped and nothing was made in its place.
    pub folder_gone: bool,
}

/// The 0.x image folder an import handed to the cards worker, waiting for
/// its copy (which runs before any download). A later import replaces it.
/// Kept in memory; each start rebuilds it from the stored import
/// (`pending_at_start`), and a copy that fails gives it back (`restore`).
#[derive(Debug, Default)]
pub struct PendingCopy {
    folder: Mutex<Option<PathBuf>>,
}

impl PendingCopy {
    pub fn hand(&self, folder: PathBuf) {
        *crate::bridge::lock(&self.folder) = Some(folder);
    }

    /// The folder to copy from, once.
    pub fn take(&self) -> Option<PathBuf> {
        crate::bridge::lock(&self.folder).take()
    }

    /// Give back a folder whose copy failed, unless a later import handed
    /// another meanwhile.
    pub fn restore(&self, folder: PathBuf) {
        let mut slot = crate::bridge::lock(&self.folder);
        if slot.is_none() {
            *slot = Some(folder);
        }
    }
}

/// 0.x's image folder: its `card_images_path` setting when set (it already
/// includes 0.x's `mymtgo-card-images` subfolder), else 0.x's default
/// `storage/app/cards` under the 0.x app data folder.
pub fn legacy_images_folder(
    legacy_card_images_path: Option<&str>,
    legacy_app_data: &Path,
) -> PathBuf {
    // 0.x `cardImagesPath()`: a non-empty string, else the default.
    match legacy_card_images_path {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => legacy_app_data.join("storage").join("app").join("cards"),
    }
}

/// The 0.x app data folder of a 0.x database path
/// (`<app data>/database/nativephp.sqlite`), as `importer::settings_path`
/// finds it.
pub fn legacy_app_data(db_path: &Path) -> Option<PathBuf> {
    let app_data = db_path.parent()?.parent()?;
    if app_data.as_os_str().is_empty() {
        return None;
    }
    Some(app_data.to_path_buf())
}

/// The folder to copy from after an import (`kept` is the kept 0.x settings
/// JSON, `legacy_0x_settings`). Local images are always on in v1, so the
/// folder is copied whatever 0.x's own switch said: a 0.x that kept images
/// off has an empty folder, and the copy then finds nothing.
pub fn legacy_images_source(kept: &str, db_path: &Path) -> Option<PathBuf> {
    let Ok(serde_json::Value::Object(kept)) = serde_json::from_str(kept) else {
        return None;
    };
    let custom = kept
        .get(crate::settings::CARD_IMAGES_PATH)
        .and_then(serde_json::Value::as_str);
    Some(legacy_images_folder(custom, &legacy_app_data(db_path)?))
}

/// The folder to copy from at start, rebuilt from the stored settings rows
/// (the kept 0.x settings and the last import's database path): a copy the
/// app never got to (a quit first, or a copy that failed) runs again, and it
/// only fills the files still missing, so a copy that did run costs a
/// listing. `None` without an import.
pub fn pending_at_start(rows: &[(String, String)]) -> Option<PathBuf> {
    let value = |key: &str| {
        rows.iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    };
    let kept = value(crate::legacy::LEGACY_SETTINGS)?;
    let last_import = value(crate::legacy::LEGACY_LAST_IMPORT)?;
    let summary: crate::legacy::ImportSummary = serde_json::from_str(last_import).ok()?;
    legacy_images_source(kept, Path::new(&summary.path))
}

/// Copy each job's missing files (`{id}.jpg` for a card with an image URL and
/// no local image, `{id}_art.jpg` for an art crop) from `source` to `dest`.
/// `folder_ready` is asked before each file (`images::prepare_folder`): when
/// it fails (a chosen folder that went), the copy stops (`folder_gone`).
pub fn copy_legacy_images_into(
    source: &Path,
    dest: &Path,
    jobs: &[ImageJob],
    folder_ready: &dyn Fn() -> Result<(), String>,
) -> CopyReport {
    let present = source_names(source);
    let mut report = CopyReport::default();
    'jobs: for job in jobs {
        let mut wanted = Vec::new();
        if job.image.is_some() && !job.has_local_image {
            wanted.push(image_file(job.catalog_id));
        }
        if job.art_crop.is_some() && !job.has_local_art {
            wanted.push(art_file(job.catalog_id));
        }
        for name in wanted {
            if folder_ready().is_err() {
                report.folder_gone = true;
                break 'jobs;
            }
            match copy_one(source, dest, &present, &name) {
                Ok(()) => report.copied.push((job.catalog_id, name)),
                Err(reason) => report.skipped.push((job.catalog_id, name, reason)),
            }
        }
    }
    report
}

/// `copy_legacy_images_into` a folder made when missing (the default
/// folder's rule).
pub fn copy_legacy_images(source: &Path, dest: &Path, jobs: &[ImageJob]) -> CopyReport {
    copy_legacy_images_into(source, dest, jobs, &|| {
        crate::cards::images::prepare_folder(dest, true)
    })
}

/// The names in the source folder (none when it cannot be read). Listing it
/// once lets a name match exactly, also on a case-insensitive disk.
fn source_names(source: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let Ok(entries) = fs::read_dir(source) else {
        return names;
    };
    for entry in entries.flatten() {
        if let Some(name) = entry.file_name().to_str() {
            names.insert(name.to_string());
        }
    }
    names
}

/// Copy one file, or say why it was not copied.
fn copy_one(
    source: &Path,
    dest: &Path,
    present: &BTreeSet<String>,
    name: &str,
) -> Result<(), String> {
    if !is_owned_file(name) {
        return Err(WRONGLY_NAMED.to_string());
    }
    if !present.contains(name) {
        return Err(if is_misnamed(present, name) {
            WRONGLY_NAMED.to_string()
        } else {
            MISSING.to_string()
        });
    }
    let from = source.join(name);
    // `symlink_metadata`: a link in 0.x's folder is never followed.
    let meta = fs::symlink_metadata(&from).map_err(|error| format!("unreadable: {error}"))?;
    if !meta.is_file() {
        return Err(NOT_A_FILE.to_string());
    }
    if meta.len() == 0 {
        return Err(EMPTY.to_string());
    }
    let to = dest.join(name);
    // `symlink_metadata`: a link with the name is not v1's file, so it is
    // neither kept as the copy nor replaced.
    match fs::symlink_metadata(&to) {
        Ok(kept) if kept.is_file() && kept.len() == meta.len() => return Ok(()),
        Ok(kept) if !kept.is_file() => return Err(format!("v1's folder: {NOT_A_FILE}")),
        _ => {}
    }
    // Read only: 0.x's file is never written, moved or deleted.
    let mut reader = File::open(&from).map_err(|error| format!("unreadable: {error}"))?;
    let temporary = dest.join(format!("{name}.part"));
    let mut writer = crate::cards::images::create_temporary(&temporary)
        .map_err(|error| format!("v1's folder: {error}"))?;
    let written = io::copy(&mut reader, &mut writer).and_then(|copied| {
        writer.sync_all()?;
        Ok(copied)
    });
    drop(writer);
    let outcome = match written {
        Ok(0) => Err(EMPTY.to_string()),
        Ok(_) => fs::rename(&temporary, &to).map_err(|error| format!("v1's folder: {error}")),
        Err(error) => Err(format!("unreadable: {error}")),
    };
    if outcome.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    outcome
}

/// A file with the wanted name in another case or with another extension
/// (`12.JPG`, `12.jpeg`): 0.x never wrote it, so it is not copied.
fn is_misnamed(present: &BTreeSet<String>, name: &str) -> bool {
    let stem = name.strip_suffix(".jpg").unwrap_or(name).to_lowercase();
    present.iter().any(|found| {
        let lower = found.to_lowercase();
        lower == name
            || lower
                .rsplit_once('.')
                .is_some_and(|(found_stem, _)| found_stem == stem)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fixture_tools::TestDir;
    use std::time::SystemTime;

    fn job(catalog_id: u32) -> ImageJob {
        ImageJob {
            catalog_id,
            image: Some(format!("https://cards.scryfall.io/q/{catalog_id}.jpg")),
            art_crop: Some(format!("https://cards.scryfall.io/q/{catalog_id}-art.jpg")),
            has_local_image: false,
            has_local_art: false,
        }
    }

    /// Every entry of a folder: name, bytes (files only) and modification time.
    fn snapshot(folder: &Path) -> Vec<(String, Option<Vec<u8>>, SystemTime)> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(folder).expect("folder") {
            let entry = entry.expect("entry");
            let meta = entry.metadata().expect("metadata");
            let bytes = if meta.is_file() {
                fs::read(entry.path()).ok()
            } else {
                None
            };
            entries.push((
                entry.file_name().to_string_lossy().into_owned(),
                bytes,
                meta.modified().expect("modified"),
            ));
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries
    }

    fn names(folder: &Path) -> Vec<String> {
        snapshot(folder).into_iter().map(|entry| entry.0).collect()
    }

    fn owned(pairs: &[(u32, &str)]) -> Vec<(u32, String)> {
        pairs
            .iter()
            .map(|(id, name)| (*id, name.to_string()))
            .collect()
    }

    #[test]
    fn a_handed_folder_is_taken_once_and_a_later_import_replaces_it() {
        let pending = PendingCopy::default();
        assert_eq!(pending.take(), None);
        pending.hand(PathBuf::from("/q/one"));
        pending.hand(PathBuf::from("/q/two"));
        assert_eq!(pending.take(), Some(PathBuf::from("/q/two")));
        assert_eq!(pending.take(), None);
    }

    #[test]
    fn legacy_images_folder_uses_the_setting_else_the_default() {
        let app_data = Path::new("/q/mymtgo");
        assert_eq!(
            legacy_images_folder(Some("/q/pics/mymtgo-card-images"), app_data),
            PathBuf::from("/q/pics/mymtgo-card-images"),
            "the setting already includes 0.x's subfolder"
        );
        let default = app_data.join("storage").join("app").join("cards");
        assert_eq!(legacy_images_folder(None, app_data), default);
        assert_eq!(legacy_images_folder(Some(""), app_data), default);

        let db = Path::new("/q/mymtgo/database/nativephp.sqlite");
        assert_eq!(legacy_app_data(db), Some(PathBuf::from("/q/mymtgo")));
        assert_eq!(legacy_app_data(Path::new("nativephp.sqlite")), None);

        assert_eq!(
            legacy_images_source(r#"{"local_images":true}"#, db),
            Some(default.clone())
        );
        assert_eq!(
            legacy_images_source(
                r#"{"local_images":1,"card_images_path":"/q/pics/mymtgo-card-images"}"#,
                db
            ),
            Some(PathBuf::from("/q/pics/mymtgo-card-images"))
        );
        assert_eq!(
            legacy_images_source(r#"{"local_images":false}"#, db),
            Some(default.clone()),
            "0.x's own switch no longer gates the copy"
        );
        assert_eq!(legacy_images_source("not json", db), None);
    }

    // S3-4: the folder to copy is rebuilt from the stored import at start, so
    // a quit before the copy only costs the run it waits for.
    #[test]
    fn the_pending_copy_is_rebuilt_from_the_stored_import() {
        let summary = |path: &str| {
            serde_json::json!({
                "finishedAtMs": 1, "path": path, "added": 0, "updated": 0, "unchanged": 0,
                "skippedLive": 0, "skippedInProgress": 0, "skippedNoLocalPlayer": 0,
                "skippedDuplicateToken": 0, "skippedBadTime": 0,
            })
            .to_string()
        };
        let rows = |kept: &str, path: &str| {
            vec![
                (crate::legacy::LEGACY_SETTINGS.to_string(), kept.to_string()),
                (crate::legacy::LEGACY_LAST_IMPORT.to_string(), summary(path)),
            ]
        };
        let db = "/q/mymtgo/database/nativephp.sqlite";
        assert_eq!(
            pending_at_start(&rows(r#"{"local_images":true}"#, db)),
            Some(PathBuf::from("/q/mymtgo/storage/app/cards"))
        );
        assert_eq!(
            pending_at_start(&rows(r#"{"local_images":false}"#, db)),
            Some(PathBuf::from("/q/mymtgo/storage/app/cards"))
        );
        assert_eq!(
            pending_at_start(&rows(r#"{"local_images":true}"#, "nativephp.sqlite")),
            None
        );
        let no_import = vec![(
            crate::legacy::LEGACY_SETTINGS.to_string(),
            r#"{"local_images":true}"#.to_string(),
        )];
        assert_eq!(pending_at_start(&no_import), None);
        assert_eq!(pending_at_start(&[]), None);
    }

    #[test]
    fn the_copy_copies_existing_files_into_v1s_folder() {
        let dir = TestDir::new("legacy-images-copy");
        dir.write("legacy/11.jpg", b"card eleven");
        dir.write("legacy/11_art.jpg", b"art eleven");
        dir.write("legacy/12.jpg", b"card twelve");
        dir.write("legacy/12_art.jpg", b"art twelve");
        let mut recorded = job(12);
        recorded.has_local_art = true;
        let mut no_art_url = job(11);
        no_art_url.art_crop = None;

        let report = copy_legacy_images(
            &dir.join("legacy"),
            &dir.join("v1/card-images"),
            &[no_art_url, recorded],
        );

        assert_eq!(report.copied, owned(&[(11, "11.jpg"), (12, "12.jpg")]));
        assert!(report.skipped.is_empty(), "{:?}", report.skipped);
        assert_eq!(names(&dir.join("v1/card-images")), vec!["11.jpg", "12.jpg"]);
        assert_eq!(
            fs::read(dir.join("v1/card-images/11.jpg")).expect("copied"),
            b"card eleven"
        );
    }

    #[test]
    fn a_missing_source_file_is_left_for_the_download_queue() {
        let dir = TestDir::new("legacy-images-missing");
        dir.write("legacy/21.jpg", b"card");
        let report = copy_legacy_images(&dir.join("legacy"), &dir.join("v1"), &[job(21)]);
        assert_eq!(report.copied, owned(&[(21, "21.jpg")]));
        assert_eq!(
            report.skipped,
            vec![(21, "21_art.jpg".to_string(), MISSING.to_string())]
        );
        assert_eq!(names(&dir.join("v1")), vec!["21.jpg"]);
    }

    #[test]
    fn an_unreadable_or_wrongly_named_source_file_is_skipped_not_fatal() {
        let dir = TestDir::new("legacy-images-unreadable");
        dir.write("legacy/31.jpg", b"");
        dir.write("legacy/32.JPG", b"upper case");
        dir.write("legacy/32_art.jpeg", b"other extension");
        dir.write("legacy/33.jpg/inside", b"a folder named as a file");
        dir.write("legacy/34.jpg", b"good");
        let mut jobs = vec![job(31), job(32), job(33), job(34)];
        for job in &mut jobs {
            job.has_local_art = job.catalog_id != 32;
        }

        let report = copy_legacy_images(&dir.join("legacy"), &dir.join("v1"), &jobs);

        assert_eq!(report.copied, owned(&[(34, "34.jpg")]));
        let reasons: Vec<(u32, &str, &str)> = report
            .skipped
            .iter()
            .map(|(id, name, reason)| (*id, name.as_str(), reason.as_str()))
            .collect();
        assert_eq!(
            reasons,
            vec![
                (31, "31.jpg", EMPTY),
                (32, "32.jpg", WRONGLY_NAMED),
                (32, "32_art.jpg", WRONGLY_NAMED),
                (33, "33.jpg", NOT_A_FILE),
            ]
        );
        assert_eq!(names(&dir.join("v1")), vec!["34.jpg"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_source_file_that_cannot_be_opened_is_skipped() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TestDir::new("legacy-images-locked");
        let locked = dir.write("legacy/41.jpg", b"locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("lock");
        if File::open(&locked).is_ok() {
            // Running as root: every file opens.
            return;
        }
        let mut only_card = job(41);
        only_card.has_local_art = true;
        let report = copy_legacy_images(&dir.join("legacy"), &dir.join("v1"), &[only_card]);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).expect("unlock");
        assert!(report.copied.is_empty());
        assert_eq!(report.skipped.len(), 1);
        assert!(
            report.skipped[0].2.starts_with("unreadable: "),
            "{:?}",
            report.skipped
        );
        assert!(
            !dir.join("v1").exists() || names(&dir.join("v1")).is_empty(),
            "no partial file is left"
        );
    }

    #[test]
    fn the_copy_never_modifies_the_source_folder() {
        let dir = TestDir::new("legacy-images-read-only");
        dir.write("legacy/51.jpg", b"card");
        dir.write("legacy/51_art.jpg", b"art");
        dir.write("legacy/52.jpg", b"");
        dir.write("legacy/53.JPG", b"upper");
        dir.write("legacy/notes.txt", b"someone else's file");
        dir.write("legacy/sub/54.jpg", b"nested");
        let source = dir.join("legacy");
        let before = snapshot(&source);
        let nested = snapshot(&dir.join("legacy/sub"));

        let jobs = [job(51), job(52), job(53), job(54)];
        copy_legacy_images(&source, &dir.join("v1"), &jobs);
        copy_legacy_images(&source, &dir.join("v1"), &jobs);

        assert_eq!(
            snapshot(&source),
            before,
            "names, bytes and times unchanged"
        );
        assert_eq!(snapshot(&dir.join("legacy/sub")), nested);
        assert!(source.join("51.jpg").is_file(), "never moved or deleted");
    }

    #[test]
    fn the_copy_is_rerun_safe() {
        let dir = TestDir::new("legacy-images-rerun");
        dir.write("legacy/61.jpg", b"card");
        dir.write("legacy/61_art.jpg", b"art");
        // A different size in v1's folder is replaced by 0.x's file.
        dir.write("v1/61_art.jpg", b"a partial download");
        let jobs = [job(61)];

        let first = copy_legacy_images(&dir.join("legacy"), &dir.join("v1"), &jobs);
        assert_eq!(first.copied, owned(&[(61, "61.jpg"), (61, "61_art.jpg")]));
        let after_first = snapshot(&dir.join("v1"));
        assert_eq!(
            fs::read(dir.join("v1/61_art.jpg")).expect("art"),
            b"art",
            "replaced"
        );

        let second = copy_legacy_images(&dir.join("legacy"), &dir.join("v1"), &jobs);
        assert_eq!(second.copied, first.copied, "both are in v1's folder");
        assert_eq!(
            snapshot(&dir.join("v1")),
            after_first,
            "same-size files are kept, not written again"
        );
    }

    // S4-2: links planted in v1's folder (a `.part` name or a target name)
    // are never written through, followed or deleted: those files are left
    // for the download queue.
    #[cfg(unix)]
    #[test]
    fn planted_links_in_v1s_folder_are_never_written_through() {
        let dir = TestDir::new("legacy-images-links");
        dir.write("legacy/81.jpg", b"card");
        dir.write("legacy/81_art.jpg", b"art");
        dir.write("legacy/82.jpg", b"card");
        let outside = dir.write("outside.txt", b"keep");
        let same_size = dir.write("same-size.txt", b"q-a");
        fs::create_dir_all(dir.join("v1")).expect("v1");
        std::os::unix::fs::symlink(&outside, dir.join("v1/81.jpg.part")).expect("link");
        std::os::unix::fs::symlink(&same_size, dir.join("v1/81_art.jpg")).expect("link");
        // A stale temporary file of v1's own is replaced.
        dir.write("v1/82.jpg.part", b"half");
        let mut only_card = job(82);
        only_card.has_local_art = true;
        let report =
            copy_legacy_images(&dir.join("legacy"), &dir.join("v1"), &[job(81), only_card]);
        assert_eq!(report.copied, owned(&[(82, "82.jpg")]));
        let skipped: Vec<&str> = report
            .skipped
            .iter()
            .map(|(_, name, _)| name.as_str())
            .collect();
        assert_eq!(skipped, vec!["81.jpg", "81_art.jpg"]);
        assert_eq!(fs::read(&outside).expect("outside"), b"keep");
        assert_eq!(fs::read(&same_size).expect("same size"), b"q-a");
        assert!(
            fs::symlink_metadata(dir.join("v1/81.jpg.part")).is_ok(),
            "not deleted"
        );
        assert!(
            fs::symlink_metadata(dir.join("v1/81_art.jpg")).is_ok(),
            "not deleted"
        );
        assert!(!dir.join("v1/81.jpg").exists());
        assert_eq!(fs::read(dir.join("v1/82.jpg")).expect("copied"), b"card");
        assert!(!dir.join("v1/82.jpg.part").exists());
    }

    // Fix round 1 (review I1): a chosen folder whose drive goes mid-batch is
    // never made again; the copy stops before the next write.
    #[test]
    fn a_chosen_folder_gone_mid_copy_is_not_recreated() {
        let dir = TestDir::new("legacy-images-gone-mid-copy");
        for id in [91, 92, 93] {
            dir.write(&format!("legacy/{id}.jpg"), b"card");
        }
        let root = dir.join("drive");
        let dest = root.join("mymtgo-desktop-card-images");
        fs::create_dir_all(&dest).expect("chosen");
        let jobs: Vec<ImageJob> = [91, 92, 93]
            .into_iter()
            .map(|id| ImageJob {
                art_crop: None,
                ..job(id)
            })
            .collect();
        let checks = std::cell::Cell::new(0);
        let ready = || {
            checks.set(checks.get() + 1);
            if checks.get() == 2 {
                // The drive is pulled after the first file.
                fs::remove_dir_all(&root).expect("unplug");
            }
            crate::cards::images::prepare_folder(&dest, false)
        };
        let report = copy_legacy_images_into(&dir.join("legacy"), &dest, &jobs, &ready);
        assert!(report.folder_gone, "the copy stopped");
        assert_eq!(report.copied, owned(&[(91, "91.jpg")]));
        assert!(!root.exists(), "nothing made in the gone folder's place");
        assert!(!dest.exists());
    }

    #[test]
    fn a_missing_source_folder_copies_nothing() {
        let dir = TestDir::new("legacy-images-no-folder");
        let report = copy_legacy_images(&dir.join("gone"), &dir.join("v1"), &[job(71)]);
        assert!(report.copied.is_empty());
        assert_eq!(
            report.skipped,
            vec![
                (71, "71.jpg".to_string(), MISSING.to_string()),
                (71, "71_art.jpg".to_string(), MISSING.to_string()),
            ]
        );
        assert!(!dir.join("gone").exists(), "the source is not created");
        assert!(
            !dir.join("v1").exists() || names(&dir.join("v1")).is_empty(),
            "nothing written"
        );
    }
}
