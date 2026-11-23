//! The Metagame page and the archetype detail reads (archetypes spec
//! sections 9 and 10): the formats the player has played, the archetypes
//! faced in a window with the records against them, and one archetype's
//! decks against it, cards seen and stored lists.
//!
//! Counting rules: complete listed matches of the current account (every
//! account without one), the opponent seat's stored row (no merge grouping,
//! ruling 2). The arithmetic is `archetypes::metagame`'s.

use archetypes::metagame::{
    Faced, ListCard, SeenCard, SeenRow, Timeframe, TypicalCard, group_seen, mark_typical,
};
use archetypes::{Source, format_key, format_label};
use cards::tokens::TOKEN_RARITY;
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use chrono_tz::Tz;
use decks::stats::{Range, Record, end_of_day, local_midnight};
use rusqlite::{Connection, params};
use std::collections::HashMap;

use crate::StoreError;
use crate::archetypes::{ArchetypeList, ArchetypeRow, archetype_lists};
use crate::matchups::{MatchupDetail, deck_version_ids, matchup_detail};
use crate::platform::Platform;

/// The local day of `ms` in `tz`.
fn local_day(ms: i64, tz: Tz) -> NaiveDate {
    let instant = DateTime::<Utc>::from_timestamp_millis(ms).unwrap_or_default();
    instant.with_timezone(&tz).date_naive()
}

/// The player's window of a timeframe: today and the 29 (or 6) local days
/// before it, as the website's rolling start; all time without bounds.
pub fn metagame_window(timeframe: Timeframe, now_ms: i64, tz: Tz) -> Range {
    let Some(days) = timeframe.local_days() else {
        return Range::all_time();
    };
    let today = local_day(now_ms, tz);
    let first = today - Duration::days(i64::from(days) - 1);
    Range {
        from_ms: local_midnight(first, tz),
        to_ms: end_of_day(today, tz),
    }
}

/// The local year and month (1 to 12) of `ms` in `tz`.
pub fn local_month(ms: i64, tz: Tz) -> (i32, u32) {
    let day = local_day(ms, tz);
    (day.year(), day.month())
}

/// An Arena month's window: local midnight on the 1st to the end of the
/// month's last day in `tz`. `None` for a month that does not exist.
pub fn month_window(year: i32, month: u32, tz: Tz) -> Option<Range> {
    let first = NaiveDate::from_ymd_opt(year, month, 1)?;
    let next = match month {
        12 => NaiveDate::from_ymd_opt(year + 1, 1, 1)?,
        _ => NaiveDate::from_ymd_opt(year, month + 1, 1)?,
    };
    let last = next - Duration::days(1);
    Some(Range {
        from_ms: local_midnight(first, tz),
        to_ms: end_of_day(last, tz),
    })
}

/// The first and last local day of a window, `YYYY-MM-DD`; `None` for all
/// time.
pub fn window_days(range: Range, tz: Tz) -> Option<(String, String)> {
    if range == Range::all_time() {
        return None;
    }
    let first = local_day(range.from_ms, tz);
    let last = local_day(range.to_ms, tz);
    Some((first.to_string(), last.to_string()))
}

/// An account's name (the page header), `None` when there is no such row.
pub fn account_name(conn: &Connection, id: i64) -> Result<Option<String>, StoreError> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .prepare_cached("SELECT username FROM accounts WHERE id = ?1")?
        .query_row([id], |row| row.get(0))
        .optional()?)
}

/// One format the page offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatOption {
    /// The archetypes format key (`modern`).
    pub key: String,
    pub label: String,
    /// Complete matches, all time.
    pub matches: u32,
}

/// The formats with complete constructed matches of the platform's
/// accounts, most played first (then by key).
pub fn metagame_formats(
    conn: &Connection,
    account: Option<i64>,
    platform: Platform,
) -> Result<Vec<FormatOption>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT format, COUNT(*) FROM listed_matches
         WHERE state = 'complete' AND format IS NOT NULL
           AND (?1 IS NULL OR account_id = ?1)
           AND account_id IN (SELECT id FROM accounts WHERE platform = ?2)
         GROUP BY format",
    )?;
    let rows = stmt.query_map(params![account, platform], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
    })?;
    let mut out: Vec<FormatOption> = Vec::new();
    for row in rows {
        let (code, count) = row?;
        let Some(key) = format_key(&code) else {
            continue;
        };
        match out.iter_mut().find(|option| option.key == key) {
            Some(option) => option.matches += count,
            None => out.push(FormatOption {
                label: format_label(&key),
                key,
                matches: count,
            }),
        }
    }
    out.sort_by(|a, b| b.matches.cmp(&a.matches).then_with(|| a.key.cmp(&b.key)));
    Ok(out)
}

/// The MTGO codes of the stored matches whose key is `key`, as a JSON array.
fn codes_of(conn: &Connection, key: &str) -> Result<String, StoreError> {
    let mut stmt =
        conn.prepare_cached("SELECT DISTINCT format FROM listed_matches WHERE format IS NOT NULL")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut codes = Vec::new();
    for row in rows {
        let code = row?;
        if format_key(&code).as_deref() == Some(key) {
            codes.push(code);
        }
    }
    Ok(serde_json::to_string(&codes).unwrap_or_else(|_| "[]".to_string()))
}

/// What the page counts in one format and window.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MetagameData {
    /// Every complete listed match of the format in the window (the share
    /// denominator, ruling 17).
    pub total: u32,
    /// Those without an opponent archetype (the footer line).
    pub without_archetype: u32,
    /// The opponent archetypes faced, by id.
    pub faced: Vec<Faced>,
}

/// Complete matches of the format codes `?1` in the window `?2` to `?3`, of
/// the account `?4` (every account of the platform `?5` when NULL), whose
/// token is in the JSON array `?6` (any token when NULL).
const MATCH_SCOPE: &str = "m.state = 'complete'
  AND m.format IN (SELECT value FROM json_each(?1))
  AND m.started_at BETWEEN ?2 AND ?3
  AND (?4 IS NULL OR m.account_id = ?4)
  AND m.account_id IN (SELECT id FROM accounts WHERE platform = ?5)
  AND (?6 IS NULL OR m.token IN (SELECT value FROM json_each(?6)))";

/// `tokens` as the JSON array `MATCH_SCOPE` reads, `None` for any token.
fn tokens_json(tokens: Option<&[String]>) -> Option<String> {
    tokens.map(|tokens| serde_json::to_string(tokens).unwrap_or_else(|_| "[]".to_string()))
}

/// The tokens of the matches `metagame_data` counts in `format_key` over
/// `range` (no token limit).
pub fn match_tokens(
    conn: &Connection,
    account: Option<i64>,
    platform: Platform,
    format_key: &str,
    range: Range,
) -> Result<Vec<String>, StoreError> {
    counted_matches(
        conn,
        account,
        platform,
        format_key,
        range,
        "m.token",
        "m.started_at, m.id",
    )
}

/// The start of every match `metagame_data` would count in `format_key`
/// over all time, oldest first (the Arena month list).
pub fn match_starts(
    conn: &Connection,
    account: Option<i64>,
    platform: Platform,
    format_key: &str,
) -> Result<Vec<i64>, StoreError> {
    counted_matches(
        conn,
        account,
        platform,
        format_key,
        Range::all_time(),
        "m.started_at",
        "m.started_at",
    )
}

/// One column of the matches `metagame_data` counts (no token limit), in
/// `order`. Both arguments are fixed SQL text from the callers above.
fn counted_matches<T: rusqlite::types::FromSql>(
    conn: &Connection,
    account: Option<i64>,
    platform: Platform,
    format_key: &str,
    range: Range,
    column: &str,
    order: &str,
) -> Result<Vec<T>, StoreError> {
    let codes = codes_of(conn, format_key)?;
    let any: Option<String> = None;
    let values = params![codes, range.from_ms, range.to_ms, account, platform, any];
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {column} FROM listed_matches m WHERE {MATCH_SCOPE} ORDER BY {order}"
    ))?;
    let rows = stmt.query_map(values, |row| row.get::<_, T>(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The archetypes faced in `format_key` over `range`, counting only the
/// matches whose token is in `tokens` when given (an Arena rank band).
pub fn metagame_data(
    conn: &Connection,
    account: Option<i64>,
    platform: Platform,
    format_key: &str,
    range: Range,
    tokens: Option<&[String]>,
) -> Result<MetagameData, StoreError> {
    let codes = codes_of(conn, format_key)?;
    let tokens = tokens_json(tokens);
    let values = params![codes, range.from_ms, range.to_ms, account, platform, tokens];
    let (total, without_archetype): (u32, Option<u32>) = conn
        .prepare_cached(&format!(
            "SELECT COUNT(*), SUM(NOT EXISTS (SELECT 1 FROM match_archetypes x
                                         WHERE x.match_id = m.id AND x.is_opponent = 1))
             FROM listed_matches m WHERE {MATCH_SCOPE}"
        ))?
        .query_row(values, |row| Ok((row.get(0)?, row.get(1)?)))?;

    let mut stmt = conn.prepare_cached(&format!(
        "SELECT a.id, a.uuid, a.name, a.color_identity, a.source, a.retired_at IS NOT NULL,
                SUM(m.outcome = 'win'), SUM(m.outcome = 'loss'), COUNT(*), a.offline_held
         FROM listed_matches m
         JOIN match_archetypes ma ON ma.match_id = m.id AND ma.is_opponent = 1
         JOIN archetypes a ON a.id = ma.archetype_id
         WHERE {MATCH_SCOPE}
         GROUP BY a.id ORDER BY a.id"
    ))?;
    let rows = stmt.query_map(values, |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, bool>(5)?,
            row.get::<_, u32>(6)?,
            row.get::<_, u32>(7)?,
            row.get::<_, u32>(8)?,
            row.get::<_, bool>(9)?,
        ))
    })?;
    let mut faced = Vec::new();
    for row in rows {
        let (id, uuid, name, colors, source, retired, wins, losses, count, offline_held) = row?;
        let source = Source::parse(&source)
            .ok_or_else(|| StoreError::Corrupt(format!("archetype source {source}")))?;
        faced.push(Faced {
            archetype_id: id,
            uuid,
            name,
            color_identity: colors,
            source,
            retired,
            offline_held,
            record: Record::from_total(wins, losses, count),
        });
    }
    Ok(MetagameData {
        total,
        without_archetype: without_archetype.unwrap_or(0),
        faced,
    })
}

/// The main deck of each archetype's first stored list (a downloaded
/// archetype's key list, a manual archetype's first variant), for the
/// tiles the website sends no cards for. An archetype without lists is
/// absent; a card without a card row has no name.
pub fn tile_list_cards(
    conn: &Connection,
    archetype_ids: &[i64],
) -> Result<HashMap<i64, Vec<ListCard>>, StoreError> {
    let mut out: HashMap<i64, Vec<ListCard>> = HashMap::new();
    if archetype_ids.is_empty() {
        return Ok(out);
    }
    let ids = serde_json::to_string(archetype_ids).unwrap_or_else(|_| "[]".to_string());
    let mut stmt = conn.prepare_cached(
        "SELECT ac.archetype_id, ac.catalog_id, c.name, c.type, ac.quantity,
                COALESCE(c.image IS NOT NULL OR c.local_image IS NOT NULL, 0)
         FROM archetype_cards ac
         LEFT JOIN cards c ON c.catalog_id = ac.catalog_id
         WHERE ac.archetype_id IN (SELECT value FROM json_each(?1))
           AND ac.sideboard = 0
           AND ac.list = (SELECT MIN(f.list) FROM archetype_cards f
                          WHERE f.archetype_id = ac.archetype_id)
         ORDER BY ac.archetype_id, ac.catalog_id",
    )?;
    let rows = stmt.query_map([ids], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            ListCard {
                catalog_id: row.get(1)?,
                name: row.get(2)?,
                card_type: row.get(3)?,
                quantity: row.get(4)?,
                has_image: row.get(5)?,
            },
        ))
    })?;
    for row in rows {
        let (archetype_id, card) = row?;
        out.entry(archetype_id).or_default().push(card);
    }
    Ok(out)
}

/// One of the player's decks that played against the archetype.
#[derive(Debug, Clone, PartialEq)]
pub struct DeckAgainst {
    pub deck_id: i64,
    pub name: String,
    pub archived: bool,
    pub color_identity: Option<String>,
    /// 0.x `GetArchetypeMatchupDetail` over every version of the deck.
    pub detail: MatchupDetail,
}

/// One archetype's detail page data.
#[derive(Debug, Clone, PartialEq)]
pub struct ArchetypeDetail {
    pub archetype: ArchetypeRow,
    /// Every complete listed match against it in the window.
    pub record: Record,
    /// The player's decks against it, most matches first.
    pub decks: Vec<DeckAgainst>,
    /// The opponent cards seen against it, marked against the stored lists.
    pub seen: Vec<SeenCard>,
    /// The stored lists (a downloaded key list, manual variants).
    pub lists: Vec<ArchetypeList>,
}

/// The scope of the matches against archetype `?1`: complete, in the window
/// `?2` to `?3`, of the account `?4` (every account of the platform `?5`
/// when NULL).
const AGAINST_SCOPE: &str = "m.state = 'complete'
  AND m.started_at BETWEEN ?2 AND ?3
  AND (?4 IS NULL OR m.account_id = ?4)
  AND m.account_id IN (SELECT id FROM accounts WHERE platform = ?5)";

/// The typical cards of the stored lists.
pub fn typical_of_lists(lists: &[ArchetypeList]) -> Vec<TypicalCard> {
    let mut out = Vec::new();
    for list in lists {
        for card in &list.cards {
            out.push(TypicalCard {
                catalog_id: card.catalog_id,
                oracle_id: card.oracle_id.clone(),
            });
        }
    }
    out
}

/// One archetype's detail over `range` (dates in `tz`); `None` when there is
/// no such archetype.
pub fn archetype_detail(
    conn: &Connection,
    account: Option<i64>,
    platform: Platform,
    id: i64,
    range: Range,
    tz: Tz,
) -> Result<Option<ArchetypeDetail>, StoreError> {
    let Some(row) = crate::archetypes::archetype(conn, id)? else {
        return Ok(None);
    };
    let values = params![id, range.from_ms, range.to_ms, account, platform];

    let (wins, losses, count): (Option<u32>, Option<u32>, u32) = conn
        .prepare_cached(&format!(
            "SELECT SUM(m.outcome = 'win'), SUM(m.outcome = 'loss'), COUNT(*)
             FROM listed_matches m
             JOIN match_archetypes ma ON ma.match_id = m.id AND ma.is_opponent = 1
                                     AND ma.archetype_id = ?1
             WHERE {AGAINST_SCOPE}"
        ))?
        .query_row(values, |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    let record = Record::from_total(wins.unwrap_or(0), losses.unwrap_or(0), count);

    let mut found = Vec::new();
    {
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT d.id, d.name, d.archived_at IS NOT NULL, d.color_identity
             FROM listed_matches m
             JOIN match_archetypes ma ON ma.match_id = m.id AND ma.is_opponent = 1
                                     AND ma.archetype_id = ?1
             JOIN deck_versions v ON v.id = m.deck_version_id
             JOIN decks d ON d.id = v.deck_id
             WHERE {AGAINST_SCOPE}
             GROUP BY d.id"
        ))?;
        let rows = stmt.query_map(values, |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?;
        for row in rows {
            found.push(row?);
        }
    }
    let mut decks = Vec::new();
    for (deck_id, name, archived, color_identity) in found {
        let versions = deck_version_ids(conn, deck_id, None)?;
        let detail = matchup_detail(conn, &versions, id, range, tz, None)?;
        decks.push(DeckAgainst {
            deck_id,
            name,
            archived,
            color_identity,
            detail,
        });
    }
    decks.sort_by(|a, b| {
        b.detail
            .record
            .total
            .cmp(&a.detail.record.total)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.deck_id.cmp(&b.deck_id))
    });

    // Tokens and basic lands say nothing about what the opponent registered,
    // so they are not cards seen. A token is known by its rarity or, when its
    // card row does not mark it, by its type line.
    let mut seen_rows = Vec::new();
    {
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT m.id, g.id, gc.catalog_id, c.oracle_id, c.name, gc.seen
             FROM listed_matches m
             JOIN match_archetypes ma ON ma.match_id = m.id AND ma.is_opponent = 1
                                     AND ma.archetype_id = ?1
             JOIN games g ON g.match_id = m.id
             JOIN game_cards gc ON gc.game_id = g.id AND gc.side = 'opponent' AND gc.seen > 0
             LEFT JOIN cards c ON c.catalog_id = gc.catalog_id
             WHERE {AGAINST_SCOPE}
               AND COALESCE(c.rarity, '') <> ?6
               AND COALESCE(c.type, '') NOT LIKE 'Basic%Land%'
               AND COALESCE(c.type, '') NOT LIKE 'Token%'"
        ))?;
        let values = params![
            id,
            range.from_ms,
            range.to_ms,
            account,
            platform,
            TOKEN_RARITY
        ];
        let rows = stmt.query_map(values, |row| {
            Ok(SeenRow {
                match_id: row.get(0)?,
                game_id: row.get(1)?,
                catalog_id: row.get(2)?,
                oracle_id: row.get(3)?,
                name: row.get(4)?,
                seen: row.get(5)?,
            })
        })?;
        for row in rows {
            seen_rows.push(row?);
        }
    }
    let lists = archetype_lists(conn, id)?;
    let mut seen = group_seen(&seen_rows);
    mark_typical(&mut seen, &typical_of_lists(&lists));

    Ok(Some(ArchetypeDetail {
        archetype: row,
        record,
        decks,
        seen,
        lists,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matchups::seed::*;

    const DAY: i64 = 86_400_000;
    /// 2026-03-15T12:00:00Z.
    const MARCH_15: i64 = 1_773_576_000_000;
    const UTC: Tz = chrono_tz::UTC;

    fn downloaded(conn: &Connection, name: &str, uuid: &str) -> i64 {
        conn.query_row(
            "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
             VALUES (?1, ?2, 'modern', 'downloaded', 0, 0) RETURNING id",
            params![uuid, name],
            |row| row.get(0),
        )
        .expect("downloaded")
    }

    fn seen_card(conn: &Connection, match_id: i64, catalog_id: i64, seen: i64) {
        let game: i64 = conn
            .query_row(
                "SELECT id FROM games WHERE match_id = ?1 ORDER BY game_number LIMIT 1",
                [match_id],
                |row| row.get(0),
            )
            .expect("game");
        conn.execute(
            "INSERT INTO game_cards (game_id, side, catalog_id, seen) VALUES (?1, 'opponent', ?2, ?3)",
            params![game, catalog_id, seen],
        )
        .expect("game card");
    }

    // The tile's local cards come from the first stored list's main deck.
    #[test]
    fn tile_list_cards_read_each_first_lists_main_deck() {
        let (_dir, conn) = db("metagame-tile-cards");
        let storm = downloaded(&conn, "Qstorm", "q-storm");
        let burn = downloaded(&conn, "Qburn", "q-burn");
        let empty = downloaded(&conn, "Qempty", "q-empty");
        conn.execute_batch(&format!(
            "INSERT INTO cards (catalog_id, name, type, image) VALUES
                 (501, 'Qbolt', 'Instant', 'https://cards.scryfall.io/b.jpg'),
                 (502, 'Qvial', 'Artifact', NULL),
                 (503, 'Qhelix', 'Instant', NULL);
             UPDATE cards SET local_image = '/q/503.jpg' WHERE catalog_id = 503;
             INSERT INTO archetype_cards (archetype_id, list, catalog_id, quantity, sideboard) VALUES
                 ({storm}, 1, 501, 4, 0),
                 ({storm}, 1, 502, 2, 0),
                 ({storm}, 1, 503, 3, 1),
                 ({storm}, 2, 503, 4, 0),
                 ({burn}, 2, 503, 4, 0),
                 ({burn}, 2, 599, 1, 0);"
        ))
        .expect("lists");
        let read = tile_list_cards(&conn, &[storm, burn, empty]).expect("read");
        let card = |catalog_id: i64,
                    name: Option<&str>,
                    card_type: Option<&str>,
                    quantity: u32,
                    has_image: bool| {
            ListCard {
                catalog_id,
                name: name.map(str::to_string),
                card_type: card_type.map(str::to_string),
                quantity,
                has_image,
            }
        };
        assert_eq!(
            read.get(&storm),
            Some(&vec![
                card(501, Some("Qbolt"), Some("Instant"), 4, true),
                card(502, Some("Qvial"), Some("Artifact"), 2, false),
            ])
        );
        // A manual archetype's lists can start past 1; a card without a row
        // has no name.
        assert_eq!(
            read.get(&burn),
            Some(&vec![
                card(503, Some("Qhelix"), Some("Instant"), 4, true),
                card(599, None, None, 1, false),
            ])
        );
        assert_eq!(read.get(&empty), None);
        assert!(tile_list_cards(&conn, &[]).expect("none").is_empty());
    }

    #[test]
    fn the_window_is_today_and_the_days_before_it_in_local_days() {
        // 2026-03-15 12:00 UTC is 2026-03-15 08:00 in New York.
        let tz: Tz = chrono_tz::America::New_York;
        let range = metagame_window(Timeframe::Last30, MARCH_15, tz);
        assert_eq!(
            window_days(range, tz),
            Some(("2026-02-14".to_string(), "2026-03-15".to_string()))
        );
        // Local midnight of 14 February in New York is 05:00 UTC.
        assert_eq!(range.from_ms, 1_771_045_200_000);
        let week = metagame_window(Timeframe::Last7, MARCH_15, tz);
        assert_eq!(
            window_days(week, tz),
            Some(("2026-03-09".to_string(), "2026-03-15".to_string()))
        );
        // Late evening UTC is already the next day in Tokyo.
        let tokyo: Tz = chrono_tz::Asia::Tokyo;
        let late = MARCH_15 + 10 * 3_600_000;
        assert_eq!(
            window_days(metagame_window(Timeframe::Last7, late, tokyo), tokyo),
            Some(("2026-03-10".to_string(), "2026-03-16".to_string()))
        );
        let all = metagame_window(Timeframe::AllTime, MARCH_15, tz);
        assert_eq!(all, Range::all_time());
        assert_eq!(window_days(all, tz), None);
    }

    #[test]
    fn the_formats_are_the_played_constructed_ones_most_played_first() {
        let (_dir, conn) = db("metagame-formats");
        let deck = deck(&conn, None, "CMODERN", None);
        let version = version(&conn, deck);
        for _ in 0..2 {
            complete(&conn, version, "win", MARCH_15);
        }
        let pioneer = complete(&conn, version, "win", MARCH_15);
        let draft = complete(&conn, version, "win", MARCH_15);
        let live = played(&conn, version, "in_progress", "unknown", MARCH_15);
        conn.execute(
            "UPDATE matches SET format = 'CPIONEER' WHERE id = ?1",
            [pioneer],
        )
        .expect("pioneer");
        conn.execute(
            "UPDATE matches SET format = 'DHOBHOBHOB' WHERE id = ?1",
            [draft],
        )
        .expect("draft");
        conn.execute(
            "UPDATE matches SET format = 'CPIONEER' WHERE id = ?1",
            [live],
        )
        .expect("live");
        let formats = metagame_formats(&conn, None, Platform::Mtgo).expect("formats");
        assert_eq!(
            formats,
            vec![
                FormatOption {
                    key: "modern".to_string(),
                    label: "Modern".to_string(),
                    matches: 2
                },
                FormatOption {
                    key: "pioneer".to_string(),
                    label: "Pioneer".to_string(),
                    matches: 1
                },
            ]
        );
    }

    #[test]
    fn the_page_counts_the_formats_matches_in_the_window() {
        let (_dir, conn) = db("metagame-data");
        let me = account(&conn, "Qlocal");
        let other = account(&conn, "Qother");
        let deck = deck(&conn, Some(me), "CMODERN", None);
        let version = version(&conn, deck);
        let theirs = deck_of(&conn, other);
        let storm = downloaded(&conn, "Qstorm", "q-storm");
        let burn = archetype(&conn, "Qburn", "modern", Some("R"));
        let range = metagame_window(Timeframe::Last30, MARCH_15, UTC);

        against(&conn, version, storm, "win", MARCH_15, &[g("win", true)]);
        against(
            &conn,
            version,
            storm,
            "loss",
            MARCH_15 - DAY,
            &[g("loss", true)],
        );
        against(&conn, version, storm, "draw", MARCH_15 - DAY, &[]);
        against(&conn, version, burn, "win", MARCH_15 - 29 * DAY, &[]);
        // No opponent archetype: counted in the total and the footer.
        complete(&conn, version, "win", MARCH_15);
        // Outside the window, not complete, another format, hidden, another account.
        against(&conn, version, storm, "win", MARCH_15 - 31 * DAY, &[]);
        let live = played(&conn, version, "in_progress", "unknown", MARCH_15);
        opponent_seat(&conn, live, storm);
        let pioneer = against(&conn, version, storm, "win", MARCH_15, &[]);
        conn.execute(
            "UPDATE matches SET format = 'CPIONEER' WHERE id = ?1",
            [pioneer],
        )
        .expect("pioneer");
        let hidden = against(&conn, version, storm, "win", MARCH_15, &[]);
        conn.execute("UPDATE matches SET hidden_at = 1 WHERE id = ?1", [hidden])
            .expect("hidden");
        against(&conn, theirs, storm, "win", MARCH_15, &[]);

        let data =
            metagame_data(&conn, Some(me), Platform::Mtgo, "modern", range, None).expect("data");
        assert_eq!(data.total, 5);
        assert_eq!(data.without_archetype, 1);
        let read: Vec<(&str, Source, u32, u32, u32)> = data
            .faced
            .iter()
            .map(|f| {
                (
                    f.name.as_str(),
                    f.source,
                    f.record.wins,
                    f.record.losses,
                    f.record.draws,
                )
            })
            .collect();
        assert_eq!(
            read,
            vec![
                ("Qstorm", Source::Downloaded, 1, 1, 1),
                ("Qburn", Source::Manual, 1, 0, 0)
            ]
        );
        // Every account without a current one.
        let every =
            metagame_data(&conn, None, Platform::Mtgo, "modern", range, None).expect("every");
        assert_eq!(every.total, 6);
        // A format never played is empty.
        assert_eq!(
            metagame_data(&conn, Some(me), Platform::Mtgo, "legacy", range, None).expect("legacy"),
            MetagameData::default()
        );
    }

    #[test]
    fn the_metagame_reads_one_platform() {
        let (_dir, conn) = db("metagame-arena");
        let storm = downloaded(&conn, "Qstorm", "q-storm");
        let mtgo_deck = deck(&conn, None, "CMODERN", None);
        let mtgo_version = version(&conn, mtgo_deck);
        against(
            &conn,
            mtgo_version,
            storm,
            "win",
            MARCH_15,
            &[g("win", true)],
        );
        let arena_deck = deck(&conn, None, "CMODERN", None);
        let arena_version = version(&conn, arena_deck);
        let arena = against(
            &conn,
            arena_version,
            storm,
            "loss",
            MARCH_15,
            &[g("loss", true)],
        );
        conn.execute(
            "UPDATE matches SET format = 'CPIONEER' WHERE id = ?1",
            [arena],
        )
        .expect("pioneer");
        let pioneer = against(&conn, arena_version, storm, "loss", MARCH_15, &[]);
        crate::test_support::move_match_to_arena(&conn, &token_of(&conn, arena));
        crate::test_support::move_match_to_arena(&conn, &token_of(&conn, pioneer));
        crate::test_support::move_deck_to_arena(&conn, arena_deck);

        let formats = metagame_formats(&conn, None, Platform::Mtgo).expect("formats");
        let keys: Vec<&str> = formats.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(keys, ["modern"]);
        assert_eq!(formats[0].matches, 1);
        let range = metagame_window(Timeframe::Last30, MARCH_15, UTC);
        let data = metagame_data(&conn, None, Platform::Mtgo, "modern", range, None).expect("data");
        assert_eq!(data.total, 1);
        let detail = archetype_detail(&conn, None, Platform::Mtgo, storm, Range::all_time(), UTC)
            .expect("detail")
            .expect("found");
        assert_eq!(detail.record.total, 1);
        let decks: Vec<i64> = detail.decks.iter().map(|d| d.deck_id).collect();
        assert_eq!(decks, [mtgo_deck]);

        let formats = metagame_formats(&conn, None, Platform::Arena).expect("formats");
        let keys: Vec<(&str, u32)> = formats
            .iter()
            .map(|f| (f.key.as_str(), f.matches))
            .collect();
        assert_eq!(keys, [("modern", 1), ("pioneer", 1)]);
        let data =
            metagame_data(&conn, None, Platform::Arena, "pioneer", range, None).expect("data");
        assert_eq!(data.total, 1);
        let detail = archetype_detail(&conn, None, Platform::Arena, storm, Range::all_time(), UTC)
            .expect("detail")
            .expect("found");
        assert_eq!(detail.record.total, 2);
        let decks: Vec<i64> = detail.decks.iter().map(|d| d.deck_id).collect();
        assert_eq!(decks, [arena_deck]);
    }

    #[test]
    fn metagame_data_counts_only_the_given_tokens() {
        let (_dir, conn) = db("metagame-tokens");
        let deck = deck(&conn, None, "CMODERN", None);
        let version = version(&conn, deck);
        let storm = downloaded(&conn, "Qstorm", "q-storm");
        let range = metagame_window(Timeframe::Last30, MARCH_15, UTC);
        let won = against(&conn, version, storm, "win", MARCH_15, &[]);
        let lost = against(&conn, version, storm, "loss", MARCH_15, &[]);
        let plain = complete(&conn, version, "win", MARCH_15);
        // Outside the window: never counted, even when its token is given.
        let old = against(&conn, version, storm, "win", MARCH_15 - 40 * DAY, &[]);

        let all = metagame_data(&conn, None, Platform::Mtgo, "modern", range, None).expect("all");
        assert_eq!((all.total, all.without_archetype), (3, 1));

        let some = vec![
            token_of(&conn, won),
            token_of(&conn, plain),
            token_of(&conn, old),
        ];
        let data =
            metagame_data(&conn, None, Platform::Mtgo, "modern", range, Some(&some)).expect("some");
        assert_eq!((data.total, data.without_archetype), (2, 1));
        let records: Vec<(u32, u32)> = data
            .faced
            .iter()
            .map(|f| (f.record.wins, f.record.losses))
            .collect();
        assert_eq!(records, vec![(1, 0)]);

        let only_lost = vec![token_of(&conn, lost)];
        let data = metagame_data(
            &conn,
            None,
            Platform::Mtgo,
            "modern",
            range,
            Some(&only_lost),
        )
        .expect("lost");
        assert_eq!(data.total, 1);
        // An empty list counts nothing.
        let none =
            metagame_data(&conn, None, Platform::Mtgo, "modern", range, Some(&[])).expect("none");
        assert_eq!(none, MetagameData::default());
    }

    #[test]
    fn match_tokens_are_the_counted_matches() {
        let (_dir, conn) = db("metagame-match-tokens");
        let me = account(&conn, "Qlocal");
        let other = account(&conn, "Qother");
        let version = deck_of(&conn, me);
        let theirs = deck_of(&conn, other);
        let storm = downloaded(&conn, "Qstorm", "q-storm");
        let range = metagame_window(Timeframe::Last30, MARCH_15, UTC);
        let one = against(&conn, version, storm, "win", MARCH_15, &[]);
        let two = complete(&conn, version, "loss", MARCH_15 - DAY);
        // Outside the window, not complete, another format, another account.
        complete(&conn, version, "win", MARCH_15 - 40 * DAY);
        played(&conn, version, "in_progress", "unknown", MARCH_15);
        let pioneer = complete(&conn, version, "win", MARCH_15);
        conn.execute(
            "UPDATE matches SET format = 'CPIONEER' WHERE id = ?1",
            [pioneer],
        )
        .expect("pioneer");
        complete(&conn, theirs, "win", MARCH_15);

        let mut tokens =
            match_tokens(&conn, Some(me), Platform::Mtgo, "modern", range).expect("tokens");
        tokens.sort();
        let mut expected = vec![token_of(&conn, one), token_of(&conn, two)];
        expected.sort();
        assert_eq!(tokens, expected);
        let data =
            metagame_data(&conn, Some(me), Platform::Mtgo, "modern", range, None).expect("data");
        assert_eq!(data.total as usize, tokens.len());
        assert!(
            match_tokens(&conn, Some(me), Platform::Arena, "modern", range)
                .expect("arena")
                .is_empty()
        );

        // The month list's starts: every complete match of the format, all time.
        let mut starts = match_starts(&conn, Some(me), Platform::Mtgo, "modern").expect("starts");
        starts.sort();
        let mut expected = vec![MARCH_15 - 40 * DAY, MARCH_15 - DAY, MARCH_15];
        expected.sort();
        assert_eq!(starts, expected);
    }

    #[test]
    fn a_month_window_runs_from_the_first_to_the_last_local_day() {
        let tz: Tz = chrono_tz::America::New_York;
        let range = month_window(2026, 2, tz).expect("february");
        assert_eq!(
            window_days(range, tz),
            Some(("2026-02-01".to_string(), "2026-02-28".to_string()))
        );
        let december = month_window(2026, 12, tz).expect("december");
        assert_eq!(
            window_days(december, tz),
            Some(("2026-12-01".to_string(), "2026-12-31".to_string()))
        );
        assert_eq!(month_window(2026, 13, tz), None);
        // 2026-03-01 03:00 UTC is still February in New York.
        assert_eq!(local_month(1_772_334_000_000, tz), (2026, 2));
        assert_eq!(local_month(1_772_334_000_000, UTC), (2026, 3));
    }

    /// A deck of `account` with one version.
    fn deck_of(conn: &Connection, account: i64) -> i64 {
        let deck = deck(conn, Some(account), "CMODERN", None);
        version(conn, deck)
    }

    // 0.x ShowTest "shows archetype detail page", "returns empty decks array
    // when archetype has no decks".
    #[test]
    fn an_archetype_without_matches_has_an_empty_detail() {
        let (_dir, conn) = db("detail-empty");
        let storm = downloaded(&conn, "Qstorm", "q-storm");
        let detail = archetype_detail(&conn, None, Platform::Mtgo, storm, Range::all_time(), UTC)
            .expect("detail")
            .expect("found");
        assert_eq!(detail.archetype.name, "Qstorm");
        assert!(detail.decks.is_empty());
        assert!(detail.seen.is_empty());
        assert!(detail.lists.is_empty());
        assert_eq!(detail.record.total, 0);
        assert_eq!(
            archetype_detail(&conn, None, Platform::Mtgo, 999, Range::all_time(), UTC)
                .expect("missing"),
            None
        );
    }

    // 0.x ShowMergedArchetypeTest: the merged notice names the parent.
    #[test]
    fn the_detail_names_the_parent_of_a_merged_archetype() {
        let (_dir, conn) = db("detail-merged");
        let parent = archetype(&conn, "Qparent", "modern", None);
        let source = archetype(&conn, "Qsource", "modern", None);
        conn.execute(
            "UPDATE archetypes SET merged_into_id = ?1 WHERE id = ?2",
            [parent, source],
        )
        .expect("merge");
        let merged = archetype_detail(&conn, None, Platform::Mtgo, source, Range::all_time(), UTC)
            .expect("detail")
            .expect("found");
        let into = merged.archetype.merged_into.expect("merged into");
        assert_eq!((into.id, into.name.as_str()), (parent, "Qparent"));
        let standalone =
            archetype_detail(&conn, None, Platform::Mtgo, parent, Range::all_time(), UTC)
                .expect("detail")
                .expect("found");
        assert_eq!(standalone.archetype.merged_into, None);
    }

    /// Deck id, matches, match rate, games won and lost, on the play, on
    /// the draw and game rates.
    type DeckNumbers = (i64, u32, u32, u32, u32, u32, u32, u32);

    // The per-deck numbers are 0.x GetArchetypeMatchupDetail's (the cases of
    // GetArchetypeMatchupDetailTest "computes records", "play and draw").
    #[test]
    fn each_deck_against_it_has_the_matchup_numbers() {
        let (_dir, conn) = db("detail-decks");
        let storm = downloaded(&conn, "Qstorm", "q-storm");
        let first = deck(&conn, None, "CMODERN", None);
        let first_v1 = version(&conn, first);
        let first_v2 = version(&conn, first);
        let second = deck(&conn, None, "CMODERN", None);
        let second_v = version(&conn, second);
        let quiet = deck(&conn, None, "CMODERN", None);
        let quiet_v = version(&conn, quiet);

        // First deck: 2-1 over two versions; games 4-3.
        against(
            &conn,
            first_v1,
            storm,
            "win",
            MARCH_15,
            &[g("win", true), g("loss", false), g("win", true)],
        );
        against(
            &conn,
            first_v2,
            storm,
            "win",
            MARCH_15 + DAY,
            &[g("win", false), g("win", true)],
        );
        against(
            &conn,
            first_v2,
            storm,
            "loss",
            MARCH_15 + 2 * DAY,
            &[g("loss", true), g("loss", false)],
        );
        // Second deck: one draw.
        against(
            &conn,
            second_v,
            storm,
            "draw",
            MARCH_15,
            &[g("win", true), g("loss", false)],
        );
        // Another archetype only: not listed.
        let burn = archetype(&conn, "Qburn", "modern", None);
        against(&conn, quiet_v, burn, "win", MARCH_15, &[g("win", true)]);

        let detail = archetype_detail(&conn, None, Platform::Mtgo, storm, Range::all_time(), UTC)
            .expect("detail")
            .expect("found");
        assert_eq!(detail.record, Record::from_counts(2, 1, 1));
        let read: Vec<DeckNumbers> = detail
            .decks
            .iter()
            .map(|d| {
                (
                    d.deck_id,
                    d.detail.record.total,
                    d.detail.record.win_rate,
                    d.detail.games.won,
                    d.detail.games.lost,
                    d.detail.otp.win_rate,
                    d.detail.otd.win_rate,
                    d.detail.games.win_rate,
                )
            })
            .collect();
        assert_eq!(
            read,
            vec![
                (first, 3, 67, 4, 3, 75, 33, 57),
                (second, 1, 0, 1, 1, 100, 0, 50)
            ]
        );
        // The window applies.
        let one_day = Range {
            from_ms: MARCH_15 - 1,
            to_ms: MARCH_15 + 1,
        };
        let windowed = archetype_detail(&conn, None, Platform::Mtgo, storm, one_day, UTC)
            .expect("detail")
            .expect("found");
        assert_eq!(windowed.record.total, 2);
        assert_eq!(windowed.decks[0].detail.record.total, 1);
    }

    #[test]
    fn cards_seen_against_it_are_marked_against_the_stored_lists() {
        let (_dir, conn) = db("detail-seen");
        let storm = downloaded(&conn, "Qstorm", "q-storm");
        let deck = deck(&conn, None, "CMODERN", None);
        let version = version(&conn, deck);
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, oracle_id, name) VALUES
                 (100, 'o-ritual', 'Qritual'), (101, 'o-ritual', 'Qritual'),
                 (200, 'o-tech', 'Qtech');
             INSERT INTO cards (catalog_id, oracle_id, name, type, rarity) VALUES
                 (400, 'o-goblin', 'Qgoblin', 'Token Creature - Goblin', 'token'),
                 (401, 'o-island', 'Qisland', 'Basic Land - Island', 'common'),
                 (402, 'o-snow', 'Qsnowforest', 'Basic Snow Land - Forest', 'common'),
                 (403, 'o-tower', 'Qtower', 'Land - Urza''s Tower', 'common'),
                 (404, 'o-treasure', 'Qtreasure', 'Token Artifact - Treasure', 'common');",
        )
        .expect("cards");
        conn.execute(
            "INSERT INTO archetype_cards (archetype_id, list, catalog_id, oracle_id, quantity, sideboard)
             VALUES (?1, 1, 101, 'o-ritual', 4, 0)",
            [storm],
        )
        .expect("list");
        let one = against(&conn, version, storm, "win", MARCH_15, &[g("win", true)]);
        let two = against(&conn, version, storm, "loss", MARCH_15, &[g("loss", true)]);
        seen_card(&conn, one, 100, 2);
        seen_card(&conn, two, 101, 3);
        seen_card(&conn, two, 200, 1);
        seen_card(&conn, two, 300, 1);
        // Tokens and basic lands say nothing about the list: left out. A
        // land that is not basic stays.
        seen_card(&conn, two, 400, 2);
        seen_card(&conn, two, 401, 5);
        seen_card(&conn, two, 402, 3);
        seen_card(&conn, two, 403, 1);
        // A token whose card row is not marked as one: its type says so.
        seen_card(&conn, two, 404, 2);
        // Seen against another archetype: not counted.
        let burn = archetype(&conn, "Qburn", "modern", None);
        let other = against(&conn, version, burn, "win", MARCH_15, &[g("win", true)]);
        seen_card(&conn, other, 200, 4);

        let detail = archetype_detail(&conn, None, Platform::Mtgo, storm, Range::all_time(), UTC)
            .expect("detail")
            .expect("found");
        let read: Vec<(&str, u32, u32, bool)> = detail
            .seen
            .iter()
            .map(|c| (c.name.as_str(), c.matches, c.most_copies, c.in_typical))
            .collect();
        assert_eq!(
            read,
            vec![
                ("Qritual", 2, 3, true),
                ("Card 300", 1, 1, false),
                ("Qtech", 1, 1, false),
                ("Qtower", 1, 1, false),
            ]
        );
        assert_eq!(detail.lists.len(), 1);
    }
}
