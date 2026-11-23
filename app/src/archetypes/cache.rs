//! The in-memory caches of the archetypes API answers (archetypes spec
//! sections 9 and 10): the website's metagame shares per format and window
//! (an hour), an archetype's live decklists per uuid (10 minutes) and the
//! server's archetype list per format key for the picker (an hour, #489). They
//! are filled on the `archetypes` worker; the pages read them. Nothing is
//! stored on disk. Like Laravel's `Cache::remember` with the array driver.
//!
//! A failed fetch is remembered for `FAILURE_TTL_MS`, so a page that keeps
//! asking does not send a request on every read. The clock is passed in.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use mymtgo_api::{ArchetypeDeck, CatalogArchetype, MetagameShares};

/// Global shares are kept an hour per format and window.
pub const METAGAME_TTL_MS: i64 = 3_600_000;
/// Live decklists are kept 10 minutes per uuid.
pub const DECKLISTS_TTL_MS: i64 = 600_000;
/// The picker's server catalog is kept an hour per format key.
pub const CATALOG_TTL_MS: i64 = 3_600_000;
/// A failed fetch is not asked again for a minute.
pub const FAILURE_TTL_MS: i64 = 60_000;

/// What the cache holds for a key.
#[derive(Debug, Clone, PartialEq)]
pub enum Cached<T> {
    /// A fresh answer.
    Fresh(T),
    /// The last fetch failed, recently.
    Failed,
    /// A fetch is on its way.
    Pending,
    /// Nothing, or too old.
    Missing,
}

struct Entry<T> {
    stored_at_ms: i64,
    answer: Option<T>,
}

/// Answers by key, each kept `ttl_ms` (a failure `FAILURE_TTL_MS`), and the
/// keys a fetch is on its way for.
pub struct TimedCache<T: Clone> {
    ttl_ms: i64,
    entries: HashMap<String, Entry<T>>,
    pending: HashSet<String>,
}

impl<T: Clone> TimedCache<T> {
    pub fn new(ttl_ms: i64) -> TimedCache<T> {
        TimedCache {
            ttl_ms,
            entries: HashMap::new(),
            pending: HashSet::new(),
        }
    }

    /// What is held for `key` at `now_ms`.
    pub fn get(&self, key: &str, now_ms: i64) -> Cached<T> {
        if let Some(entry) = self.entries.get(key) {
            let age = now_ms - entry.stored_at_ms;
            match &entry.answer {
                Some(answer) if age < self.ttl_ms => return Cached::Fresh(answer.clone()),
                None if age < FAILURE_TTL_MS => return Cached::Failed,
                _ => {}
            }
        }
        if self.pending.contains(key) {
            return Cached::Pending;
        }
        Cached::Missing
    }

    /// Marks a fetch on its way; false when one already is.
    pub fn begin(&mut self, key: &str) -> bool {
        self.pending.insert(key.to_string())
    }

    /// Stores an answer (`None` for a failure) and ends the pending fetch.
    pub fn store(&mut self, key: &str, answer: Option<T>, now_ms: i64) {
        self.pending.remove(key);
        self.entries.insert(
            key.to_string(),
            Entry {
                stored_at_ms: now_ms,
                answer,
            },
        );
    }
}

/// The key of one format's window.
pub fn metagame_key(slug: &str, days: u32) -> String {
    format!("{slug}:{days}")
}

/// The caches, managed by Tauri.
pub struct ArchetypeCaches {
    pub metagame: Mutex<TimedCache<MetagameShares>>,
    pub decklists: Mutex<TimedCache<Vec<ArchetypeDeck>>>,
    pub catalog: Mutex<TimedCache<Vec<CatalogArchetype>>>,
}

impl Default for ArchetypeCaches {
    fn default() -> ArchetypeCaches {
        ArchetypeCaches {
            metagame: Mutex::new(TimedCache::new(METAGAME_TTL_MS)),
            decklists: Mutex::new(TimedCache::new(DECKLISTS_TTL_MS)),
            catalog: Mutex::new(TimedCache::new(CATALOG_TTL_MS)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    #[test]
    fn global_shares_expire_after_an_hour() {
        let mut cache: TimedCache<u32> = TimedCache::new(METAGAME_TTL_MS);
        assert_eq!(cache.get("modern:30", NOW), Cached::Missing);
        assert!(cache.begin("modern:30"));
        assert!(!cache.begin("modern:30"), "one fetch at a time");
        assert_eq!(cache.get("modern:30", NOW), Cached::Pending);
        cache.store("modern:30", Some(7), NOW);
        assert_eq!(cache.get("modern:30", NOW + 3_599_999), Cached::Fresh(7));
        assert_eq!(cache.get("modern:30", NOW + 3_600_000), Cached::Missing);
        assert_eq!(cache.get("modern:7", NOW), Cached::Missing);
    }

    #[test]
    fn decklists_expire_after_ten_minutes() {
        let mut cache: TimedCache<u32> = TimedCache::new(DECKLISTS_TTL_MS);
        cache.store("q-uuid", Some(1), NOW);
        assert_eq!(cache.get("q-uuid", NOW + 599_999), Cached::Fresh(1));
        assert_eq!(cache.get("q-uuid", NOW + 600_000), Cached::Missing);
    }

    #[test]
    fn a_failure_is_remembered_for_a_minute() {
        let mut cache: TimedCache<u32> = TimedCache::new(METAGAME_TTL_MS);
        cache.begin("modern:30");
        cache.store("modern:30", None, NOW);
        assert_eq!(cache.get("modern:30", NOW + 59_999), Cached::Failed);
        assert_eq!(cache.get("modern:30", NOW + 60_000), Cached::Missing);
        assert!(cache.begin("modern:30"), "asked again after the minute");
    }
}
