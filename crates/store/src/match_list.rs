//! The match lists' filters, sorts and pages (0.x `BuildMatchListProps`):
//! the main Matches page (every listed match) and a deck's Matches page (its
//! complete matches). Both read the same rows (`read::MATCH_ROW_SELECT`).
//!
//! Filters: a start window (the caller turns 0.x's local `filter_from` and
//! `filter_to` days into it, and for a deck intersects it with the page's
//! timeframe), the result (win or loss) and the type. "League" is a match
//! in a league the player has not deleted, or a league match the sync has
//! not placed yet that the player has not taken out of every league
//! (`leagues::SHOWN_IN_A_LEAGUE`): lookups by key per match, much cheaper
//! than folding runs. "Casual" is every other match. "Manual" is a match the
//! player entered by hand (Plan L).
//!
//! Archetypes (archetypes spec section 12, 0.x `BuildMatchListProps`): each
//! row carries its opponent seat's archetype; the archetype filter keeps the
//! matches with no opponent archetype or against one; `list_archetypes`
//! counts the scope's matches without one and offers the format's
//! archetypes with how many matches faced each.
//!
//! Sorts (0.x `applySort`): start time (the default, newest first), outcome,
//! duration, the opponent archetype's name, and the result of game 1, 2 or
//! 3 (a win sorts above a loss when descending; a game with no result or no
//! such game sorts as SQL NULL, first when ascending). Ties keep the newest
//! match first.

use std::collections::HashMap;

use decks::stats::Range;
use rusqlite::{Connection, params};

use crate::StoreError;
use crate::leagues::SHOWN_IN_A_LEAGUE;
use crate::platform::{BestOf, Platform};
use crate::read::{MATCH_ROW_SELECT, MatchRow, match_row};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultFilter {
    Win,
    Loss,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindFilter {
    League,
    Casual,
    Manual,
    /// An Arena match of a ranked queue (`match_type = 'ranked'`).
    Ranked,
    /// An Arena match of an unranked queue (`match_type = 'unranked'`).
    /// MTGO's `match_type` holds structure text, so neither Arena filter
    /// matches an MTGO row.
    Unranked,
}

/// The archetype filter (0.x `filter_archetype`: `none` or an id).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchetypeFilter {
    /// Matches with no opponent archetype.
    Unknown,
    /// Matches whose opponent seat is this archetype.
    Is(i64),
}

/// The archetype a match's seat holds, as a list row shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeatArchetype {
    pub id: i64,
    pub name: String,
    /// Comma separated colours (`U,R`), `None` when unknown.
    pub color_identity: Option<String>,
    /// A commander archetype's cards: the match's opponent commanders
    /// (partners in seat order), for the hover preview. Empty for any other
    /// archetype.
    pub commander_cards: Vec<CommanderCard>,
}

/// One commander card of a seat, with what a preview shows. The card
/// columns are `None` without card data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommanderCard {
    pub catalog_id: i64,
    pub name: Option<String>,
    pub color_identity: Option<String>,
    pub image: Option<String>,
    pub local_image: Option<String>,
}

/// The opponent commanders of the match `m` as a JSON array in seat order
/// (`commander_cards` reads it). `read::MATCH_ROW_SELECT` holds the same
/// text (a test keeps them equal).
pub(crate) const OPPONENT_COMMANDERS_JSON: &str = "(SELECT json_group_array(json_object(
         'catalog_id', mc.catalog_id, 'name', c.name, 'color_identity', c.color_identity,
         'image', c.image, 'local_image', c.local_image) ORDER BY mc.position)
       FROM match_commanders mc LEFT JOIN cards c ON c.catalog_id = mc.catalog_id
       WHERE mc.match_id = m.id AND mc.seat = 'opponent')";

/// The cards of `OPPONENT_COMMANDERS_JSON`, empty for `None` (not a
/// commander archetype) or text that does not read.
pub(crate) fn commander_cards(json: Option<&str>) -> Vec<CommanderCard> {
    let Some(json) = json else {
        return Vec::new();
    };
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str(json) else {
        return Vec::new();
    };
    let text = |item: &serde_json::Value, key: &str| item[key].as_str().map(str::to_string);
    let mut cards = Vec::new();
    for item in &items {
        let Some(catalog_id) = item["catalog_id"].as_i64() else {
            continue;
        };
        cards.push(CommanderCard {
            catalog_id,
            name: text(item, "name"),
            color_identity: text(item, "color_identity"),
            image: text(item, "image"),
            local_image: text(item, "local_image"),
        });
    }
    cards
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    StartedAt,
    Outcome,
    Duration,
    /// The opponent archetype's name (no archetype sorts as SQL NULL).
    Archetype,
    Game1,
    Game2,
    Game3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

/// What a match list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchListFilter {
    /// Matches that started in this window (inclusive).
    pub range: Range,
    pub result: Option<ResultFilter>,
    pub kind: Option<KindFilter>,
    /// Arena Bo1 or Bo3 only (`game_structure`); `None` for every match.
    /// Combines with a deck scope's own kind by AND. MTGO lists pass `None`.
    pub best_of: Option<BestOf>,
    pub archetype: Option<ArchetypeFilter>,
    pub sort: SortKey,
    pub dir: SortDir,
}

impl MatchListFilter {
    /// Every match in `range`, newest first.
    pub fn newest_first(range: Range) -> Self {
        Self {
            range,
            result: None,
            kind: None,
            best_of: None,
            archetype: None,
            sort: SortKey::StartedAt,
            dir: SortDir::Desc,
        }
    }
}

/// Which matches a list starts from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Every listed match of the platform's accounts (the main Matches page).
    All(Platform),
    /// A deck's complete matches, of one version or all (a version id that
    /// does not exist means all, as 0.x `DeckVersion::find` gave null), of
    /// one Arena kind or every match (`None`, always for MTGO decks).
    Deck {
        deck_id: i64,
        version: Option<i64>,
        best_of: Option<BestOf>,
    },
    /// The complete matches of these deck versions: an archetype's decks on
    /// the deck index's Matches tab (0.x `Decks\Archetypes\MatchesController`).
    Versions(Vec<i64>),
}

/// The deck, version and kind parameters (`?1`, `?2`, `?3`) of a deck or
/// versions scope: the deck id or the versions as a JSON array, the version
/// (or NULL) and the kind (or NULL for every match).
fn deck_params(
    conn: &Connection,
    scope: &Scope,
) -> Result<Vec<rusqlite::types::Value>, StoreError> {
    use rusqlite::types::Value;
    Ok(match scope {
        Scope::All(_) => vec![Value::Null, Value::Null, Value::Null],
        Scope::Deck {
            deck_id,
            version,
            best_of,
        } => {
            let version = crate::deck_pages::existing_version(conn, *version)?;
            let best_of = match best_of {
                Some(kind) => Value::Text(kind.key().to_string()),
                None => Value::Null,
            };
            vec![
                Value::Integer(*deck_id),
                version.map_or(Value::Null, Value::Integer),
                best_of,
            ]
        }
        Scope::Versions(ids) => vec![
            Value::Text(crate::matchups::ids_json(ids)),
            Value::Null,
            Value::Null,
        ],
    })
}

/// One page of a match list.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchListPage {
    pub rows: Vec<MatchRow>,
    /// Matches over every page.
    pub total: u32,
    pub page: u32,
    pub per_page: u32,
}

/// One page (1-based; 0 counts as 1) of `scope`'s matches through `filter`.
pub fn match_list(
    conn: &Connection,
    scope: Scope,
    filter: &MatchListFilter,
    page: u32,
    per_page: u32,
) -> Result<MatchListPage, StoreError> {
    let page = page.max(1);
    let per_page = per_page.max(1);
    let offset = (page - 1).saturating_mul(per_page);
    let from = filter.range.from_ms;
    let to = filter.range.to_ms;
    let mut rows = Vec::new();
    let total: u32;
    match scope {
        Scope::All(platform) => {
            total = conn
                .prepare_cached(&count_sql(&scope, filter))?
                .query_row(params![from, to, platform], |row| row.get(0))?;
            let mut stmt = conn.prepare_cached(&rows_sql(&scope, filter))?;
            let found = stmt.query_map(params![from, to, platform, per_page, offset], match_row)?;
            for row in found {
                rows.push(row?);
            }
        }
        Scope::Deck { .. } | Scope::Versions(_) => {
            use rusqlite::types::Value;
            let mut values = deck_params(conn, &scope)?;
            values.push(Value::Integer(from));
            values.push(Value::Integer(to));
            total = conn
                .prepare_cached(&count_sql(&scope, filter))?
                .query_row(rusqlite::params_from_iter(values.iter()), |row| row.get(0))?;
            values.push(Value::Integer(i64::from(per_page)));
            values.push(Value::Integer(i64::from(offset)));
            let mut stmt = conn.prepare_cached(&rows_sql(&scope, filter))?;
            let found = stmt.query_map(rusqlite::params_from_iter(values.iter()), match_row)?;
            for row in found {
                rows.push(row?);
            }
        }
    }
    Ok(MatchListPage {
        rows,
        total,
        page,
        per_page,
    })
}

/// The scope's joins and conditions, then the filter's. `All` binds the
/// window as `?1` and `?2` and the platform as `?3`; `Deck` binds the deck
/// `?1`, the version `?2` (NULL for all), the kind `?3` (NULL for every
/// match) and the window as `?4` and `?5`; `Versions` binds the versions as
/// a JSON array `?1`, NULL `?2`, NULL `?3` and the window.
fn scope_sql(scope: &Scope, filter: &MatchListFilter) -> (&'static str, String) {
    let (joins, mut conditions) = match scope {
        Scope::All(_) => (
            "",
            "m.started_at BETWEEN ?1 AND ?2
  AND m.account_id IN (SELECT id FROM accounts WHERE platform = ?3)"
                .to_string(),
        ),
        Scope::Deck { .. } => (
            "\nJOIN deck_versions v ON v.id = m.deck_version_id",
            "v.deck_id = ?1 AND (?2 IS NULL OR v.id = ?2)
  AND (?3 IS NULL OR m.game_structure = ?3)
  AND m.state = 'complete' AND m.started_at BETWEEN ?4 AND ?5"
                .to_string(),
        ),
        Scope::Versions(_) => (
            "\nJOIN deck_versions v ON v.id = m.deck_version_id",
            "v.id IN (SELECT value FROM json_each(?1)) AND (?2 IS NULL OR v.id = ?2)
  AND (?3 IS NULL OR m.game_structure = ?3)
  AND m.state = 'complete' AND m.started_at BETWEEN ?4 AND ?5"
                .to_string(),
        ),
    };
    match filter.result {
        Some(ResultFilter::Win) => conditions.push_str("\n  AND m.outcome = 'win'"),
        Some(ResultFilter::Loss) => conditions.push_str("\n  AND m.outcome = 'loss'"),
        None => {}
    }
    match filter.kind {
        Some(KindFilter::League) => {
            conditions.push_str("\n  AND ");
            conditions.push_str(SHOWN_IN_A_LEAGUE);
        }
        Some(KindFilter::Casual) => {
            conditions.push_str("\n  AND NOT ");
            conditions.push_str(SHOWN_IN_A_LEAGUE);
        }
        // A match the player entered by hand (Plan L).
        Some(KindFilter::Manual) => conditions.push_str("\n  AND m.source = 'manual'"),
        Some(KindFilter::Ranked) => conditions.push_str("\n  AND m.match_type = 'ranked'"),
        Some(KindFilter::Unranked) => conditions.push_str("\n  AND m.match_type = 'unranked'"),
        None => {}
    }
    // The kind's key is fixed text (`bo1`, `bo3`), so writing it in is safe
    // and no placeholder shifts.
    if let Some(kind) = filter.best_of {
        conditions.push_str(&format!("\n  AND m.game_structure = '{}'", kind.key()));
    }
    match filter.archetype {
        Some(ArchetypeFilter::Unknown) => conditions.push_str(
            "\n  AND NOT EXISTS (SELECT 1 FROM match_archetypes x
                  WHERE x.match_id = m.id AND x.is_opponent = 1)",
        ),
        // An integer id, so writing it into the text is safe.
        Some(ArchetypeFilter::Is(id)) => conditions.push_str(&format!(
            "\n  AND EXISTS (SELECT 1 FROM match_archetypes x
                  WHERE x.match_id = m.id AND x.is_opponent = 1 AND x.archetype_id = {id})"
        )),
        None => {}
    }
    (joins, conditions)
}

/// The `ORDER BY` terms of a sort.
fn order_sql(filter: &MatchListFilter) -> String {
    let dir = match filter.dir {
        SortDir::Asc => "ASC",
        SortDir::Desc => "DESC",
    };
    let newest = "m.started_at DESC, m.id DESC";
    let game = |offset: u32| {
        format!(
            "(SELECT CASE g.result WHEN 'win' THEN 1 WHEN 'loss' THEN 0 END FROM games g
   WHERE g.match_id = m.id ORDER BY g.game_number LIMIT 1 OFFSET {offset}) {dir}, {newest}"
        )
    };
    match filter.sort {
        SortKey::StartedAt => format!("m.started_at {dir}, m.id {dir}"),
        SortKey::Outcome => format!("m.outcome {dir}, {newest}"),
        SortKey::Duration => format!("(m.ended_at - m.started_at) {dir}, {newest}"),
        // `oa` is `MATCH_ROW_SELECT`'s opponent archetype.
        SortKey::Archetype => format!("oa.name {dir}, {newest}"),
        SortKey::Game1 => game(0),
        SortKey::Game2 => game(1),
        SortKey::Game3 => game(2),
    }
}

/// One page of rows: the limit and offset follow the scope's parameters.
fn rows_sql(scope: &Scope, filter: &MatchListFilter) -> String {
    let (joins, conditions) = scope_sql(scope, filter);
    let (limit, offset) = match scope {
        Scope::All(_) => ("?4", "?5"),
        Scope::Deck { .. } | Scope::Versions(_) => ("?6", "?7"),
    };
    format!(
        "{MATCH_ROW_SELECT}{joins}\nWHERE {conditions}\nORDER BY {}\nLIMIT {limit} OFFSET {offset}",
        order_sql(filter)
    )
}

/// How many rows `rows_sql` has over every page.
fn count_sql(scope: &Scope, filter: &MatchListFilter) -> String {
    let (_, conditions) = scope_sql(scope, filter);
    let from = match scope {
        Scope::All(_) => "listed_matches m",
        Scope::Deck { .. } | Scope::Versions(_) => {
            "deck_versions v
JOIN listed_matches m ON m.deck_version_id = v.id"
        }
    };
    format!("SELECT COUNT(*) FROM {from}\nWHERE {conditions}")
}

/// The opponent seat's archetype of the match `match_id`.
pub fn opponent_archetype(
    conn: &Connection,
    match_id: i64,
) -> Result<Option<SeatArchetype>, StoreError> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .prepare_cached(&format!(
            "SELECT a.id, a.name, a.color_identity,
                     CASE WHEN a.source = 'commander' THEN {OPPONENT_COMMANDERS_JSON} END
                 FROM (SELECT ?1 AS id) m
                 JOIN match_archetypes x ON x.match_id = m.id AND x.is_opponent = 1
                 JOIN archetypes a ON a.id = x.archetype_id"
        ))?
        .query_row([match_id], |row| {
            Ok(SeatArchetype {
                id: row.get(0)?,
                name: row.get(1)?,
                color_identity: row.get(2)?,
                commander_cards: commander_cards(row.get::<_, Option<String>>(3)?.as_deref()),
            })
        })
        .optional()?)
}

/// Every archetype a picker can offer when no format narrows it (a bulk
/// set over matches of several formats, 0.x's null format): the ones not
/// retired, by name, then the fallbacks.
pub fn every_archetype_option(
    conn: &Connection,
) -> Result<Vec<crate::archetypes::ArchetypeOption>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, uuid, name, format, color_identity, source, offline_held FROM archetypes
         WHERE retired_at IS NULL
         ORDER BY source = 'fallback', name, id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, bool>(6)?,
        ))
    })?;
    let mut options = Vec::new();
    for row in rows {
        let (id, uuid, name, format, color_identity, source, offline_held) = row?;
        let source = archetypes::Source::parse(&source)
            .ok_or_else(|| StoreError::Corrupt(format!("archetype source {source}")))?;
        options.push(crate::archetypes::ArchetypeOption {
            id,
            uuid,
            name,
            format,
            color_identity,
            source,
            offline_held,
        });
    }
    Ok(options)
}

/// The ids of the listed matches among `tokens`, in the order given; a
/// token with no listed match is left out.
pub fn listed_match_ids(conn: &Connection, tokens: &[String]) -> Result<Vec<i64>, StoreError> {
    use rusqlite::OptionalExtension;
    let mut stmt = conn.prepare_cached("SELECT id FROM listed_matches WHERE token = ?1")?;
    let mut ids = Vec::new();
    for token in tokens {
        if let Some(id) = stmt.query_row([token], |row| row.get(0)).optional()? {
            ids.push(id);
        }
    }
    Ok(ids)
}

/// One archetype the list's filter offers, with the scope's matches
/// against it (0.x `archetypeOptions`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchetypeCount {
    pub id: i64,
    pub name: String,
    /// The archetypes format key (`modern`); `None` for a fallback.
    pub format: Option<String>,
    pub color_identity: Option<String>,
    pub is_fallback: bool,
    pub matches: u32,
}

/// What the list's archetype filter shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListArchetypes {
    /// Complete matches of the scope with no opponent archetype (0.x
    /// `unknownArchetypeCount`).
    pub unknown: u32,
    pub options: Vec<ArchetypeCount>,
}

/// The scope's complete matches in `range`: `All` binds the window as `?1`
/// and `?2` and the platform as `?3`; `Deck` the deck `?1`, the version `?2`,
/// the kind `?3` and the window `?4`, `?5`.
fn complete_scope_sql(scope: &Scope) -> &'static str {
    match scope {
        Scope::All(_) => {
            "listed_matches m
WHERE m.state = 'complete' AND m.started_at BETWEEN ?1 AND ?2
  AND m.account_id IN (SELECT id FROM accounts WHERE platform = ?3)"
        }
        Scope::Deck { .. } => {
            "deck_versions v
JOIN listed_matches m ON m.deck_version_id = v.id
WHERE v.deck_id = ?1 AND (?2 IS NULL OR v.id = ?2)
  AND (?3 IS NULL OR m.game_structure = ?3)
  AND m.state = 'complete' AND m.started_at BETWEEN ?4 AND ?5"
        }
        Scope::Versions(_) => {
            "deck_versions v
JOIN listed_matches m ON m.deck_version_id = v.id
WHERE v.id IN (SELECT value FROM json_each(?1)) AND (?2 IS NULL OR v.id = ?2)
  AND (?3 IS NULL OR m.game_structure = ?3)
  AND m.state = 'complete' AND m.started_at BETWEEN ?4 AND ?5"
        }
    }
}

/// The scope's parameters for `complete_scope_sql`.
fn scope_params(
    conn: &Connection,
    scope: &Scope,
    range: Range,
) -> Result<Vec<rusqlite::types::Value>, StoreError> {
    use rusqlite::types::Value;
    Ok(match scope {
        Scope::All(platform) => vec![
            Value::Integer(range.from_ms),
            Value::Integer(range.to_ms),
            Value::Text(platform.as_str().to_string()),
        ],
        Scope::Deck { .. } | Scope::Versions(_) => {
            let mut values = deck_params(conn, scope)?;
            values.push(Value::Integer(range.from_ms));
            values.push(Value::Integer(range.to_ms));
            values
        }
    })
}

/// The archetype filter of a list over `scope`'s complete matches in
/// `range`, before the list's own filters (0.x `BuildMatchListProps`): how
/// many have no opponent archetype, and the archetypes offered with the
/// matches against each, most faced first, then by name. With a format key
/// (a deck's lists) they are that format's archetypes that are not retired
/// and the fallbacks (0.x `forFormat`); without one (the main list) the
/// archetypes faced and the fallbacks. An archetype faced is always offered.
pub fn list_archetypes(
    conn: &Connection,
    scope: Scope,
    range: Range,
    format: Option<&str>,
) -> Result<ListArchetypes, StoreError> {
    let from = complete_scope_sql(&scope);
    let values = scope_params(conn, &scope, range)?;
    let unknown: u32 = conn
        .prepare_cached(&format!(
            "SELECT COUNT(*) FROM {from}
  AND NOT EXISTS (SELECT 1 FROM match_archetypes x
                  WHERE x.match_id = m.id AND x.is_opponent = 1)"
        ))?
        .query_row(rusqlite::params_from_iter(values.iter()), |row| row.get(0))?;

    let mut counts: HashMap<i64, u32> = HashMap::new();
    {
        // The join goes before the WHERE clause.
        let (head, conditions) = from.split_once("\nWHERE ").unwrap_or((from, "1"));
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT x.archetype_id, COUNT(*) FROM {head}
JOIN match_archetypes x ON x.match_id = m.id AND x.is_opponent = 1
WHERE {conditions}
GROUP BY x.archetype_id"
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(values.iter()), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, u32>(1)?))
        })?;
        for row in rows {
            let (id, count) = row?;
            counts.insert(id, count);
        }
    }

    let faced: Vec<i64> = counts.keys().copied().collect();
    let faced_json = serde_json::to_string(&faced).unwrap_or_else(|_| "[]".to_string());
    let mut options = Vec::new();
    {
        let mut stmt = conn.prepare_cached(
            "SELECT id, name, format, color_identity, source, retired_at FROM archetypes
             WHERE (?1 IS NOT NULL AND format = ?1) OR source = 'fallback'
                OR id IN (SELECT value FROM json_each(?2))",
        )?;
        let rows = stmt.query_map(params![format, faced_json], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<i64>>(5)?,
            ))
        })?;
        for row in rows {
            let (id, name, format, color_identity, source, retired_at) = row?;
            let matches = counts.get(&id).copied().unwrap_or(0);
            if retired_at.is_some() && matches == 0 {
                continue;
            }
            options.push(ArchetypeCount {
                id,
                name,
                format,
                color_identity,
                is_fallback: source == "fallback",
                matches,
            });
        }
    }
    options.sort_by(|a, b| {
        b.matches
            .cmp(&a.matches)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(ListArchetypes { unknown, options })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archetypes::tests::{insert_archetype, seat};
    use crate::open_writer;
    use crate::test_support::{hide, insert_manual_league, link_league_match};
    use fixture_tools::TestDir;
    use rusqlite::OptionalExtension;

    const HOUR: i64 = 3_600_000;
    /// 2026-10-07T12:00:00Z.
    const NOW: i64 = 1_791_374_400_000;

    fn db(label: &str) -> (TestDir, Connection) {
        let dir = TestDir::new(label);
        let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        conn.execute_batch(
            "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);
             INSERT INTO decks (id, mtgo_id, account_id, name, format, created_at, updated_at, source)
                 VALUES (5, 'q-deck', 1, 'Qburn', 'CMODERN', 0, 0, 'file'),
                        (6, 'q-other', 1, 'Qother', 'CMODERN', 0, 0, 'file');
             INSERT INTO deck_versions (id, deck_id, signature, modified_at)
                 VALUES (10, 5, 'SIG-1', 0), (11, 5, 'SIG-2', 0), (12, 6, 'SIG-3', 0);",
        )
        .expect("seed");
        (dir, conn)
    }

    /// A match `token` started at `started_at`, lasting `minutes`, with game
    /// results in order. Returns the match id.
    fn played(
        conn: &Connection,
        token: &str,
        started_at: i64,
        minutes: Option<i64>,
        state: &str,
        outcome: &str,
        games: &[&str],
    ) -> i64 {
        let ended_at = minutes.map(|minutes| started_at + minutes * 60_000);
        let id: i64 = conn
            .query_row(
                "INSERT INTO matches (token, account_id, source, state, outcome, started_at,
                     ended_at, last_activity_at, revision, derived_version)
                 VALUES (?1, 1, 'log', ?2, ?3, ?4, ?5, ?4, 1, 1) RETURNING id",
                params![token, state, outcome, started_at, ended_at],
                |row| row.get(0),
            )
            .expect("match");
        for (index, result) in games.iter().enumerate() {
            conn.execute(
                "INSERT INTO games (match_id, game_number, result, end_reason, started_at)
                 VALUES (?1, ?2, ?3, 'win', ?4)",
                params![id, index as i64 + 1, result, started_at + index as i64],
            )
            .expect("game");
        }
        id
    }

    fn link(conn: &Connection, token: &str, version: i64) {
        conn.execute(
            "UPDATE matches SET deck_version_id = ?2 WHERE token = ?1",
            params![token, version],
        )
        .expect("link");
    }

    fn tokens(page: &MatchListPage) -> Vec<&str> {
        page.rows.iter().map(|row| row.token.as_str()).collect()
    }

    fn list(conn: &Connection, scope: Scope, filter: &MatchListFilter) -> Vec<String> {
        match_list(conn, scope, filter, 1, 50)
            .expect("list")
            .rows
            .into_iter()
            .map(|row| row.token)
            .collect()
    }

    fn all() -> MatchListFilter {
        MatchListFilter::newest_first(Range::all_time())
    }

    fn sorted(sort: SortKey, dir: SortDir) -> MatchListFilter {
        MatchListFilter { sort, dir, ..all() }
    }

    /// Five complete deck matches and one in progress, plus one of another deck.
    fn seeded(label: &str) -> (TestDir, Connection) {
        let (dir, conn) = db(label);
        played(
            &conn,
            "q-a",
            NOW - 5 * HOUR,
            Some(20),
            "complete",
            "win",
            &["win", "win"],
        );
        played(
            &conn,
            "q-b",
            NOW - 4 * HOUR,
            Some(50),
            "complete",
            "loss",
            &["loss", "win", "loss"],
        );
        played(
            &conn,
            "q-c",
            NOW - 3 * HOUR,
            Some(35),
            "complete",
            "win",
            &["loss", "win", "win"],
        );
        played(
            &conn,
            "q-d",
            NOW - 2 * HOUR,
            None,
            "complete",
            "draw",
            &["unknown"],
        );
        played(
            &conn,
            "q-e",
            NOW - HOUR,
            Some(10),
            "complete",
            "loss",
            &["loss", "loss"],
        );
        played(
            &conn,
            "q-live",
            NOW,
            None,
            "in_progress",
            "unknown",
            &["win"],
        );
        played(
            &conn,
            "q-other",
            NOW - 6 * HOUR,
            Some(5),
            "complete",
            "win",
            &["win", "win"],
        );
        for token in ["q-a", "q-b", "q-c", "q-live"] {
            link(&conn, token, 10);
        }
        for token in ["q-d", "q-e"] {
            link(&conn, token, 11);
        }
        link(&conn, "q-other", 12);
        (dir, conn)
    }

    const DECK: Scope = Scope::Deck {
        deck_id: 5,
        version: None,
        best_of: None,
    };

    #[test]
    fn a_deck_lists_its_complete_matches_newest_first_and_the_main_list_every_match() {
        let (_dir, conn) = seeded("match-list-scopes");
        assert_eq!(
            list(&conn, DECK, &all()),
            ["q-e", "q-d", "q-c", "q-b", "q-a"]
        );
        let version = Scope::Deck {
            deck_id: 5,
            version: Some(11),
            best_of: None,
        };
        assert_eq!(list(&conn, version, &all()), ["q-e", "q-d"]);
        let stale = Scope::Deck {
            deck_id: 5,
            version: Some(99_999),
            best_of: None,
        };
        assert_eq!(list(&conn, stale, &all()).len(), 5);
        assert_eq!(
            list(&conn, Scope::All(Platform::Mtgo), &all()),
            ["q-live", "q-e", "q-d", "q-c", "q-b", "q-a", "q-other"]
        );
    }

    #[test]
    fn the_main_list_reads_one_platform() {
        let (_dir, conn) = seeded("match-list-arena");
        crate::test_support::move_match_to_arena(&conn, "q-other");
        assert_eq!(
            list(&conn, Scope::All(Platform::Mtgo), &all()),
            ["q-live", "q-e", "q-d", "q-c", "q-b", "q-a"]
        );
        let page = match_list(&conn, Scope::All(Platform::Mtgo), &all(), 1, 50).expect("page");
        assert_eq!(page.total, 6);
        assert_eq!(
            list(&conn, Scope::All(Platform::Arena), &all()),
            ["q-other"]
        );
        // The archetype filter counts the platform's matches only.
        let mtgo = list_archetypes(&conn, Scope::All(Platform::Mtgo), Range::all_time(), None)
            .expect("mtgo");
        let arena = list_archetypes(&conn, Scope::All(Platform::Arena), Range::all_time(), None)
            .expect("arena");
        assert_eq!(
            (mtgo.unknown, arena.unknown),
            (5, 1),
            "no archetypes seeded"
        );
    }

    #[test]
    fn the_window_keeps_matches_started_inside_it() {
        let (_dir, conn) = seeded("match-list-window");
        let window = MatchListFilter {
            range: Range {
                from_ms: NOW - 4 * HOUR,
                to_ms: NOW - 2 * HOUR,
            },
            ..all()
        };
        assert_eq!(list(&conn, DECK, &window), ["q-d", "q-c", "q-b"]);
        assert_eq!(
            list(&conn, Scope::All(Platform::Mtgo), &window),
            ["q-d", "q-c", "q-b"]
        );
    }

    #[test]
    fn the_result_filter_keeps_wins_or_losses() {
        let (_dir, conn) = seeded("match-list-result");
        let wins = MatchListFilter {
            result: Some(ResultFilter::Win),
            ..all()
        };
        let losses = MatchListFilter {
            result: Some(ResultFilter::Loss),
            ..all()
        };
        assert_eq!(list(&conn, DECK, &wins), ["q-c", "q-a"]);
        assert_eq!(list(&conn, DECK, &losses), ["q-e", "q-b"]);
        assert_eq!(
            list(&conn, Scope::All(Platform::Mtgo), &wins),
            ["q-c", "q-a", "q-other"]
        );
    }

    #[test]
    fn league_means_a_league_token_or_an_imported_or_manual_run_and_casual_the_rest() {
        let (_dir, conn) = seeded("match-list-type");
        conn.execute_batch(
            "UPDATE matches SET league_token = 'q-league' WHERE token = 'q-a';
             INSERT INTO leagues (id, run_key, source, kind, state, token, name, started_at,
                 created_at, imported_from, imported_seed)
                 VALUES (3, '0x:3', 'imported', 'constructed', 'complete', 'q-run', 'Q league', 0,
                     0, 3, '{}');
             UPDATE matches SET league_id = 3, league_linked = 1 WHERE token = 'q-b';",
        )
        .expect("leagues");
        insert_manual_league(&conn, 4, Some(1), Some(5), "Q manual", 0, 0);
        link_league_match(&conn, 4, "q-c");
        let league = MatchListFilter {
            kind: Some(KindFilter::League),
            ..all()
        };
        let casual = MatchListFilter {
            kind: Some(KindFilter::Casual),
            ..all()
        };
        let manual = MatchListFilter {
            kind: Some(KindFilter::Manual),
            ..all()
        };
        assert_eq!(list(&conn, DECK, &league), ["q-c", "q-b", "q-a"]);
        assert_eq!(list(&conn, DECK, &casual), ["q-e", "q-d"]);
        assert!(
            list(&conn, DECK, &manual).is_empty(),
            "none of these matches was entered by hand"
        );
        let both = MatchListFilter {
            result: Some(ResultFilter::Win),
            kind: Some(KindFilter::League),
            ..all()
        };
        assert_eq!(
            list(&conn, Scope::All(Platform::Mtgo), &both),
            ["q-c", "q-a"]
        );
    }

    #[test]
    fn the_best_of_filter_keeps_one_kind_in_every_scope() {
        let (_dir, conn) = seeded("match-list-best-of");
        conn.execute_batch(
            "UPDATE matches SET game_structure = 'bo1' WHERE token IN ('q-a', 'q-c', 'q-d');
             UPDATE matches SET game_structure = 'bo3' WHERE token IN ('q-b', 'q-e');",
        )
        .expect("kinds");
        let bo1 = MatchListFilter {
            best_of: Some(BestOf::One),
            ..all()
        };
        let bo3 = MatchListFilter {
            best_of: Some(BestOf::Three),
            ..all()
        };
        assert_eq!(list(&conn, DECK, &bo1), ["q-d", "q-c", "q-a"]);
        assert_eq!(list(&conn, DECK, &bo3), ["q-e", "q-b"]);
        let versions = Scope::Versions(vec![10, 11]);
        assert_eq!(list(&conn, versions.clone(), &bo1), ["q-d", "q-c", "q-a"]);
        assert_eq!(list(&conn, versions.clone(), &bo3), ["q-e", "q-b"]);
        let page = match_list(&conn, versions, &bo3, 1, 50).expect("page");
        assert_eq!(page.total, 2);
        // The main list: the Arena platform holds none of these rows, so the
        // kind narrows within the platform.
        let main = Scope::All(Platform::Mtgo);
        let mtgo_bo1 = list(&conn, main.clone(), &bo1);
        assert!(
            mtgo_bo1
                .iter()
                .all(|t| ["q-a", "q-c", "q-d"].contains(&t.as_str()))
        );
        assert!(mtgo_bo1.contains(&"q-a".to_string()));
        assert!(!mtgo_bo1.contains(&"q-b".to_string()));
        // It combines with a deck scope's own kind by AND.
        let deck_bo1 = Scope::Deck {
            deck_id: 5,
            version: None,
            best_of: Some(BestOf::One),
        };
        assert!(list(&conn, deck_bo1, &bo3).is_empty());
        // And with the other filters.
        let bo1_wins = MatchListFilter {
            best_of: Some(BestOf::One),
            result: Some(ResultFilter::Win),
            ..all()
        };
        assert_eq!(list(&conn, DECK, &bo1_wins), ["q-c", "q-a"]);
    }

    #[test]
    fn the_ranked_filter_keeps_ranked_matches() {
        let (_dir, conn) = seeded("match-list-ranked");
        conn.execute_batch(
            "UPDATE matches SET match_type = 'ranked' WHERE token IN ('q-a', 'q-c');
             UPDATE matches SET match_type = 'unranked' WHERE token = 'q-b';",
        )
        .expect("types");
        let ranked = MatchListFilter {
            kind: Some(KindFilter::Ranked),
            ..all()
        };
        assert_eq!(list(&conn, DECK, &ranked), ["q-c", "q-a"]);
    }

    #[test]
    fn the_unranked_filter_keeps_unranked_matches() {
        let (_dir, conn) = seeded("match-list-unranked");
        conn.execute_batch(
            "UPDATE matches SET match_type = 'ranked' WHERE token IN ('q-a', 'q-c');
             UPDATE matches SET match_type = 'unranked' WHERE token = 'q-b';",
        )
        .expect("types");
        let unranked = MatchListFilter {
            kind: Some(KindFilter::Unranked),
            ..all()
        };
        assert_eq!(list(&conn, DECK, &unranked), ["q-b"]);
    }

    #[test]
    fn a_match_of_a_removed_league_or_taken_out_of_leagues_is_casual() {
        let (_dir, conn) = seeded("match-list-removed-league");
        insert_manual_league(&conn, 4, Some(1), Some(5), "Q manual", 0, 0);
        link_league_match(&conn, 4, "q-c");
        conn.execute_batch(
            "UPDATE matches SET league_token = 'q-league' WHERE token IN ('q-a', 'q-b');
             UPDATE matches SET no_league = 1 WHERE token = 'q-b';
             UPDATE leagues SET removed_at = 5 WHERE id = 4;",
        )
        .expect("edits");
        let league = MatchListFilter {
            kind: Some(KindFilter::League),
            ..all()
        };
        let casual = MatchListFilter {
            kind: Some(KindFilter::Casual),
            ..all()
        };
        assert_eq!(list(&conn, DECK, &league), ["q-a"]);
        assert_eq!(list(&conn, DECK, &casual), ["q-e", "q-d", "q-c", "q-b"]);
    }

    #[test]
    fn sorts_by_start_outcome_and_duration_each_way() {
        let (_dir, conn) = seeded("match-list-sorts");
        assert_eq!(
            list(&conn, DECK, &sorted(SortKey::StartedAt, SortDir::Asc)),
            ["q-a", "q-b", "q-c", "q-d", "q-e"]
        );
        // Outcome text order (0.x sorted the enum's string): draw, loss, win;
        // ties newest first.
        assert_eq!(
            list(&conn, DECK, &sorted(SortKey::Outcome, SortDir::Asc)),
            ["q-d", "q-e", "q-b", "q-c", "q-a"]
        );
        assert_eq!(
            list(&conn, DECK, &sorted(SortKey::Outcome, SortDir::Desc)),
            ["q-c", "q-a", "q-e", "q-b", "q-d"]
        );
        // No end time sorts as NULL: first ascending, last descending.
        assert_eq!(
            list(&conn, DECK, &sorted(SortKey::Duration, SortDir::Asc)),
            ["q-d", "q-e", "q-a", "q-c", "q-b"]
        );
        assert_eq!(
            list(&conn, DECK, &sorted(SortKey::Duration, SortDir::Desc)),
            ["q-b", "q-c", "q-a", "q-e", "q-d"]
        );
    }

    #[test]
    fn sorts_by_the_result_of_game_one_two_or_three() {
        let (_dir, conn) = seeded("match-list-game-sorts");
        // Game 1: a win, loss, loss, unknown, loss.
        assert_eq!(
            list(&conn, DECK, &sorted(SortKey::Game1, SortDir::Desc)),
            ["q-a", "q-e", "q-c", "q-b", "q-d"]
        );
        assert_eq!(
            list(&conn, DECK, &sorted(SortKey::Game1, SortDir::Asc)),
            ["q-d", "q-e", "q-c", "q-b", "q-a"]
        );
        // Game 2: a win, b win, c win, d none, e loss.
        assert_eq!(
            list(&conn, DECK, &sorted(SortKey::Game2, SortDir::Asc)),
            ["q-d", "q-e", "q-c", "q-b", "q-a"]
        );
        // Game 3: only b (loss) and c (win) went to three.
        assert_eq!(
            list(&conn, DECK, &sorted(SortKey::Game3, SortDir::Desc)),
            ["q-c", "q-b", "q-e", "q-d", "q-a"]
        );
        assert_eq!(
            list(&conn, DECK, &sorted(SortKey::Game3, SortDir::Asc)),
            ["q-e", "q-d", "q-a", "q-b", "q-c"]
        );
    }

    #[test]
    fn pages_count_every_match_through_the_filter() {
        let (_dir, conn) = seeded("match-list-pages");
        let losses = MatchListFilter {
            result: Some(ResultFilter::Loss),
            ..sorted(SortKey::StartedAt, SortDir::Asc)
        };
        let first = match_list(&conn, DECK, &losses, 1, 1).expect("page 1");
        assert_eq!((first.total, first.page, first.per_page), (2, 1, 1));
        assert_eq!(tokens(&first), ["q-b"]);
        let second = match_list(&conn, DECK, &losses, 2, 1).expect("page 2");
        assert_eq!(tokens(&second), ["q-e"]);
        let zero = match_list(&conn, Scope::All(Platform::Mtgo), &all(), 0, 0).expect("page 0");
        assert_eq!((zero.total, zero.page, zero.per_page), (7, 1, 1));
        assert_eq!(tokens(&zero), ["q-live"]);
    }

    #[test]
    fn hidden_matches_are_in_no_list() {
        let (_dir, conn) = seeded("match-list-hidden");
        hide(&conn, "q-c");
        hide(&conn, "q-live");
        assert_eq!(list(&conn, DECK, &all()), ["q-e", "q-d", "q-b", "q-a"]);
        let page = match_list(&conn, Scope::All(Platform::Mtgo), &all(), 1, 50).expect("all");
        assert_eq!(page.total, 5);
        assert!(!tokens(&page).contains(&"q-c"));
    }

    // ---- Archetypes (0.x BuildMatchListProps) ---------------------------------

    /// The seeded deck matches with opponent archetypes: Qburn faced in q-a
    /// and q-c, Qtron in q-e, Homebrew in q-b; q-d has none. q-b also has a
    /// player seat, which is never the opponent's.
    fn with_archetypes(label: &str) -> (TestDir, Connection, i64, i64) {
        let (dir, conn) = seeded(label);
        let burn = insert_archetype(&conn, "q-burn", "Qburn", Some("modern"), "downloaded");
        let tron = insert_archetype(&conn, "q-tron", "Qtron", Some("modern"), "manual");
        insert_archetype(&conn, "q-unseen", "Qunseen", Some("modern"), "downloaded");
        insert_archetype(&conn, "q-legacy", "Qlegacy", Some("legacy"), "downloaded");
        let gone = insert_archetype(&conn, "q-gone", "Qgone", Some("modern"), "downloaded");
        conn.execute("UPDATE archetypes SET retired_at = 1 WHERE id = ?1", [gone])
            .expect("retire");
        let id = |token: &str| -> i64 {
            conn.query_row("SELECT id FROM matches WHERE token = ?1", [token], |row| {
                row.get(0)
            })
            .expect("id")
        };
        seat(&conn, id("q-a"), true, burn);
        seat(&conn, id("q-c"), true, burn);
        seat(&conn, id("q-e"), true, tron);
        seat(&conn, id("q-b"), true, 1);
        seat(&conn, id("q-b"), false, tron);
        (dir, conn, burn, tron)
    }

    fn archetype_of(page: &MatchListPage, token: &str) -> Option<String> {
        page.rows
            .iter()
            .find(|row| row.token == token)
            .and_then(|row| row.opponent_archetype.as_ref())
            .map(|archetype| archetype.name.clone())
    }

    /// A commander archetype's row carries the opponent's commander cards
    /// (partners in seat order) for the hover preview; any other archetype
    /// none, even when the match has commander rows.
    #[test]
    fn a_commander_archetype_row_carries_the_opponents_commander_cards() {
        let (_dir, conn, burn, _) = with_archetypes("match-list-commander-cards");
        let id = |token: &str| -> i64 {
            conn.query_row("SELECT id FROM matches WHERE token = ?1", [token], |row| {
                row.get(0)
            })
            .expect("id")
        };
        let commander = insert_archetype(
            &conn,
            "q-cmd",
            "Qkraum & Qtymna",
            Some("brawl"),
            "commander",
        );
        conn.execute(
            "UPDATE match_archetypes SET archetype_id = ?1 WHERE match_id = ?2 AND is_opponent = 1",
            params![commander, id("q-a")],
        )
        .expect("commander seat");
        conn.execute_batch(&format!(
            "INSERT INTO cards (catalog_id, name, color_identity, image, local_image)
                 VALUES (9001, 'Qkraum', 'U,R', 'https://cards/9001.jpg', '9001.jpg');
             INSERT INTO match_commanders VALUES
                 ({a}, 'opponent', 1, 9002), ({a}, 'opponent', 0, 9001), ({a}, 'player', 0, 9003),
                 ({c}, 'opponent', 0, 9001);",
            a = id("q-a"),
            c = id("q-c")
        ))
        .expect("commanders");
        let page = match_list(&conn, DECK, &all(), 1, 50).expect("list");
        let seat = |token: &str| {
            page.rows
                .iter()
                .find(|row| row.token == token)
                .and_then(|row| row.opponent_archetype.clone())
                .expect("an archetype")
        };
        assert_eq!(
            seat("q-a").commander_cards,
            vec![
                CommanderCard {
                    catalog_id: 9001,
                    name: Some("Qkraum".to_string()),
                    color_identity: Some("U,R".to_string()),
                    image: Some("https://cards/9001.jpg".to_string()),
                    local_image: Some("9001.jpg".to_string()),
                },
                CommanderCard {
                    catalog_id: 9002,
                    name: None,
                    color_identity: None,
                    image: None,
                    local_image: None,
                },
            ]
        );
        // q-c keeps Qburn: not a commander archetype, so no cards.
        assert_eq!(seat("q-c").id, burn);
        assert!(seat("q-c").commander_cards.is_empty());
        // The match page's read carries them too.
        let found = opponent_archetype(&conn, id("q-a"))
            .expect("read")
            .expect("seat");
        assert_eq!(found.commander_cards.len(), 2);
        assert!(
            opponent_archetype(&conn, id("q-c"))
                .expect("read")
                .expect("seat")
                .commander_cards
                .is_empty()
        );
    }

    #[test]
    fn the_list_rows_read_the_same_commander_cards_as_the_match_page() {
        assert!(MATCH_ROW_SELECT.contains(OPPONENT_COMMANDERS_JSON));
    }

    #[test]
    fn a_row_carries_its_opponent_archetype_never_the_players() {
        let (_dir, conn, burn, _) = with_archetypes("match-list-archetype-row");
        conn.execute(
            "UPDATE archetypes SET color_identity = 'R' WHERE id = ?1",
            [burn],
        )
        .expect("colours");
        let page = match_list(&conn, DECK, &all(), 1, 50).expect("list");
        let a = page
            .rows
            .iter()
            .find(|row| row.token == "q-a")
            .expect("q-a");
        assert_eq!(
            a.opponent_archetype,
            Some(SeatArchetype {
                id: burn,
                name: "Qburn".to_string(),
                color_identity: Some("R".to_string()),
                commander_cards: Vec::new(),
            })
        );
        assert_eq!(archetype_of(&page, "q-b").as_deref(), Some("Homebrew"));
        assert_eq!(archetype_of(&page, "q-d"), None);
    }

    #[test]
    fn the_archetype_filter_keeps_matches_without_one_or_against_one() {
        // 0.x MtgoMatch::scopeFilteredByArchetype: `none` or an id.
        let (_dir, conn, burn, tron) = with_archetypes("match-list-archetype-filter");
        let against = |id: i64| MatchListFilter {
            archetype: Some(ArchetypeFilter::Is(id)),
            ..all()
        };
        let unknown = MatchListFilter {
            archetype: Some(ArchetypeFilter::Unknown),
            ..all()
        };
        assert_eq!(list(&conn, DECK, &against(burn)), ["q-c", "q-a"]);
        // q-b's Qtron row is the player's seat.
        assert_eq!(list(&conn, DECK, &against(tron)), ["q-e"]);
        assert_eq!(list(&conn, DECK, &unknown), ["q-d"]);
        assert_eq!(
            list(&conn, Scope::All(Platform::Mtgo), &unknown),
            ["q-live", "q-d", "q-other"]
        );
        let page = match_list(&conn, DECK, &against(burn), 1, 50).expect("page");
        assert_eq!(page.total, 2);
    }

    #[test]
    fn sorts_by_the_opponent_archetype_name_each_way() {
        // 0.x applySort `archetype`: no archetype sorts as NULL (first
        // ascending); ties keep the newest first.
        let (_dir, conn, _, _) = with_archetypes("match-list-archetype-sort");
        assert_eq!(
            list(&conn, DECK, &sorted(SortKey::Archetype, SortDir::Asc)),
            ["q-d", "q-b", "q-c", "q-a", "q-e"]
        );
        assert_eq!(
            list(&conn, DECK, &sorted(SortKey::Archetype, SortDir::Desc)),
            ["q-e", "q-c", "q-a", "q-b", "q-d"]
        );
    }

    #[test]
    fn the_list_counts_unknown_archetypes_and_offers_the_formats_archetypes_by_count() {
        // 0.x BuildMatchListProps `unknownArchetypeCount` and
        // `archetypeOptions`: over the scoped complete matches, before the
        // list's own filters; the format's archetypes and the fallbacks,
        // most faced first, then by name.
        let (_dir, conn, _, _) = with_archetypes("match-list-archetype-options");
        let found =
            list_archetypes(&conn, DECK, Range::all_time(), Some("modern")).expect("options");
        assert_eq!(found.unknown, 1);
        let shown: Vec<(&str, u32)> = found
            .options
            .iter()
            .map(|option| (option.name.as_str(), option.matches))
            .collect();
        assert_eq!(
            shown,
            [
                ("Qburn", 2),
                ("Homebrew", 1),
                ("Qtron", 1),
                ("Qunseen", 0),
                ("Rogue", 0)
            ]
        );
        assert!(found.options[1].is_fallback);
        assert!(!found.options[0].is_fallback);

        // A window narrows both.
        let window = Range {
            from_ms: NOW - 3 * HOUR,
            to_ms: NOW,
        };
        let narrow = list_archetypes(&conn, DECK, window, Some("modern")).expect("window");
        assert_eq!(narrow.unknown, 1);
        assert_eq!(narrow.options[0].name, "Qburn");
        assert_eq!(narrow.options[0].matches, 1);

        // The main list has no format: the archetypes faced, then the
        // fallbacks. In-progress matches are not counted as unknown.
        let main = list_archetypes(&conn, Scope::All(Platform::Mtgo), Range::all_time(), None)
            .expect("main");
        assert_eq!(main.unknown, 2, "q-d and q-other");
        let names: Vec<&str> = main
            .options
            .iter()
            .map(|option| option.name.as_str())
            .collect();
        assert_eq!(names, ["Qburn", "Homebrew", "Qtron", "Rogue"]);
    }

    fn plan(conn: &Connection, sql: &str) -> String {
        let explain = format!("EXPLAIN QUERY PLAN {sql}");
        let mut stmt = conn.prepare(&explain).expect("explain");
        let count = stmt.parameter_count();
        let values: Vec<Option<i64>> = vec![Some(1); count];
        let rows = stmt
            .query_map(rusqlite::params_from_iter(values), |row| {
                row.get::<_, String>(3)
            })
            .expect("rows");
        rows.map(|row| row.expect("row"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn deck_lists_walk_the_deck_s_links_and_the_default_main_list_the_start_index() {
        let (_dir, conn) = db("match-list-plans");
        let filters = [
            all(),
            sorted(SortKey::Game2, SortDir::Asc),
            MatchListFilter {
                result: Some(ResultFilter::Win),
                kind: Some(KindFilter::League),
                ..sorted(SortKey::Duration, SortDir::Desc)
            },
        ];
        for filter in &filters {
            for sql in [rows_sql(&DECK, filter), count_sql(&DECK, filter)] {
                let query = plan(&conn, &sql);
                assert!(!query.contains("SCAN m"), "plan:\n{query}");
                assert!(!query.contains("SCAN g"), "plan:\n{query}");
                assert!(!query.contains("SCAN l"), "plan:\n{query}");
            }
        }
        let main = plan(&conn, &rows_sql(&Scope::All(Platform::Mtgo), &all()));
        assert!(main.contains("matches_started_at"), "plan:\n{main}");
        assert!(!main.contains("TEMP B-TREE FOR ORDER BY"), "plan:\n{main}");
        let _ = conn
            .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
            .optional();
    }
}
