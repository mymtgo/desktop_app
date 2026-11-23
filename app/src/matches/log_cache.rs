//! Remembers where each match's `Match_GameLog` file is and what it holds,
//! so the match page and its game log dialog do not walk MTGO's data folder
//! and re-parse the file on every request (0.x kept the found path and the
//! decoded entries in its `GameLog` row).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono_tz::Tz;
use game_detail::{GameLogEntry, parse_game_log};

/// How long a "no game log file" answer is trusted: the file appears while a
/// match is being played, so a miss is searched again after this long.
pub const MISS_TTL_MS: i64 = 30_000;

/// How many parsed files are kept (the most recently used ones).
pub const MAX_PARSED_FILES: usize = 32;

/// How many token lookups are kept. Past this the remembered misses go
/// first, then everything: a path is only a shortcut and is found again.
pub const MAX_PATHS: usize = 512;

/// The game log lookups of the running app. Held in `AppState` behind a
/// mutex. Time is passed in, so tests never sleep.
#[derive(Debug, Default)]
pub struct GameLogCache {
    /// The data folder setting the paths below were found under. A new
    /// setting forgets them.
    data_folder: Option<PathBuf>,
    /// What the last search for each match token found.
    paths: HashMap<String, Lookup>,
    /// Parsed files, least recently used first.
    parsed: Vec<ParsedLog>,
}

/// What a lookup knows before any folder walk.
#[derive(Debug)]
pub(crate) enum Step {
    /// The remembered file, still there.
    Known(PathBuf, FileStamp),
    /// A recent search found nothing: trusted for `MISS_TTL_MS`.
    FreshMiss,
    /// Walk the folder, then `record` the result.
    Search,
}

/// What a search for one token found.
#[derive(Debug, Clone)]
enum Lookup {
    Found(PathBuf),
    /// Nothing, at `checked_at_ms`.
    Missing {
        checked_at_ms: i64,
    },
}

/// What tells a file's contents apart without reading it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileStamp {
    length: u64,
    modified: Option<SystemTime>,
}

/// One file's parsed entries, valid while its stamp is unchanged.
#[derive(Debug)]
struct ParsedLog {
    path: PathBuf,
    stamp: FileStamp,
    /// Ticks are read as wall-clock time in this timezone.
    tz: Tz,
    entries: Vec<GameLogEntry>,
}

/// The file's stamp, `None` when it is gone (or is not a file).
pub(crate) fn stamp_of(path: &Path) -> Option<FileStamp> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    Some(FileStamp {
        length: meta.len(),
        modified: meta.modified().ok(),
    })
}

/// A miss is trusted for `MISS_TTL_MS`. A clock that went back ends it too.
fn miss_is_fresh(checked_at_ms: i64, now_ms: i64) -> bool {
    now_ms >= checked_at_ms && now_ms - checked_at_ms < MISS_TTL_MS
}

impl GameLogCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The parsed entries of `token`'s game log, empty when there is none.
    /// `find` is the folder search, run only when the token's file is not
    /// known (or has gone) and no fresh miss is remembered. `data_folder`
    /// is the chosen MTGO data folder setting the search uses.
    pub fn entries(
        &mut self,
        data_folder: Option<&Path>,
        token: &str,
        tz: Tz,
        now_ms: i64,
        find: impl FnOnce(&str) -> Option<PathBuf>,
    ) -> Vec<GameLogEntry> {
        let located = match self.begin(data_folder, token, now_ms) {
            Step::Known(path, stamp) => Some((path, stamp)),
            Step::FreshMiss => None,
            Step::Search => {
                let found = find(token);
                self.record(data_folder, token, now_ms, found)
            }
        };
        let Some((path, stamp)) = located else {
            return Vec::new();
        };
        self.parsed_entries(&path, stamp, tz)
    }

    /// The first half of a lookup, for a caller that must not hold the cache
    /// while the folder is walked: what is already known about `token`. On
    /// `Search`, walk the folder with no lock held, then hand the result to
    /// `record`.
    pub(crate) fn begin(&mut self, data_folder: Option<&Path>, token: &str, now_ms: i64) -> Step {
        if self.data_folder.as_deref() != data_folder {
            self.data_folder = data_folder.map(Path::to_path_buf);
            self.paths.clear();
        }
        match self.paths.get(token) {
            Some(Lookup::Found(path)) => {
                if let Some(stamp) = stamp_of(path) {
                    return Step::Known(path.clone(), stamp);
                }
                // Moved or deleted: search again, which replaces the entry.
            }
            Some(Lookup::Missing { checked_at_ms }) if miss_is_fresh(*checked_at_ms, now_ms) => {
                return Step::FreshMiss;
            }
            // A stale miss, or never searched.
            _ => {}
        }
        Step::Search
    }

    /// The second half of a lookup: remember what the search for `token`
    /// found (only that token's file, never the others the walk passed) and
    /// give its stamp. Nothing is remembered when the data folder setting
    /// changed while the search ran.
    pub(crate) fn record(
        &mut self,
        data_folder: Option<&Path>,
        token: &str,
        now_ms: i64,
        found: Option<PathBuf>,
    ) -> Option<(PathBuf, FileStamp)> {
        let found = found.and_then(|path| stamp_of(&path).map(|stamp| (path, stamp)));
        if self.data_folder.as_deref() != data_folder {
            return found;
        }
        let lookup = match &found {
            Some((path, _)) => Lookup::Found(path.clone()),
            None => Lookup::Missing {
                checked_at_ms: now_ms,
            },
        };
        if !self.paths.contains_key(token) {
            self.make_room();
        }
        self.paths.insert(token.to_string(), lookup);
        found
    }

    /// The parsed entries of a located file (the second step after `record`
    /// or a `Known` step).
    pub(crate) fn entries_of(
        &mut self,
        path: &Path,
        stamp: FileStamp,
        tz: Tz,
    ) -> Vec<GameLogEntry> {
        self.parsed_entries(path, stamp, tz)
    }

    /// How many token lookups are kept.
    pub fn path_count(&self) -> usize {
        self.paths.len()
    }

    /// Make room for one more lookup: misses are dropped first, then all.
    fn make_room(&mut self) {
        if self.paths.len() < MAX_PATHS {
            return;
        }
        self.paths
            .retain(|_, lookup| matches!(lookup, Lookup::Found(_)));
        if self.paths.len() >= MAX_PATHS {
            self.paths.clear();
        }
    }

    /// How many files' entries are kept.
    pub fn parsed_count(&self) -> usize {
        self.parsed.len()
    }

    /// The file's entries: the kept ones while its stamp is unchanged, else
    /// read and parsed again. An unreadable file gives none and is not kept.
    fn parsed_entries(&mut self, path: &Path, stamp: FileStamp, tz: Tz) -> Vec<GameLogEntry> {
        if let Some(index) = self.parsed.iter().position(|kept| kept.path == path) {
            let kept = self.parsed.remove(index);
            if kept.stamp == stamp && kept.tz == tz {
                let entries = kept.entries.clone();
                // Now the most recently used.
                self.parsed.push(kept);
                return entries;
            }
        }
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(%error, "could not read a game log file");
                return Vec::new();
            }
        };
        let entries = match parse_game_log(&bytes, tz) {
            Some(log) => log.entries,
            None => Vec::new(),
        };
        if self.parsed.len() >= MAX_PARSED_FILES {
            self.parsed.remove(0);
        }
        self.parsed.push(ParsedLog {
            path: path.to_path_buf(),
            stamp,
            tz,
            entries: entries.clone(),
        });
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::time::Duration;

    use fixture_tools::{TestDir, load_game_log};

    const UTC: Tz = chrono_tz::UTC;
    const TOKEN: &str = "q-cache-token";
    const NOW: i64 = 1_800_000_000_000;

    /// A data folder holding one real game log file for `TOKEN`.
    fn with_game_log(label: &str) -> (TestDir, PathBuf, Vec<GameLogEntry>) {
        let dir = TestDir::new(label);
        let bytes = load_game_log("clean_2_0_win");
        let path = dir.write(&format!("data/Q/Match_GameLog_{TOKEN}.dat"), &bytes);
        let entries = parse_game_log(&bytes, UTC).expect("parses").entries;
        (dir, path, entries)
    }

    fn set_mtime(path: &Path, at: SystemTime) {
        let file = std::fs::File::options()
            .write(true)
            .open(path)
            .expect("open");
        file.set_modified(at).expect("set mtime");
    }

    #[test]
    fn a_second_lookup_does_not_search_again() {
        let (_dir, path, entries) = with_game_log("log-cache-hit");
        let mut cache = GameLogCache::new();
        let walks = Cell::new(0);
        let find = |_: &str| {
            walks.set(walks.get() + 1);
            Some(path.clone())
        };

        assert_eq!(cache.entries(None, TOKEN, UTC, NOW, find), entries);
        assert_eq!(cache.entries(None, TOKEN, UTC, NOW + 1, find), entries);
        assert_eq!(
            cache.entries(None, TOKEN, UTC, NOW + 10 * MISS_TTL_MS, find),
            entries
        );
        assert_eq!(walks.get(), 1);
    }

    #[test]
    fn a_missing_file_is_searched_again_after_the_miss_expires() {
        let (_dir, path, entries) = with_game_log("log-cache-miss");
        let mut cache = GameLogCache::new();
        let walks = Cell::new(0);
        let nothing = |_: &str| {
            walks.set(walks.get() + 1);
            None
        };

        assert!(cache.entries(None, TOKEN, UTC, NOW, nothing).is_empty());
        assert!(
            cache
                .entries(None, TOKEN, UTC, NOW + MISS_TTL_MS - 1, nothing)
                .is_empty()
        );
        assert_eq!(walks.get(), 1, "a fresh miss is trusted");

        // The file has appeared since (a live match).
        let found = |_: &str| {
            walks.set(walks.get() + 1);
            Some(path.clone())
        };
        assert_eq!(
            cache.entries(None, TOKEN, UTC, NOW + MISS_TTL_MS, found),
            entries
        );
        assert_eq!(walks.get(), 2);
    }

    #[test]
    fn a_clock_that_went_back_does_not_keep_a_miss() {
        let mut cache = GameLogCache::new();
        let walks = Cell::new(0);
        let nothing = |_: &str| {
            walks.set(walks.get() + 1);
            None
        };
        assert!(cache.entries(None, TOKEN, UTC, NOW, nothing).is_empty());
        assert!(
            cache
                .entries(None, TOKEN, UTC, NOW - 3_600_000, nothing)
                .is_empty()
        );
        assert_eq!(walks.get(), 2);
    }

    #[test]
    fn an_unchanged_file_is_not_parsed_again_and_a_changed_mtime_is() {
        let (_dir, path, entries) = with_game_log("log-cache-mtime");
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        set_mtime(&path, at);
        let mut cache = GameLogCache::new();
        let find = |_: &str| Some(path.clone());
        assert_eq!(cache.entries(None, TOKEN, UTC, NOW, find), entries);

        // Same length and mtime: the remembered entries are used, so the
        // new bytes are never read.
        let length = std::fs::metadata(&path).expect("meta").len() as usize;
        std::fs::write(&path, vec![0u8; length]).expect("rewrite");
        set_mtime(&path, at);
        assert_eq!(cache.entries(None, TOKEN, UTC, NOW, find), entries);

        // A new mtime: read and parsed again, so the new bytes show.
        set_mtime(&path, at + Duration::from_secs(5));
        assert_ne!(cache.entries(None, TOKEN, UTC, NOW, find), entries);
    }

    #[test]
    fn a_deleted_file_is_searched_for_again() {
        let (dir, path, entries) = with_game_log("log-cache-deleted");
        let mut cache = GameLogCache::new();
        let walks = Cell::new(0);
        let at_first = |_: &str| {
            walks.set(walks.get() + 1);
            Some(path.clone())
        };
        assert_eq!(cache.entries(None, TOKEN, UTC, NOW, at_first), entries);

        // MTGO moved it.
        let moved = dir.join(&format!("data/R/Match_GameLog_{TOKEN}.dat"));
        std::fs::create_dir_all(moved.parent().expect("parent")).expect("dir");
        std::fs::rename(&path, &moved).expect("move");
        let elsewhere = |_: &str| {
            walks.set(walks.get() + 1);
            Some(moved.clone())
        };
        assert_eq!(cache.entries(None, TOKEN, UTC, NOW, elsewhere), entries);
        assert_eq!(walks.get(), 2);

        // Gone for good: an empty log, remembered as a miss.
        std::fs::remove_file(&moved).expect("delete");
        let nothing = |_: &str| {
            walks.set(walks.get() + 1);
            None
        };
        assert!(cache.entries(None, TOKEN, UTC, NOW, nothing).is_empty());
        assert!(cache.entries(None, TOKEN, UTC, NOW + 1, nothing).is_empty());
        assert_eq!(walks.get(), 3);
    }

    #[test]
    fn a_new_data_folder_forgets_the_found_paths() {
        let (dir, path, entries) = with_game_log("log-cache-folder");
        let mut cache = GameLogCache::new();
        let walks = Cell::new(0);
        let find = |_: &str| {
            walks.set(walks.get() + 1);
            Some(path.clone())
        };
        let chosen = dir.join("data");
        assert_eq!(cache.entries(None, TOKEN, UTC, NOW, find), entries);
        assert_eq!(cache.entries(Some(&chosen), TOKEN, UTC, NOW, find), entries);
        assert_eq!(cache.entries(Some(&chosen), TOKEN, UTC, NOW, find), entries);
        assert_eq!(walks.get(), 2);
    }

    #[test]
    fn the_remembered_lookups_are_bounded() {
        let mut cache = GameLogCache::new();
        // Many tokens with no file: each miss is remembered, never past the bound.
        for index in 0..(MAX_PATHS * 2 + 5) {
            let token = format!("q-missing-{index}");
            assert!(
                cache
                    .entries(None, &token, UTC, NOW, |_: &str| None)
                    .is_empty()
            );
            assert!(cache.path_count() <= MAX_PATHS);
        }
        assert!(cache.path_count() > 0, "recent misses are still trusted");
    }

    #[test]
    fn only_the_most_recently_used_files_stay_parsed() {
        let dir = TestDir::new("log-cache-bound");
        let bytes = load_game_log("clean_2_0_win");
        let mut cache = GameLogCache::new();
        for index in 0..=MAX_PARSED_FILES {
            let token = format!("q-token-{index}");
            let path = dir.write(&format!("data/Match_GameLog_{token}.dat"), &bytes);
            assert!(
                !cache
                    .entries(None, &token, UTC, NOW, |_: &str| Some(path))
                    .is_empty()
            );
        }
        assert_eq!(cache.parsed_count(), MAX_PARSED_FILES);
    }

    /// Look `token` up the way `dat_entries` does: `begin`, a walk with no
    /// cache in hand, then `record`.
    fn find_with(
        cache: &mut GameLogCache,
        token: &str,
        found: Option<PathBuf>,
        now_ms: i64,
    ) -> Option<(PathBuf, FileStamp)> {
        match cache.begin(None, token, now_ms) {
            Step::Known(path, stamp) => Some((path, stamp)),
            Step::FreshMiss => None,
            Step::Search => cache.record(None, token, now_ms, found),
        }
    }

    #[test]
    fn found_paths_survive_a_flood_of_misses() {
        let (_dir, path, _entries) = with_game_log("log-cache-flood");
        let mut cache = GameLogCache::new();
        assert!(find_with(&mut cache, TOKEN, Some(path), NOW).is_some());
        for index in 0..(MAX_PATHS * 3) {
            let token = format!("q-missing-{index}");
            assert!(find_with(&mut cache, &token, None, NOW).is_none());
        }
        assert!(cache.path_count() <= MAX_PATHS);
        // Still known: no search is asked for.
        assert!(matches!(cache.begin(None, TOKEN, NOW + 1), Step::Known(..)));
    }

    #[test]
    fn a_table_full_of_found_paths_is_cleared_for_the_next_lookup() {
        let dir = TestDir::new("log-cache-all-found");
        let bytes = load_game_log("clean_2_0_win");
        let mut cache = GameLogCache::new();
        for index in 0..MAX_PATHS {
            let token = format!("q-token-{index}");
            let path = dir.write(&format!("data/Match_GameLog_{token}.dat"), &bytes);
            assert!(find_with(&mut cache, &token, Some(path), NOW).is_some());
        }
        assert_eq!(cache.path_count(), MAX_PATHS);
        // No miss to drop: everything goes, and the new lookup is kept.
        assert!(find_with(&mut cache, "q-one-more", None, NOW).is_none());
        assert_eq!(cache.path_count(), 1);
        assert!(matches!(cache.begin(None, "q-token-0", NOW), Step::Search));
    }

    #[test]
    fn a_search_is_recorded_for_the_asked_token_only() {
        let (_dir, path, _entries) = with_game_log("log-cache-requested-only");
        let mut cache = GameLogCache::new();
        // The walk passed other files; only the asked token is remembered.
        assert!(find_with(&mut cache, TOKEN, Some(path), NOW).is_some());
        assert_eq!(cache.path_count(), 1);
    }

    #[test]
    fn a_search_that_finishes_after_the_folder_changed_is_not_remembered() {
        let (dir, path, _entries) = with_game_log("log-cache-folder-race");
        let mut cache = GameLogCache::new();
        assert!(matches!(cache.begin(None, TOKEN, NOW), Step::Search));
        // Another request switched the data folder while the walk ran.
        let chosen = dir.join("data");
        assert!(matches!(
            cache.begin(Some(&chosen), "q-other", NOW),
            Step::Search
        ));
        let found = cache.record(None, TOKEN, NOW, Some(path));
        assert!(found.is_some(), "the caller still gets its file");
        assert_eq!(cache.path_count(), 0);
    }
}
