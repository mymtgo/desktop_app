//! Read queries for the Decks pages (decks spec sections 7 and 8): the deck
//! list, a deck's header and version rows, its dashboard numbers, its
//! decklist and its matches. The number rules themselves are pure and live
//! in `decks::stats`; this module reads the rows they need.
//!
//! Counting rules (0.x `Deck::matches`, `MatchRecord`): only `complete`
//! matches count; a match belongs to a deck through the deck version it
//! links to (`matches.deck_version_id`).

use std::collections::HashMap;

use cards::standout::{Standout, standout_cards};
use chrono_tz::Tz;
use decks::stats::{
    BoardingSplit, DayRow, GameRecord, PlayDraw, PlayedMatch, Range, Record, TREND_HISTORY,
    daily_results, sparkline, version_date_label,
};
use rusqlite::{Connection, OptionalExtension, params};

use crate::StoreError;
use crate::card_stats::{CardStatsFilter, card_stats};
use crate::decks::card_infos;
use crate::names::stored_outcome;
use crate::platform::{BestOf, Platform, deck_of_platform};

/// The "Recently played" filter shows this many decks (0.x
/// `BuildDeckSidebarOptions::RECENT_LIMIT`).
pub const RECENT_LIMIT: u32 = 8;

/// One deck card of the deck list, or the deck in a deck page's header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckSummary {
    pub id: i64,
    pub name: String,
    /// The file's name when the user renamed the deck.
    pub original_name: Option<String>,
    /// The raw format code (`CMODERN`).
    pub format: String,
    /// The format for display (`Modern`).
    pub format_label: String,
    /// Comma separated colours (`U,R`).
    pub color_identity: Option<String>,
    /// The cover card's art crop URL, when the deck has a cover with art.
    pub cover_art: Option<String>,
    /// The cover's art crop file in v1's images folder.
    pub local_cover_art: Option<String>,
    /// The deck's own cover image, a file in the deck covers folder.
    pub cover_file: Option<String>,
    pub archived: bool,
    /// Every complete match of the deck, all versions, all time.
    pub record: Record,
    /// The kinds the deck has a complete match of, Bo3 first then Bo1.
    /// Empty for an MTGO deck and for a deck with no Arena match.
    pub kinds: Vec<BestOf>,
    /// When the latest complete match started.
    pub last_played_at: Option<i64>,
    /// The deck's archetype (archetypes spec section 8); `None` is
    /// "Unclassified".
    pub archetype: Option<DeckArchetype>,
}

/// A deck's archetype as the deck pages show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckArchetype {
    pub id: i64,
    pub name: String,
    /// Canonical comma form (`U,R`).
    pub color_identity: Option<String>,
    /// The player's own archetype, not MyMTGO's (a row the offline
    /// download holds as Custom is MyMTGO's).
    pub custom: bool,
}

/// The deck list's archetype filter (0.x `archetype` query: `none` or an
/// id).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeckArchetypeFilter {
    /// Decks without an archetype.
    Unclassified,
    /// One archetype's decks.
    Is(i64),
}

/// How the deck list is sorted (0.x `IndexController` sorts). Ties go by
/// name, then id, so the order is stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeckSort {
    /// Latest complete match first; never played decks last.
    #[default]
    LastPlayed,
    /// Highest match win rate first (draws count as played).
    WinRate,
    /// Most complete matches first.
    MatchCount,
    /// By name.
    Name,
}

impl DeckSort {
    /// `last_played` (also anything unknown), `win_rate`, `match_count`,
    /// `name`.
    pub fn from_key(key: &str) -> DeckSort {
        match key {
            "win_rate" => DeckSort::WinRate,
            "match_count" => DeckSort::MatchCount,
            "name" => DeckSort::Name,
            _ => DeckSort::LastPlayed,
        }
    }
}

/// What the deck list shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckListFilter {
    /// Only this account's decks (0.x `forActiveAccount`); `None` shows every
    /// account's of `platform`. See `current_account_id`.
    pub account_id: Option<i64>,
    pub platform: Platform,
    /// Only this format code.
    pub format: Option<String>,
    /// Only the recently played decks (`RECENT_LIMIT`).
    pub recent: bool,
    /// Part of the name, any case.
    pub search: Option<String>,
    pub hide_archived: bool,
    pub sort: DeckSort,
    /// 1-based.
    pub page: u32,
    pub per_page: u32,
    /// Only unclassified decks or one archetype's. It replaces the recent
    /// filter (0.x keeps both in one `archetype` value); an archetype with
    /// no deck in the list's scope is no filter (0.x drops a stale id).
    pub archetype: Option<DeckArchetypeFilter>,
}

/// One entry of the Format select: the decks of the list's scope per format,
/// search and the recent filter ignored (0.x `formatOptions`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatOption {
    pub code: String,
    pub label: String,
    pub count: u32,
}

/// One page of the deck list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckListPage {
    pub decks: Vec<DeckSummary>,
    /// Decks matching the filter, every page.
    pub total: u32,
    /// The page shown: a page past the end shows the last page.
    pub page: u32,
    pub per_page: u32,
    /// Sorted by label.
    pub format_options: Vec<FormatOption>,
    /// Whether the recent filter applied. When no deck has a match or a list
    /// it falls back to every deck (0.x), and this is false.
    pub recent: bool,
    /// The archetype filter that applied: a stale one is dropped.
    pub archetype: Option<DeckArchetypeFilter>,
}

/// One page of the deck list (0.x `IndexController`).
pub fn deck_list(conn: &Connection, filter: &DeckListFilter) -> Result<DeckListPage, StoreError> {
    let hide = i64::from(filter.hide_archived);
    // 0.x trims every input (`TrimStrings`), and a blank search is no search.
    let search = filter
        .search
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty());

    let archetype = applied_archetype(conn, filter)?;
    let (archetype_mode, archetype_id) = match archetype {
        None => (None, None),
        Some(DeckArchetypeFilter::Unclassified) => (Some("none"), None),
        Some(DeckArchetypeFilter::Is(id)) => (Some("id"), Some(id)),
    };

    let mut recent_ids = Vec::new();
    if filter.recent && archetype.is_none() {
        let mut stmt = conn.prepare_cached(&recent_decks_sql())?;
        let rows = stmt.query_map(
            params![
                filter.account_id,
                filter.format,
                hide,
                RECENT_LIMIT,
                filter.platform
            ],
            |row| row.get::<_, i64>(0),
        )?;
        for row in rows {
            recent_ids.push(row?.to_string());
        }
    }
    // No deck activity yet: every deck rather than an empty grid (0.x).
    let recent = !recent_ids.is_empty();
    let recent_json = format!("[{}]", recent_ids.join(","));

    let total: u32 = conn.prepare_cached(&count_sql())?.query_row(
        params![
            filter.account_id,
            filter.format,
            hide,
            search,
            i64::from(recent),
            recent_json,
            archetype_mode,
            archetype_id,
            filter.platform
        ],
        |row| row.get(0),
    )?;

    // A page past the end shows the last page (0.x walks back to it).
    let per_page = filter.per_page.max(1);
    let last_page = total.div_ceil(per_page).max(1);
    let page = filter.page.clamp(1, last_page);
    let offset = (page - 1) * per_page;

    let mut stmt = conn.prepare_cached(&list_sql(filter.sort))?;
    let rows = stmt.query_map(
        params![
            filter.account_id,
            filter.format,
            hide,
            search,
            i64::from(recent),
            recent_json,
            archetype_mode,
            archetype_id,
            per_page,
            offset,
            filter.platform
        ],
        summary_row,
    )?;
    let mut decks = Vec::new();
    for row in rows {
        let mut summary = row?;
        with_commander_cover(conn, &mut summary)?;
        decks.push(summary);
    }

    Ok(DeckListPage {
        decks,
        total,
        page,
        per_page,
        format_options: format_options(conn, filter.account_id, filter.platform, hide)?,
        recent,
        archetype,
    })
}

/// The list's archetype filter, dropped when it names an archetype with no
/// deck in the list's scope (0.x `IndexController`: a stale id must not
/// strand the player on an empty grid). Unclassified always applies.
fn applied_archetype(
    conn: &Connection,
    filter: &DeckListFilter,
) -> Result<Option<DeckArchetypeFilter>, StoreError> {
    let Some(DeckArchetypeFilter::Is(id)) = filter.archetype else {
        return Ok(filter.archetype);
    };
    let sql = format!(
        "SELECT EXISTS (SELECT 1 FROM decks d WHERE {} AND d.archetype_id = ?4)",
        deck_scope(5)
    );
    let in_scope: bool = conn.prepare_cached(&sql)?.query_row(
        params![
            filter.account_id,
            filter.format,
            i64::from(filter.hide_archived),
            id,
            filter.platform
        ],
        |row| row.get(0),
    )?;
    Ok(in_scope.then_some(DeckArchetypeFilter::Is(id)))
}

/// The decks the list can show: not Limited, the account (`?1`, NULL for
/// every account's of the platform bound to `?{platform}`), the format
/// (`?2`, NULL for all), archived ones only when `?3` is 0.
fn deck_scope(platform: u32) -> String {
    format!(
        "d.format <> 'Limited'
  AND (?1 IS NULL OR d.account_id = ?1)
  AND {of_platform}
  AND (?2 IS NULL OR d.format = ?2)
  AND (?3 = 0 OR d.archived_at IS NULL)",
        of_platform = deck_of_platform("d", platform)
    )
}

/// The list's own filters on top of `deck_scope`: part of the name (`?4`,
/// NULL for any), when `?5` is 1 only the deck ids of the JSON array `?6`,
/// and the archetype: `?7` NULL for any, `none` for decks without one, `id`
/// for archetype `?8`.
const LIST_FILTERS: &str = "(?4 IS NULL OR d.name LIKE '%' || ?4 || '%')
  AND (?5 = 0 OR d.id IN (SELECT value FROM json_each(?6)))
  AND (?7 IS NULL OR (?7 = 'none' AND d.archetype_id IS NULL)
       OR (?7 = 'id' AND d.archetype_id = ?8))";

/// Per deck: complete matches, wins, losses and the latest start, over every
/// version, for the decks `deck_filter` (a condition on `v`) allows.
///
/// `CROSS JOIN` keeps SQLite's join order: versions, then their matches by
/// the `matches_deck_version_id` index.
/// Left to itself, SQLite may scan every match instead (most matches are
/// complete, so an index on `state` would not narrow that scan).
fn played_cte(deck_filter: &str) -> String {
    format!(
        "played AS (
    SELECT v.deck_id AS deck_id, COUNT(*) AS total,
           SUM(m.outcome = 'win') AS wins, SUM(m.outcome = 'loss') AS losses,
           MAX(m.started_at) AS last_played,
           COALESCE(SUM(m.game_structure = 'bo1'), 0) AS bo1_total,
           COALESCE(SUM(m.game_structure = 'bo3'), 0) AS bo3_total
    FROM deck_versions v
    CROSS JOIN listed_matches m ON m.deck_version_id = v.id
    WHERE m.state = 'complete' AND {deck_filter}
    GROUP BY v.deck_id)"
    )
}

/// The `DeckSummary` columns (`summary_row`) of `decks d` with its `played`
/// numbers and cover card.
pub(crate) const SUMMARY_SELECT: &str = "
SELECT d.id, d.name, d.original_name, d.format, d.color_identity, c.art_crop,
       d.archived_at IS NOT NULL,
       COALESCE(p.wins, 0), COALESCE(p.losses, 0), COALESCE(p.total, 0), p.last_played,
       c.local_art_crop, da.id, da.name, da.color_identity,
       da.source = 'manual' AND da.offline_held = 0,
       COALESCE(p.bo3_total, 0), COALESCE(p.bo1_total, 0), d.cover_file
FROM decks d
LEFT JOIN played p ON p.deck_id = d.id
LEFT JOIN cards c ON c.catalog_id = d.cover_catalog_id
LEFT JOIN archetypes da ON da.id = d.archetype_id";

pub(crate) fn summary_row(row: &rusqlite::Row) -> rusqlite::Result<DeckSummary> {
    let format: String = row.get(3)?;
    let kinds = played_kinds(row)?;
    Ok(DeckSummary {
        id: row.get(0)?,
        name: row.get(1)?,
        original_name: row.get(2)?,
        format_label: decks::display_format(&format),
        format,
        color_identity: row.get(4)?,
        cover_art: row.get(5)?,
        local_cover_art: row.get(11)?,
        cover_file: row.get(18)?,
        archived: row.get(6)?,
        record: Record::from_total(row.get(7)?, row.get(8)?, row.get(9)?),
        kinds,
        last_played_at: row.get(10)?,
        archetype: match row.get::<_, Option<i64>>(12)? {
            Some(id) => Some(DeckArchetype {
                id,
                name: row.get(13)?,
                color_identity: archetypes::normalize_colors(
                    row.get::<_, Option<String>>(14)?.as_deref(),
                ),
                custom: row.get(15)?,
            }),
            None => None,
        },
    })
}

/// A commander format deck without a chosen cover (no cover card and no
/// cover image) shows its commander's art (`decks::commander_cover`). A
/// chosen cover card wins even while its art is not fetched yet.
pub(crate) fn with_commander_cover(
    conn: &Connection,
    summary: &mut DeckSummary,
) -> Result<(), StoreError> {
    if summary.cover_file.is_some()
        || !crate::match_commanders::is_commander_format(Some(&summary.format))
    {
        return Ok(());
    }
    let chosen: Option<i64> = conn
        .prepare_cached("SELECT cover_catalog_id FROM decks WHERE id = ?1")?
        .query_row([summary.id], |row| row.get(0))?;
    if chosen.is_some() {
        return Ok(());
    }
    let Some(catalog_id) = crate::decks::commander_cover(conn, summary.id, &summary.format)? else {
        return Ok(());
    };
    if let Some(card) = crate::decks::card_infos(conn, &[catalog_id])?.remove(&catalog_id) {
        summary.cover_art = card.art_crop;
        summary.local_cover_art = card.local_art_crop;
    }
    Ok(())
}

/// The kinds a deck played, from the per-kind totals (columns 16 and 17 of
/// `SUMMARY_SELECT`): Bo3 first, then Bo1. An MTGO deck has no Bo1 or Bo3
/// matches, so it has none.
fn played_kinds(row: &rusqlite::Row) -> rusqlite::Result<Vec<BestOf>> {
    let bo3_total: u32 = row.get(16)?;
    let bo1_total: u32 = row.get(17)?;
    let mut kinds = Vec::new();
    if bo3_total > 0 {
        kinds.push(BestOf::Three);
    }
    if bo1_total > 0 {
        kinds.push(BestOf::One);
    }
    Ok(kinds)
}

/// One page of the deck list: `deck_scope` (the platform `?11`) and
/// `LIST_FILTERS`, `?9` rows from row `?10`. One query: the numbers come from one pass over the
/// links, never a query per deck.
fn list_sql(sort: DeckSort) -> String {
    let order = match sort {
        DeckSort::LastPlayed => "p.last_played DESC",
        DeckSort::WinRate => {
            "CASE WHEN COALESCE(p.total, 0) > 0 THEN CAST(p.wins AS REAL) / p.total ELSE 0 END DESC"
        }
        DeckSort::MatchCount => "COALESCE(p.total, 0) DESC",
        DeckSort::Name => "d.name",
    };
    format!(
        "WITH {played}
{SUMMARY_SELECT}
WHERE {in_scope}
  AND {LIST_FILTERS}
ORDER BY {order}, d.name, d.id
LIMIT ?9 OFFSET ?10",
        played = played_cte("1 = 1"),
        in_scope = deck_scope(11)
    )
}

/// How many decks `list_sql` has over every page (the platform is `?9`).
fn count_sql() -> String {
    format!(
        "SELECT COUNT(*) FROM decks d WHERE {} AND {LIST_FILTERS}",
        deck_scope(9)
    )
}

/// The `?4` decks of `deck_scope` (the platform `?5`) with the latest
/// activity, newest first:
/// a complete match (0.x `recentDeckIds`) or a new list (a version's MTGO
/// timestamp), so a deck made in MTGO shows before it is played. Not the
/// deck's `created_at`: that is when v1 first read the file, the same for
/// every deck after a fresh install.
fn recent_decks_sql() -> String {
    format!(
        "
WITH activity AS (
    SELECT v.deck_id AS deck_id, MAX(m.started_at) AS at
    FROM deck_versions v
    CROSS JOIN listed_matches m ON m.deck_version_id = v.id
    WHERE m.state = 'complete'
    GROUP BY v.deck_id
    UNION ALL
    SELECT deck_id, MAX(modified_at) FROM deck_versions GROUP BY deck_id),
latest AS (SELECT deck_id, MAX(at) AS at FROM activity GROUP BY deck_id)
SELECT d.id FROM decks d JOIN latest l ON l.deck_id = d.id
WHERE {}
ORDER BY l.at DESC, d.id DESC
LIMIT ?4",
        deck_scope(5)
    )
}

/// The Format select: decks per format in the list's scope (account and
/// platform, archived ones when `hide` is 0), any format, by label.
pub fn format_options(
    conn: &Connection,
    account_id: Option<i64>,
    platform: Platform,
    hide: i64,
) -> Result<Vec<FormatOption>, StoreError> {
    let sql = format!(
        "SELECT d.format, COUNT(*) FROM decks d WHERE {} GROUP BY d.format",
        deck_scope(4)
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let no_format: Option<String> = None;
    let rows = stmt.query_map(params![account_id, no_format, hide, platform], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
    })?;
    let mut options = Vec::new();
    for row in rows {
        let (code, count) = row?;
        options.push(FormatOption {
            label: decks::display_format(&code),
            code,
            count,
        });
    }
    options.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.code.cmp(&b.code)));
    Ok(options)
}

/// One row of the version table and the version select (0.x
/// `GetDeckVersionStats`): "All versions" first, then `v1` to `vN` oldest
/// first, numbers over complete matches in a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionStats {
    /// `None` for "All versions".
    pub id: Option<i64>,
    /// `All versions`, `v1`, `v2` ...
    pub label: String,
    /// The latest version (by `modified_at`).
    pub is_current: bool,
    /// `Oct 01 - Oct 05`, `Oct 05 - now`; `None` for "All versions".
    pub date_label: Option<String>,
    pub record: Record,
    pub games: GameRecord,
    pub play_draw: PlayDraw,
}

/// A deck page's header: the deck and its version rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckHeader {
    pub summary: DeckSummary,
    pub versions: Vec<VersionStats>,
    /// The latest version, `None` when the deck has none.
    pub current_version_id: Option<i64>,
}

/// A deck's header, its version rows counting matches in `range` (of one
/// Arena kind, or every match when `best_of` is `None`); dates in `tz`.
/// `None` when there is no such deck (archived decks are found).
pub fn deck_header(
    conn: &Connection,
    deck_id: i64,
    range: Range,
    tz: Tz,
    best_of: Option<BestOf>,
) -> Result<Option<DeckHeader>, StoreError> {
    let summary = conn
        .prepare_cached(&header_sql())?
        .query_row([deck_id], summary_row)
        .optional()?;
    let Some(mut summary) = summary else {
        return Ok(None);
    };
    with_commander_cover(conn, &mut summary)?;

    let tallies = version_tallies(conn, deck_id, range, best_of)?;
    let mut all = VersionStats {
        id: None,
        label: "All versions".to_string(),
        is_current: false,
        date_label: None,
        record: Record::default(),
        games: GameRecord::default(),
        play_draw: PlayDraw::default(),
    };
    let mut rows = Vec::new();
    for (index, tally) in tallies.iter().enumerate() {
        let next = tallies.get(index + 1).map(|next| next.modified_at);
        all.record = all.record.plus(tally.record);
        all.games = all.games.plus(tally.games);
        all.play_draw = all.play_draw.plus(tally.play_draw);
        rows.push(VersionStats {
            id: Some(tally.id),
            label: format!("v{}", index + 1),
            is_current: next.is_none(),
            date_label: Some(version_date_label(tally.modified_at, next, tz)),
            record: tally.record,
            games: tally.games,
            play_draw: tally.play_draw,
        });
    }
    let current_version_id = tallies.last().map(|tally| tally.id);
    let mut versions = vec![all];
    versions.extend(rows);
    Ok(Some(DeckHeader {
        summary,
        versions,
        current_version_id,
    }))
}

/// The deck's matches with a known outcome (of `version` when given, else
/// every version), whatever their state: 0.x `GetLeagueInProgress`'s overall
/// fallback counts `outcome IS NOT NULL`. Hidden matches are left out.
pub fn deck_outcome_record(
    conn: &Connection,
    deck_id: i64,
    version: Option<i64>,
) -> Result<Record, StoreError> {
    let (wins, losses, total): (u32, u32, u32) = conn
        .prepare_cached(
            "SELECT COALESCE(SUM(m.outcome = 'win'), 0), COALESCE(SUM(m.outcome = 'loss'), 0),
                    COUNT(m.id)
             FROM deck_versions v
             JOIN listed_matches m ON m.deck_version_id = v.id
             WHERE v.deck_id = ?1 AND (?2 IS NULL OR v.id = ?2) AND m.outcome <> 'unknown'",
        )?
        .query_row(params![deck_id, version], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    Ok(Record::from_total(wins, losses, total))
}

/// The `DeckSummary` of deck `?1`, archived or not, any account or format.
pub(crate) fn header_sql() -> String {
    format!(
        "WITH {played}\n{SUMMARY_SELECT}\nWHERE d.id = ?1",
        played = played_cte("v.deck_id = ?1")
    )
}

/// One version's numbers over complete matches in a window.
pub(crate) struct VersionTally {
    pub(crate) id: i64,
    pub(crate) modified_at: i64,
    pub(crate) record: Record,
    pub(crate) games: GameRecord,
    pub(crate) play_draw: PlayDraw,
}

/// Every version of deck `?1`, oldest first, with its complete matches
/// started from `?2` to `?3` of the kind `?4` (NULL: every match).
const VERSION_MATCHES: &str = "
SELECT v.id, v.modified_at, COUNT(m.id),
       COALESCE(SUM(m.outcome = 'win'), 0), COALESCE(SUM(m.outcome = 'loss'), 0)
FROM deck_versions v
LEFT JOIN listed_matches m ON m.deck_version_id = v.id
     AND m.state = 'complete' AND m.started_at BETWEEN ?2 AND ?3
     AND (?4 IS NULL OR m.game_structure = ?4)
WHERE v.deck_id = ?1
GROUP BY v.id
ORDER BY v.modified_at, v.id";

/// Per version of deck `?1`: the games of its complete matches started from
/// `?2` to `?3` of the kind `?4` (NULL: every match), won and lost, then on
/// the play and on the draw.
const VERSION_GAMES: &str = "
SELECT v.id,
       COALESCE(SUM(g.result = 'win'), 0), COALESCE(SUM(g.result = 'loss'), 0),
       COALESCE(SUM(g.on_play = 1 AND g.result = 'win'), 0),
       COALESCE(SUM(g.on_play = 1 AND g.result = 'loss'), 0),
       COALESCE(SUM(g.on_play = 0 AND g.result = 'win'), 0),
       COALESCE(SUM(g.on_play = 0 AND g.result = 'loss'), 0),
       COALESCE(SUM(g.on_play IS NOT NULL), 0)
FROM deck_versions v
JOIN listed_matches m ON m.deck_version_id = v.id
JOIN games g ON g.match_id = m.id
WHERE v.deck_id = ?1 AND m.state = 'complete' AND m.started_at BETWEEN ?2 AND ?3
  AND (?4 IS NULL OR m.game_structure = ?4)
GROUP BY v.id";

/// Per version of deck `?1`: the history's game counts of its complete
/// matches started from `?2` to `?3` of the kind `?4` (NULL: every match)
/// that have no game rows (a history
/// import without a game log, Plan M). 0.x `GetDeckStats` and
/// `GetDeckVersionStats` fold these into the game totals.
const VERSION_GAMELESS: &str = "
SELECT v.id, COALESCE(SUM(m.games_won_imported), 0), COALESCE(SUM(m.games_lost_imported), 0)
FROM deck_versions v
JOIN listed_matches m ON m.deck_version_id = v.id
WHERE v.deck_id = ?1 AND m.state = 'complete' AND m.started_at BETWEEN ?2 AND ?3
  AND (?4 IS NULL OR m.game_structure = ?4)
  AND (m.games_won_imported IS NOT NULL OR m.games_lost_imported IS NOT NULL)
  AND NOT EXISTS (SELECT 1 FROM games g WHERE g.match_id = m.id)
GROUP BY v.id";

/// A deck's versions, oldest first, with their numbers in `range` (0.x
/// `GetDeckVersionStats`, the same numbers as `GetDeckStats` per version),
/// over the matches of one Arena kind or every match (`best_of` `None`).
pub(crate) fn version_tallies(
    conn: &Connection,
    deck_id: i64,
    range: Range,
    best_of: Option<BestOf>,
) -> Result<Vec<VersionTally>, StoreError> {
    let mut games: HashMap<i64, (GameRecord, PlayDraw)> = HashMap::new();
    let mut stmt = conn.prepare_cached(VERSION_GAMES)?;
    let rows = stmt.query_map(
        params![deck_id, range.from_ms, range.to_ms, best_of],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                GameRecord::new(row.get(1)?, row.get(2)?),
                PlayDraw::new(
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ),
            ))
        },
    )?;
    for row in rows {
        let (id, record, play_draw) = row?;
        games.insert(id, (record, play_draw));
    }

    // Gameless history imports add their game counts (they have no play or
    // draw side).
    let mut stmt = conn.prepare_cached(VERSION_GAMELESS)?;
    let rows = stmt.query_map(
        params![deck_id, range.from_ms, range.to_ms, best_of],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                GameRecord::new(row.get(1)?, row.get(2)?),
            ))
        },
    )?;
    for row in rows {
        let (id, imported) = row?;
        let entry = games.entry(id).or_default();
        entry.0 = entry.0.plus(imported);
    }

    let mut stmt = conn.prepare_cached(VERSION_MATCHES)?;
    let rows = stmt.query_map(
        params![deck_id, range.from_ms, range.to_ms, best_of],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                Record::from_total(row.get(3)?, row.get(4)?, row.get(2)?),
            ))
        },
    )?;
    let mut tallies = Vec::new();
    for row in rows {
        let (id, modified_at, record) = row?;
        let (games, play_draw) = games.get(&id).copied().unwrap_or_default();
        tallies.push(VersionTally {
            id,
            modified_at,
            record,
            games,
            play_draw,
        });
    }
    Ok(tallies)
}

/// How the match win rate moved against the window before (0.x
/// `GetDeckWinrateDelta`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WinRateDelta {
    pub previous_rate: Option<u32>,
    pub previous_total: u32,
    /// Current rate minus previous rate; `None` without a previous window or
    /// when it has no matches.
    pub delta: Option<i32>,
}

/// A deck's Dashboard numbers (0.x `DashboardController`).
#[derive(Debug, Clone, PartialEq)]
pub struct Dashboard {
    pub record: Record,
    pub delta: WinRateDelta,
    pub games: GameRecord,
    pub play_draw: PlayDraw,
    pub boarding: BoardingSplit,
    /// Local days from the first to the last day with a match.
    pub daily: Vec<DayRow>,
    /// The win rate sparkline over `daily`.
    pub sparkline: Vec<f64>,
    /// The six standout cards (0.x `GetStandoutCards`): from the deck's
    /// card stats over every game of the version, whatever the timeframe.
    pub standout: Standout,
}

/// A deck's Dashboard over complete matches in `range` (one version, or all
/// when `version` is `None`; one Arena kind, or every match when `best_of`
/// is `None`). `previous` is the window before it
/// (`decks::stats::previous_range`), `None` for all time. Chart days are
/// local days in `tz`.
pub fn deck_dashboard(
    conn: &Connection,
    deck_id: i64,
    version: Option<i64>,
    range: Range,
    previous: Option<Range>,
    tz: Tz,
    best_of: Option<BestOf>,
) -> Result<Dashboard, StoreError> {
    let version = existing_version(conn, version)?;
    let wanted = |id: i64| version.is_none_or(|version| version == id);

    let mut record = Record::default();
    let mut games = GameRecord::default();
    let mut play_draw = PlayDraw::default();
    for tally in version_tallies(conn, deck_id, range, best_of)? {
        if wanted(tally.id) {
            record = record.plus(tally.record);
            games = games.plus(tally.games);
            play_draw = play_draw.plus(tally.play_draw);
        }
    }

    // 0.x `GetDeckWinrateDelta`: none for all time or an empty window before.
    let mut delta = WinRateDelta::default();
    if let Some(previous) = previous {
        let mut before = Record::default();
        for tally in version_tallies(conn, deck_id, previous, best_of)? {
            if wanted(tally.id) {
                before = before.plus(tally.record);
            }
        }
        if !before.is_empty() {
            delta = WinRateDelta {
                previous_rate: Some(before.win_rate),
                previous_total: before.total,
                delta: Some(record.win_rate as i32 - before.win_rate as i32),
            };
        }
    }

    let boarding = conn.prepare_cached(BOARDING)?.query_row(
        params![deck_id, version, range.from_ms, range.to_ms, best_of],
        |row| {
            Ok(BoardingSplit::new(
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
            ))
        },
    )?;

    let mut matches = Vec::new();
    let mut stmt = conn.prepare_cached(DAILY_MATCHES)?;
    let rows = stmt.query_map(
        params![deck_id, version, range.from_ms, range.to_ms, best_of],
        |row| {
            Ok(PlayedMatch {
                started_at_ms: row.get(0)?,
                outcome: stored_outcome(&row.get::<_, String>(1)?),
            })
        },
    )?;
    for row in rows {
        matches.push(row?);
    }
    let earlier = earlier_matches(conn, deck_id, version, range.from_ms, best_of)?;
    let daily = daily_results(&earlier, &matches, tz);
    let sparkline = sparkline(&daily);

    let mut filter = CardStatsFilter::all();
    filter.version_id = version;
    filter.best_of = best_of;
    let standout = standout_cards(&card_stats(conn, deck_id, &filter)?);

    Ok(Dashboard {
        record,
        delta,
        games,
        play_draw,
        boarding,
        daily,
        sparkline,
        standout,
    })
}

/// A version filter as 0.x reads it (`DeckVersion::find`): an id with no
/// version row (a stale link) means every version. Another deck's version
/// still filters, so this deck shows nothing for it.
pub(crate) fn existing_version(
    conn: &Connection,
    version: Option<i64>,
) -> Result<Option<i64>, StoreError> {
    let Some(id) = version else {
        return Ok(None);
    };
    Ok(conn
        .prepare_cached("SELECT id FROM deck_versions WHERE id = ?1")?
        .query_row([id], |row| row.get(0))
        .optional()?)
}

/// Game one against the games after it (0.x `GetBoardingSplit`): games with
/// a result, numbered within their match by start time, then id; `?5` the
/// kind (NULL for every match).
const BOARDING: &str = "
SELECT COALESCE(SUM(n = 1 AND result = 'win'), 0), COALESCE(SUM(n = 1 AND result = 'loss'), 0),
       COALESCE(SUM(n > 1 AND result = 'win'), 0), COALESCE(SUM(n > 1 AND result = 'loss'), 0)
FROM (
    SELECT g.result AS result,
           ROW_NUMBER() OVER (PARTITION BY g.match_id ORDER BY g.started_at, g.id) AS n
    FROM deck_versions v
    JOIN listed_matches m ON m.deck_version_id = v.id
    JOIN games g ON g.match_id = m.id
    WHERE v.deck_id = ?1 AND (?2 IS NULL OR v.id = ?2)
      AND m.state = 'complete' AND m.started_at BETWEEN ?3 AND ?4
      AND (?5 IS NULL OR m.game_structure = ?5)
      AND g.result IN ('win', 'loss'))";

/// The start and outcome of each match for the daily chart (`?5` the kind,
/// NULL for every match).
const DAILY_MATCHES: &str = "
SELECT m.started_at, m.outcome
FROM deck_versions v
JOIN listed_matches m ON m.deck_version_id = v.id
WHERE v.deck_id = ?1 AND (?2 IS NULL OR v.id = ?2)
  AND m.state = 'complete' AND m.started_at BETWEEN ?3 AND ?4
  AND (?5 IS NULL OR m.game_structure = ?5)";

/// The latest `TREND_HISTORY` matches of a deck (one version, or all) before
/// `before_ms`, oldest first: they only feed a chart's trend.
pub(crate) fn earlier_matches(
    conn: &Connection,
    deck_id: i64,
    version: Option<i64>,
    before_ms: i64,
    best_of: Option<BestOf>,
) -> Result<Vec<PlayedMatch>, StoreError> {
    let mut stmt = conn.prepare_cached(DAILY_EARLIER_MATCHES)?;
    let rows = stmt.query_map(
        params![deck_id, version, before_ms, TREND_HISTORY, best_of],
        |row| {
            Ok(PlayedMatch {
                started_at_ms: row.get(0)?,
                outcome: stored_outcome(&row.get::<_, String>(1)?),
            })
        },
    )?;
    let mut earlier = Vec::new();
    for row in rows {
        earlier.push(row?);
    }
    earlier.reverse();
    Ok(earlier)
}

/// The latest `?4` matches before `?3` in the same scope as `DAILY_MATCHES`,
/// newest first.
pub(crate) const DAILY_EARLIER_MATCHES: &str = "
SELECT m.started_at, m.outcome
FROM deck_versions v
JOIN listed_matches m ON m.deck_version_id = v.id
WHERE v.deck_id = ?1 AND (?2 IS NULL OR v.id = ?2)
  AND m.state = 'complete' AND m.started_at < ?3
  AND (?5 IS NULL OR m.game_structure = ?5)
ORDER BY m.started_at DESC, m.id DESC
LIMIT ?4";

/// One card of a decklist. A catalog id without card data is listed as
/// `Card <id>` with `known` false and nothing else (decks spec 5).
#[derive(Debug, Clone, PartialEq)]
pub struct ListCard {
    pub catalog_id: u32,
    pub name: String,
    /// The card's type, `Basic Land` shown as `Land` (0.x `CardData`).
    pub type_line: Option<String>,
    /// Comma separated colours.
    pub color_identity: Option<String>,
    pub image: Option<String>,
    pub art_crop: Option<String>,
    pub local_image: Option<String>,
    pub local_art_crop: Option<String>,
    pub cmc: Option<f64>,
    /// Scryfall's form (`{1}{G}`), when stored.
    pub mana_cost: Option<String>,
    pub quantity: u32,
    pub sideboard: bool,
    pub known: bool,
}

/// The main deck cards of one type.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeGroup {
    /// `Creature`, `Planeswalker`, `Battle`, `Instant`, `Sorcery`,
    /// `Enchantment`, `Artifact`, `Land`; another type line as it is;
    /// `Unknown` for cards without card data or type.
    pub type_name: String,
    /// The group's cards, quantities added.
    pub count: u32,
    pub cards: Vec<ListCard>,
}

/// A deck version's list (0.x `BuildDecklist` and `CurveAndColours`).
#[derive(Debug, Clone, PartialEq)]
pub struct Decklist {
    /// The version shown, `None` when the deck has no versions.
    pub version_id: Option<i64>,
    /// A commander format version's commanders (its sideboard rows), shown
    /// first; empty for every other format, where `side` holds the sideboard.
    pub commanders: Vec<ListCard>,
    /// Groups in type order (`TypeGroup::type_name`).
    pub main: Vec<TypeGroup>,
    /// By type line (0.x order), cards without card data last.
    pub side: Vec<ListCard>,
    /// Main deck nonland cards by mana value (`7` is 7 or more), only values
    /// with cards, lowest first.
    pub curve: Vec<(u8, u32)>,
    /// Main deck nonland cards per colour, in W U B R G C order, only colours
    /// with cards. A card counts towards each of its colours.
    pub colours: Vec<(String, u32)>,
    /// Main deck nonland cards with card data, the colours' denominator.
    pub nonland_total: u32,
}

/// A deck's list: `version` when it is one of the deck's, else its latest
/// version (by `modified_at`).
pub fn deck_list_cards(
    conn: &Connection,
    deck_id: i64,
    version: Option<i64>,
) -> Result<Decklist, StoreError> {
    let versions = crate::decks::versions_of(conn, deck_id)?;
    let chosen = version
        .and_then(|id| versions.iter().find(|row| row.id == id))
        .or(versions.last());
    let Some(chosen) = chosen else {
        return Ok(Decklist {
            version_id: None,
            commanders: Vec::new(),
            main: Vec::new(),
            side: Vec::new(),
            curve: Vec::new(),
            colours: Vec::new(),
            nonland_total: 0,
        });
    };

    let rows = decks::decode_signature(&chosen.signature);
    let ids: Vec<u32> = rows.iter().map(|row| row.catalog_id).collect();
    let infos = card_infos(conn, &ids)?;
    let cards: Vec<ListCard> = rows
        .iter()
        .map(|row| match infos.get(&row.catalog_id) {
            Some(info) => ListCard {
                catalog_id: row.catalog_id,
                name: info
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("Card {}", row.catalog_id)),
                type_line: info.card_type.as_deref().map(|line| {
                    if line == "Basic Land" {
                        "Land".to_string()
                    } else {
                        line.to_string()
                    }
                }),
                color_identity: info.color_identity.clone(),
                image: info.image.clone(),
                art_crop: info.art_crop.clone(),
                local_image: info.local_image.clone(),
                local_art_crop: info.local_art_crop.clone(),
                cmc: info.cmc,
                mana_cost: info.mana_cost.clone(),
                quantity: row.quantity,
                sideboard: row.sideboard,
                known: true,
            },
            None => ListCard {
                catalog_id: row.catalog_id,
                name: format!("Card {}", row.catalog_id),
                type_line: None,
                color_identity: None,
                image: None,
                art_crop: None,
                local_image: None,
                local_art_crop: None,
                cmc: None,
                mana_cost: None,
                quantity: row.quantity,
                sideboard: row.sideboard,
                known: false,
            },
        })
        .collect();

    let main: Vec<ListCard> = cards
        .iter()
        .filter(|card| !card.sideboard)
        .cloned()
        .collect();
    let mut side: Vec<ListCard> = cards
        .iter()
        .filter(|card| card.sideboard)
        .cloned()
        .collect();
    // 0.x sorts every card by type line (stable, no type first) before
    // splitting off the sideboard. It drops unknown cards; v1 lists them last.
    side.sort_by(|a, b| {
        let a_key = (!a.known, a.type_line.as_deref().unwrap_or(""));
        let b_key = (!b.known, b.type_line.as_deref().unwrap_or(""));
        a_key.cmp(&b_key)
    });
    let numbers = curve_and_colours(&main);
    let is_commander = crate::decks::deck_by_id(conn, deck_id)?
        .is_some_and(|deck| crate::match_commanders::is_commander_format(Some(&deck.format)));
    let (commanders, side) = if is_commander {
        (side, Vec::new())
    } else {
        (Vec::new(), side)
    };
    Ok(Decklist {
        version_id: Some(chosen.id),
        commanders,
        main: type_groups(main),
        side,
        curve: numbers.curve,
        colours: numbers.colours,
        nonland_total: numbers.nonland_total,
    })
}

/// Main deck types in display order (0.x `CANONICAL_TYPES`): a type line
/// joins the first one it contains, so `Artifact Creature` is a Creature.
const TYPE_ORDER: [&str; 8] = [
    "Creature",
    "Planeswalker",
    "Battle",
    "Instant",
    "Sorcery",
    "Enchantment",
    "Artifact",
    "Land",
];

/// The group of cards without card data or a type line.
const UNKNOWN_GROUP: &str = "Unknown";

/// The main deck grouped by type (0.x `BuildDecklist` then `DeckList.vue`):
/// groups in `TYPE_ORDER`, then other type lines by name, then `Unknown`.
/// Inside a group, cards keep 0.x's order: by the server's type rank
/// (Creature, Instant, Sorcery, others, Land) and type line, then in list
/// order.
fn type_groups(main: Vec<ListCard>) -> Vec<TypeGroup> {
    // (group position, group name, server rank, type line, list position, card)
    let mut keyed: Vec<(usize, String, u8, String, usize, ListCard)> = Vec::new();
    for (position, card) in main.into_iter().enumerate() {
        let line = card.type_line.clone().unwrap_or_default();
        let (group_position, group_name) = if !card.known || line.is_empty() {
            (TYPE_ORDER.len() + 1, UNKNOWN_GROUP.to_string())
        } else {
            match TYPE_ORDER.iter().position(|kind| line.contains(kind)) {
                Some(index) => (index, TYPE_ORDER[index].to_string()),
                None => (TYPE_ORDER.len(), line.clone()),
            }
        };
        let rank = match line.as_str() {
            "Creature" => 1,
            "Instant" => 2,
            "Sorcery" => 3,
            "Land" => 10,
            _ => 5,
        };
        keyed.push((group_position, group_name, rank, line, position, card));
    }
    keyed.sort_by(|a, b| (a.0, &a.1, a.2, &a.3, a.4).cmp(&(b.0, &b.1, b.2, &b.3, b.4)));

    let mut groups: Vec<TypeGroup> = Vec::new();
    for (_, name, _, _, _, card) in keyed {
        match groups.last_mut() {
            Some(group) if group.type_name == name => {
                group.count += card.quantity;
                group.cards.push(card);
            }
            _ => groups.push(TypeGroup {
                type_name: name,
                count: card.quantity,
                cards: vec![card],
            }),
        }
    }
    groups
}

/// Colours in display order (0.x `CurveAndColours`).
const COLOUR_ORDER: [&str; 6] = ["W", "U", "B", "R", "G", "C"];

/// Mana curve and colour split of the main deck's nonland cards with card
/// data (0.x `CurveAndColours`): a card is a land when its type line
/// contains `Land`; mana value rounds down, 7 and up share a bucket; a card
/// with no colours is colourless.
fn curve_and_colours(main: &[ListCard]) -> CurveAndColours {
    let nonland: Vec<&ListCard> = main
        .iter()
        .filter(|card| card.known)
        .filter(|card| !card.type_line.as_deref().unwrap_or("").contains("Land"))
        .collect();

    let mut buckets = [0u32; 8];
    let mut colours = [0u32; 6];
    let mut total = 0;
    for card in &nonland {
        let value = card.cmc.unwrap_or(0.0).floor().clamp(0.0, 7.0) as usize;
        buckets[value] += card.quantity;
        total += card.quantity;
        let identity = card.color_identity.as_deref().unwrap_or("");
        for (index, colour) in COLOUR_ORDER.iter().enumerate() {
            let has = if *colour == "C" {
                identity.is_empty() || identity == "C"
            } else {
                identity.split(',').any(|part| part == *colour)
            };
            if has {
                colours[index] += card.quantity;
            }
        }
    }

    let mut curve = Vec::new();
    for (value, count) in buckets.iter().enumerate() {
        if *count > 0 {
            curve.push((value as u8, *count));
        }
    }
    let mut split = Vec::new();
    for (index, count) in colours.iter().enumerate() {
        if *count > 0 {
            split.push((COLOUR_ORDER[index].to_string(), *count));
        }
    }
    CurveAndColours {
        curve,
        colours: split,
        nonland_total: total,
    }
}

/// `Decklist::curve`, `Decklist::colours` and `Decklist::nonland_total`.
struct CurveAndColours {
    curve: Vec<(u8, u32)>,
    colours: Vec<(String, u32)>,
    nonland_total: u32,
}

/// How many of a deck's matches are of each Arena kind, and the kind of the
/// most recent one. An MTGO deck counts nothing (its matches hold MTGO's
/// structure text, never `bo1` or `bo3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeckKinds {
    pub bo1: u32,
    pub bo3: u32,
    pub latest: Option<BestOf>,
}

/// The deck's listed complete matches (hidden, deleted and in-progress left
/// out, as the deck pages leave them out) counted by kind, any timeframe.
pub fn deck_kinds(conn: &Connection, deck_id: i64) -> Result<DeckKinds, StoreError> {
    let (bo1, bo3): (u32, u32) = conn
        .prepare_cached(
            "SELECT COALESCE(SUM(m.game_structure = 'bo1'), 0),
                    COALESCE(SUM(m.game_structure = 'bo3'), 0)
             FROM deck_versions v
             JOIN listed_matches m ON m.deck_version_id = v.id
             WHERE v.deck_id = ?1 AND m.state = 'complete'",
        )?
        .query_row([deck_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let latest: Option<String> = conn
        .prepare_cached(
            "SELECT m.game_structure
             FROM deck_versions v
             JOIN listed_matches m ON m.deck_version_id = v.id
             WHERE v.deck_id = ?1 AND m.state = 'complete'
               AND m.game_structure IN ('bo1', 'bo3')
             ORDER BY m.started_at DESC, m.id DESC
             LIMIT 1",
        )?
        .query_row([deck_id], |row| row.get(0))
        .optional()?;
    Ok(DeckKinds {
        bo1,
        bo3,
        latest: latest.as_deref().and_then(BestOf::from_key),
    })
}

/// One page of a deck's matches.
pub type DeckMatchPage = crate::match_list::MatchListPage;

/// A deck's complete matches in `range` (one version, or all; one Arena
/// kind, or all), newest first: `match_list::match_list` with no list filter
/// (0.x `Decks\MatchesController`).
pub fn deck_matches(
    conn: &Connection,
    deck_id: i64,
    version: Option<i64>,
    range: Range,
    page: u32,
    per_page: u32,
    best_of: Option<BestOf>,
) -> Result<DeckMatchPage, StoreError> {
    crate::match_list::match_list(
        conn,
        crate::match_list::Scope::Deck {
            deck_id,
            version,
            best_of,
        },
        &crate::match_list::MatchListFilter::newest_first(range),
        page,
        per_page,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        account, arena_account, db, hide, new_log_file, number as count, set_game_structure,
    };
    use crate::write::insert_log_file;
    use decks::stats::{previous_range, timeframe_range, winrate_percent};
    use decks::{DeckCard, signature};

    const HOUR: i64 = 3_600_000;
    const DAY: i64 = 24 * HOUR;
    /// 2026-10-07T12:00:00Z.
    const NOW: i64 = 1_791_374_400_000;
    const UTC: Tz = chrono_tz::UTC;

    fn deck(conn: &Connection, account_id: Option<i64>, name: &str, format: &str) -> i64 {
        let n = count(conn, "SELECT COUNT(*) FROM decks") + 1;
        conn.query_row(
            "INSERT INTO decks (mtgo_id, account_id, name, format, created_at, updated_at, source)
             VALUES (?1, ?2, ?3, ?4, 0, 0, 'file') RETURNING id",
            params![format!("deck-{n}"), account_id, name, format],
            |row| row.get(0),
        )
        .expect("deck")
    }

    fn version_with(conn: &Connection, deck_id: i64, signature: &str, modified_at: i64) -> i64 {
        conn.query_row(
            "INSERT INTO deck_versions (deck_id, signature, modified_at) VALUES (?1, ?2, ?3)
             RETURNING id",
            params![deck_id, signature, modified_at],
            |row| row.get(0),
        )
        .expect("version")
    }

    fn version(conn: &Connection, deck_id: i64, modified_at: i64) -> i64 {
        let n = count(conn, "SELECT COUNT(*) FROM deck_versions") + 1;
        version_with(conn, deck_id, &format!("SIG-{n}"), modified_at)
    }

    fn archive(conn: &Connection, deck_id: i64) {
        conn.execute("UPDATE decks SET archived_at = 5 WHERE id = ?1", [deck_id])
            .expect("archive");
    }

    /// A match linked to `version`, by the deck's account (or Qlocal).
    /// Returns the match id.
    fn played(conn: &Connection, version: i64, started_at: i64, state: &str, outcome: &str) -> i64 {
        let n = count(conn, "SELECT COUNT(*) FROM matches") + 1;
        let token = format!("tok-{n}");
        let deck_account: Option<i64> = conn
            .query_row(
                "SELECT d.account_id FROM deck_versions v JOIN decks d ON d.id = v.deck_id
                 WHERE v.id = ?1",
                [version],
                |row| row.get(0),
            )
            .expect("version's deck");
        let account_id = match deck_account {
            Some(id) => id,
            None => conn
                .query_row(
                    "SELECT id FROM accounts WHERE username = 'Qlocal'",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .expect("lookup")
                .unwrap_or_else(|| account(conn, "Qlocal")),
        };
        let match_id: i64 = conn
            .query_row(
                "INSERT INTO matches (token, account_id, source, state, outcome, started_at,
                     last_activity_at, revision, derived_version)
                 VALUES (?1, ?2, 'log', ?3, ?4, ?5, ?5, 1, 1) RETURNING id",
                params![token, account_id, state, outcome, started_at],
                |row| row.get(0),
            )
            .expect("match");
        conn.execute(
            "UPDATE matches SET deck_version_id = ?2 WHERE token = ?1",
            params![token, version],
        )
        .expect("link");
        match_id
    }

    fn won(conn: &Connection, version: i64, started_at: i64) -> i64 {
        played(conn, version, started_at, "complete", "win")
    }

    fn lost(conn: &Connection, version: i64, started_at: i64) -> i64 {
        played(conn, version, started_at, "complete", "loss")
    }

    fn game(
        conn: &Connection,
        match_id: i64,
        number: u32,
        result: &str,
        on_play: Option<bool>,
        started_at: Option<i64>,
    ) {
        conn.execute(
            "INSERT INTO games (match_id, game_number, result, end_reason, on_play, started_at)
             VALUES (?1, ?2, ?3, 'win', ?4, ?5)",
            params![match_id, number, result, on_play, started_at],
        )
        .expect("game");
    }

    /// Games played in this order, each 10 minutes after the last.
    fn games(conn: &Connection, match_id: i64, started_at: i64, results: &[&str]) {
        for (index, result) in results.iter().enumerate() {
            let at = started_at + index as i64 * 600_000;
            game(conn, match_id, index as u32 + 1, result, None, Some(at));
        }
    }

    fn filter() -> DeckListFilter {
        DeckListFilter {
            account_id: None,
            platform: Platform::Mtgo,
            format: None,
            recent: false,
            search: None,
            hide_archived: true,
            sort: DeckSort::LastPlayed,
            page: 1,
            per_page: 12,
            archetype: None,
        }
    }

    fn names(page: &DeckListPage) -> Vec<&str> {
        page.decks.iter().map(|deck| deck.name.as_str()).collect()
    }

    #[test]
    fn the_deck_list_reads_one_platform() {
        let (_dir, conn) = db("deck-list-arena");
        let me = account(&conn, "Qlocal");
        let mtgo = deck(&conn, Some(me), "Qburn", "CMODERN");
        let orphan = deck(&conn, None, "Qorphan", "CMODERN");
        let arena = deck(&conn, None, "Qarena deck", "CPIONEER");
        crate::test_support::move_deck_to_arena(&conn, arena);
        let mtgo_version = version(&conn, mtgo, 0);
        version(&conn, orphan, 0);
        let arena_version = version(&conn, arena, 0);
        won(&conn, mtgo_version, NOW - HOUR);
        won(&conn, arena_version, NOW);

        let page = deck_list(&conn, &filter()).expect("list");
        assert_eq!(names(&page), ["Qburn", "Qorphan"]);
        assert_eq!(page.total, 2);
        let codes: Vec<&str> = page
            .format_options
            .iter()
            .map(|o| o.code.as_str())
            .collect();
        assert_eq!(codes, ["CMODERN"]);
        let recent = deck_list(
            &conn,
            &DeckListFilter {
                recent: true,
                ..filter()
            },
        )
        .expect("recent");
        assert_eq!(names(&recent), ["Qburn", "Qorphan"]);

        let arena_filter = DeckListFilter {
            platform: Platform::Arena,
            ..filter()
        };
        let page = deck_list(&conn, &arena_filter).expect("list");
        assert_eq!(names(&page), ["Qarena deck"]);
        assert_eq!(page.total, 1);
        let codes: Vec<&str> = page
            .format_options
            .iter()
            .map(|o| o.code.as_str())
            .collect();
        assert_eq!(codes, ["CPIONEER"]);
        let recent = deck_list(
            &conn,
            &DeckListFilter {
                recent: true,
                ..arena_filter
            },
        )
        .expect("recent");
        assert_eq!(names(&recent), ["Qarena deck"]);
    }

    /// The query plan of `sql`, every parameter bound to 1.
    fn plan(conn: &Connection, sql: &str) -> String {
        let mut stmt = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .expect("explain");
        let params = vec![1i64; stmt.parameter_count()];
        let details: Vec<String> = stmt
            .query_map(rusqlite::params_from_iter(params), |row| {
                row.get::<_, String>(3)
            })
            .expect("rows")
            .map(|row| row.expect("row"))
            .collect();
        details.join("\n")
    }

    // Deck list (0.x IndexControllerTest)

    #[test]
    fn the_list_counts_draws_and_unknown_outcomes_as_played_and_only_complete_matches() {
        let (_dir, conn) = db("deck-pages-list-draws");
        let local = account(&conn, "Qlocal");
        let plain = version(&conn, deck(&conn, Some(local), "Qno draws", "CMODERN"), 0);
        for at in 1..=6 {
            won(&conn, plain, at * HOUR);
        }
        for at in 7..=10 {
            lost(&conn, plain, at * HOUR);
        }
        let drawn = version(&conn, deck(&conn, Some(local), "Qwith draws", "CMODERN"), 0);
        for at in 1..=7 {
            won(&conn, drawn, at * HOUR);
        }
        for at in 8..=11 {
            lost(&conn, drawn, at * HOUR);
        }
        played(&conn, drawn, 12 * HOUR, "complete", "draw");
        played(&conn, drawn, 13 * HOUR, "complete", "draw");
        played(&conn, drawn, 14 * HOUR, "complete", "unknown");
        // Not complete: never counted, not even as last played.
        played(&conn, drawn, 20 * HOUR, "in_progress", "win");
        played(&conn, drawn, 21 * HOUR, "ended", "win");
        played(&conn, drawn, 22 * HOUR, "abandoned", "loss");
        played(&conn, drawn, 23 * HOUR, "pending", "unknown");

        let page = deck_list(
            &conn,
            &DeckListFilter {
                sort: DeckSort::WinRate,
                ..filter()
            },
        )
        .expect("list");
        // 6 / 10 = 60% beats 7 / 14 = 50%; ignoring draws it would be 7 / 11.
        assert_eq!(names(&page), vec!["Qno draws", "Qwith draws"]);
        assert_eq!(
            page.decks[1].record,
            Record {
                wins: 7,
                losses: 4,
                draws: 3,
                total: 14,
                win_rate: 50
            }
        );
        assert_eq!(page.decks[1].last_played_at, Some(14 * HOUR));
        assert_eq!(page.decks[0].record, Record::from_counts(6, 4, 0));
        assert_eq!(page.decks[0].last_played_at, Some(10 * HOUR));
    }

    #[test]
    fn the_list_adds_up_matches_linked_to_every_version_of_a_deck() {
        let (_dir, conn) = db("deck-pages-list-versions");
        let id = deck(&conn, None, "Qversions", "CMODERN");
        let first = version(&conn, id, 10);
        let second = version(&conn, id, 20);
        let third = version(&conn, id, 30);
        won(&conn, first, 100);
        lost(&conn, second, 200);
        played(&conn, second, 300, "complete", "draw");
        won(&conn, third, 400);
        played(&conn, third, 500, "in_progress", "loss");
        let other = version(&conn, deck(&conn, None, "Qother", "CMODERN"), 0);
        won(&conn, other, 600);

        let page = deck_list(
            &conn,
            &DeckListFilter {
                sort: DeckSort::Name,
                ..filter()
            },
        )
        .expect("list");
        assert_eq!(names(&page), vec!["Qother", "Qversions"]);
        let versions = &page.decks[1];
        assert_eq!(versions.record, Record::from_counts(2, 1, 1));
        assert_eq!(versions.last_played_at, Some(400));
        assert_eq!(page.decks[0].record, Record::from_counts(1, 0, 0));
    }

    #[test]
    fn the_list_sorts_by_last_played_win_rate_match_count_and_name() {
        let (_dir, conn) = db("deck-pages-list-sorts");
        let alpha = version(&conn, deck(&conn, None, "Qalpha", "CMODERN"), 0);
        deck(&conn, None, "Qbravo", "CMODERN");
        let charlie = version(&conn, deck(&conn, None, "Qcharlie", "CMODERN"), 0);
        let delta = version(&conn, deck(&conn, None, "Qdelta", "CMODERN"), 0);
        won(&conn, alpha, 3_000);
        won(&conn, charlie, 5_000);
        lost(&conn, charlie, 4_000);
        lost(&conn, charlie, 2_000);
        lost(&conn, delta, 1_000);
        lost(&conn, delta, 500);

        let sorted = |sort: DeckSort| {
            let page = deck_list(&conn, &DeckListFilter { sort, ..filter() }).expect("list");
            names(&page)
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            sorted(DeckSort::LastPlayed),
            vec!["Qcharlie", "Qalpha", "Qdelta", "Qbravo"],
            "never played decks last"
        );
        assert_eq!(
            sorted(DeckSort::WinRate),
            vec!["Qalpha", "Qcharlie", "Qbravo", "Qdelta"],
            "100%, 33%, then the 0% decks by name"
        );
        assert_eq!(
            sorted(DeckSort::MatchCount),
            vec!["Qcharlie", "Qdelta", "Qalpha", "Qbravo"]
        );
        assert_eq!(
            sorted(DeckSort::Name),
            vec!["Qalpha", "Qbravo", "Qcharlie", "Qdelta"]
        );
        assert_eq!(DeckSort::from_key("win_rate"), DeckSort::WinRate);
        assert_eq!(DeckSort::from_key("match_count"), DeckSort::MatchCount);
        assert_eq!(DeckSort::from_key("name"), DeckSort::Name);
        assert_eq!(DeckSort::from_key("last_played"), DeckSort::LastPlayed);
        assert_eq!(DeckSort::from_key("nonsense"), DeckSort::LastPlayed);
    }

    #[test]
    fn the_list_leaves_out_limited_decks_and_other_accounts() {
        let (_dir, conn) = db("deck-pages-list-scope");
        let local = account(&conn, "Qlocal");
        let second = account(&conn, "Qsecond");
        deck(&conn, Some(local), "Qmodern", "CMODERN");
        deck(&conn, Some(local), "Qdraft", "Limited");
        deck(&conn, Some(second), "Qpauper", "CPAUPER");
        deck(&conn, None, "Qorphan", "CPAUPER");

        let mine = deck_list(
            &conn,
            &DeckListFilter {
                account_id: Some(local),
                ..filter()
            },
        )
        .expect("list");
        assert_eq!(names(&mine), vec!["Qmodern"]);
        assert_eq!(mine.total, 1);
        assert_eq!(
            mine.format_options,
            vec![FormatOption {
                code: "CMODERN".to_string(),
                label: "Modern".to_string(),
                count: 1
            }]
        );

        let every = deck_list(&conn, &filter()).expect("list");
        assert_eq!(every.total, 3, "no account: every account's decks");
        assert!(!names(&every).contains(&"Qdraft"));
    }

    #[test]
    fn archived_decks_show_only_when_asked_and_say_so() {
        let (_dir, conn) = db("deck-pages-list-archived");
        deck(&conn, None, "Qkept", "CMODERN");
        let gone = deck(&conn, None, "Qgone", "CMODERN");
        archive(&conn, gone);

        let hidden = deck_list(&conn, &filter()).expect("list");
        assert_eq!(names(&hidden), vec!["Qkept"]);
        assert_eq!(hidden.format_options[0].count, 1);
        assert!(!hidden.decks[0].archived);

        let shown = deck_list(
            &conn,
            &DeckListFilter {
                hide_archived: false,
                sort: DeckSort::Name,
                ..filter()
            },
        )
        .expect("list");
        assert_eq!(names(&shown), vec!["Qgone", "Qkept"]);
        assert!(shown.decks[0].archived);
        assert_eq!(shown.format_options[0].count, 2);
    }

    #[test]
    fn a_decks_archetype_says_whether_it_is_the_players_own() {
        let (_dir, conn) = db("deck-pages-archetype-custom");
        let archetype = |uuid: &str, source: &str, held: bool| -> i64 {
            conn.query_row(
                "INSERT INTO archetypes (uuid, name, format, source, offline_held, created_at, updated_at)
                 VALUES (?1, 'Qtron', 'modern', ?2, ?3, 0, 0) RETURNING id",
                params![uuid, source, held],
                |row| row.get(0),
            )
            .expect("archetype")
        };
        let own = archetype("q-own", "manual", false);
        let server = archetype("q-server", "downloaded", false);
        let held = archetype("q-held", "manual", true);
        for (name, archetype_id) in [("Qa", own), ("Qb", server), ("Qc", held)] {
            let id = deck(&conn, None, name, "CMODERN");
            conn.execute(
                "UPDATE decks SET archetype_id = ?1 WHERE id = ?2",
                [archetype_id, id],
            )
            .expect("link");
        }
        let list = deck_list(
            &conn,
            &DeckListFilter {
                sort: DeckSort::Name,
                ..filter()
            },
        )
        .expect("list");
        let flags: Vec<bool> = list
            .decks
            .iter()
            .map(|deck| deck.archetype.as_ref().expect("archetype").custom)
            .collect();
        assert_eq!(flags, vec![true, false, false]);
    }

    #[test]
    fn the_list_filters_by_format_and_search_but_format_options_ignore_search() {
        let (_dir, conn) = db("deck-pages-list-filters");
        deck(&conn, None, "Qburn", "CMODERN");
        deck(&conn, None, "Qtron", "CMODERN");
        deck(&conn, None, "Qelves", "CPAUPER");
        deck(&conn, None, "Qstompy", "CPAUPER");
        deck(&conn, None, "Qtron pauper", "CPAUPER");
        let options = vec![
            FormatOption {
                code: "CMODERN".to_string(),
                label: "Modern".to_string(),
                count: 2,
            },
            FormatOption {
                code: "CPAUPER".to_string(),
                label: "Pauper".to_string(),
                count: 3,
            },
        ];

        let searched = deck_list(
            &conn,
            &DeckListFilter {
                search: Some("TRON".to_string()),
                sort: DeckSort::Name,
                ..filter()
            },
        )
        .expect("list");
        assert_eq!(names(&searched), vec!["Qtron", "Qtron pauper"]);
        assert_eq!(searched.total, 2);
        assert_eq!(searched.format_options, options);

        let pauper = deck_list(
            &conn,
            &DeckListFilter {
                format: Some("CPAUPER".to_string()),
                search: Some("tron".to_string()),
                ..filter()
            },
        )
        .expect("list");
        assert_eq!(names(&pauper), vec!["Qtron pauper"]);
        assert_eq!(pauper.format_options, options);

        // Surrounding spaces are trimmed (0.x `TrimStrings`).
        let padded = deck_list(
            &conn,
            &DeckListFilter {
                search: Some("  tron  ".to_string()),
                ..filter()
            },
        )
        .expect("list");
        assert_eq!(padded.total, 2);
    }

    #[test]
    fn recently_played_is_the_eight_decks_with_the_latest_complete_match() {
        let (_dir, conn) = db("deck-pages-list-recent");
        let mut ids = Vec::new();
        for n in 1..=10 {
            let id = deck(&conn, None, &format!("Qdeck {n:02}"), "CMODERN");
            won(&conn, version(&conn, id, 0), n * 1_000);
            ids.push(id);
        }
        deck(&conn, None, "Qnever", "CMODERN");
        let unfinished = deck(&conn, None, "Qunfinished", "CMODERN");
        played(
            &conn,
            version(&conn, unfinished, 0),
            99_000,
            "in_progress",
            "win",
        );

        let page = deck_list(
            &conn,
            &DeckListFilter {
                recent: true,
                ..filter()
            },
        )
        .expect("list");
        assert!(page.recent);
        assert_eq!(page.total, 8);
        let shown: Vec<i64> = page.decks.iter().map(|deck| deck.id).collect();
        let expected: Vec<i64> = ids[2..].iter().rev().copied().collect();
        assert_eq!(shown, expected);
    }

    // A new deck shows before it is played: its newest list change ranks
    // against the other decks' last complete match (#352).
    #[test]
    fn recent_decks_count_a_list_change_like_a_match() {
        let (_dir, conn) = db("deck-pages-list-recent-changed");
        let mut played = Vec::new();
        for n in 1..=8 {
            let id = deck(&conn, None, &format!("Qplayed {n}"), "CMODERN");
            won(&conn, version(&conn, id, 0), n * 10_000);
            played.push(id);
        }
        // Newer than all but the last two played decks: in the eight.
        let fresh = deck(&conn, None, "Qfresh", "CMODERN");
        version(&conn, fresh, 65_000);

        let page = deck_list(
            &conn,
            &DeckListFilter {
                recent: true,
                ..filter()
            },
        )
        .expect("list");
        assert!(page.recent);
        let mut shown: Vec<i64> = page.decks.iter().map(|deck| deck.id).collect();
        shown.sort_unstable();
        let mut expected = played[1..].to_vec();
        expected.push(fresh);
        assert_eq!(shown, expected);
    }

    #[test]
    fn recently_played_falls_back_to_every_deck_when_nothing_was_played() {
        let (_dir, conn) = db("deck-pages-list-recent-empty");
        deck(&conn, None, "Qone", "CMODERN");
        deck(&conn, None, "Qtwo", "CMODERN");
        let page = deck_list(
            &conn,
            &DeckListFilter {
                recent: true,
                ..filter()
            },
        )
        .expect("list");
        assert!(!page.recent);
        assert_eq!(page.total, 2);
    }

    #[test]
    fn a_page_past_the_end_walks_back_to_the_last_page() {
        let (_dir, conn) = db("deck-pages-list-walk-back");
        for n in 1..=5 {
            deck(&conn, None, &format!("Qdeck {n}"), "CMODERN");
        }
        let page = |page: u32| {
            deck_list(
                &conn,
                &DeckListFilter {
                    sort: DeckSort::Name,
                    page,
                    per_page: 2,
                    ..filter()
                },
            )
            .expect("list")
        };
        let past = page(9);
        assert_eq!((past.page, past.total, past.per_page), (3, 5, 2));
        assert_eq!(names(&past), vec!["Qdeck 5"]);
        assert_eq!(page(0).page, 1);
        assert_eq!(names(&page(2)), vec!["Qdeck 3", "Qdeck 4"]);

        let (_empty_dir, empty) = db("deck-pages-list-empty");
        let none = deck_list(
            &empty,
            &DeckListFilter {
                page: 3,
                ..filter()
            },
        )
        .expect("list");
        assert_eq!((none.page, none.total), (1, 0));
        assert!(none.decks.is_empty());
        assert!(none.format_options.is_empty());
    }

    #[test]
    fn a_deck_card_shows_its_cover_art_format_label_and_identity() {
        let (_dir, conn) = db("deck-pages-list-card");
        let id = deck(&conn, None, "Qrenamed", "CMODERN");
        conn.execute(
            "UPDATE decks SET original_name = 'Qfile name', color_identity = 'U,R',
                 cover_catalog_id = 1001, cover_file = '1-5.png' WHERE id = ?1",
            [id],
        )
        .expect("deck");
        insert_card(&conn, 1001, "Qbolt", Some("Instant"), Some("R"), Some(1.0));
        conn.execute(
            "UPDATE cards SET art_crop = 'https://cards.scryfall.io/art.jpg',
                 local_art_crop = '1001_art.jpg' WHERE catalog_id = 1001",
            [],
        )
        .expect("art");
        let bare = deck(&conn, None, "Qbare", "pauper commander");

        let page = deck_list(
            &conn,
            &DeckListFilter {
                sort: DeckSort::Name,
                ..filter()
            },
        )
        .expect("list");
        assert_eq!(
            page.decks[1],
            DeckSummary {
                id,
                name: "Qrenamed".to_string(),
                original_name: Some("Qfile name".to_string()),
                format: "CMODERN".to_string(),
                format_label: "Modern".to_string(),
                color_identity: Some("U,R".to_string()),
                cover_art: Some("https://cards.scryfall.io/art.jpg".to_string()),
                local_cover_art: Some("1001_art.jpg".to_string()),
                cover_file: Some("1-5.png".to_string()),
                archived: false,
                record: Record::default(),
                kinds: Vec::new(),
                last_played_at: None,
                archetype: None,
            }
        );
        assert_eq!(page.decks[0].id, bare);
        assert_eq!(page.decks[0].format_label, "Pauper Commander");
        assert_eq!(page.decks[0].cover_art, None);
        assert_eq!(page.decks[0].cover_file, None);
    }

    /// Release spec 5 replaced "the latest log file's player": a file's
    /// player no longer counts, a Login does (`accounts` has the rule's
    /// own tests).
    #[test]
    fn the_current_account_follows_logins_not_log_files() {
        let (_dir, conn) = db("deck-pages-current-account");
        assert_eq!(
            crate::accounts::current_account_id(&conn, Platform::Mtgo).expect("none"),
            None
        );
        let local = account(&conn, "Qlocal");
        assert_eq!(
            crate::accounts::current_account_id(&conn, Platform::Mtgo).expect("only"),
            Some(local)
        );
        let second = account(&conn, "Qsecond");
        let file = insert_log_file(&conn, &new_log_file("C:/new/mtgo.log")).expect("file");
        conn.execute(
            "UPDATE log_files SET local_username = 'Qsecond', last_seen_at = 1000 WHERE id = ?1",
            [file],
        )
        .expect("file player");
        assert_eq!(
            crate::accounts::current_account_id(&conn, Platform::Mtgo).expect("two, no login"),
            None,
            "two accounts and no Login line: every account"
        );
        // The tracker keeps each account's newest Login as it stores the line.
        conn.execute(
            "UPDATE accounts SET last_login_at = 5 WHERE id = ?1",
            [second],
        )
        .expect("login");
        assert_eq!(
            crate::accounts::current_account_id(&conn, Platform::Mtgo).expect("login"),
            Some(second)
        );
    }

    #[test]
    fn the_list_query_never_scans_matches_or_games() {
        let (_dir, conn) = db("deck-pages-list-plan");
        for sort in [
            DeckSort::LastPlayed,
            DeckSort::WinRate,
            DeckSort::MatchCount,
            DeckSort::Name,
        ] {
            let query = plan(&conn, &list_sql(sort));
            assert!(!query.contains("SCAN m"), "{sort:?} plan:\n{query}");
            assert!(!query.contains("SCAN g"), "{sort:?} plan:\n{query}");
            assert!(
                query.contains("matches_deck_version_id"),
                "{sort:?} plan:\n{query}"
            );
        }
        let recent = plan(&conn, &recent_decks_sql());
        assert!(!recent.contains("SCAN m"), "plan:\n{recent}");
    }

    // Header and version rows (0.x GetDeckVersionStatsTest)

    #[test]
    fn the_header_has_all_versions_then_each_version_oldest_first() {
        let (_dir, conn) = db("deck-pages-header");
        let id = deck(&conn, None, "Qdeck", "CMODERN");
        // 2026-10-01T00:00Z and 2026-10-05T00:00Z.
        let first = version(&conn, id, NOW - 6 * DAY - 12 * HOUR);
        let second = version(&conn, id, NOW - 2 * DAY - 12 * HOUR);
        let opener = won(&conn, first, NOW - 5 * DAY);
        game(&conn, opener, 1, "win", Some(true), Some(1));
        game(&conn, opener, 2, "loss", Some(false), Some(2));
        game(&conn, opener, 3, "win", Some(true), Some(3));
        won(&conn, first, NOW - 4 * DAY);
        lost(&conn, first, NOW - 4 * DAY);
        let later = won(&conn, second, NOW - DAY);
        game(&conn, later, 1, "unknown", Some(true), Some(1));
        played(&conn, second, NOW - DAY, "complete", "draw");
        played(&conn, second, NOW, "in_progress", "loss");

        let header = deck_header(&conn, id, Range::all_time(), UTC, None)
            .expect("header")
            .expect("the deck exists");
        assert_eq!(header.summary.record, Record::from_counts(3, 1, 1));
        assert_eq!(header.summary.last_played_at, Some(NOW - DAY));
        assert_eq!(header.current_version_id, Some(second));
        assert_eq!(
            header.versions,
            vec![
                VersionStats {
                    id: None,
                    label: "All versions".to_string(),
                    is_current: false,
                    date_label: None,
                    record: Record::from_counts(3, 1, 1),
                    games: GameRecord::new(2, 1),
                    play_draw: PlayDraw::new(2, 0, 0, 1, 4),
                },
                VersionStats {
                    id: Some(first),
                    label: "v1".to_string(),
                    is_current: false,
                    date_label: Some("Oct 01 - Oct 05".to_string()),
                    record: Record::from_counts(2, 1, 0),
                    games: GameRecord::new(2, 1),
                    play_draw: PlayDraw::new(2, 0, 0, 1, 3),
                },
                VersionStats {
                    id: Some(second),
                    label: "v2".to_string(),
                    is_current: true,
                    date_label: Some("Oct 05 - now".to_string()),
                    record: Record::from_counts(1, 0, 1),
                    games: GameRecord::new(0, 0),
                    play_draw: PlayDraw::new(0, 0, 0, 0, 1),
                },
            ]
        );
        assert_eq!(header.versions[0].record.win_rate, 60);
        assert_eq!(header.versions[1].record.win_rate, winrate_percent(2, 1, 0));

        // A window leaves out older matches but the summary counts them all.
        let recent = deck_header(
            &conn,
            id,
            Range {
                from_ms: NOW - 2 * DAY,
                to_ms: NOW,
            },
            UTC,
            None,
        )
        .expect("header")
        .expect("deck");
        assert_eq!(recent.versions[0].record, Record::from_counts(1, 0, 1));
        assert_eq!(recent.versions[1].record, Record::default());
        assert_eq!(recent.summary.record.total, 5);
    }

    #[test]
    fn a_deck_without_versions_or_matches_has_an_empty_all_versions_row() {
        let (_dir, conn) = db("deck-pages-header-empty");
        let id = deck(&conn, None, "Qempty", "CMODERN");
        archive(&conn, id);
        let header = deck_header(&conn, id, Range::all_time(), UTC, None)
            .expect("header")
            .expect("archived decks are found");
        assert!(header.summary.archived);
        assert_eq!(header.current_version_id, None);
        assert_eq!(header.versions.len(), 1);
        assert_eq!(header.versions[0].record, Record::default());
        assert_eq!(header.versions[0].record.win_rate, 0);
        assert_eq!(
            deck_header(&conn, 999, Range::all_time(), UTC, None).expect("read"),
            None
        );
    }

    // Dashboard (0.x GetDeckStatsTest, GetDeckWinrateDeltaTest,
    // GetBoardingSplitTest, DashboardChartDataTest)

    fn dashboard(conn: &Connection, deck_id: i64, version: Option<i64>, range: Range) -> Dashboard {
        deck_dashboard(conn, deck_id, version, range, None, UTC, None).expect("dashboard")
    }

    #[test]
    fn the_dashboard_counts_matches_games_and_play_draw() {
        let (_dir, conn) = db("deck-pages-dashboard-stats");
        let id = deck(&conn, None, "Qdeck", "CMODERN");
        let v = version(&conn, id, 0);
        for i in 0..5 {
            let at = NOW - i * HOUR;
            let match_id = won(&conn, v, at);
            game(&conn, match_id, 1, "win", Some(i % 2 == 0), Some(at));
        }
        let stats = dashboard(&conn, id, None, Range::all_time());
        assert_eq!(stats.record, Record::from_counts(5, 0, 0));
        assert_eq!(stats.games, GameRecord::new(5, 0));
        assert_eq!(stats.play_draw.otp_won, 3);
        assert_eq!(stats.play_draw.otd_won, 2);
        assert_eq!(stats.play_draw.play_draw_games, 5);
    }

    #[test]
    fn the_dashboard_counts_draws_and_only_games_with_a_known_result() {
        let (_dir, conn) = db("deck-pages-dashboard-draws");
        let id = deck(&conn, None, "Qdeck", "CMODERN");
        let v = version(&conn, id, 0);
        let tracked = won(&conn, v, NOW - HOUR);
        game(&conn, tracked, 1, "win", Some(true), Some(1));
        game(&conn, tracked, 2, "unknown", None, Some(2));
        lost(&conn, v, NOW - 2 * HOUR);
        let drawn = played(&conn, v, NOW - 3 * HOUR, "complete", "draw");
        game(&conn, drawn, 1, "draw", Some(false), Some(1));
        let open = played(&conn, v, NOW, "in_progress", "unknown");
        game(&conn, open, 1, "loss", Some(true), Some(1));

        let stats = dashboard(&conn, id, None, Range::all_time());
        assert_eq!(stats.record, Record::from_counts(1, 1, 1));
        assert_eq!(stats.record.win_rate, 33);
        assert_eq!(decks::stats::record_label(&stats.record), "1 - 1 - 1");
        assert_eq!(stats.games, GameRecord::new(1, 0));
        // Games that know who was on the play, their result known or not.
        assert_eq!(stats.play_draw, PlayDraw::new(1, 0, 0, 0, 2));
    }

    /// Make a stored match a history import with its history's game counts.
    fn history_counts(conn: &Connection, match_id: i64, won: i64, lost: i64) {
        conn.execute(
            "UPDATE matches SET source = 'imported', games_won_imported = ?2,
                 games_lost_imported = ?3 WHERE id = ?1",
            params![match_id, won, lost],
        )
        .expect("imported counts");
    }

    #[test]
    fn the_dashboard_folds_gameless_imported_game_counts_into_the_games() {
        // 0.x GetDeckStatsTest "counts draws and folds gameless imported
        // games into totals".
        let (_dir, conn) = db("deck-pages-dashboard-gameless");
        let id = deck(&conn, None, "Qdeck", "CMODERN");
        let v = version(&conn, id, 0);
        let tracked = won(&conn, v, NOW - HOUR);
        game(&conn, tracked, 1, "win", None, Some(1));
        let gameless = lost(&conn, v, NOW - 2 * HOUR);
        history_counts(&conn, gameless, 1, 2);
        played(&conn, v, NOW - 3 * HOUR, "complete", "draw");

        let stats = dashboard(&conn, id, None, Range::all_time());
        assert_eq!(stats.record, Record::from_counts(1, 1, 1));
        assert_eq!(stats.record.win_rate, 33);
        assert_eq!(decks::stats::record_label(&stats.record), "1 - 1 - 1");
        assert_eq!(stats.games, GameRecord::new(2, 2));
        assert_eq!(
            stats.play_draw,
            PlayDraw::default(),
            "imported counts know nothing of play or draw"
        );
    }

    #[test]
    fn the_version_rows_fold_gameless_imported_game_counts() {
        // 0.x GetDeckVersionStatsTest "includes gameless imported matches in
        // match and game counts".
        let (_dir, conn) = db("deck-pages-header-gameless");
        let id = deck(&conn, None, "Qdeck", "CMODERN");
        let v = version(&conn, id, 0);
        let tracked = won(&conn, v, NOW - HOUR);
        game(&conn, tracked, 1, "win", Some(true), Some(1));
        let first = won(&conn, v, NOW - 2 * HOUR);
        history_counts(&conn, first, 2, 1);
        let second = lost(&conn, v, NOW - 3 * HOUR);
        history_counts(&conn, second, 0, 2);
        // A history import with games counts its games, not its counts.
        let with_games = won(&conn, v, NOW - 4 * HOUR);
        history_counts(&conn, with_games, 5, 5);
        game(&conn, with_games, 1, "win", None, Some(1));
        // Outside the window, and not complete: never folded.
        let old = lost(&conn, v, NOW - 30 * DAY);
        history_counts(&conn, old, 0, 2);
        let open = played(&conn, v, NOW, "in_progress", "unknown");
        history_counts(&conn, open, 0, 2);

        let window = Range {
            from_ms: NOW - 7 * DAY,
            to_ms: NOW,
        };
        let header = deck_header(&conn, id, window, UTC, None)
            .expect("header")
            .expect("deck");
        let all = &header.versions[0];
        assert_eq!(all.record, Record::from_counts(3, 1, 0));
        assert_eq!(all.games, GameRecord::new(4, 3));
        assert_eq!(header.versions[1].games, GameRecord::new(4, 3));
    }

    #[test]
    fn an_imported_0x_match_without_games_folds_its_score_into_the_deck_games() {
        // 0.x GetDeckStatsTest "counts draws and folds gameless imported
        // games into totals", with the gameless match coming in through the
        // 0.x import (Plan O).
        let (_dir, mut conn) = db("deck-pages-0x-gameless");
        let id = deck(&conn, None, "Qdeck", "CMODERN");
        let v = version(&conn, id, 0);
        let tracked = won(&conn, v, NOW - HOUR);
        game(&conn, tracked, 1, "win", None, Some(1));
        played(&conn, v, NOW - 3 * HOUR, "complete", "draw");
        let mut gameless = crate::test_support::imported("q-0x-gameless", Some(77));
        gameless.games.clear();
        gameless.outcome = "loss".to_string();
        gameless.started_at_ms = NOW - 2 * HOUR;
        gameless.ended_at_ms = None;
        gameless.games_won_imported = Some(1);
        gameless.games_lost_imported = Some(2);
        crate::import::import_matches(&mut conn, &[gameless], NOW).expect("0.x import");
        conn.execute(
            "UPDATE matches SET deck_version_id = ?1 WHERE token = 'q-0x-gameless'",
            [v],
        )
        .expect("deck link");

        let stats = dashboard(&conn, id, None, Range::all_time());
        assert_eq!(stats.record, Record::from_counts(1, 1, 1));
        assert_eq!(stats.games, GameRecord::new(2, 2));
        let header = deck_header(&conn, id, Range::all_time(), UTC, None)
            .expect("header")
            .expect("deck");
        assert_eq!(header.versions[0].games, GameRecord::new(2, 2));
    }

    /// S9-6: the Dashboard's standout cards come from the deck's card stats
    /// (the store read, end to end).
    #[test]
    fn the_dashboard_picks_standout_cards_from_the_card_stats() {
        let (_dir, conn) = db("deck-pages-dashboard-standout");
        let id = deck(&conn, None, "Qdeck", "CMODERN");
        let v = version(&conn, id, 0);
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, oracle_id, name, type, image, local_image)
                 VALUES (77, 'q-bolt', 'Qbolt', 'Instant', 'https://cards.scryfall.io/77.jpg',
                     '77.jpg'),
                     (78, 'q-land', 'Qland', 'Basic Land', NULL, NULL);",
        )
        .expect("cards");
        for (index, result) in ["win", "loss"].iter().enumerate() {
            let at = NOW - index as i64 * HOUR;
            let match_id = played(&conn, v, at, "complete", result);
            game(&conn, match_id, 1, result, Some(true), Some(at));
            let game_id: i64 = conn
                .query_row(
                    "SELECT id FROM games WHERE match_id = ?1",
                    [match_id],
                    |row| row.get(0),
                )
                .expect("game");
            conn.execute(
                "INSERT INTO game_cards (game_id, side, catalog_id, main_qty, cast, seen, played)
                 VALUES (?1, 'local', 77, 4, 1, 1, 0), (?1, 'local', 78, 10, 0, 1, 1)",
                [game_id],
            )
            .expect("rows");
        }
        let stats = dashboard(&conn, id, None, Range::all_time());
        let most_cast = stats.standout.most_cast.expect("a most cast card");
        assert_eq!(most_cast.name, "Qbolt");
        assert_eq!(
            most_cast.image.as_deref(),
            Some("https://cards.scryfall.io/77.jpg")
        );
        assert_eq!(most_cast.local_image.as_deref(), Some("77.jpg"));
        assert_eq!(
            stats.standout.most_played_land.expect("a land").name,
            "Qland"
        );
        assert!(stats.standout.most_sided_in.is_none(), "nothing sided in");
    }

    #[test]
    fn the_dashboards_standout_cards_read_one_kind() {
        let (_dir, conn) = db("deck-pages-dashboard-standout-kind");
        let id = deck(&conn, None, "Qdeck", "CMODERN");
        let v = version(&conn, id, 0);
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, oracle_id, name, type)
             VALUES (77, 'q-bolt', 'Qbolt', 'Instant');",
        )
        .expect("card");
        let match_id = played(&conn, v, NOW, "complete", "win");
        game(&conn, match_id, 1, "win", Some(true), Some(NOW));
        conn.execute(
            "INSERT INTO game_cards (game_id, side, catalog_id, main_qty, cast, seen, played)
             SELECT id, 'local', 77, 4, 1, 1, 0 FROM games WHERE match_id = ?1",
            [match_id],
        )
        .expect("row");
        set_game_structure(&conn, &token_of(&conn, match_id), "bo3");
        let read = |best_of| {
            deck_dashboard(&conn, id, None, Range::all_time(), None, UTC, best_of)
                .expect("dashboard")
                .standout
                .most_cast
        };
        assert!(read(Some(BestOf::One)).is_none(), "no Bo1 match");
        assert!(read(Some(BestOf::Three)).is_some());
        assert!(read(None).is_some());
    }

    #[test]
    fn a_deck_without_matches_has_an_empty_dashboard() {
        let (_dir, conn) = db("deck-pages-dashboard-empty");
        let id = deck(&conn, None, "Qdeck", "CMODERN");
        let stats = deck_dashboard(
            &conn,
            id,
            None,
            Range::all_time(),
            Some(Range {
                from_ms: 0,
                to_ms: 10,
            }),
            UTC,
            None,
        )
        .expect("dashboard");
        assert_eq!(
            stats,
            Dashboard {
                record: Record::default(),
                delta: WinRateDelta::default(),
                games: GameRecord::default(),
                play_draw: PlayDraw::default(),
                boarding: BoardingSplit::default(),
                daily: Vec::new(),
                sparkline: Vec::new(),
                standout: Standout::default(),
            }
        );
    }

    fn week() -> (Range, Option<Range>) {
        let current = timeframe_range("week", NOW, UTC).expect("week");
        (current, previous_range("week", current.from_ms, UTC))
    }

    #[test]
    fn the_win_rate_delta_compares_against_the_window_before() {
        let (_dir, conn) = db("deck-pages-delta");
        let id = deck(&conn, None, "Qdeck", "CMODERN");
        let v = version(&conn, id, 0);
        // This week: 3 wins, 1 loss = 75%.
        for _ in 0..3 {
            won(&conn, v, NOW - 2 * DAY);
        }
        lost(&conn, v, NOW - 2 * DAY);
        // The week before: 1 win, 3 losses = 25%.
        won(&conn, v, NOW - 10 * DAY);
        for _ in 0..3 {
            lost(&conn, v, NOW - 10 * DAY);
        }
        let (current, previous) = week();
        let stats =
            deck_dashboard(&conn, id, None, current, previous, UTC, None).expect("dashboard");
        assert_eq!(stats.record, Record::from_counts(3, 1, 0));
        assert_eq!(
            stats.delta,
            WinRateDelta {
                previous_rate: Some(25),
                previous_total: 4,
                delta: Some(50),
            }
        );

        // All time has no window before it.
        let all = deck_dashboard(&conn, id, None, Range::all_time(), None, UTC, None).expect("all");
        assert_eq!(all.delta, WinRateDelta::default());
    }

    #[test]
    fn an_empty_previous_window_has_no_delta() {
        let (_dir, conn) = db("deck-pages-delta-empty");
        let id = deck(&conn, None, "Qdeck", "CMODERN");
        won(&conn, version(&conn, id, 0), NOW - 2 * DAY);
        let (current, previous) = week();
        let stats =
            deck_dashboard(&conn, id, None, current, previous, UTC, None).expect("dashboard");
        assert_eq!(stats.delta, WinRateDelta::default());
    }

    #[test]
    fn the_delta_narrows_to_one_version() {
        let (_dir, conn) = db("deck-pages-delta-version");
        let id = deck(&conn, None, "Qdeck", "CMODERN");
        let v = version(&conn, id, 0);
        let other = version(&conn, id, 1);
        won(&conn, v, NOW - 2 * DAY);
        won(&conn, v, NOW - 10 * DAY);
        won(&conn, v, NOW - 10 * DAY);
        for _ in 0..4 {
            lost(&conn, other, NOW - 10 * DAY);
        }
        let (current, previous) = week();
        let stats =
            deck_dashboard(&conn, id, Some(v), current, previous, UTC, None).expect("dashboard");
        assert_eq!(
            stats.delta,
            WinRateDelta {
                previous_rate: Some(100),
                previous_total: 2,
                delta: Some(0),
            }
        );
    }

    #[test]
    fn a_version_id_that_does_not_exist_means_every_version() {
        let (_dir, conn) = db("deck-pages-unknown-version");
        let id = deck(&conn, None, "Qdeck", "CMODERN");
        let first = version(&conn, id, 0);
        let second = version(&conn, id, 1);
        let a = won(&conn, first, NOW - HOUR);
        games(&conn, a, NOW - HOUR, &["win", "loss", "win"]);
        lost(&conn, second, NOW - 2 * HOUR);
        let foreign = version(&conn, deck(&conn, None, "Qother", "CMODERN"), 0);

        // 0.x `DeckVersion::find` gives null for a stale id: all versions.
        let stale = deck_dashboard(&conn, id, Some(99_999), Range::all_time(), None, UTC, None)
            .expect("dashboard");
        let all = dashboard(&conn, id, None, Range::all_time());
        assert_eq!(stale, all);
        assert_eq!(stale.record, Record::from_counts(1, 1, 0));
        let matches =
            deck_matches(&conn, id, Some(99_999), Range::all_time(), 1, 10, None).expect("matches");
        assert_eq!(matches.total, 2);

        // Another deck's version exists, so it filters this deck to nothing.
        let other = dashboard(&conn, id, Some(foreign), Range::all_time());
        assert_eq!(other.record, Record::default());
        let none =
            deck_matches(&conn, id, Some(foreign), Range::all_time(), 1, 10, None).expect("none");
        assert_eq!(none.total, 0);
    }

    #[test]
    fn the_boarding_split_separates_game_one_from_the_games_after() {
        let (_dir, conn) = db("deck-pages-boarding");
        let id = deck(&conn, None, "Qdeck", "CMODERN");
        let v = version(&conn, id, 0);
        let m = played(&conn, v, NOW - HOUR, "complete", "win");
        games(&conn, m, NOW - HOUR, &["loss", "win", "win"]);
        assert_eq!(
            dashboard(&conn, id, None, Range::all_time()).boarding,
            BoardingSplit::new(0, 1, 2, 0)
        );
        assert_eq!(BoardingSplit::new(0, 1, 2, 0).delta, 100);
    }

    #[test]
    fn games_are_numbered_by_when_they_started_not_by_insertion() {
        let (_dir, conn) = db("deck-pages-boarding-order");
        let id = deck(&conn, None, "Qdeck", "CMODERN");
        let m = won(&conn, version(&conn, id, 0), NOW - HOUR);
        // The later game is inserted first (and numbered 1).
        game(&conn, m, 1, "win", None, Some(NOW - 20 * 60_000));
        game(&conn, m, 2, "loss", None, Some(NOW - 50 * 60_000));
        let split = dashboard(&conn, id, None, Range::all_time()).boarding;
        assert_eq!((split.game_one_lost, split.post_board_won), (1, 1));
    }

    #[test]
    fn the_boarding_split_counts_each_match_separately_and_only_in_range() {
        let (_dir, conn) = db("deck-pages-boarding-matches");
        let id = deck(&conn, None, "Qdeck", "CMODERN");
        let v = version(&conn, id, 0);
        let other = version(&conn, id, 1);
        let a = won(&conn, v, NOW - HOUR);
        games(&conn, a, NOW - HOUR, &["win", "loss", "loss"]);
        let b = won(&conn, v, NOW - 2 * HOUR);
        games(&conn, b, NOW - 2 * HOUR, &["win", "win"]);
        let old = won(&conn, v, NOW - 40 * DAY);
        games(&conn, old, NOW - 40 * DAY, &["win", "win"]);
        let c = lost(&conn, other, NOW - 3 * HOUR);
        games(&conn, c, NOW - 3 * HOUR, &["loss", "loss"]);
        let open = played(&conn, v, NOW, "in_progress", "unknown");
        games(&conn, open, NOW, &["loss", "loss"]);

        let last_week = Range {
            from_ms: NOW - 7 * DAY,
            to_ms: NOW,
        };
        let split = dashboard(&conn, id, Some(v), last_week).boarding;
        assert_eq!(split, BoardingSplit::new(2, 0, 1, 2));
        assert_eq!(split.games, 5);

        let all = dashboard(&conn, id, None, last_week).boarding;
        assert_eq!(all, BoardingSplit::new(2, 1, 1, 3));

        let empty = dashboard(
            &conn,
            id,
            None,
            Range {
                from_ms: NOW - 30 * DAY,
                to_ms: NOW - 20 * DAY,
            },
        )
        .boarding;
        assert_eq!((empty.games, empty.delta), (0, 0));
    }

    #[test]
    fn the_daily_chart_buckets_by_local_day_with_its_sparkline() {
        let (_dir, conn) = db("deck-pages-daily");
        let id = deck(&conn, None, "Qdeck", "CMODERN");
        let v = version(&conn, id, 0);
        // 2026-09-08T03:00Z, 03:30Z (20:00 and 20:30 PDT on the 7th) and
        // 17:00Z (10:00 PDT on the 8th).
        let sept_8 = NOW - 29 * DAY - 12 * HOUR;
        won(&conn, v, sept_8 + 3 * HOUR);
        played(&conn, v, sept_8 + 3 * HOUR + HOUR / 2, "complete", "draw");
        lost(&conn, v, sept_8 + 17 * HOUR);
        played(&conn, v, sept_8 + 18 * HOUR, "abandoned", "loss");

        let stats = deck_dashboard(
            &conn,
            id,
            None,
            Range::all_time(),
            None,
            chrono_tz::America::Los_Angeles,
            None,
        )
        .expect("dashboard");
        let days: Vec<(&str, u32, u32, u32)> = stats
            .daily
            .iter()
            .map(|day| (day.day.as_str(), day.wins, day.losses, day.draws))
            .collect();
        assert_eq!(days, vec![("2026-09-07", 1, 0, 1), ("2026-09-08", 0, 1, 0)]);
        assert_eq!(stats.sparkline.len(), 2);
        assert_eq!(stats.sparkline[0], 50.0);
    }

    #[test]
    fn an_earlier_match_moves_the_daily_charts_first_day_trend() {
        let (_dir, conn) = db("deck-pages-daily-trend");
        let id = deck(&conn, None, "Qdeck", "CMODERN");
        let v = version(&conn, id, 0);
        lost(&conn, v, NOW - 40 * DAY);
        won(&conn, v, NOW - DAY);
        let window = Range {
            from_ms: NOW - 10 * DAY,
            to_ms: NOW,
        };
        let stats = dashboard(&conn, id, None, window);
        assert_eq!(stats.daily.len(), 1, "the earlier loss is not a chart day");
        // A loss then a win: 1 / (1 + 49/51) = 0.51, not 100.
        assert_eq!(stats.daily[0].trend, Some(51));
    }

    #[test]
    fn the_trend_history_is_the_decks_own_version_and_kind() {
        let (_dir, conn) = db("deck-pages-daily-trend-scope");
        let id = deck(&conn, None, "Qdeck", "CMODERN");
        let first = version(&conn, id, 0);
        let second = version(&conn, id, 1);
        let other_deck = deck(&conn, None, "Qother", "CMODERN");
        let other = version(&conn, other_deck, 0);
        let on_first = lost(&conn, first, NOW - 40 * DAY);
        lost(&conn, other, NOW - 40 * DAY);
        set_game_structure(&conn, &token_of(&conn, on_first), "bo3");
        let in_window = won(&conn, second, NOW - DAY);
        set_game_structure(&conn, &token_of(&conn, in_window), "bo1");
        let window = Range {
            from_ms: NOW - 10 * DAY,
            to_ms: NOW,
        };
        let trend = |version, best_of| {
            deck_dashboard(&conn, id, version, window, None, UTC, best_of)
                .expect("dashboard")
                .daily[0]
                .trend
        };
        assert_eq!(
            trend(None, None),
            Some(51),
            "this deck's loss, not the other's"
        );
        assert_eq!(
            trend(Some(second), None),
            Some(100),
            "the first version's loss is another version's"
        );
        assert_eq!(
            trend(None, Some(BestOf::One)),
            Some(100),
            "a Bo3 loss is another kind's"
        );
    }

    // Decklist (0.x BuildDecklist, DecklistPrintingsTest, CurveAndColours)

    fn insert_card(
        conn: &Connection,
        catalog_id: u32,
        name: &str,
        card_type: Option<&str>,
        identity: Option<&str>,
        cmc: Option<f64>,
    ) {
        conn.execute(
            "INSERT INTO cards (catalog_id, oracle_id, name, type, color_identity, cmc, image)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                catalog_id,
                format!("oracle-{catalog_id}"),
                name,
                card_type,
                identity,
                cmc,
                format!("https://cards.scryfall.io/{catalog_id}.jpg")
            ],
        )
        .expect("card");
    }

    fn row(catalog_id: u32, quantity: u32, sideboard: bool) -> DeckCard {
        DeckCard {
            catalog_id,
            quantity,
            sideboard,
        }
    }

    /// A group's name, count and `(card name, quantity)` list.
    type GroupView<'a> = (&'a str, u32, Vec<(&'a str, u32)>);

    fn card_names(cards: &[ListCard]) -> Vec<(&str, u32)> {
        cards
            .iter()
            .map(|card| (card.name.as_str(), card.quantity))
            .collect()
    }

    #[test]
    fn the_decklist_groups_the_main_deck_by_type_and_lists_unknown_cards() {
        let (_dir, conn) = db("deck-pages-decklist");
        insert_card(&conn, 1001, "Qbolt", Some("Instant"), Some("R"), Some(1.0));
        insert_card(&conn, 1002, "Qgoyf", Some("Creature"), Some("G"), Some(2.0));
        insert_card(
            &conn,
            1003,
            "Qconstruct",
            Some("Artifact Creature"),
            Some(""),
            Some(7.0),
        );
        insert_card(
            &conn,
            1004,
            "Qmountain",
            Some("Basic Land"),
            Some("R"),
            Some(0.0),
        );
        insert_card(
            &conn,
            1005,
            "Qwalker",
            Some("Planeswalker"),
            Some("U,R"),
            Some(4.0),
        );
        insert_card(
            &conn,
            1006,
            "Qritual",
            Some("Sorcery"),
            Some("B"),
            Some(8.0),
        );
        insert_card(&conn, 1007, "Qrelic", Some("Artifact"), None, Some(1.5));
        insert_card(
            &conn,
            1008,
            "Qaura",
            Some("Enchantment"),
            Some("W"),
            Some(3.0),
        );
        insert_card(&conn, 1009, "Qforest", Some("Land"), Some("G"), Some(0.0));
        let cards = vec![
            row(1001, 4, false),
            row(1002, 4, false),
            row(1003, 2, false),
            row(1004, 10, false),
            row(1005, 1, false),
            row(1006, 1, false),
            row(1007, 2, false),
            row(1007, 1, true),
            row(1008, 1, false),
            row(1009, 2, false),
            row(9999, 3, false),
            row(9998, 2, true),
        ];
        let id = deck(&conn, None, "Qdeck", "CMODERN");
        let v = version_with(&conn, id, &signature(&cards), 0);

        let list = deck_list_cards(&conn, id, None).expect("decklist");
        assert_eq!(list.version_id, Some(v));
        let groups: Vec<GroupView> = list
            .main
            .iter()
            .map(|group| {
                (
                    group.type_name.as_str(),
                    group.count,
                    card_names(&group.cards),
                )
            })
            .collect();
        assert_eq!(
            groups,
            vec![
                ("Creature", 6, vec![("Qgoyf", 4), ("Qconstruct", 2)]),
                ("Planeswalker", 1, vec![("Qwalker", 1)]),
                ("Instant", 4, vec![("Qbolt", 4)]),
                ("Sorcery", 1, vec![("Qritual", 1)]),
                ("Enchantment", 1, vec![("Qaura", 1)]),
                ("Artifact", 2, vec![("Qrelic", 2)]),
                ("Land", 12, vec![("Qmountain", 10), ("Qforest", 2)]),
                ("Unknown", 3, vec![("Card 9999", 3)]),
            ]
        );
        assert_eq!(
            card_names(&list.side),
            vec![("Qrelic", 1), ("Card 9998", 2)]
        );

        let mountain = &list.main[6].cards[0];
        assert_eq!(mountain.type_line.as_deref(), Some("Land"));
        assert_eq!(
            mountain.image.as_deref(),
            Some("https://cards.scryfall.io/1004.jpg")
        );
        assert!(mountain.known);
        let unknown = &list.main[7].cards[0];
        assert_eq!(
            unknown,
            &ListCard {
                catalog_id: 9999,
                name: "Card 9999".to_string(),
                type_line: None,
                color_identity: None,
                image: None,
                art_crop: None,
                local_image: None,
                local_art_crop: None,
                cmc: None,
                mana_cost: None,
                quantity: 3,
                sideboard: false,
                known: false,
            }
        );
        assert!(list.side[1].sideboard);

        // Nonland main deck cards with card data: 4 + 4 + 2 + 1 + 1 + 2 + 1.
        assert_eq!(list.nonland_total, 15);
        assert_eq!(list.curve, vec![(1, 6), (2, 4), (3, 1), (4, 1), (7, 3)]);
        let colours: Vec<(&str, u32)> = list
            .colours
            .iter()
            .map(|(colour, count)| (colour.as_str(), *count))
            .collect();
        assert_eq!(
            colours,
            vec![("W", 1), ("U", 1), ("B", 1), ("R", 5), ("G", 4), ("C", 4)]
        );
    }

    #[test]
    fn the_sideboard_is_sorted_by_type_line_with_unknown_cards_last() {
        let (_dir, conn) = db("deck-pages-decklist-side");
        insert_card(&conn, 1002, "Qgoyf", Some("Creature"), Some("G"), Some(2.0));
        insert_card(&conn, 1007, "Qrelic", Some("Artifact"), None, Some(1.0));
        insert_card(
            &conn,
            1008,
            "Qaura",
            Some("Enchantment"),
            Some("W"),
            Some(3.0),
        );
        insert_card(&conn, 1010, "Qwall", Some("Creature"), Some("W"), Some(2.0));
        let id = deck(&conn, None, "Qdeck", "CMODERN");
        let cards = [
            row(1002, 2, true),
            row(1007, 1, true),
            row(1008, 1, true),
            row(1010, 3, true),
            row(1000, 2, true),
        ];
        version_with(&conn, id, &signature(&cards), 0);
        let list = deck_list_cards(&conn, id, None).expect("decklist");
        // 0.x `BuildDecklist` sorts every card by type (stable) before
        // splitting off the sideboard; 0.x drops unknown cards, v1 lists them
        // last.
        assert_eq!(
            card_names(&list.side),
            vec![
                ("Qrelic", 1),
                ("Qgoyf", 2),
                ("Qwall", 3),
                ("Qaura", 1),
                ("Card 1000", 2)
            ]
        );
    }

    #[test]
    fn a_brawl_deck_without_a_chosen_cover_shows_its_commanders_art() {
        let (_dir, conn) = db("deck-pages-commander-cover");
        insert_card(&conn, 1002, "Qgoyf", Some("Creature"), Some("G"), Some(2.0));
        for (id, name) in [(1003, "Qking"), (1004, "Qpal"), (1005, "Qbolt")] {
            insert_card(
                &conn,
                id,
                name,
                Some("Legendary Creature"),
                Some("G"),
                Some(3.0),
            );
            conn.execute(
                "UPDATE cards SET art_crop = ?2, local_art_crop = ?3 WHERE catalog_id = ?1",
                params![id, format!("https://art/{id}.jpg"), format!("{id}_art.jpg")],
            )
            .expect("art");
        }
        // Partners: the first commander row is the cover.
        let cards = [row(1002, 1, false), row(1003, 1, true), row(1004, 1, true)];
        let cover_of = |deck_id: i64| {
            let header = deck_header(&conn, deck_id, Range::all_time(), UTC, None)
                .expect("read")
                .expect("deck");
            (header.summary.cover_art, header.summary.local_cover_art)
        };

        let brawl = deck(&conn, None, "Qbrawl", "ARENA_HISTORIC_BRAWL");
        version_with(&conn, brawl, &signature(&cards), 0);
        assert_eq!(
            cover_of(brawl),
            (
                Some("https://art/1003.jpg".to_string()),
                Some("1003_art.jpg".to_string())
            )
        );
        let listed = deck_list(&conn, &filter()).expect("list");
        let card = listed.decks.iter().find(|deck| deck.id == brawl);
        assert_eq!(
            card.expect("listed").cover_art.as_deref(),
            Some("https://art/1003.jpg")
        );

        // A chosen cover card still wins.
        let chosen = deck(&conn, None, "Qchosen", "ARENA_BRAWL");
        version_with(&conn, chosen, &signature(&cards), 0);
        conn.execute(
            "UPDATE decks SET cover_catalog_id = 1005 WHERE id = ?1",
            [chosen],
        )
        .expect("cover");
        assert_eq!(cover_of(chosen).0.as_deref(), Some("https://art/1005.jpg"));

        // A chosen cover card wins even before its art is fetched.
        let pending = deck(&conn, None, "Qpending", "ARENA_BRAWL");
        version_with(&conn, pending, &signature(&cards), 0);
        insert_card(&conn, 1006, "Qnoart", Some("Instant"), Some("R"), Some(1.0));
        conn.execute(
            "UPDATE decks SET cover_catalog_id = 1006 WHERE id = ?1",
            [pending],
        )
        .expect("cover");
        assert_eq!(cover_of(pending), (None, None));

        // So does the deck's own cover image: no card art is added under it.
        let own = deck(&conn, None, "Qown", "ARENA_BRAWL");
        version_with(&conn, own, &signature(&cards), 0);
        conn.execute(
            "UPDATE decks SET cover_file = '1-5.png' WHERE id = ?1",
            [own],
        )
        .expect("file");
        assert_eq!(cover_of(own), (None, None));

        // Another format's sideboard is no commander.
        let modern = deck(&conn, None, "Qmodern", "CMODERN");
        version_with(&conn, modern, &signature(&cards), 0);
        assert_eq!(cover_of(modern), (None, None));
    }

    #[test]
    fn a_brawl_decklist_puts_the_commander_first() {
        let (_dir, conn) = db("deck-pages-decklist-commander");
        insert_card(&conn, 1002, "Qgoyf", Some("Creature"), Some("G"), Some(2.0));
        insert_card(
            &conn,
            1003,
            "Qking",
            Some("Legendary Creature"),
            Some("G"),
            Some(3.0),
        );
        insert_card(
            &conn,
            1004,
            "Qpal",
            Some("Legendary Creature"),
            Some("G"),
            Some(3.0),
        );
        let cards = [row(1002, 1, false), row(1003, 1, true), row(1004, 1, true)];
        let brawl = deck(&conn, None, "Qbrawl", "ARENA_HISTORIC_BRAWL");
        version_with(&conn, brawl, &signature(&cards), 0);
        let list = deck_list_cards(&conn, brawl, None).expect("decklist");
        assert_eq!(
            card_names(&list.commanders),
            vec![("Qking", 1), ("Qpal", 1)]
        );
        assert!(list.side.is_empty());
        assert_eq!(list.main[0].cards[0].name, "Qgoyf");

        let modern = deck(&conn, None, "Qmodern", "CMODERN");
        version_with(&conn, modern, &signature(&cards), 0);
        let list = deck_list_cards(&conn, modern, None).expect("decklist");
        assert!(list.commanders.is_empty());
        assert_eq!(card_names(&list.side), vec![("Qking", 1), ("Qpal", 1)]);

        let standard = deck(&conn, None, "Qstandard", "ARENA_STANDARD");
        version_with(&conn, standard, &signature(&cards), 0);
        let list = deck_list_cards(&conn, standard, None).expect("decklist");
        assert!(list.commanders.is_empty());
        assert_eq!(card_names(&list.side), vec![("Qking", 1), ("Qpal", 1)]);

        // MTGO Brawl keeps its sideboard: only the Arena codes are commander formats.
        let mtgo_brawl = deck(&conn, None, "Qcbrawl", "CBRAWL");
        version_with(&conn, mtgo_brawl, &signature(&cards), 0);
        let list = deck_list_cards(&conn, mtgo_brawl, None).expect("decklist");
        assert!(list.commanders.is_empty());
        assert_eq!(card_names(&list.side), vec![("Qking", 1), ("Qpal", 1)]);
    }

    #[test]
    fn a_stub_reads_as_card_id() {
        // A stub (a `cards` row without a name yet) is an unknown card on
        // every read: "Card {id}", listed last, never found by a search.
        let (_dir, mut conn) = db("deck-pages-stub");
        insert_card(&conn, 1001, "Qbolt", Some("Instant"), Some("R"), Some(1.0));
        let tx = conn.transaction().expect("tx");
        crate::cards::ensure_id_stubs(&tx, &[1000]).expect("stub");
        tx.commit().expect("commit");
        let id = deck(&conn, None, "Qdeck", "CMODERN");
        version_with(
            &conn,
            id,
            &signature(&[row(1000, 2, true), row(1001, 1, true)]),
            0,
        );
        let list = deck_list_cards(&conn, id, None).expect("decklist");
        assert_eq!(card_names(&list.side), vec![("Qbolt", 1), ("Card 1000", 2)]);
        assert!(!list.side[1].known);

        let infos = card_infos(&conn, &[1000, 1001]).expect("infos");
        assert_eq!(infos.keys().copied().collect::<Vec<u32>>(), vec![1001]);
        let found = crate::decks::search_cards(&conn, "", 20).expect("search");
        assert_eq!(
            found
                .iter()
                .map(|card| card.catalog_id)
                .collect::<Vec<u32>>(),
            vec![1001]
        );
        assert!(
            crate::decks::cover_options(&conn, "Card 1000")
                .expect("covers")
                .is_empty()
        );
    }

    #[test]
    fn the_decklist_shows_the_asked_version_else_the_latest() {
        let (_dir, conn) = db("deck-pages-decklist-version");
        insert_card(&conn, 1001, "Qbolt", Some("Instant"), Some("R"), Some(1.0));
        insert_card(&conn, 1002, "Qgoyf", Some("Creature"), Some("G"), Some(2.0));
        let id = deck(&conn, None, "Qdeck", "CMODERN");
        let old = version_with(&conn, id, &signature(&[row(1001, 4, false)]), 10);
        let latest = version_with(&conn, id, &signature(&[row(1002, 4, false)]), 20);
        let other_deck = deck(&conn, None, "Qother", "CMODERN");
        let foreign = version_with(&conn, other_deck, &signature(&[row(1001, 1, false)]), 30);

        let shown = |version: Option<i64>| {
            let list = deck_list_cards(&conn, id, version).expect("decklist");
            (list.version_id, list.main[0].cards[0].name.clone())
        };
        assert_eq!(shown(None), (Some(latest), "Qgoyf".to_string()));
        assert_eq!(shown(Some(old)), (Some(old), "Qbolt".to_string()));
        assert_eq!(shown(Some(foreign)), (Some(latest), "Qgoyf".to_string()));

        let empty = deck(&conn, None, "Qempty", "CMODERN");
        assert_eq!(
            deck_list_cards(&conn, empty, None).expect("decklist"),
            Decklist {
                version_id: None,
                commanders: Vec::new(),
                main: Vec::new(),
                side: Vec::new(),
                curve: Vec::new(),
                colours: Vec::new(),
                nonland_total: 0,
            }
        );
    }

    // Matches

    #[test]
    fn a_deck_s_matches_are_its_complete_matches_newest_first() {
        let (_dir, conn) = db("deck-pages-matches");
        let id = deck(&conn, None, "Qdeck", "CMODERN");
        let first = version(&conn, id, 0);
        let second = version(&conn, id, 1);
        let other = version(&conn, deck(&conn, None, "Qother", "CMODERN"), 0);
        let opener = won(&conn, first, 1_000);
        games(&conn, opener, 1_000, &["win", "loss", "win"]);
        lost(&conn, second, 3_000);
        played(&conn, second, 4_000, "in_progress", "unknown");
        won(&conn, other, 2_000);

        let page = deck_matches(&conn, id, None, Range::all_time(), 1, 10, None).expect("matches");
        let tokens: Vec<(i64, &str)> = page
            .rows
            .iter()
            .map(|row| (row.started_at_ms, row.outcome.as_str()))
            .collect();
        assert_eq!(tokens, vec![(3_000, "loss"), (1_000, "win")]);
        assert_eq!((page.total, page.page, page.per_page), (2, 1, 10));
        assert_eq!((page.rows[1].wins, page.rows[1].losses), (2, 1));
        assert_eq!(page.rows[1].opponent, None);

        let v1 = deck_matches(&conn, id, Some(first), Range::all_time(), 1, 10, None).expect("v1");
        assert_eq!(v1.total, 1);
        assert_eq!(v1.rows[0].started_at_ms, 1_000);

        let window = Range {
            from_ms: 2_000,
            to_ms: 5_000,
        };
        let ranged = deck_matches(&conn, id, None, window, 1, 10, None).expect("ranged");
        assert_eq!(ranged.total, 1);
        assert_eq!(ranged.rows[0].started_at_ms, 3_000);

        let second_page =
            deck_matches(&conn, id, None, Range::all_time(), 2, 1, None).expect("page 2");
        assert_eq!(second_page.total, 2);
        assert_eq!(second_page.rows[0].started_at_ms, 1_000);
    }

    // Hidden matches (Plan K, "Remove from stats")

    fn token_of(conn: &Connection, match_id: i64) -> String {
        conn.query_row(
            "SELECT token FROM matches WHERE id = ?1",
            [match_id],
            |row| row.get(0),
        )
        .expect("token")
    }

    #[test]
    fn a_hidden_match_counts_in_no_deck_page() {
        let (_dir, conn) = db("deck-pages-hidden");
        let local = account(&conn, "Qlocal");
        let id = deck(&conn, Some(local), "Qdeck", "CMODERN");
        let v = version(&conn, id, 0);
        let kept = won(&conn, v, NOW - 2 * HOUR);
        games(&conn, kept, NOW - 2 * HOUR, &["win", "win"]);
        let gone = lost(&conn, v, NOW - HOUR);
        games(&conn, gone, NOW - HOUR, &["loss", "win", "loss"]);
        // A deck whose only match is hidden has not been played.
        let only_hidden = deck(&conn, Some(local), "Qhidden only", "CMODERN");
        let hidden_version = version(&conn, only_hidden, 0);
        let lone = won(&conn, hidden_version, NOW);
        hide(&conn, &token_of(&conn, gone));
        hide(&conn, &token_of(&conn, lone));

        let list = deck_list(&conn, &filter()).expect("list");
        let card = list
            .decks
            .iter()
            .find(|deck| deck.id == id)
            .expect("the deck");
        assert_eq!(card.record, Record::from_counts(1, 0, 0));
        assert_eq!(card.last_played_at, Some(NOW - 2 * HOUR));
        let lone_card = list
            .decks
            .iter()
            .find(|deck| deck.id == only_hidden)
            .expect("the other deck");
        assert_eq!(lone_card.record, Record::default());
        assert_eq!(lone_card.last_played_at, None);
        let recent = deck_list(
            &conn,
            &DeckListFilter {
                recent: true,
                ..filter()
            },
        )
        .expect("recent");
        // Listed for its deck list, never for its hidden match.
        assert_eq!(
            names(&recent),
            vec!["Qdeck", "Qhidden only"],
            "recently played"
        );

        let header = deck_header(&conn, id, Range::all_time(), UTC, None)
            .expect("header")
            .expect("found");
        assert_eq!(header.summary.record, Record::from_counts(1, 0, 0));
        assert_eq!(header.versions[0].record, Record::from_counts(1, 0, 0));
        assert_eq!(header.versions[1].record, Record::from_counts(1, 0, 0));
        assert_eq!(header.versions[1].games, GameRecord::new(2, 0));

        let stats = dashboard(&conn, id, None, Range::all_time());
        assert_eq!(stats.record, Record::from_counts(1, 0, 0));
        assert_eq!(stats.games, GameRecord::new(2, 0));
        assert_eq!(stats.boarding, BoardingSplit::new(1, 0, 1, 0));
        let days: u32 = stats.daily.iter().map(|day| day.wins + day.losses).sum();
        assert_eq!(days, 1, "the daily chart");

        let page = deck_matches(&conn, id, None, Range::all_time(), 1, 10, None).expect("matches");
        assert_eq!(page.total, 1);
        let tokens: Vec<String> = page.rows.iter().map(|row| row.token.clone()).collect();
        assert_eq!(tokens, vec![token_of(&conn, kept)]);
    }

    #[test]
    fn deck_queries_walk_indexes_not_matches_or_games() {
        let (_dir, conn) = db("deck-pages-plans");
        for (name, sql) in [
            ("versions", VERSION_MATCHES.to_string()),
            ("version games", VERSION_GAMES.to_string()),
            ("version gameless", VERSION_GAMELESS.to_string()),
            ("boarding", BOARDING.to_string()),
            ("daily", DAILY_MATCHES.to_string()),
            ("daily earlier", DAILY_EARLIER_MATCHES.to_string()),
            ("header", header_sql()),
        ] {
            let query = plan(&conn, &sql);
            assert!(!query.contains("SCAN m"), "{name} plan:\n{query}");
            assert!(!query.contains("SCAN g"), "{name} plan:\n{query}");
            assert!(!query.contains("SCAN l"), "{name} plan:\n{query}");
        }
    }
    #[test]
    fn the_outcome_record_counts_every_match_with_an_outcome() {
        let (_dir, conn) = db("deck-outcome-record");
        let id = deck(&conn, None, "Q deck", "Modern");
        let first = version(&conn, id, 10);
        let second = version(&conn, id, 20);
        won(&conn, first, NOW - 3 * DAY);
        lost(&conn, second, NOW - 2 * DAY);
        played(&conn, second, NOW - DAY, "complete", "draw");
        played(&conn, second, NOW, "in_progress", "unknown");

        assert_eq!(
            deck_outcome_record(&conn, id, None).expect("all"),
            Record::from_counts(1, 1, 1)
        );
        assert_eq!(
            deck_outcome_record(&conn, id, Some(second)).expect("version"),
            Record::from_counts(0, 1, 1)
        );
    }

    // Kinds (Arena Bo1 and Bo3)

    /// An Arena deck with two Bo1 matches (a v1 win, a v2 loss), a newer Bo3
    /// win on v2 and a hidden Bo1 match newer still. Returns the deck and
    /// its versions.
    fn kinds_deck(conn: &Connection) -> (i64, i64, i64) {
        let arena = arena_account(conn);
        let id = deck(conn, Some(arena), "Qarena deck", "Standard");
        let first = version(conn, id, 0);
        let second = version(conn, id, 1);
        let one = won(conn, first, NOW - 4 * HOUR);
        games(conn, one, NOW - 4 * HOUR, &["win"]);
        let two = lost(conn, second, NOW - 3 * HOUR);
        games(conn, two, NOW - 3 * HOUR, &["loss"]);
        let three = won(conn, second, NOW - 2 * HOUR);
        games(conn, three, NOW - 2 * HOUR, &["win", "loss", "win"]);
        let hidden = won(conn, second, NOW - HOUR);
        for (match_id, kind) in [(one, "bo1"), (two, "bo1"), (three, "bo3"), (hidden, "bo1")] {
            set_game_structure(conn, &token_of(conn, match_id), kind);
        }
        hide(conn, &token_of(conn, hidden));
        (id, first, second)
    }

    #[test]
    fn deck_kinds_counts_each_kind() {
        let (_dir, conn) = db("deck-kinds-counts");
        let (id, first, _) = kinds_deck(&conn);
        // A live Bo1, the newest match: not counted, not the latest kind, as
        // the deck pages count complete matches only.
        let live = played(&conn, first, NOW - HOUR / 2, "in_progress", "unknown");
        set_game_structure(&conn, &token_of(&conn, live), "bo1");
        assert_eq!(
            deck_kinds(&conn, id).expect("kinds"),
            DeckKinds {
                bo1: 2,
                bo3: 1,
                latest: Some(BestOf::Three),
            }
        );
    }

    fn arena_list(conn: &Connection) -> DeckListPage {
        deck_list(
            conn,
            &DeckListFilter {
                platform: Platform::Arena,
                ..filter()
            },
        )
        .expect("list")
    }

    #[test]
    fn a_deck_card_lists_each_kind_played() {
        let (_dir, conn) = db("deck-card-kinds");
        kinds_deck(&conn);
        let page = arena_list(&conn);
        let card = &page.decks[0];
        assert_eq!(card.kinds, vec![BestOf::Three, BestOf::One]);
        assert_eq!(card.record, Record::from_counts(2, 1, 0));
    }

    #[test]
    fn a_deck_card_with_one_kind_lists_only_it() {
        let (_dir, conn) = db("deck-card-one-kind");
        let arena = arena_account(&conn);
        let id = deck(&conn, Some(arena), "Qarena deck", "Standard");
        let v = version(&conn, id, 0);
        let one = won(&conn, v, NOW - HOUR);
        games(&conn, one, NOW - HOUR, &["win"]);
        set_game_structure(&conn, &token_of(&conn, one), "bo1");
        assert_eq!(arena_list(&conn).decks[0].kinds, vec![BestOf::One]);
    }

    #[test]
    fn an_mtgo_deck_record_is_unchanged() {
        let (_dir, conn) = db("deck-card-mtgo-kind");
        let id = deck(&conn, None, "Qdeck", "CMODERN");
        let v = version(&conn, id, 0);
        let played = won(&conn, v, NOW);
        set_game_structure(&conn, &token_of(&conn, played), "Modern");
        let page = deck_list(&conn, &filter()).expect("list");
        assert_eq!(page.decks[0].kinds, Vec::<BestOf>::new());
        assert_eq!(page.decks[0].record, Record::from_counts(1, 0, 0));
    }

    #[test]
    fn deck_kinds_of_an_mtgo_deck_counts_nothing() {
        let (_dir, conn) = db("deck-kinds-mtgo");
        let id = deck(&conn, None, "Qdeck", "CMODERN");
        let v = version(&conn, id, 0);
        let played = won(&conn, v, NOW);
        set_game_structure(&conn, &token_of(&conn, played), "Modern");
        assert_eq!(
            deck_kinds(&conn, id).expect("kinds"),
            DeckKinds {
                bo1: 0,
                bo3: 0,
                latest: None,
            }
        );
    }

    #[test]
    fn the_deck_dashboard_reads_one_kind() {
        let (_dir, conn) = db("deck-kinds-dashboard");
        let (id, _, _) = kinds_deck(&conn);
        let all_time = Range::all_time();
        let bo1 =
            deck_dashboard(&conn, id, None, all_time, None, UTC, Some(BestOf::One)).expect("bo1");
        assert_eq!(bo1.record, Record::from_counts(1, 1, 0));
        assert_eq!(bo1.games, GameRecord::new(1, 1));
        assert_eq!(bo1.boarding, BoardingSplit::new(1, 1, 0, 0));
        let days: u32 = bo1.daily.iter().map(|day| day.wins + day.losses).sum();
        assert_eq!(days, 2, "the daily chart");
        let every = deck_dashboard(&conn, id, None, all_time, None, UTC, None).expect("all");
        assert_eq!(every.record, Record::from_counts(2, 1, 0));
        assert_eq!(every.games, GameRecord::new(3, 2));
    }

    #[test]
    fn a_deck_match_list_reads_one_kind() {
        let (_dir, conn) = db("deck-kinds-matches");
        let (id, _, second) = kinds_deck(&conn);
        let all_time = Range::all_time();
        let bo1 = deck_matches(&conn, id, None, all_time, 1, 10, Some(BestOf::One)).expect("bo1");
        assert_eq!(bo1.total, 2);
        assert_eq!(bo1.rows.len(), 2);
        let bo3 = deck_matches(
            &conn,
            id,
            Some(second),
            all_time,
            1,
            10,
            Some(BestOf::Three),
        )
        .expect("bo3");
        assert_eq!(bo3.total, 1);
        assert_eq!(bo3.rows[0].wins, 2);
        let every = deck_matches(&conn, id, None, all_time, 1, 10, None).expect("all");
        assert_eq!(every.total, 3);
        // The archetype filter counts follow the kind too.
        let scope = crate::match_list::Scope::Deck {
            deck_id: id,
            version: None,
            best_of: Some(BestOf::One),
        };
        let archetypes =
            crate::match_list::list_archetypes(&conn, scope, all_time, None).expect("archetypes");
        assert_eq!(archetypes.unknown, 2);
    }

    #[test]
    fn version_tallies_read_one_kind() {
        let (_dir, conn) = db("deck-kinds-versions");
        let (id, first, second) = kinds_deck(&conn);
        let tallies =
            version_tallies(&conn, id, Range::all_time(), Some(BestOf::One)).expect("tallies");
        let records: Vec<(i64, Record, GameRecord)> = tallies
            .iter()
            .map(|tally| (tally.id, tally.record, tally.games))
            .collect();
        assert_eq!(
            records,
            vec![
                (first, Record::from_counts(1, 0, 0), GameRecord::new(1, 0)),
                (second, Record::from_counts(0, 1, 0), GameRecord::new(0, 1)),
            ]
        );
        let header = deck_header(&conn, id, Range::all_time(), UTC, Some(BestOf::Three))
            .expect("header")
            .expect("found");
        assert_eq!(header.versions[0].record, Record::from_counts(1, 0, 0));
        assert_eq!(header.versions[1].record, Record::default());
    }
}
