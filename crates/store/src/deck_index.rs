//! The deck index's archetype reads (archetypes spec section 12, 0.x
//! `BuildDeckSidebarOptions`, `BuildDeckArchetypeHeader`,
//! `GetArchetypeDeckVersionIds`): the sidebar's archetype rows and
//! "Unclassified" count, the header band of one archetype (or
//! Unclassified), and the deck versions behind the archetype tabs.
//!
//! Every read starts from the deck list's scope (`DeckScope`: the account,
//! no Limited decks, the format, archived decks only when shown), so the
//! sidebar, the header and the grid describe one set of decks.

use std::collections::BTreeMap;

use chrono_tz::Tz;
use decks::stats::{PlayedMatch, Range, Record, TREND_HISTORY, daily_results};
use rusqlite::{Connection, OptionalExtension, params};

use crate::StoreError;
pub use crate::deck_pages::DeckArchetypeFilter;
use crate::deck_pages::earlier_matches;
use crate::matchups::{MatchupRow, best_and_worst, matchup_spread};
use crate::names::stored_outcome;
use crate::platform::{Platform, deck_of_platform};

/// The decks the deck index describes (0.x `BuildDeckSidebarOptions::
/// scopedDecks`): the account's (every account's of the platform when
/// `None`), never Limited, of one format when given, archived ones only when
/// shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckScope {
    pub account_id: Option<i64>,
    pub platform: Platform,
    /// A raw format code (`CMODERN`).
    pub format: Option<String>,
    pub hide_archived: bool,
}

/// `DeckScope` over `decks d`, bound as `?1` (account), `?2` (format), `?3`
/// (1 hides archived decks) and `?4` (platform).
fn scope_sql() -> String {
    format!(
        "d.format <> 'Limited'
  AND (?1 IS NULL OR d.account_id = ?1)
  AND {of_platform}
  AND (?2 IS NULL OR d.format = ?2)
  AND (?3 = 0 OR d.archived_at IS NULL)",
        of_platform = deck_of_platform("d", 4)
    )
}

/// One archetype row of the sidebar (0.x `DeckArchetypeOptionData`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckArchetypeOption {
    pub id: i64,
    pub name: String,
    /// Canonical comma form.
    pub color_identity: Option<String>,
    /// The archetypes format key (`modern`); `None` for a fallback.
    pub format: Option<String>,
    pub deck_count: u32,
    /// The decks' listed complete matches, draws counted as played.
    pub record: Record,
    /// When the last of those matches started; `None` if never played.
    pub last_played_at: Option<i64>,
    /// The player's own archetype, not MyMTGO's (a row the offline
    /// download holds as Custom is MyMTGO's).
    pub custom: bool,
}

/// The sidebar's archetype rows: every archetype a deck in scope has, with
/// the decks and their complete matches (0.x
/// `BuildDeckSidebarOptions::archetypeOptions`). The one played last comes
/// first; ties and never-played ones go by name in any case, then format
/// (0.x sorted by name only).
pub fn archetype_options(
    conn: &Connection,
    scope: &DeckScope,
) -> Result<Vec<DeckArchetypeOption>, StoreError> {
    let sql = format!(
        "WITH per_version AS (
    SELECT m.deck_version_id AS version_id, COUNT(*) AS total,
           SUM(m.outcome = 'win') AS wins, SUM(m.outcome = 'loss') AS losses,
           MAX(m.started_at) AS last_played
    FROM listed_matches m
    WHERE m.state = 'complete' AND m.deck_version_id IS NOT NULL
    GROUP BY m.deck_version_id)
SELECT a.id, a.name, a.color_identity, a.format, COUNT(DISTINCT d.id),
       COALESCE(SUM(pv.wins), 0), COALESCE(SUM(pv.losses), 0), COALESCE(SUM(pv.total), 0),
       MAX(pv.last_played), a.source = 'manual' AND a.offline_held = 0
FROM decks d
JOIN archetypes a ON a.id = d.archetype_id
LEFT JOIN deck_versions v ON v.deck_id = d.id
LEFT JOIN per_version pv ON pv.version_id = v.id
WHERE {in_scope}
GROUP BY a.id",
        in_scope = scope_sql()
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(bind(scope), |row| {
        let colors: Option<String> = row.get(2)?;
        Ok(DeckArchetypeOption {
            id: row.get(0)?,
            name: row.get(1)?,
            color_identity: archetypes::normalize_colors(colors.as_deref()),
            format: row.get(3)?,
            deck_count: row.get(4)?,
            record: Record::from_total(row.get(5)?, row.get(6)?, row.get(7)?),
            last_played_at: row.get(8)?,
            custom: row.get(9)?,
        })
    })?;
    let mut options = Vec::new();
    for row in rows {
        options.push(row?);
    }
    options.sort_by(|a, b| {
        // Newest first; `None` (never played) sorts below every time.
        b.last_played_at
            .cmp(&a.last_played_at)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| {
                let a_format = a.format.as_deref().unwrap_or("").to_lowercase();
                let b_format = b.format.as_deref().unwrap_or("").to_lowercase();
                a_format.cmp(&b_format)
            })
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(options)
}

/// The scope's parameters.
fn bind(scope: &DeckScope) -> (Option<i64>, Option<String>, i64, Platform) {
    (
        scope.account_id,
        scope.format.clone(),
        i64::from(scope.hide_archived),
        scope.platform,
    )
}

/// The decks in scope without an archetype (0.x `unclassifiedCount`).
pub fn unclassified_count(conn: &Connection, scope: &DeckScope) -> Result<u32, StoreError> {
    let sql = format!(
        "SELECT COUNT(*) FROM decks d WHERE {} AND d.archetype_id IS NULL",
        scope_sql()
    );
    Ok(conn
        .prepare_cached(&sql)?
        .query_row(bind(scope), |row| row.get(0))?)
}

/// The archetype a header names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderArchetype {
    pub id: i64,
    pub name: String,
    pub color_identity: Option<String>,
    /// The archetypes format key; `None` for a fallback.
    pub format: Option<String>,
    /// Homebrew or Rogue: no matchup tabs (0.x).
    pub is_fallback: bool,
}

/// The header band of one archetype or Unclassified (0.x
/// `DeckArchetypeHeaderData`).
#[derive(Debug, Clone, PartialEq)]
pub struct ArchetypeHeader {
    /// `None` for Unclassified.
    pub archetype: Option<HeaderArchetype>,
    pub deck_count: u32,
    /// The decks' complete matches in the range asked for (the tab's
    /// timeframe; all time gives the sidebar row's record).
    pub record: Record,
    /// In the range, every version (none for Unclassified).
    pub best: Option<MatchupRow>,
    pub worst: Option<MatchupRow>,
}

/// The ids of the decks in scope under `filter`, by id.
fn deck_ids_in_scope(
    conn: &Connection,
    scope: &DeckScope,
    filter: DeckArchetypeFilter,
) -> Result<Vec<i64>, StoreError> {
    let (mode, id) = match filter {
        DeckArchetypeFilter::Unclassified => ("none", None),
        DeckArchetypeFilter::Is(id) => ("id", Some(id)),
    };
    let sql = format!(
        "SELECT d.id FROM decks d
WHERE {in_scope}
  AND ((?5 = 'none' AND d.archetype_id IS NULL) OR (?5 = 'id' AND d.archetype_id = ?6))
ORDER BY d.id",
        in_scope = scope_sql()
    );
    let (account, format, hide, platform) = bind(scope);
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(params![account, format, hide, platform, mode, id], |row| {
        row.get(0)
    })?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row?);
    }
    Ok(ids)
}

/// Every version of the decks given.
fn versions_of_decks(conn: &Connection, deck_ids: &[i64]) -> Result<Vec<i64>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id FROM deck_versions WHERE deck_id IN (SELECT value FROM json_each(?1))
         ORDER BY id",
    )?;
    let rows = stmt.query_map([crate::matchups::ids_json(deck_ids)], |row| row.get(0))?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row?);
    }
    Ok(ids)
}

/// The complete listed matches of these versions that started in `range`.
fn record_in_range(
    conn: &Connection,
    version_ids: &[i64],
    range: Range,
) -> Result<Record, StoreError> {
    let (wins, losses, total): (u32, u32, u32) = conn
        .prepare_cached(
            "SELECT COALESCE(SUM(m.outcome = 'win'), 0), COALESCE(SUM(m.outcome = 'loss'), 0),
                    COUNT(*)
             FROM listed_matches m
             WHERE m.deck_version_id IN (SELECT value FROM json_each(?1))
               AND m.state = 'complete' AND m.started_at BETWEEN ?2 AND ?3",
        )?
        .query_row(
            params![
                crate::matchups::ids_json(version_ids),
                range.from_ms,
                range.to_ms
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
    Ok(Record::from_total(wins, losses, total))
}

/// The header of `filter` over the decks in scope, counting the matches
/// that started in `range` (0.x `BuildDeckArchetypeHeader::run`); `None`
/// when no deck in scope matches. `deck_count` is the decks in scope, not
/// the ones with a match in the range.
pub fn archetype_header(
    conn: &Connection,
    scope: &DeckScope,
    filter: DeckArchetypeFilter,
    range: Range,
) -> Result<Option<ArchetypeHeader>, StoreError> {
    let deck_ids = deck_ids_in_scope(conn, scope, filter)?;
    if deck_ids.is_empty() {
        return Ok(None);
    }
    let versions = versions_of_decks(conn, &deck_ids)?;
    let record = record_in_range(conn, &versions, range)?;
    let (archetype, best, worst) = match filter {
        DeckArchetypeFilter::Unclassified => (None, None, None),
        DeckArchetypeFilter::Is(id) => {
            let archetype = conn
                .prepare_cached(
                    "SELECT id, name, color_identity, format, source = 'fallback'
                     FROM archetypes WHERE id = ?1",
                )?
                .query_row([id], |row| {
                    let colors: Option<String> = row.get(2)?;
                    Ok(HeaderArchetype {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        color_identity: archetypes::normalize_colors(colors.as_deref()),
                        format: row.get(3)?,
                        is_fallback: row.get(4)?,
                    })
                })
                .optional()?;
            let spread = matchup_spread(conn, &versions, range, None)?;
            let (best, worst) = best_and_worst(&spread);
            (archetype, best, worst)
        }
    };
    Ok(Some(ArchetypeHeader {
        archetype,
        deck_count: deck_ids.len() as u32,
        record,
        best,
        worst,
    }))
}

/// One local day of a trend line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrendDay {
    /// `YYYY-MM-DD`, the local day in the timezone given.
    pub date: String,
    pub record: Record,
    /// The win rate trend after the day's last match (`daily_results`).
    pub trend: Option<u32>,
}

/// One deck of an archetype and its results by day in a range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckTrend {
    pub deck_id: i64,
    /// The deck's position among every deck of the archetype in scope, by
    /// id, whether or not it has a match in the range: a stable line style.
    pub slot: u32,
    pub deck_name: String,
    /// The matches in the range.
    pub record: Record,
    /// The days with a match, oldest first.
    pub days: Vec<TrendDay>,
}

/// The archetype Trends tab's lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchetypeTrends {
    /// Every deck's matches together, by day, oldest first.
    pub overall: Vec<TrendDay>,
    /// The decks with a match in the range: the most played first, then by
    /// deck id.
    pub decks: Vec<DeckTrend>,
}

/// A deck's complete listed matches in a range (`?2` to `?3`), over every
/// version, with the deck's name.
const TREND_MATCHES: &str = "
SELECT d.id, d.name, m.started_at, m.outcome
FROM decks d
JOIN deck_versions v ON v.deck_id = d.id
JOIN listed_matches m ON m.deck_version_id = v.id
WHERE d.id IN (SELECT value FROM json_each(?1))
  AND m.state = 'complete' AND m.started_at BETWEEN ?2 AND ?3";

/// Days with a match, as the lines draw them.
fn trend_days(earlier: &[PlayedMatch], matches: &[PlayedMatch], tz: Tz) -> Vec<TrendDay> {
    daily_results(earlier, matches, tz)
        .into_iter()
        .filter(|day| day.win_rate.is_some())
        .map(|day| TrendDay {
            date: day.day,
            record: Record::from_counts(day.wins, day.losses, day.draws),
            trend: day.trend,
        })
        .collect()
}

/// The archetype's decks in scope with their complete listed matches in
/// `range` by local day in `tz` (as `matchups::peer_chart` groups them), and
/// the same days for all of them together. Each line's trend is fed by the
/// latest `TREND_HISTORY` matches before the range of its own scope: a
/// deck's own matches, or every deck's of the archetype in scope for
/// Overall (the newest of those lie among each deck's newest).
/// Decks without a match in the range are left out (their slots stay
/// taken); the most played come first, then by deck id.
pub fn archetype_trends(
    conn: &Connection,
    scope: &DeckScope,
    archetype_id: i64,
    range: Range,
    tz: Tz,
) -> Result<ArchetypeTrends, StoreError> {
    let deck_ids = deck_ids_in_scope(conn, scope, DeckArchetypeFilter::Is(archetype_id))?;
    let mut stmt = conn.prepare_cached(TREND_MATCHES)?;
    let rows = stmt.query_map(
        params![
            crate::matchups::ids_json(&deck_ids),
            range.from_ms,
            range.to_ms
        ],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                PlayedMatch {
                    started_at_ms: row.get(2)?,
                    outcome: stored_outcome(&row.get::<_, String>(3)?),
                },
            ))
        },
    )?;
    let mut by_deck: BTreeMap<i64, (String, Vec<PlayedMatch>)> = BTreeMap::new();
    let mut all_in_range = Vec::new();
    for row in rows {
        let (deck_id, name, played) = row?;
        all_in_range.push(played.clone());
        by_deck
            .entry(deck_id)
            .or_insert((name, Vec::new()))
            .1
            .push(played);
    }

    let mut all_earlier = Vec::new();
    let mut deck_earlier: BTreeMap<i64, Vec<PlayedMatch>> = BTreeMap::new();
    if !all_in_range.is_empty() {
        for deck_id in &deck_ids {
            let earlier = earlier_matches(conn, *deck_id, None, range.from_ms, None)?;
            all_earlier.extend(earlier.iter().cloned());
            deck_earlier.insert(*deck_id, earlier);
        }
    }
    all_earlier.sort_by_key(|played| played.started_at_ms);
    let excess = all_earlier.len().saturating_sub(TREND_HISTORY as usize);
    all_earlier.drain(..excess);
    let overall = trend_days(&all_earlier, &all_in_range, tz);

    let mut decks = Vec::new();
    for (deck_id, (deck_name, matches)) in by_deck {
        let earlier = deck_earlier.remove(&deck_id).unwrap_or_default();
        let days = trend_days(&earlier, &matches, tz);
        let record = days
            .iter()
            .fold(Record::default(), |sum, day| sum.plus(day.record));
        let slot = deck_ids.iter().position(|id| *id == deck_id).unwrap_or(0) as u32;
        decks.push(DeckTrend {
            deck_id,
            slot,
            deck_name,
            record,
            days,
        });
    }
    decks.sort_by(|a, b| {
        b.record
            .total
            .cmp(&a.record.total)
            .then_with(|| a.deck_id.cmp(&b.deck_id))
    });
    Ok(ArchetypeTrends { overall, decks })
}

/// Every version of the archetype's decks in scope: what the archetype
/// tabs read (0.x `GetArchetypeDeckVersionIds`).
pub fn archetype_version_ids(
    conn: &Connection,
    scope: &DeckScope,
    archetype_id: i64,
) -> Result<Vec<i64>, StoreError> {
    let deck_ids = deck_ids_in_scope(conn, scope, DeckArchetypeFilter::Is(archetype_id))?;
    versions_of_decks(conn, &deck_ids)
}

/// The deck a listed match was played with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowDeck {
    pub token: String,
    pub deck_id: i64,
    pub deck_name: String,
}

/// The decks of the matches with these tokens (a table's Deck column);
/// matches without a deck are left out.
pub fn decks_of_matches(conn: &Connection, tokens: &[String]) -> Result<Vec<RowDeck>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT m.token, d.id, d.name FROM matches m
         JOIN deck_versions v ON v.id = m.deck_version_id
         JOIN decks d ON d.id = v.deck_id
         WHERE m.token = ?1",
    )?;
    let mut found = Vec::new();
    for token in tokens {
        if let Some(row) = stmt
            .query_row([token], |row| {
                Ok(RowDeck {
                    token: row.get(0)?,
                    deck_id: row.get(1)?,
                    deck_name: row.get(2)?,
                })
            })
            .optional()?
        {
            found.push(row);
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deck_pages::{DeckListFilter, DeckSort, deck_list};
    use crate::matchups::seed::*;

    /// 2026-03-15T12:00:00Z.
    const MARCH_15: i64 = 1_773_576_000_000;

    fn everything() -> DeckScope {
        DeckScope {
            account_id: None,
            platform: Platform::Mtgo,
            format: None,
            hide_archived: true,
        }
    }

    fn modern() -> DeckScope {
        DeckScope {
            format: Some("CMODERN".to_string()),
            ..everything()
        }
    }

    #[test]
    fn the_sidebar_reads_one_platform() {
        let (_dir, conn) = db("deck-index-arena");
        let burn = archetype(&conn, "Qburn", "modern", Some("R"));
        played_deck(&conn, Some(burn), "CMODERN", [1, 0, 0]);
        played_deck(&conn, None, "CMODERN", [1, 0, 0]);
        let arena = played_deck(&conn, Some(burn), "CMODERN", [0, 2, 0]);
        let arena_plain = played_deck(&conn, None, "CMODERN", [0, 1, 0]);
        crate::test_support::move_deck_to_arena(&conn, arena);
        crate::test_support::move_deck_to_arena(&conn, arena_plain);

        let options = archetype_options(&conn, &everything()).expect("options");
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].deck_count, 1);
        assert_eq!(options[0].record.total, 1);
        assert_eq!(unclassified_count(&conn, &everything()).expect("count"), 1);
        let header = archetype_header(
            &conn,
            &everything(),
            DeckArchetypeFilter::Is(burn),
            Range::all_time(),
        )
        .expect("header")
        .expect("some");
        assert_eq!(header.deck_count, 1);

        let arena_scope = DeckScope {
            platform: Platform::Arena,
            ..everything()
        };
        let options = archetype_options(&conn, &arena_scope).expect("options");
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].deck_count, 1);
        assert_eq!(options[0].record.total, 2);
        assert_eq!(unclassified_count(&conn, &arena_scope).expect("count"), 1);
    }

    /// A deck with one version and `won`, `lost` and `drawn` complete
    /// matches.
    fn played_deck(
        conn: &Connection,
        archetype: Option<i64>,
        format: &str,
        results: [u32; 3],
    ) -> i64 {
        let deck = deck(conn, None, format, archetype);
        let version = version(conn, deck);
        for (outcome, times) in ["win", "loss", "draw"].into_iter().zip(results) {
            for _ in 0..times {
                complete(conn, version, outcome, MARCH_15);
            }
        }
        deck
    }

    // 0.x BuildDeckSidebarOptionsTest "aggregates deck count and match
    // record per archetype, draws included in total".
    #[test]
    fn an_archetype_row_adds_up_its_decks_and_their_matches() {
        let (_dir, conn) = db("index-options");
        let qtron = archetype(&conn, "Qeldrazi tron", "modern", Some("G"));
        played_deck(&conn, Some(qtron), "CMODERN", [6, 4, 0]);
        played_deck(&conn, Some(qtron), "CMODERN", [1, 1, 2]);

        let options = archetype_options(&conn, &everything()).expect("options");
        assert_eq!(options.len(), 1);
        let row = &options[0];
        assert_eq!(row.id, qtron);
        assert_eq!(row.name, "Qeldrazi tron");
        assert_eq!(row.color_identity.as_deref(), Some("G"));
        assert_eq!(row.format.as_deref(), Some("modern"));
        assert_eq!(row.deck_count, 2);
        assert_eq!(
            (
                row.record.wins,
                row.record.losses,
                row.record.draws,
                row.record.total
            ),
            (7, 5, 2, 14)
        );
        assert_eq!(row.record.win_rate, 50);
    }

    // A player's own archetype can share its name with MyMTGO's: the row
    // says which is which (a row the offline download holds is MyMTGO's).
    #[test]
    fn an_archetype_row_says_whether_it_is_the_players_own() {
        let (_dir, conn) = db("index-options-custom");
        let own = archetype(&conn, "Qtron", "modern", Some("U"));
        let server = archetype(&conn, "Qtron", "modern", Some("U"));
        let held = archetype(&conn, "Qheld", "modern", None);
        conn.execute(
            "UPDATE archetypes SET source = 'downloaded' WHERE id = ?1",
            [server],
        )
        .expect("server");
        conn.execute(
            "UPDATE archetypes SET offline_held = 1 WHERE id = ?1",
            [held],
        )
        .expect("held");
        for id in [own, server, held] {
            deck(&conn, None, "CMODERN", Some(id));
        }
        let mut custom: Vec<(i64, bool)> = archetype_options(&conn, &everything())
            .expect("options")
            .into_iter()
            .map(|row| (row.id, row.custom))
            .collect();
        custom.sort();
        assert_eq!(custom, vec![(own, true), (server, false), (held, false)]);
    }

    // "keeps archetypes whose decks have no matches and only counts
    // complete matches".
    #[test]
    fn an_archetype_without_complete_matches_keeps_its_row() {
        let (_dir, conn) = db("index-options-empty");
        let qburn = archetype(&conn, "Qburn", "modern", None);
        let version = version(&conn, deck(&conn, None, "CMODERN", Some(qburn)));
        played(&conn, version, "in_progress", "win", MARCH_15);

        let row = &archetype_options(&conn, &everything()).expect("options")[0];
        assert_eq!(row.deck_count, 1);
        assert_eq!(row.record.total, 0);
        assert_eq!(row.record.win_rate, 0);
    }

    // "sorts archetypes alphabetically, then by format for same-named
    // archetypes".
    #[test]
    fn archetype_rows_sort_by_name_any_case_then_format() {
        let (_dir, conn) = db("index-options-sort");
        let zoo = archetype(&conn, "Qzoo", "modern", None);
        let affinity = archetype(&conn, "qaffinity", "modern", None);
        let tron_pioneer = archetype(&conn, "Qtron", "pioneer", None);
        let tron_modern = archetype(&conn, "Qtron", "modern", None);
        for (archetype, format) in [
            (zoo, "CMODERN"),
            (affinity, "CMODERN"),
            (tron_pioneer, "CPIONEER"),
            (tron_modern, "CMODERN"),
        ] {
            deck(&conn, None, format, Some(archetype));
        }

        let rows: Vec<String> = archetype_options(&conn, &everything())
            .expect("options")
            .into_iter()
            .map(|row| format!("{}/{}", row.name, row.format.unwrap_or_default()))
            .collect();
        assert_eq!(
            rows,
            [
                "qaffinity/modern",
                "Qtron/modern",
                "Qtron/pioneer",
                "Qzoo/modern"
            ]
        );
    }

    // v1: the archetype whose decks were played last comes first; one
    // never played goes after every played one, by name.
    #[test]
    fn archetype_rows_sort_by_their_decks_last_complete_match() {
        let (_dir, conn) = db("index-options-recent");
        let qaffinity = archetype(&conn, "Qaffinity", "modern", None);
        let qburn = archetype(&conn, "Qburn", "modern", None);
        let qtron = archetype(&conn, "Qtron", "modern", None);
        let qzoo = archetype(&conn, "Qzoo", "modern", None);
        let burn = version(&conn, deck(&conn, None, "CMODERN", Some(qburn)));
        complete(&conn, burn, "win", MARCH_15);
        // Qtron's newest match is in its second deck.
        let tron_old = version(&conn, deck(&conn, None, "CMODERN", Some(qtron)));
        complete(&conn, tron_old, "win", MARCH_15 - 1_000);
        let tron_new = version(&conn, deck(&conn, None, "CMODERN", Some(qtron)));
        complete(&conn, tron_new, "loss", MARCH_15 + 1_000);
        // An unfinished match does not count as played.
        let zoo = version(&conn, deck(&conn, None, "CMODERN", Some(qzoo)));
        played(&conn, zoo, "in_progress", "win", MARCH_15 + 5_000);
        deck(&conn, None, "CMODERN", Some(qaffinity));

        let rows: Vec<String> = archetype_options(&conn, &everything())
            .expect("options")
            .into_iter()
            .map(|row| row.name)
            .collect();
        assert_eq!(rows, ["Qtron", "Qburn", "Qaffinity", "Qzoo"]);
    }

    // "respects the format filter and excludes limited decks from archetype
    // options".
    #[test]
    fn archetype_rows_keep_to_the_format_and_leave_out_limited_decks() {
        let (_dir, conn) = db("index-options-format");
        let qmodern = archetype(&conn, "Qmodern tron", "modern", None);
        let qlegacy = archetype(&conn, "Qlegacy tron", "legacy", None);
        deck(&conn, None, "CMODERN", Some(qmodern));
        deck(&conn, None, "CLEGACY", Some(qlegacy));
        deck(&conn, None, "Limited", Some(qmodern));

        let options = archetype_options(&conn, &modern()).expect("options");
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].name, "Qmodern tron");
        assert_eq!(options[0].deck_count, 1);
    }

    // "counts unclassified decks within the format and archived scope".
    #[test]
    fn unclassified_counts_decks_without_an_archetype_in_scope() {
        let (_dir, conn) = db("index-unclassified");
        deck(&conn, None, "CMODERN", None);
        deck(&conn, None, "CMODERN", None);
        deck(&conn, None, "CPAUPER", None);
        let archived = deck(&conn, None, "CMODERN", None);
        archive(&conn, archived);
        let qburn = archetype(&conn, "Qburn", "modern", None);
        deck(&conn, None, "CMODERN", Some(qburn));

        assert_eq!(unclassified_count(&conn, &everything()).expect("count"), 3);
        assert_eq!(unclassified_count(&conn, &modern()).expect("count"), 2);
        let with_archived = DeckScope {
            hide_archived: false,
            ..modern()
        };
        assert_eq!(unclassified_count(&conn, &with_archived).expect("count"), 3);
    }

    /// A complete match of `version` against `opponent` with one game.
    fn seed_matchup(conn: &Connection, version: i64, opponent: i64, outcome: &str) {
        let result = if outcome == "win" { "win" } else { "loss" };
        against(
            conn,
            version,
            opponent,
            outcome,
            MARCH_15,
            &[g(result, true)],
        );
    }

    // 0.x BuildDeckArchetypeHeaderTest "returns null when no archetype filter
    // is set" (the filter is never empty here) and "returns null for an
    // archetype with no decks in scope".
    #[test]
    fn an_archetype_without_decks_in_scope_has_no_header() {
        let (_dir, conn) = db("index-header-none");
        let qburn = archetype(&conn, "Qburn", "modern", None);
        assert_eq!(
            archetype_header(
                &conn,
                &everything(),
                DeckArchetypeFilter::Is(qburn),
                Range::all_time()
            )
            .expect("header"),
            None
        );
    }

    // "reports the same record and deck count as the sidebar row".
    #[test]
    fn the_header_has_the_sidebar_rows_record() {
        let (_dir, conn) = db("index-header-record");
        let mine = archetype(&conn, "Qesper blink", "modern", None);
        let opponent = archetype(&conn, "Qopponent", "modern", None);
        let one = version(&conn, deck(&conn, None, "CMODERN", Some(mine)));
        let two = version(&conn, deck(&conn, None, "CMODERN", Some(mine)));
        seed_matchup(&conn, one, opponent, "win");
        seed_matchup(&conn, one, opponent, "loss");
        seed_matchup(&conn, two, opponent, "draw");

        let header = archetype_header(
            &conn,
            &everything(),
            DeckArchetypeFilter::Is(mine),
            Range::all_time(),
        )
        .expect("header")
        .expect("decks");
        let sidebar = archetype_options(&conn, &everything()).expect("options");
        let row = sidebar.iter().find(|row| row.id == mine).expect("row");
        assert_eq!(
            header.archetype.as_ref().map(|a| a.name.as_str()),
            Some("Qesper blink")
        );
        assert_eq!(header.deck_count, 2);
        assert_eq!(header.record, row.record);
        assert_eq!(header.record.total, 3);
        assert_eq!(header.record.wins, 1);
    }

    // "picks best and worst matchups by win rate with a three match floor".
    #[test]
    fn the_header_names_the_best_and_worst_matchups() {
        let (_dir, conn) = db("index-header-matchups");
        let mine = archetype(&conn, "Qmine", "modern", None);
        let good = archetype(&conn, "Qboros energy", "modern", None);
        let bad = archetype(&conn, "Qtron", "modern", None);
        let thin = archetype(&conn, "Qthin sample", "modern", None);
        let version = version(&conn, deck(&conn, None, "CMODERN", Some(mine)));
        for outcome in ["win", "win", "win", "loss"] {
            seed_matchup(&conn, version, good, outcome);
        }
        for outcome in ["win", "loss", "loss", "loss"] {
            seed_matchup(&conn, version, bad, outcome);
        }
        for outcome in ["win", "win"] {
            seed_matchup(&conn, version, thin, outcome);
        }

        let header = archetype_header(
            &conn,
            &everything(),
            DeckArchetypeFilter::Is(mine),
            Range::all_time(),
        )
        .expect("header")
        .expect("decks");
        let best = header.best.expect("best");
        assert_eq!(best.name, "Qboros energy");
        assert_eq!(best.record.win_rate, 75);
        assert_eq!(best.record.total, 4);
        let worst = header.worst.expect("worst");
        assert_eq!(worst.name, "Qtron");
        assert_eq!(worst.record.win_rate, 25);
    }

    // "returns null matchups when fewer than two opponents qualify".
    #[test]
    fn one_qualifying_matchup_names_neither() {
        let (_dir, conn) = db("index-header-one");
        let mine = archetype(&conn, "Qmine", "modern", None);
        let only = archetype(&conn, "Qonly", "modern", None);
        let version = version(&conn, deck(&conn, None, "CMODERN", Some(mine)));
        for outcome in ["win", "win", "loss"] {
            seed_matchup(&conn, version, only, outcome);
        }
        let header = archetype_header(
            &conn,
            &everything(),
            DeckArchetypeFilter::Is(mine),
            Range::all_time(),
        )
        .expect("header")
        .expect("decks");
        assert_eq!((header.best, header.worst), (None, None));
    }

    // "builds an unclassified header with record and no matchups".
    #[test]
    fn the_unclassified_header_has_a_record_and_no_matchups() {
        let (_dir, conn) = db("index-header-unclassified");
        let opponent = archetype(&conn, "Qopponent", "modern", None);
        let version = version(&conn, deck(&conn, None, "CMODERN", None));
        deck(&conn, None, "CMODERN", None);
        for outcome in ["win", "win", "win", "loss"] {
            seed_matchup(&conn, version, opponent, outcome);
        }
        let header = archetype_header(
            &conn,
            &everything(),
            DeckArchetypeFilter::Unclassified,
            Range::all_time(),
        )
        .expect("header")
        .expect("decks");
        assert_eq!(header.archetype, None);
        assert_eq!(header.deck_count, 2);
        assert_eq!(header.record.total, 4);
        assert_eq!((header.best, header.worst), (None, None));
    }

    // "respects the format and archived scope".
    #[test]
    fn the_header_keeps_to_the_format_and_archived_scope() {
        let (_dir, conn) = db("index-header-scope");
        let mine = archetype(&conn, "Qmine", "modern", None);
        deck(&conn, None, "CMODERN", Some(mine));
        deck(&conn, None, "CLEGACY", Some(mine));
        let archived = deck(&conn, None, "CMODERN", Some(mine));
        archive(&conn, archived);

        let count = |scope: &DeckScope| {
            archetype_header(
                &conn,
                scope,
                DeckArchetypeFilter::Is(mine),
                Range::all_time(),
            )
            .expect("header")
            .expect("decks")
            .deck_count
        };
        assert_eq!(count(&modern()), 1);
        let with_archived = DeckScope {
            hide_archived: false,
            ..modern()
        };
        assert_eq!(count(&with_archived), 2);
        assert_eq!(count(&everything()), 2);
    }

    #[test]
    fn a_fallback_header_says_so() {
        let (_dir, conn) = db("index-header-fallback");
        let homebrew = homebrew(&conn);
        deck(&conn, None, "CMODERN", Some(homebrew));
        let header = archetype_header(
            &conn,
            &everything(),
            DeckArchetypeFilter::Is(homebrew),
            Range::all_time(),
        )
        .expect("header")
        .expect("decks");
        assert!(header.archetype.expect("archetype").is_fallback);
    }

    // 0.x GetArchetypeDeckVersionIdsTest, three cases.
    #[test]
    fn the_tab_versions_are_every_version_of_the_archetypes_decks_in_scope() {
        let (_dir, conn) = db("index-versions");
        let mine = archetype(&conn, "Qmine", "modern", None);
        let deck_a = deck(&conn, None, "CMODERN", Some(mine));
        let deck_b = deck(&conn, None, "CMODERN", Some(mine));
        let other = deck(&conn, None, "CMODERN", None);
        let a1 = version(&conn, deck_a);
        let a2 = version(&conn, deck_a);
        let b1 = version(&conn, deck_b);
        version(&conn, other);
        let no_hiding = DeckScope {
            hide_archived: false,
            ..everything()
        };
        let mut ids = archetype_version_ids(&conn, &no_hiding, mine).expect("ids");
        ids.sort_unstable();
        assert_eq!(ids, [a1, a2, b1]);

        // "drops archived decks when hide-archived is on".
        archive(&conn, deck_b);
        let mut live = archetype_version_ids(&conn, &everything(), mine).expect("ids");
        live.sort_unstable();
        assert_eq!(live, [a1, a2]);
        assert_eq!(
            archetype_version_ids(&conn, &no_hiding, mine)
                .expect("ids")
                .len(),
            3
        );

        // "respects the format filter".
        let pioneer = DeckScope {
            format: Some("CPIONEER".to_string()),
            ..no_hiding
        };
        assert!(
            archetype_version_ids(&conn, &pioneer, mine)
                .expect("ids")
                .is_empty()
        );
    }

    #[test]
    fn a_rows_deck_is_found_by_match_token() {
        let (_dir, conn) = db("index-row-decks");
        let deck_id = deck(&conn, None, "CMODERN", None);
        let version = version(&conn, deck_id);
        let id = complete(&conn, version, "win", MARCH_15);
        let token = token_of(&conn, id);
        let found =
            decks_of_matches(&conn, &[token.clone(), "missing".to_string()]).expect("decks");
        assert_eq!(
            found,
            [RowDeck {
                token,
                deck_id,
                deck_name: "Qdeck 1".to_string(),
            }]
        );
    }

    fn list_filter() -> DeckListFilter {
        DeckListFilter {
            account_id: None,
            platform: Platform::Mtgo,
            format: None,
            recent: false,
            search: None,
            hide_archived: true,
            sort: DeckSort::Name,
            page: 1,
            per_page: 12,
            archetype: None,
        }
    }

    // 0.x IndexController: `none` lists the unclassified decks, an id that
    // archetype's decks, and a stale id (no deck of it in scope) no filter.
    // Each deck carries its archetype.
    #[test]
    fn the_deck_list_filters_by_archetype_and_shows_each_decks_archetype() {
        let (_dir, conn) = db("index-list-archetype");
        let qburn = archetype(&conn, "Qburn", "modern", Some("R"));
        let qtron = archetype(&conn, "Qtron", "pioneer", None);
        let burn = deck(&conn, None, "CMODERN", Some(qburn));
        let bare = deck(&conn, None, "CMODERN", None);
        deck(&conn, None, "CPIONEER", Some(qtron));

        let page = deck_list(
            &conn,
            &DeckListFilter {
                archetype: Some(DeckArchetypeFilter::Is(qburn)),
                ..list_filter()
            },
        )
        .expect("list");
        let ids: Vec<i64> = page.decks.iter().map(|deck| deck.id).collect();
        assert_eq!(ids, [burn]);
        assert_eq!(page.archetype, Some(DeckArchetypeFilter::Is(qburn)));
        let shown = page.decks[0].archetype.as_ref().expect("archetype");
        assert_eq!((shown.id, shown.name.as_str()), (qburn, "Qburn"));
        assert_eq!(shown.color_identity.as_deref(), Some("R"));

        let page = deck_list(
            &conn,
            &DeckListFilter {
                archetype: Some(DeckArchetypeFilter::Unclassified),
                ..list_filter()
            },
        )
        .expect("list");
        let ids: Vec<i64> = page.decks.iter().map(|deck| deck.id).collect();
        assert_eq!(ids, [bare]);
        assert_eq!(page.decks[0].archetype, None);

        // Pioneer's archetype is stale in a Modern list: the filter drops.
        let page = deck_list(
            &conn,
            &DeckListFilter {
                format: Some("CMODERN".to_string()),
                archetype: Some(DeckArchetypeFilter::Is(qtron)),
                ..list_filter()
            },
        )
        .expect("list");
        assert_eq!(page.decks.len(), 2);
        assert_eq!(page.archetype, None);
    }

    /// 0.x ArchetypeTabsTest's seed: two decks of one archetype, each with a
    /// complete won match against the same opponent, and a deck without one.
    fn seed_tabs(conn: &Connection) -> (i64, Vec<i64>, Vec<i64>, i64) {
        let mine = archetype(conn, "Qeldrazi tron", "modern", None);
        let opponent = archetype(conn, "Qrakdos midrange", "modern", None);
        let mut decks = Vec::new();
        let mut matches = Vec::new();
        for _ in 0..2 {
            let deck_id = deck(conn, None, "CMODERN", Some(mine));
            let version = version(conn, deck_id);
            matches.push(against(
                conn,
                version,
                opponent,
                "win",
                MARCH_15,
                &[g("win", true)],
            ));
            decks.push(deck_id);
        }
        let other = version(conn, deck(conn, None, "CMODERN", None));
        against(conn, other, opponent, "win", MARCH_15, &[g("win", true)]);
        (mine, decks, matches, opponent)
    }

    // 0.x ArchetypeTabsTest: the matches tab lists every deck's matches,
    // filters by result and timeframe, and leaves archived decks out when
    // they are hidden; the matchups tab spreads every deck; the card stats
    // tab counts every deck's games.
    #[test]
    fn the_archetype_tabs_read_every_deck_of_the_archetype() {
        use crate::card_stats::{CardStatsFilter, card_stats_of_versions, deck_prior_of_versions};
        use crate::match_list::{MatchListFilter, ResultFilter, Scope, match_list};
        use decks::stats::Range;

        let (_dir, conn) = db("index-tabs");
        let (mine, decks, matches, opponent) = seed_tabs(&conn);
        let ids = archetype_version_ids(&conn, &everything(), mine).expect("ids");
        let list = |ids: &[i64], filter: &MatchListFilter| {
            match_list(&conn, Scope::Versions(ids.to_vec()), filter, 1, 50).expect("list")
        };

        let all = MatchListFilter::newest_first(Range::all_time());
        assert_eq!(list(&ids, &all).total, 2);
        let rows = list(&ids, &all).rows;
        let tokens: Vec<String> = rows.iter().map(|row| row.token.clone()).collect();
        let deck_ids: Vec<i64> = decks_of_matches(&conn, &tokens)
            .expect("decks")
            .into_iter()
            .map(|row| row.deck_id)
            .collect();
        assert!(deck_ids.iter().all(|id| decks.contains(id)));

        conn.execute(
            "UPDATE matches SET outcome = 'loss' WHERE id = ?1",
            [matches[0]],
        )
        .expect("loss");
        let losses = MatchListFilter {
            result: Some(ResultFilter::Loss),
            ..MatchListFilter::newest_first(Range::all_time())
        };
        let found = list(&ids, &losses);
        assert_eq!(found.total, 1);
        assert_eq!(found.rows[0].token, token_of(&conn, matches[0]));

        let week = MatchListFilter::newest_first(Range {
            from_ms: MARCH_15 + 1,
            to_ms: MARCH_15 + 7 * 86_400_000,
        });
        conn.execute(
            "UPDATE matches SET started_at = started_at + 1000 WHERE id = ?1",
            [matches[1]],
        )
        .expect("move");
        assert_eq!(list(&ids, &week).total, 1);

        let spread =
            crate::matchups::matchup_spread(&conn, &ids, Range::all_time(), None).expect("spread");
        assert_eq!(spread.len(), 1);
        assert_eq!(spread[0].archetype_id, opponent);
        assert_eq!(spread[0].record.total, 2);

        let prior = deck_prior_of_versions(&conn, &ids, &CardStatsFilter::all()).expect("prior");
        assert_eq!(prior.games, 2);
        assert!(
            card_stats_of_versions(&conn, &ids, &CardStatsFilter::all())
                .expect("stats")
                .is_empty()
        );

        archive(&conn, decks[0]);
        let live = archetype_version_ids(&conn, &everything(), mine).expect("ids");
        assert_eq!(list(&live, &all).total, 1);
        let header = archetype_header(
            &conn,
            &everything(),
            DeckArchetypeFilter::Is(mine),
            Range::all_time(),
        )
        .expect("header")
        .expect("decks");
        assert_eq!(header.deck_count, 1);
    }

    /// A day later than the one before, in March 2026 (UTC midday).
    fn march(day: i64) -> i64 {
        MARCH_15 + (day - 15) * 86_400_000
    }

    #[test]
    fn the_header_counts_only_matches_in_range() {
        let (_dir, conn) = db("index-header-range");
        let mine = archetype(&conn, "Qmine", "modern", None);
        let good = archetype(&conn, "Qgood", "modern", None);
        let bad = archetype(&conn, "Qbad", "modern", None);
        let first = version(&conn, deck(&conn, None, "CMODERN", Some(mine)));
        let second = version(&conn, deck(&conn, None, "CMODERN", Some(mine)));
        let play = |version: i64, opponent: i64, outcome: &str, day: i64| {
            let result = if outcome == "win" { "win" } else { "loss" };
            against(
                &conn,
                version,
                opponent,
                outcome,
                march(day),
                &[g(result, true)],
            );
        };
        // Before the range: four wins against bad, four losses against good.
        for day in [1, 2, 3, 4] {
            play(first, bad, "win", day);
        }
        for day in [5, 6, 7, 8] {
            play(second, good, "loss", day);
        }
        // In the range: three wins against good, three losses against bad.
        for day in [11, 12, 13] {
            play(first, good, "win", day);
        }
        for day in [14, 15, 16] {
            play(second, bad, "loss", day);
        }
        let window = Range {
            from_ms: march(10),
            to_ms: march(20),
        };

        let all = archetype_header(
            &conn,
            &everything(),
            DeckArchetypeFilter::Is(mine),
            Range::all_time(),
        )
        .expect("header")
        .expect("decks");
        assert_eq!(all.record.total, 14);
        let ranged = archetype_header(&conn, &everything(), DeckArchetypeFilter::Is(mine), window)
            .expect("header")
            .expect("decks");
        assert_eq!(ranged.deck_count, 2);
        assert_eq!(
            (
                ranged.record.wins,
                ranged.record.losses,
                ranged.record.total
            ),
            (3, 3, 6)
        );
        assert_eq!(ranged.record.win_rate, 50);
        assert_eq!(ranged.best.expect("best").name, "Qgood");
        assert_eq!(ranged.worst.expect("worst").name, "Qbad");
        let all_best = all.best.expect("best");
        assert_eq!(all_best.name, "Qbad");
        assert_eq!(all_best.record.total, 7);
    }

    #[test]
    fn a_range_without_matches_keeps_the_header_with_an_empty_record() {
        let (_dir, conn) = db("index-header-empty-range");
        let mine = archetype(&conn, "Qmine", "modern", None);
        let v = version(&conn, deck(&conn, None, "CMODERN", Some(mine)));
        complete(&conn, v, "win", march(1));
        let window = Range {
            from_ms: march(10),
            to_ms: march(20),
        };
        let header = archetype_header(&conn, &everything(), DeckArchetypeFilter::Is(mine), window)
            .expect("header")
            .expect("decks");
        assert_eq!(header.deck_count, 1);
        assert_eq!(header.record.total, 0);
        assert_eq!((header.best, header.worst), (None, None));
    }

    #[test]
    fn trends_group_each_deck_by_local_day() {
        let (_dir, conn) = db("index-trends");
        let mine = archetype(&conn, "Qmine", "modern", None);
        let other = archetype(&conn, "Qother", "modern", None);
        let busy = deck(&conn, None, "CMODERN", Some(mine));
        let busy_version = version(&conn, busy);
        let quiet = deck(&conn, None, "CMODERN", Some(mine));
        let quiet_version = version(&conn, quiet);
        let idle = deck(&conn, None, "CMODERN", Some(mine));
        let idle_version = version(&conn, idle);
        let elsewhere = version(&conn, deck(&conn, None, "CMODERN", Some(other)));

        // 2026-09-08T03:00:00Z is 20:00 on the 7th in Los Angeles.
        let late_night = 1_788_836_400_000;
        let next_morning = late_night + 15 * 3_600_000;
        let two_days_on = late_night + 57 * 3_600_000;
        complete(&conn, busy_version, "win", late_night);
        complete(&conn, busy_version, "loss", late_night + 60_000);
        complete(&conn, busy_version, "win", next_morning);
        complete(&conn, quiet_version, "loss", next_morning);
        complete(&conn, quiet_version, "win", two_days_on);
        complete(&conn, busy_version, "win", two_days_on);
        complete(&conn, elsewhere, "win", next_morning);
        // Out of range, not complete, hidden: none count.
        complete(&conn, busy_version, "win", 1_788_220_800_000 - 86_400_000);
        played(&conn, busy_version, "in_progress", "win", next_morning);
        let hidden = complete(&conn, busy_version, "win", next_morning);
        conn.execute("UPDATE matches SET hidden_at = 1 WHERE id = ?1", [hidden])
            .expect("hide");
        complete(&conn, idle_version, "win", 1_788_220_800_000 - 86_400_000);

        let september = Range {
            from_ms: 1_788_220_800_000,
            to_ms: 1_790_812_800_000,
        };
        let trends = archetype_trends(
            &conn,
            &everything(),
            mine,
            september,
            chrono_tz::America::Los_Angeles,
        )
        .expect("trends")
        .decks;

        assert_eq!(trends.len(), 2);
        assert_eq!((trends[0].deck_id, trends[1].deck_id), (busy, quiet));
        assert_eq!(trends[0].record.total, 4);
        assert_eq!(trends[1].record.total, 2);
        let busy_days: Vec<(&str, u32, u32)> = trends[0]
            .days
            .iter()
            .map(|day| (day.date.as_str(), day.record.wins, day.record.losses))
            .collect();
        assert_eq!(
            busy_days,
            vec![
                ("2026-09-07", 1, 1),
                ("2026-09-08", 1, 0),
                ("2026-09-10", 1, 0)
            ]
        );
        let quiet_days: Vec<&str> = trends[1].days.iter().map(|day| day.date.as_str()).collect();
        assert_eq!(quiet_days, vec!["2026-09-08", "2026-09-10"]);
    }

    #[test]
    fn trend_slots_count_decks_without_a_match_in_the_range() {
        let (_dir, conn) = db("index-trends-slots");
        let mine = archetype(&conn, "Qmine", "modern", None);
        let idle = deck(&conn, None, "CMODERN", Some(mine));
        let first = deck(&conn, None, "CMODERN", Some(mine));
        let first_version = version(&conn, first);
        let second = deck(&conn, None, "CMODERN", Some(mine));
        let second_version = version(&conn, second);
        let day = 1_788_836_400_000;
        complete(&conn, second_version, "win", day);
        complete(&conn, first_version, "win", day);
        complete(&conn, first_version, "loss", day + 60_000);
        assert!(idle < first);

        let range = Range {
            from_ms: 1_788_220_800_000,
            to_ms: 1_790_812_800_000,
        };
        let trends = archetype_trends(&conn, &everything(), mine, range, chrono_tz::UTC)
            .expect("trends")
            .decks;

        let slots: Vec<(i64, u32)> = trends.iter().map(|t| (t.deck_id, t.slot)).collect();
        assert_eq!(slots, vec![(first, 1), (second, 2)]);
    }

    #[test]
    fn trends_with_equal_totals_come_in_deck_id_order() {
        let (_dir, conn) = db("index-trends-ties");
        let mine = archetype(&conn, "Qmine", "modern", None);
        let first = deck(&conn, None, "CMODERN", Some(mine));
        let first_version = version(&conn, first);
        let second = deck(&conn, None, "CMODERN", Some(mine));
        let second_version = version(&conn, second);
        let day = 1_788_836_400_000;
        // The higher id is seeded first, so insertion order cannot explain
        // the result.
        complete(&conn, second_version, "win", day);
        complete(&conn, second_version, "loss", day + 60_000);
        complete(&conn, first_version, "win", day);
        complete(&conn, first_version, "loss", day + 60_000);

        let range = Range {
            from_ms: 1_788_220_800_000,
            to_ms: 1_790_812_800_000,
        };
        let trends = archetype_trends(&conn, &everything(), mine, range, chrono_tz::UTC)
            .expect("trends")
            .decks;

        assert_eq!(trends.len(), 2);
        assert_eq!(trends[0].record.total, trends[1].record.total);
        assert_eq!((trends[0].deck_id, trends[1].deck_id), (first, second));
    }

    #[test]
    fn earlier_matches_of_the_same_scope_move_each_lines_first_trend() {
        let (_dir, conn) = db("index-trends-earlier");
        let mine = archetype(&conn, "Qmine", "modern", None);
        let other = archetype(&conn, "Qother", "modern", None);
        let first = deck(&conn, None, "CMODERN", Some(mine));
        let first_version = version(&conn, first);
        let second = deck(&conn, None, "CMODERN", Some(mine));
        let second_version = version(&conn, second);
        let idle = version(&conn, deck(&conn, None, "CMODERN", Some(mine)));
        let stranger = version(&conn, deck(&conn, None, "CMODERN", Some(other)));
        let window = Range {
            from_ms: MARCH_15 - 10 * 86_400_000,
            to_ms: MARCH_15 + 86_400_000,
        };

        complete(&conn, first_version, "win", MARCH_15);
        complete(&conn, second_version, "win", MARCH_15 + 60_000);
        let read = || {
            archetype_trends(&conn, &everything(), mine, window, chrono_tz::UTC).expect("trends")
        };
        let alone = read();
        assert_eq!(alone.overall[0].trend, Some(100));
        assert_eq!(alone.decks[0].days[0].trend, Some(100));

        // Before the window: a loss of the first deck, a loss of a deck with
        // no match in the window, and a loss of another archetype.
        let before = MARCH_15 - 40 * 86_400_000;
        complete(&conn, first_version, "loss", before);
        complete(&conn, idle, "loss", before + 60_000);
        complete(&conn, stranger, "loss", before + 120_000);

        let trends = read();
        // Overall: two earlier losses, then two wins.
        let overall = &trends.overall;
        assert_eq!(overall.len(), 1);
        assert_eq!(overall[0].record.total, 2, "earlier matches are not days");
        let keep = 1.0 - 2.0 / 51.0_f64;
        let weights = [keep.powi(3), keep.powi(2), keep, 1.0];
        let expected = (100.0 * (weights[2] + weights[3]) / weights.iter().sum::<f64>()).round();
        assert_eq!(overall[0].trend, Some(expected as u32));
        // The first deck's own loss then win: 51. The second deck has none.
        let by_deck: Vec<(i64, Option<u32>)> = trends
            .decks
            .iter()
            .map(|deck| (deck.deck_id, deck.days[0].trend))
            .collect();
        assert_eq!(by_deck, vec![(first, Some(51)), (second, Some(100))]);
    }

    #[test]
    fn a_trend_day_without_a_deck_in_the_range_has_no_lines() {
        let (_dir, conn) = db("index-trends-none");
        let mine = archetype(&conn, "Qmine", "modern", None);
        let v = version(&conn, deck(&conn, None, "CMODERN", Some(mine)));
        complete(&conn, v, "win", MARCH_15 - 40 * 86_400_000);
        let window = Range {
            from_ms: MARCH_15,
            to_ms: MARCH_15 + 86_400_000,
        };
        let trends =
            archetype_trends(&conn, &everything(), mine, window, chrono_tz::UTC).expect("trends");
        assert!(trends.overall.is_empty());
        assert!(trends.decks.is_empty());
    }

    #[test]
    fn the_trend_queries_walk_indexes_not_matches() {
        let (_dir, conn) = db("index-trends-plans");
        for (name, sql) in [
            ("in range", TREND_MATCHES.to_string()),
            (
                "earlier",
                crate::deck_pages::DAILY_EARLIER_MATCHES.to_string(),
            ),
        ] {
            let mut stmt = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .expect("explain");
            let params = vec![1i64; stmt.parameter_count()];
            let query: Vec<String> = stmt
                .query_map(rusqlite::params_from_iter(params), |row| {
                    row.get::<_, String>(3)
                })
                .expect("rows")
                .map(|row| row.expect("row"))
                .collect();
            let query = query.join("\n");
            assert!(!query.contains("SCAN m"), "{name} plan:\n{query}");
        }
    }
}
