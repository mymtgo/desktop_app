//! The last pushed view of each match (spec 6): the list command serves
//! in-progress matches from memory, so a refetch never shows older data than a
//! push. Storage may be a batch behind the projector; revisions decide.

use std::collections::{HashMap, HashSet};

use store::match_list::{MatchListFilter, Scope, match_list};
use store::{Connection, Platform, StoreError};

use crate::platform::PlatformView;
use crate::summary::{MatchSummary, SummaryState};

/// Rows per page of the matches list.
pub const PER_PAGE: u32 = 50;

enum LiveEntry {
    /// Boxed: a summary is far larger than `Removed` (clippy `large_enum_variant`).
    Shown(Box<MatchSummary>),
    /// Discarded by the tracker, or removed from stats by the player; kept
    /// out of the list until storage deletes or hides the row too.
    Removed,
}

#[derive(Default)]
pub struct LiveMatches {
    entries: HashMap<String, LiveEntry>,
}

/// A match no longer in progress can be served from storage once storage
/// has caught up. `Ended` counts: MTGO may never log the completion, and a
/// later change is pushed again anyway (#49).
fn is_settled(state: SummaryState) -> bool {
    matches!(
        state,
        SummaryState::Ended | SummaryState::Complete | SummaryState::Abandoned
    )
}

impl LiveMatches {
    /// Pushes arrive in order, so the latest always replaces what is held.
    pub fn update(&mut self, summary: MatchSummary) {
        self.entries
            .insert(summary.token.clone(), LiveEntry::Shown(Box::new(summary)));
    }

    /// The last pushed view of this match was complete.
    pub fn is_complete(&self, token: &str) -> bool {
        matches!(
            self.entries.get(token),
            Some(LiveEntry::Shown(summary)) if summary.state == SummaryState::Complete
        )
    }

    /// The last pushed view of this match was shown and not yet complete.
    pub fn is_unfinished(&self, token: &str) -> bool {
        matches!(
            self.entries.get(token),
            Some(LiveEntry::Shown(summary)) if summary.state != SummaryState::Complete
        )
    }

    pub fn remove(&mut self, token: &str) {
        self.entries.insert(token.to_string(), LiveEntry::Removed);
    }

    pub fn mark_attention(&mut self, token: &str, reason: &str) {
        if let Some(LiveEntry::Shown(summary)) = self.entries.get_mut(token) {
            summary.needs_attention = true;
            summary.attention_reason = Some(reason.to_string());
        }
    }

    /// The tracker restarted: revisions may repeat, so trust storage again.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Lay live views over one page of stored rows of `platform`. A live
    /// view replaces its row when its revision is at least as new; a removed
    /// match is hidden. The first page also shows live matches of `platform`
    /// storage has not written yet.
    pub fn merge(
        &self,
        stored: Vec<MatchSummary>,
        first_page: bool,
        per_page: u32,
        platform: Platform,
    ) -> Vec<MatchSummary> {
        let platform = PlatformView::from(platform);
        let full = stored.len() >= per_page as usize;
        let oldest_ms = stored.last().map(|row| row.started_at_ms);
        let mut on_page = HashSet::new();
        let mut rows = Vec::new();
        for row in stored {
            on_page.insert(row.token.clone());
            match self.entries.get(&row.token) {
                Some(LiveEntry::Removed) => {}
                Some(LiveEntry::Shown(live)) if live.revision >= row.revision => {
                    // A push carries no id, note or archetype: keep the stored ones.
                    let mut shown = MatchSummary::clone(live);
                    shown.id = row.id;
                    shown.notes = row.notes;
                    shown.opponent_archetype = row.opponent_archetype;
                    rows.push(shown);
                }
                _ => rows.push(row),
            }
        }
        if first_page {
            let mut live_only = Vec::new();
            for entry in self.entries.values() {
                let LiveEntry::Shown(live) = entry else {
                    continue;
                };
                if on_page.contains(&live.token) || live.platform != platform {
                    continue;
                }
                let belongs = !full || oldest_ms.is_none_or(|oldest| live.started_at_ms > oldest);
                if belongs {
                    live_only.push(MatchSummary::clone(live));
                }
            }
            // The map has no order, so fix one: matches starting in the same
            // millisecond then sort the same way on every call.
            live_only.sort_by(|a, b| a.token.cmp(&b.token));
            rows.extend(live_only);
            // Stable sort on start time only: stored rows that tie keep the
            // SQL order (`id DESC`), so refetches never swap them.
            rows.sort_by_key(|row| std::cmp::Reverse(row.started_at_ms));
        }
        rows
    }

    /// Tokens whose entry can go once storage agrees: finished matches and
    /// removals.
    pub fn settled_tokens(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|(_, entry)| match entry {
                LiveEntry::Removed => true,
                LiveEntry::Shown(summary) => is_settled(summary.state),
            })
            .map(|(token, _)| token.clone())
            .collect()
    }

    /// Drop an entry storage has caught up with. `stored_revision` is the
    /// stored row's revision, `None` if there is no row; `hidden` says the
    /// player removed the match from stats, so the lists leave its row out.
    pub fn forget_stored(&mut self, token: &str, stored_revision: Option<u64>, hidden: bool) {
        let caught_up = match self.entries.get(token) {
            Some(LiveEntry::Removed) => stored_revision.is_none() || hidden,
            Some(LiveEntry::Shown(summary)) => {
                is_settled(summary.state)
                    && stored_revision.is_some_and(|stored| stored >= summary.revision)
            }
            None => false,
        };
        if caught_up {
            self.entries.remove(token);
        }
    }
}

/// One stored page (1-based; 0 counts as 1) of the main list of
/// `platform`'s matches through `filter`, and the total row count. A row the
/// list cannot show is logged and skipped.
pub fn read_page(
    conn: &Connection,
    platform: Platform,
    filter: &MatchListFilter,
    page: u32,
) -> Result<(Vec<MatchSummary>, u32), StoreError> {
    let found = match_list(conn, Scope::All(platform), filter, page, PER_PAGE)?;
    let total = found.total;
    Ok((crate::matches::page_view(found).rows, total))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summary::testing::{summary, view};
    use crate::summary::{SummaryState, from_view};
    use fixture_tools::TestDir;
    use mtgo_match::MatchState;
    use store::open_writer;
    use store::write::{MatchWrite, WriteBatch, commit};

    use SummaryState::{Abandoned, Complete, Ended, InProgress};

    fn tokens(rows: &[MatchSummary]) -> Vec<&str> {
        rows.iter().map(|row| row.token.as_str()).collect()
    }

    #[test]
    fn a_stored_row_older_than_the_live_view_is_replaced() {
        let mut live = LiveMatches::default();
        let mut newer = summary("t1", 1_000, 5, InProgress);
        newer.wins = 2;
        live.update(newer.clone());
        let merged = live.merge(
            vec![summary("t1", 1_000, 4, InProgress)],
            true,
            PER_PAGE,
            Platform::Mtgo,
        );
        assert_eq!(merged, vec![newer]);
    }

    #[test]
    fn a_live_view_keeps_the_stored_row_s_note_and_id() {
        let mut live = LiveMatches::default();
        live.update(summary("t1", 1_000, 5, InProgress));
        let mut stored = summary("t1", 1_000, 4, InProgress);
        stored.notes = Some("Q note".to_string());
        stored.id = Some(7);
        let merged = live.merge(vec![stored], true, PER_PAGE, Platform::Mtgo);
        assert_eq!(merged[0].revision, 5);
        assert_eq!(merged[0].notes.as_deref(), Some("Q note"));
        assert_eq!(merged[0].id, Some(7));
    }

    #[test]
    fn a_stored_row_newer_than_the_live_view_wins() {
        let mut live = LiveMatches::default();
        live.update(summary("t1", 1_000, 3, InProgress));
        let stored = summary("t1", 1_000, 4, Complete);
        assert_eq!(
            live.merge(vec![stored.clone()], true, PER_PAGE, Platform::Mtgo),
            vec![stored]
        );
    }

    #[test]
    fn a_removed_match_stays_hidden_until_storage_catches_up() {
        let mut live = LiveMatches::default();
        live.update(summary("t1", 1_000, 2, InProgress));
        live.remove("t1");
        let stored = vec![
            summary("t1", 1_000, 2, InProgress),
            summary("t2", 900, 1, Complete),
        ];
        assert_eq!(
            tokens(&live.merge(stored, true, PER_PAGE, Platform::Mtgo)),
            vec!["t2"]
        );
        // Storage still has the row: keep hiding it.
        live.forget_stored("t1", Some(2), false);
        assert_eq!(live.settled_tokens(), vec!["t1".to_string()]);
        // Storage deleted it: the tombstone is no longer needed.
        live.forget_stored("t1", None, false);
        assert!(live.is_empty());
    }

    #[test]
    fn a_removed_match_storage_hides_is_forgotten_though_its_row_stays() {
        let mut live = LiveMatches::default();
        live.update(summary("t1", 1_000, 2, Complete));
        live.remove("t1");
        live.forget_stored("t1", Some(2), false);
        assert_eq!(live.len(), 1, "still listed: keep hiding it");
        live.forget_stored("t1", Some(2), true);
        assert!(live.is_empty(), "lists leave a hidden match out");

        // A live view is forgotten by its revision, hidden or not.
        live.update(summary("t2", 1_000, 3, Complete));
        live.forget_stored("t2", Some(2), true);
        assert_eq!(live.len(), 1, "storage is behind");
        live.forget_stored("t2", Some(3), true);
        assert!(live.is_empty());
    }

    #[test]
    fn the_first_page_shows_live_matches_storage_has_not_written_yet() {
        let mut live = LiveMatches::default();
        live.update(summary("new", 5_000, 1, InProgress));
        live.update(summary("old", 10, 1, InProgress));
        let stored = vec![
            summary("a", 3_000, 1, Complete),
            summary("b", 2_000, 1, Complete),
        ];

        // A full first page: only live matches newer than its oldest row join it.
        let full = live.merge(stored.clone(), true, 2, Platform::Mtgo);
        assert_eq!(tokens(&full), vec!["new", "a", "b"]);
        // A short first page is the whole list: every live match joins it.
        let short = live.merge(stored.clone(), true, PER_PAGE, Platform::Mtgo);
        assert_eq!(tokens(&short), vec!["new", "a", "b", "old"]);
        // Later pages never add live matches.
        assert_eq!(
            tokens(&live.merge(stored, false, 2, Platform::Mtgo)),
            vec!["a", "b"]
        );
    }

    #[test]
    fn live_rows_stay_on_their_platform() {
        let mut live = LiveMatches::default();
        live.update(summary("mtgo", 5_000, 1, InProgress));
        let mut arena = summary("arena", 6_000, 1, InProgress);
        arena.platform = PlatformView::Arena;
        live.update(arena);
        let mtgo_page = live.merge(Vec::new(), true, PER_PAGE, Platform::Mtgo);
        assert_eq!(tokens(&mtgo_page), vec!["mtgo"]);
        let arena_page = live.merge(Vec::new(), true, PER_PAGE, Platform::Arena);
        assert_eq!(tokens(&arena_page), vec!["arena"]);
    }

    #[test]
    fn live_only_matches_starting_together_are_ordered_by_token() {
        let mut live = LiveMatches::default();
        for n in (0..20).rev() {
            live.update(summary(&format!("t{n:02}"), 5_000, 1, InProgress));
        }
        let merged = live.merge(Vec::new(), true, PER_PAGE, Platform::Mtgo);
        let expected: Vec<String> = (0..20).map(|n| format!("t{n:02}")).collect();
        assert_eq!(tokens(&merged), expected);
    }

    #[test]
    fn restarting_forgets_every_live_view() {
        let mut live = LiveMatches::default();
        live.update(summary("t1", 1_000, 9, InProgress));
        live.remove("t2");
        live.clear();
        assert!(live.is_empty());
        let stored = summary("t1", 1_000, 4, InProgress);
        assert_eq!(
            live.merge(vec![stored.clone()], true, PER_PAGE, Platform::Mtgo),
            vec![stored]
        );
    }

    #[test]
    fn settled_matches_are_forgotten_once_storage_has_them() {
        let mut live = LiveMatches::default();
        live.update(summary("done", 1_000, 4, Complete));
        live.update(summary("gone", 900, 2, Abandoned));
        live.update(summary("playing", 800, 7, InProgress));
        let mut settled = live.settled_tokens();
        settled.sort();
        assert_eq!(settled, vec!["done".to_string(), "gone".to_string()]);

        live.forget_stored("done", Some(3), false);
        assert_eq!(live.len(), 3, "storage is behind: keep the live view");
        live.forget_stored("done", Some(4), false);
        live.forget_stored("gone", Some(2), false);
        live.forget_stored("playing", Some(7), false);
        assert_eq!(live.len(), 1, "in-progress matches always stay live");
    }

    /// An ended match (no completion signal yet) is not in progress: once
    /// storage has its revision it is forgotten too, so it does not stay in
    /// memory until a restart when MTGO never logs its completion (#49).
    #[test]
    fn an_ended_match_is_forgotten_once_storage_has_it() {
        let mut live = LiveMatches::default();
        live.update(summary("ended", 1_000, 5, Ended));
        assert_eq!(live.settled_tokens(), vec!["ended".to_string()]);
        live.forget_stored("ended", Some(4), false);
        assert_eq!(live.len(), 1, "storage is behind: keep the live view");
        live.forget_stored("ended", Some(5), false);
        assert!(live.is_empty());
    }

    #[test]
    fn attention_marks_the_live_view() {
        let mut live = LiveMatches::default();
        live.update(summary("t1", 1_000, 3, InProgress));
        live.mark_attention("t1", "segments and game ids disagree");
        live.mark_attention("unknown", "ignored: nothing live to mark");
        let merged = live.merge(
            vec![summary("t1", 1_000, 3, InProgress)],
            true,
            PER_PAGE,
            Platform::Mtgo,
        );
        assert!(merged[0].needs_attention);
        assert_eq!(
            merged[0].attention_reason.as_deref(),
            Some("segments and game ids disagree")
        );
    }

    #[test]
    fn a_page_reads_stored_rows_and_the_total() {
        let dir = TestDir::new("live-page");
        let mut conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        let matches = (0..3)
            .map(|n| MatchWrite::Upsert {
                view: Box::new(view(&format!("t{n}"), 1_000 * n, MatchState::Complete)),
                revision: 1,
            })
            .collect();
        commit(
            &mut conn,
            &WriteBatch {
                matches,
                ..WriteBatch::default()
            },
        )
        .expect("commit");
        let all = MatchListFilter::newest_first(decks::stats::Range::all_time());
        let (rows, total) = read_page(&conn, Platform::Mtgo, &all, 1).expect("page");
        assert_eq!(total, 3);
        assert_eq!(tokens(&rows), vec!["t2", "t1", "t0"]);
        // A stored row carries its id; a pushed view has none.
        assert_eq!(rows[0].id, Some(3));
        assert_eq!(
            MatchSummary {
                id: None,
                ..rows[0].clone()
            },
            from_view(
                &view("t2", 2_000, MatchState::Complete),
                1,
                store::Platform::Mtgo,
                None
            )
        );
        let (none, _) = read_page(&conn, Platform::Mtgo, &all, 2).expect("page 2");
        assert!(none.is_empty());
        // Page 0 is treated as page 1.
        assert_eq!(
            read_page(&conn, Platform::Mtgo, &all, 0)
                .expect("page 0")
                .0
                .len(),
            3
        );
    }
}
