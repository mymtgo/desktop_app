//! Archetype reads (archetypes spec sections 4 and 6): the held candidates
//! detection scores against, a match's detection input, the detection
//! queue, one archetype, the picker options and the downloaded versions the
//! change check sends.
//!
//! Formats: `archetypes.format` holds the archetypes format key (`modern`,
//! `archetypes::format_key`), a match or deck holds MTGO's code (`CMODERN`).

use archetypes::{Candidate, ListCard, ObservedCard, Source, parse_definition};
use rusqlite::{OptionalExtension, params};

use crate::platform::{Platform, account_of_platform};
use crate::{Connection, StoreError};

/// Copies of one opponent card counted at most (0.x
/// `AggregateOpponentCards`).
pub const OPPONENT_COPIES_CAP: i64 = 4;

/// What detection reads for one match (archetypes spec section 6.2).
#[derive(Debug, Clone, PartialEq)]
pub struct DetectionInput {
    pub match_id: i64,
    /// MTGO's format code of the match (`CMODERN`), `None` when unknown.
    pub format: Option<String>,
    /// The archetype of the deck the match was played with.
    pub deck_archetype_id: Option<i64>,
    /// The seat has a hand-set row, which detection leaves alone.
    pub player_manual: bool,
    /// The opponent seat has a hand-set row, or the player chose "none"
    /// for it (`matches.opponent_archetype_none`): detection leaves it.
    pub opponent_manual: bool,
    /// Game 1's local deck: one card per catalog id and board.
    pub player_cards: Vec<ObservedCard>,
    /// The opponent's cards seen, summed across games and capped at
    /// `OPPONENT_COPIES_CAP`, all main.
    pub opponent_cards: Vec<ObservedCard>,
    /// A commander format match's commanders per seat (Arena Brawl spec
    /// section 6); `None` for any other format.
    pub commanders: Option<CommanderSeats>,
}

/// Both seats' commanders; a seat with no commander rows is `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommanderSeats {
    pub player: Option<CommanderSet>,
    pub opponent: Option<CommanderSet>,
}

/// One seat's commanders (one, or two partners) from their card rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommanderSet {
    pub oracle_ids: Vec<String>,
    pub names: Vec<String>,
    pub color_identities: Vec<String>,
}

/// One archetype as a picker or a link shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct ArchetypeOption {
    pub id: i64,
    pub uuid: String,
    pub name: String,
    pub format: Option<String>,
    pub color_identity: Option<String>,
    pub source: Source,
    /// A server archetype held as Custom by the offline download.
    pub offline_held: bool,
}

/// One archetype row with its merge links (0.x `mergedInto`, `mergedFrom`).
#[derive(Debug, Clone, PartialEq)]
pub struct ArchetypeRow {
    pub id: i64,
    pub uuid: String,
    pub name: String,
    pub format: Option<String>,
    pub color_identity: Option<String>,
    pub source: Source,
    pub definition_version: Option<String>,
    pub retired_at: Option<i64>,
    pub source_match_id: Option<i64>,
    pub incomplete: bool,
    /// A server archetype held as Custom by the offline download.
    pub offline_held: bool,
    pub created_at: i64,
    pub updated_at: i64,
    /// The parent it was merged into.
    pub merged_into: Option<ArchetypeOption>,
    /// The archetypes merged into it, by id.
    pub merged_from: Vec<ArchetypeOption>,
}

const OPTION_COLUMNS: &str = "id, uuid, name, format, color_identity, source, offline_held";

fn read_source(text: String) -> Result<Source, StoreError> {
    Source::parse(&text).ok_or_else(|| StoreError::Corrupt(format!("archetype source {text}")))
}

/// The raw columns of `OPTION_COLUMNS`, before the source is checked.
type OptionColumns = (
    i64,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    bool,
);

fn option_columns(row: &rusqlite::Row) -> Result<OptionColumns, rusqlite::Error> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

fn to_option(columns: OptionColumns) -> Result<ArchetypeOption, StoreError> {
    let (id, uuid, name, format, color_identity, source, offline_held) = columns;
    Ok(ArchetypeOption {
        id,
        uuid,
        name,
        format,
        color_identity,
        source: read_source(source)?,
        offline_held,
    })
}

fn options(
    conn: &Connection,
    sql: &str,
    param: rusqlite::types::Value,
) -> Result<Vec<ArchetypeOption>, StoreError> {
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map([param], option_columns)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(to_option(row?)?);
    }
    Ok(out)
}

/// The archetypes a picker offers for a format key (0.x `forFormat`): the
/// format's archetypes that are not retired, by name, then the fallbacks.
/// Without a format, only the fallbacks.
pub fn archetype_options(
    conn: &Connection,
    format: Option<&str>,
) -> Result<Vec<ArchetypeOption>, StoreError> {
    let sql = format!(
        "SELECT {OPTION_COLUMNS} FROM archetypes
         WHERE retired_at IS NULL AND (format = ?1 OR source = 'fallback')
         ORDER BY source = 'fallback', name, id"
    );
    let param = match format {
        Some(format) => rusqlite::types::Value::Text(format.to_string()),
        None => rusqlite::types::Value::Null,
    };
    options(conn, &sql, param)
}

/// One archetype with its merge links, `None` when there is no such row.
pub fn archetype(conn: &Connection, id: i64) -> Result<Option<ArchetypeRow>, StoreError> {
    let row = conn
        .prepare_cached(
            "SELECT id, uuid, name, format, color_identity, source, offline_held,
                 definition_version, retired_at, source_match_id, incomplete, created_at,
                 updated_at, merged_into_id
             FROM archetypes WHERE id = ?1",
        )?
        .query_row([id], |row| {
            Ok((
                option_columns(row)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<i64>>(8)?,
                row.get::<_, Option<i64>>(9)?,
                row.get::<_, i64>(10)?,
                row.get::<_, i64>(11)?,
                row.get::<_, i64>(12)?,
                row.get::<_, Option<i64>>(13)?,
                row.get::<_, bool>(6)?,
            ))
        })
        .optional()?;
    let Some((
        columns,
        version,
        retired_at,
        source_match_id,
        incomplete,
        created,
        updated,
        into,
        offline_held,
    )) = row
    else {
        return Ok(None);
    };
    let option = to_option(columns)?;
    let merged_into = match into {
        Some(parent) => options(
            conn,
            &format!("SELECT {OPTION_COLUMNS} FROM archetypes WHERE id = ?1"),
            rusqlite::types::Value::Integer(parent),
        )?
        .into_iter()
        .next(),
        None => None,
    };
    let merged_from = options(
        conn,
        &format!("SELECT {OPTION_COLUMNS} FROM archetypes WHERE merged_into_id = ?1 ORDER BY id"),
        rusqlite::types::Value::Integer(id),
    )?;
    Ok(Some(ArchetypeRow {
        id: option.id,
        uuid: option.uuid,
        name: option.name,
        format: option.format,
        color_identity: option.color_identity,
        source: option.source,
        definition_version: version,
        retired_at,
        source_match_id,
        incomplete: incomplete == 1,
        offline_held,
        created_at: created,
        updated_at: updated,
        merged_into,
        merged_from,
    }))
}

/// Every downloaded archetype's uuid and definition version (NULL before
/// its definition is fetched), by uuid: what the change check sends.
/// Retired ones are included (the server may bring them back).
pub fn downloaded_versions(conn: &Connection) -> Result<Vec<(String, Option<String>)>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT uuid, definition_version FROM archetypes
         WHERE source = 'downloaded' ORDER BY uuid",
    )?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Every downloaded archetype as the change check plans over it (spec
/// section 5), by uuid. Manual and fallback rows are never checked.
pub fn held_definitions(conn: &Connection) -> Result<Vec<archetypes::HeldDefinition>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT uuid, format, definition_version, retired_at IS NOT NULL FROM archetypes
         WHERE source = 'downloaded' ORDER BY uuid",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(archetypes::HeldDefinition {
            uuid: row.get(0)?,
            format: row.get(1)?,
            version: row.get(2)?,
            retired: row.get(3)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// The archetypes detection may answer with in a format key (archetypes
/// spec section 6.1): downloaded ones with a definition and manual ones,
/// not retired, by id. A merged one is included and resolved one hop after
/// the decision (0.x scored merged archetypes' decks too). A stored
/// definition that no longer parses counts as none (the lists still score).
pub fn held_candidates(conn: &Connection, format: &str) -> Result<Vec<Candidate>, StoreError> {
    let mut candidates: Vec<Candidate> = Vec::new();
    {
        let mut stmt = conn.prepare_cached(
            "SELECT id, uuid, source, color_identity, definition FROM archetypes
             WHERE format = ?1 AND retired_at IS NULL
               AND (source = 'manual' OR (source = 'downloaded' AND definition IS NOT NULL))
             ORDER BY id",
        )?;
        let rows = stmt.query_map([format], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?;
        for row in rows {
            let (archetype_id, uuid, source, color_identity, definition) = row?;
            candidates.push(Candidate {
                archetype_id,
                uuid,
                source: read_source(source)?,
                color_identity,
                definition: definition.and_then(|json| parse_definition(&json).ok()),
                lists: Vec::new(),
            });
        }
    }
    let mut stmt = conn.prepare_cached(
        "SELECT l.archetype_id, l.list, l.catalog_id, COALESCE(l.oracle_id, c.oracle_id),
             c.type, l.quantity, l.sideboard
         FROM archetype_cards l
         JOIN archetypes a ON a.id = l.archetype_id
         LEFT JOIN cards c ON c.catalog_id = l.catalog_id
         WHERE a.format = ?1 AND a.retired_at IS NULL
         ORDER BY l.archetype_id, l.list, l.sideboard, l.catalog_id",
    )?;
    let rows = stmt.query_map([format], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            ListCard {
                catalog_id: row.get(2)?,
                oracle_id: row.get(3)?,
                card_type: row.get(4)?,
                quantity: row.get(5)?,
                sideboard: row.get::<_, i64>(6)? == 1,
            },
        ))
    })?;
    // Candidates are by id and so are the cards: walk both in step.
    let mut at = 0;
    let mut current_list: Option<i64> = None;
    for row in rows {
        let (archetype_id, list, card) = row?;
        while at < candidates.len() && candidates[at].archetype_id < archetype_id {
            at += 1;
            current_list = None;
        }
        let Some(candidate) = candidates.get_mut(at) else {
            break;
        };
        if candidate.archetype_id != archetype_id {
            // A list of an archetype that is not a candidate.
            continue;
        }
        if current_list != Some(list) {
            candidate.lists.push(Vec::new());
            current_list = Some(list);
        }
        if let Some(cards) = candidate.lists.last_mut() {
            cards.push(card);
        }
    }
    Ok(candidates)
}

/// The detection queue (archetypes spec section 6.3): complete, listed
/// matches not detected yet, newest first. A log match waits for its game
/// sync (`games_synced_at`, decision C2) so it never blocks the queue
/// before its rows exist. A commander match with a commander card the cards
/// worker has not looked up yet is not in it (it would fill every slice and
/// starve older matches); the backfill runs every second, so it is queued
/// once the cards worker fills the rows. A card the API does not know
/// (`api_unknown_at`) is settled: the match is queued and that seat stays
/// empty until the card resolves (`cards::apply_resolutions` then queues the
/// match again). Limited matches are in it too: the worker marks
/// them detected without rows.
pub const DUE_MATCHES_SQL: &str = "SELECT id FROM listed_matches
     WHERE archetypes_detected_at IS NULL AND state = 'complete'
       AND (source <> 'log' OR games_synced_at IS NOT NULL)
       AND NOT EXISTS (SELECT 1 FROM match_commanders mc
                       LEFT JOIN cards c ON c.catalog_id = mc.catalog_id
                       WHERE mc.match_id = listed_matches.id
                         AND (c.oracle_id IS NULL OR c.name IS NULL)
                         AND c.api_unknown_at IS NULL)
     ORDER BY started_at DESC, id DESC
     LIMIT ?1";

/// The queue while a 0.x import runs: log matches only, so detection
/// never decides an imported match before the import's own archetype rows
/// land (archetypes spec section 15).
pub const DUE_LOG_MATCHES_SQL: &str = "SELECT id FROM listed_matches
     WHERE archetypes_detected_at IS NULL AND state = 'complete'
       AND source = 'log' AND games_synced_at IS NOT NULL
       AND NOT EXISTS (SELECT 1 FROM match_commanders mc
                       LEFT JOIN cards c ON c.catalog_id = mc.catalog_id
                       WHERE mc.match_id = listed_matches.id
                         AND (c.oracle_id IS NULL OR c.name IS NULL)
                         AND c.api_unknown_at IS NULL)
     ORDER BY started_at DESC, id DESC
     LIMIT ?1";

/// The next `limit` matches to detect (see `DUE_MATCHES_SQL`).
pub fn due_matches(conn: &Connection, limit: u32) -> Result<Vec<i64>, StoreError> {
    due_from(conn, DUE_MATCHES_SQL, limit)
}

/// The next `limit` log matches to detect (see `DUE_LOG_MATCHES_SQL`).
pub fn due_log_matches(conn: &Connection, limit: u32) -> Result<Vec<i64>, StoreError> {
    due_from(conn, DUE_LOG_MATCHES_SQL, limit)
}

fn due_from(conn: &Connection, sql: &str, limit: u32) -> Result<Vec<i64>, StoreError> {
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map([limit], |row| row.get(0))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// What detection reads for one match, `None` when there is no such match
/// or it is a log match the game sync has not written yet (decision C2).
pub fn detection_input(
    conn: &Connection,
    match_id: i64,
) -> Result<Option<DetectionInput>, StoreError> {
    let head = conn
        .prepare_cached(
            "SELECT m.format, d.archetype_id,
                 EXISTS (SELECT 1 FROM match_archetypes x
                         WHERE x.match_id = m.id AND x.is_opponent = 0 AND x.manual = 1),
                 m.opponent_archetype_none = 1
                     OR EXISTS (SELECT 1 FROM match_archetypes x
                                WHERE x.match_id = m.id AND x.is_opponent = 1 AND x.manual = 1)
             FROM matches m
             LEFT JOIN deck_versions v ON v.id = m.deck_version_id
             LEFT JOIN decks d ON d.id = v.deck_id
             WHERE m.id = ?1 AND (m.source <> 'log' OR m.games_synced_at IS NOT NULL)",
        )?
        .query_row([match_id], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, bool>(2)?,
                row.get::<_, bool>(3)?,
            ))
        })
        .optional()?;
    let Some((format, deck_archetype_id, player_manual, opponent_manual)) = head else {
        return Ok(None);
    };
    // Only the Arena commander codes: MTGO Brawl (`CBRAWL`) keys as `brawl`
    // too but keeps the usual detection.
    let commanders = if crate::match_commanders::is_commander_format(format.as_deref()) {
        match commander_seats(conn, match_id)? {
            Some(seats) => Some(seats),
            // A commander card is still waiting for the cards worker.
            None => return Ok(None),
        }
    } else {
        None
    };
    Ok(Some(DetectionInput {
        match_id,
        format,
        deck_archetype_id,
        player_manual,
        opponent_manual,
        player_cards: player_cards(conn, match_id)?,
        opponent_cards: opponent_cards(conn, match_id)?,
        commanders,
    }))
}

/// A pulled commander format match's commander archetypes (cross-device
/// sync spec section 7), made here from its stored commanders by the rules
/// detection uses (`archetypes::commander`), so the bundle's commander
/// uuids name rows of this device. Nothing while a commander's card data
/// is still waiting for the cards worker: detection makes them then. Only
/// a pulled (`imported`) row.
pub(crate) fn ensure_pulled_commanders(
    tx: &rusqlite::Transaction,
    match_id: i64,
    now_ms: i64,
) -> Result<(), StoreError> {
    let format: Option<Option<String>> = tx
        .prepare_cached("SELECT format FROM matches WHERE id = ?1 AND source = 'imported'")?
        .query_row([match_id], |row| row.get(0))
        .optional()?;
    let Some(format) = format else {
        return Ok(());
    };
    if !crate::match_commanders::is_commander_format(format.as_deref()) {
        return Ok(());
    }
    let Some(key) = format.as_deref().and_then(archetypes::format_key) else {
        return Ok(());
    };
    let Some(seats) = commander_seats(tx, match_id)? else {
        return Ok(());
    };
    for set in [seats.player, seats.opponent].into_iter().flatten() {
        crate::archetype_edit::ensure_commander(
            tx,
            &archetypes::commander::commander_uuid(&key, &set.oracle_ids),
            &archetypes::commander::commander_name(&set.names),
            &key,
            archetypes::commander::commander_colors(&set.color_identities).as_deref(),
            now_ms,
        )?;
    }
    Ok(())
}

/// The match's commanders with their card data, `None` while a commander's
/// card row is still waiting for the cards worker (no oracle id or name,
/// not marked unknown). Each seat is read on its own: a seat with a card the
/// API does not know (`api_unknown_at`) is `None` (no archetype for now) and
/// the other seat is still read.
fn commander_seats(conn: &Connection, match_id: i64) -> Result<Option<CommanderSeats>, StoreError> {
    let stored = crate::match_commanders::commanders_of(conn, match_id)?;
    let SeatCommanders::Settled(player) = commander_set(conn, &stored.player)? else {
        return Ok(None);
    };
    let SeatCommanders::Settled(opponent) = commander_set(conn, &stored.opponent)? else {
        return Ok(None);
    };
    Ok(Some(CommanderSeats { player, opponent }))
}

/// One seat's commander cards as detection reads them.
enum SeatCommanders {
    /// A card is waiting for the cards worker: the match is not ready.
    Pending,
    /// The seat's set, `None` when it has no commander rows or a card the
    /// API does not know.
    Settled(Option<CommanderSet>),
}

fn commander_set(conn: &Connection, catalog_ids: &[i64]) -> Result<SeatCommanders, StoreError> {
    if catalog_ids.is_empty() {
        return Ok(SeatCommanders::Settled(None));
    }
    let mut stmt = conn.prepare_cached(
        "SELECT oracle_id, name, color_identity, api_unknown_at FROM cards WHERE catalog_id = ?1",
    )?;
    let mut set = CommanderSet {
        oracle_ids: Vec::new(),
        names: Vec::new(),
        color_identities: Vec::new(),
    };
    let mut unknown = false;
    for catalog_id in catalog_ids {
        type Row = (Option<String>, Option<String>, Option<String>, Option<i64>);
        let card: Option<Row> = stmt
            .query_row([catalog_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .optional()?;
        match card {
            Some((Some(oracle_id), Some(name), identity, _)) => {
                set.oracle_ids.push(oracle_id);
                set.names.push(name);
                set.color_identities.push(identity.unwrap_or_default());
            }
            Some((_, _, _, Some(_))) => unknown = true,
            _ => return Ok(SeatCommanders::Pending),
        }
    }
    if unknown {
        return Ok(SeatCommanders::Settled(None));
    }
    Ok(SeatCommanders::Settled(Some(set)))
}

/// The card columns of `cards c` that `observed` reads from column 3 on.
const CARD_COLUMNS: &str = "c.oracle_id, c.name, c.type, c.color_identity, c.mana_cost";

fn observed(
    row: &rusqlite::Row,
    catalog_id: i64,
    quantity: i64,
    sideboard: bool,
) -> Result<ObservedCard, rusqlite::Error> {
    Ok(ObservedCard {
        catalog_id,
        quantity: u32::try_from(quantity).unwrap_or(0),
        sideboard,
        oracle_id: row.get(3)?,
        name: row.get(4)?,
        card_type: row.get(5)?,
        color_identity: row.get(6)?,
        mana_cost: row.get(7)?,
    })
}

/// Game 1's local deck (0.x's first-game `deck_json`): `main_qty` as main
/// and `side_qty` as side, one card per catalog id and board.
fn player_cards(conn: &Connection, match_id: i64) -> Result<Vec<ObservedCard>, StoreError> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT g.catalog_id, g.main_qty, g.side_qty, {CARD_COLUMNS}
         FROM game_cards g
         LEFT JOIN cards c ON c.catalog_id = g.catalog_id
         WHERE g.side = 'local' AND g.game_id = (
             SELECT id FROM games WHERE match_id = ?1 ORDER BY game_number LIMIT 1)
         ORDER BY g.catalog_id"
    ))?;
    let mut rows = stmt.query([match_id])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let catalog_id: i64 = row.get(0)?;
        let main: i64 = row.get(1)?;
        let side: i64 = row.get(2)?;
        if main > 0 {
            out.push(observed(row, catalog_id, main, false)?);
        }
        if side > 0 {
            out.push(observed(row, catalog_id, side, true)?);
        }
    }
    Ok(out)
}

/// The opponent's cards (0.x `AggregateOpponentCards`): `seen` summed per
/// catalog id across games, capped at `OPPONENT_COPIES_CAP`, all main.
fn opponent_cards(conn: &Connection, match_id: i64) -> Result<Vec<ObservedCard>, StoreError> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT s.catalog_id, MIN(s.seen, ?2), 0, {CARD_COLUMNS}
         FROM (SELECT g.catalog_id, SUM(g.seen) AS seen
               FROM game_cards g JOIN games m ON m.id = g.game_id
               WHERE m.match_id = ?1 AND g.side = 'opponent'
               GROUP BY g.catalog_id) s
         LEFT JOIN cards c ON c.catalog_id = s.catalog_id
         WHERE s.seen > 0
         ORDER BY s.catalog_id"
    ))?;
    let mut rows = stmt.query(params![match_id, OPPONENT_COPIES_CAP])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let catalog_id: i64 = row.get(0)?;
        let seen: i64 = row.get(1)?;
        out.push(observed(row, catalog_id, seen, false)?);
    }
    Ok(out)
}

/// A held archetype found by uuid: what detection needs to write an API
/// answer (spec section 6.1 step 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeldUuid {
    pub id: i64,
    /// The server retired it: detection skips it.
    pub retired: bool,
    pub merged_into_id: Option<i64>,
    /// A downloaded row known only by uuid (a sync pull, the 0.x import):
    /// its definition is still to be fetched.
    pub needs_definition: bool,
}

/// The archetype row with this uuid, `None` when v1 does not hold it.
pub fn held_uuid(conn: &Connection, uuid: &str) -> Result<Option<HeldUuid>, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT id, retired_at IS NOT NULL, merged_into_id,
                 source = 'downloaded' AND definition IS NULL
             FROM archetypes WHERE uuid = ?1",
        )?
        .query_row([uuid], |row| {
            Ok(HeldUuid {
                id: row.get(0)?,
                retired: row.get(1)?,
                merged_into_id: row.get(2)?,
                needs_definition: row.get(3)?,
            })
        })
        .optional()?)
}

/// The uuids of every archetype a picker can show (not retired), for the
/// picker's server catalog to leave out in one read (#489).
pub fn shown_uuids(conn: &Connection) -> Result<std::collections::HashSet<String>, StoreError> {
    let mut stmt = conn.prepare_cached("SELECT uuid FROM archetypes WHERE retired_at IS NULL")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut uuids = std::collections::HashSet::new();
    for row in rows {
        uuids.insert(row?);
    }
    Ok(uuids)
}

/// The parent an archetype was merged into, `None` when it is not merged
/// or does not exist (detection follows one hop, 0.x
/// `ResolveMergedArchetype`).
pub fn merged_into(conn: &Connection, archetype_id: i64) -> Result<Option<i64>, StoreError> {
    let parent: Option<Option<i64>> = conn
        .prepare_cached("SELECT merged_into_id FROM archetypes WHERE id = ?1")?
        .query_row([archetype_id], |row| row.get(0))
        .optional()?;
    Ok(parent.flatten())
}

/// What the deck prefill reads (spec section 6.3, 0.x
/// `SyncDecks::prefillArchetype`).
#[derive(Debug, Clone, PartialEq)]
pub struct DeckPrefillInput {
    /// MTGO's format code of the deck (`CMODERN`).
    pub format: String,
    pub archetype_id: Option<i64>,
    /// The latest version's maindeck, joined to the card rows.
    pub cards: Vec<ObservedCard>,
}

/// The deck's format, archetype and latest maindeck, `None` when there is
/// no such deck.
pub fn deck_prefill_input(
    conn: &Connection,
    deck_id: i64,
) -> Result<Option<DeckPrefillInput>, StoreError> {
    let head: Option<(String, Option<i64>)> = conn
        .prepare_cached("SELECT format, archetype_id FROM decks WHERE id = ?1")?
        .query_row([deck_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let Some((format, archetype_id)) = head else {
        return Ok(None);
    };
    let versions = crate::decks::versions_of(conn, deck_id)?;
    let mut cards = Vec::new();
    if let Some(latest) = versions.last() {
        let mut card_row = conn.prepare_cached(&format!(
            "SELECT NULL, NULL, NULL, {CARD_COLUMNS} FROM cards c WHERE c.catalog_id = ?1"
        ))?;
        for card in decks::decode_signature(&latest.signature) {
            if card.sideboard || card.quantity == 0 {
                continue;
            }
            let catalog_id = i64::from(card.catalog_id);
            let quantity = i64::from(card.quantity);
            let known = card_row
                .query_row([catalog_id], |row| {
                    observed(row, catalog_id, quantity, false)
                })
                .optional()?;
            cards.push(known.unwrap_or(ObservedCard {
                catalog_id,
                quantity: card.quantity,
                sideboard: false,
                oracle_id: None,
                name: None,
                card_type: None,
                color_identity: None,
                mana_cost: None,
            }));
        }
    }
    Ok(Some(DeckPrefillInput {
        format,
        archetype_id,
        cards,
    }))
}

/// The deck "Detect archetypes" banner's filter (0.x
/// `scopeFilteredByArchetype`): matches without an opponent archetype, or
/// whose opponent seat is this archetype.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeckFilter {
    NoOpponentArchetype,
    Opponent(i64),
}

/// The deck's complete listed matches under the filter (0.x
/// `QueueArchetypeDetectionForDeck`), by id.
pub fn deck_detection_matches(
    conn: &Connection,
    deck_id: i64,
    filter: DeckFilter,
) -> Result<Vec<i64>, StoreError> {
    let (seat, archetype_id) = match filter {
        DeckFilter::NoOpponentArchetype => ("NOT EXISTS", None),
        DeckFilter::Opponent(id) => ("EXISTS", Some(id)),
    };
    let sql = format!(
        "SELECT m.id FROM listed_matches m
         JOIN deck_versions v ON v.id = m.deck_version_id
         WHERE v.deck_id = ?1 AND m.state = 'complete'
           AND {seat} (SELECT 1 FROM match_archetypes x
                       WHERE x.match_id = m.id AND x.is_opponent = 1
                         AND (?2 IS NULL OR x.archetype_id = ?2))
         ORDER BY m.id"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(params![deck_id, archetype_id], |row| row.get(0))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// The deck's complete listed matches waiting for detection (0.x
/// `pendingArchetypeCount`).
pub fn pending_archetype_count(conn: &Connection, deck_id: i64) -> Result<u32, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT COUNT(*) FROM listed_matches m
             JOIN deck_versions v ON v.id = m.deck_version_id
             WHERE v.deck_id = ?1 AND m.state = 'complete'
               AND m.archetypes_detected_at IS NULL",
        )?
        .query_row([deck_id], |row| row.get(0))?)
}

/// The decks among these MTGO deck ids without an archetype, by id: what
/// the deck sync asks the prefill for.
pub fn decks_without_archetype(
    conn: &Connection,
    mtgo_ids: &[String],
) -> Result<Vec<i64>, StoreError> {
    let mut stmt =
        conn.prepare_cached("SELECT id FROM decks WHERE mtgo_id = ?1 AND archetype_id IS NULL")?;
    let mut out = Vec::new();
    for mtgo_id in mtgo_ids {
        if let Some(id) = stmt.query_row([mtgo_id], |row| row.get(0)).optional()? {
            out.push(id);
        }
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// Whether v1 holds any downloaded archetype with its definition (0.x
/// `PrivacyController` `hasArchetypeCatalog`): without one, offline
/// detection has only manual archetypes and the fallbacks to go on. A row
/// held from the offline download counts too.
pub fn has_downloaded_definition(conn: &Connection) -> Result<bool, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM archetypes
                 WHERE (source = 'downloaded' AND definition IS NOT NULL) OR offline_held = 1)",
        )?
        .query_row([], |row| row.get(0))?)
}

/// Whether any archetype is held from the offline download: the offline
/// mode card offers "Run detection" only then.
pub fn has_offline_held(conn: &Connection) -> Result<bool, StoreError> {
    Ok(conn
        .prepare_cached("SELECT EXISTS (SELECT 1 FROM archetypes WHERE offline_held = 1)")?
        .query_row([], |row| row.get(0))?)
}

/// Downloaded archetypes known only by uuid (a sync pull or the 0.x
/// import), by uuid: the worker downloads their definitions. Retired ones
/// are left to the full change check (which still sends them, in case the
/// server brings them back), so they are not asked about every round.
pub fn undefined_uuids(conn: &Connection) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT uuid FROM archetypes
         WHERE source = 'downloaded' AND definition IS NULL AND retired_at IS NULL
         ORDER BY uuid",
    )?;
    let rows = stmt.query_map([], |row| row.get(0))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Archetypes a manage page shows (0.x `GetFilteredArchetypes`).
pub const MANAGE_PER_PAGE: u32 = 25;

/// The manage screen's filters (spec section 11). Blank text is no filter.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ManageQuery {
    pub search: Option<String>,
    /// An archetypes format key; fallbacks have none, so they leave.
    pub format: Option<String>,
    /// `manual` or `downloaded` (`archetypes.source`); none for every source.
    pub source: Option<String>,
    /// 1-based.
    pub page: u32,
}

/// One manage row.
#[derive(Debug, Clone, PartialEq)]
pub struct ManageRow {
    pub archetype: ArchetypeOption,
    pub retired: bool,
    pub incomplete: bool,
    /// A server archetype held as Custom by the offline download.
    pub offline_held: bool,
    /// The parent it was merged into.
    pub merged_into: Option<ArchetypeOption>,
    /// Its lists (a manual archetype's variants, a downloaded one's key
    /// list).
    pub variants: u32,
    /// Complete listed matches whose opponent seat is this archetype.
    pub matches_faced: u32,
}

/// One manage page.
#[derive(Debug, Clone, PartialEq)]
pub struct ManagePage {
    pub rows: Vec<ManageRow>,
    pub total: u32,
    pub page: u32,
    pub per_page: u32,
    /// Every held format key, sorted.
    pub formats: Vec<String>,
}

fn blank_to_none(text: Option<&String>) -> Option<String> {
    let text = text?.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

/// The held archetypes, every source, fallbacks last, then by name; 25 a
/// page, searched by name (any case, wildcards as text) and filtered by
/// format and source.
pub fn manage_archetypes(conn: &Connection, query: &ManageQuery) -> Result<ManagePage, StoreError> {
    let search = blank_to_none(query.search.as_ref()).map(|text| text.to_lowercase());
    let format = blank_to_none(query.format.as_ref());
    let source = blank_to_none(query.source.as_ref());
    let page = query.page.max(1);
    let filter = "(?1 IS NULL OR instr(lower(a.name), ?1) > 0)
                  AND (?2 IS NULL OR a.format = ?2)
                  AND (?3 IS NULL OR a.source = ?3)";
    let total: u32 = conn
        .prepare_cached(&format!("SELECT COUNT(*) FROM archetypes a WHERE {filter}"))?
        .query_row(params![search, format, source], |row| row.get(0))?;
    let sql = format!(
        "SELECT a.id, a.uuid, a.name, a.format, a.color_identity, a.source,
             a.retired_at IS NOT NULL, a.incomplete,
             p.id, p.uuid, p.name, p.format, p.color_identity, p.source,
             (SELECT COUNT(DISTINCT list) FROM archetype_cards c WHERE c.archetype_id = a.id),
             (SELECT COUNT(*) FROM match_archetypes x
                  JOIN listed_matches m ON m.id = x.match_id
                  WHERE x.archetype_id = a.id AND x.is_opponent = 1 AND m.state = 'complete'),
             a.offline_held, p.offline_held
         FROM archetypes a
         LEFT JOIN archetypes p ON p.id = a.merged_into_id
         WHERE {filter}
         ORDER BY a.source = 'fallback', a.name, a.id
         LIMIT ?4 OFFSET ?5"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let offset = i64::from(page - 1) * i64::from(MANAGE_PER_PAGE);
    let mut found = stmt.query(params![search, format, source, MANAGE_PER_PAGE, offset])?;
    let mut rows = Vec::new();
    while let Some(row) = found.next()? {
        // Not `option_columns`: column 6 here is the retired flag.
        let archetype = to_option((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(16)?,
        ))?;
        let parent_id: Option<i64> = row.get(8)?;
        let merged_into = match parent_id {
            Some(id) => Some(to_option((
                id,
                row.get(9)?,
                row.get(10)?,
                row.get(11)?,
                row.get(12)?,
                row.get(13)?,
                row.get(17)?,
            ))?),
            None => None,
        };
        rows.push(ManageRow {
            archetype,
            retired: row.get(6)?,
            incomplete: row.get::<_, i64>(7)? == 1,
            offline_held: row.get(16)?,
            merged_into,
            variants: row.get(14)?,
            matches_faced: row.get(15)?,
        });
    }
    let mut formats = Vec::new();
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT format FROM archetypes WHERE format IS NOT NULL ORDER BY format",
    )?;
    for format in stmt.query_map([], |row| row.get(0))? {
        formats.push(format?);
    }
    Ok(ManagePage {
        rows,
        total,
        page,
        per_page: MANAGE_PER_PAGE,
        formats,
    })
}

/// What an archetype may be merged into or a variant moved to (0.x
/// `MergeCandidatesController`): the same format, not itself, not a
/// fallback, not merged, not retired, by name.
pub fn merge_candidates(conn: &Connection, id: i64) -> Result<Vec<ArchetypeOption>, StoreError> {
    let sql = format!(
        "SELECT {OPTION_COLUMNS} FROM archetypes
         WHERE format = (SELECT format FROM archetypes WHERE id = ?1)
           AND id <> ?1 AND source <> 'fallback' AND merged_into_id IS NULL
           AND retired_at IS NULL
         ORDER BY name, id"
    );
    options(conn, &sql, rusqlite::types::Value::Integer(id))
}

/// Where a variant may move (`reassign_variant`'s targets, T7 review I1):
/// the other manual archetypes of the format, not merged, not retired, by
/// name. 0.x also offered downloaded archetypes; in v1 their only list is
/// the change check's key list (spec section 7).
pub fn reassign_candidates(conn: &Connection, id: i64) -> Result<Vec<ArchetypeOption>, StoreError> {
    let sql = format!(
        "SELECT {OPTION_COLUMNS} FROM archetypes
         WHERE format = (SELECT format FROM archetypes WHERE id = ?1)
           AND id <> ?1 AND source = 'manual' AND merged_into_id IS NULL
           AND retired_at IS NULL
         ORDER BY name, id"
    );
    options(conn, &sql, rusqlite::types::Value::Integer(id))
}

/// A match the create form offers as a source (0.x
/// `CreateController::matches`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormMatch {
    pub id: i64,
    pub token: String,
    /// "Unknown" without one (0.x).
    pub opponent: String,
    pub started_at: i64,
}

/// The newest complete listed matches of the platform's accounts in a
/// format key (10, or 50 when searching opponent names); none without a
/// format.
pub fn form_matches(
    conn: &Connection,
    platform: Platform,
    format: Option<&str>,
    search: Option<&str>,
) -> Result<Vec<FormMatch>, StoreError> {
    let Some(format) = format.map(str::trim).filter(|text| !text.is_empty()) else {
        return Ok(Vec::new());
    };
    let search = search
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_lowercase);
    let limit = if search.is_some() { 50 } else { 10 };
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT m.id, m.token, COALESCE(o.display_name, o.username, 'Unknown'), m.started_at
         FROM listed_matches m
         LEFT JOIN opponents o ON o.id = m.opponent_id
         WHERE m.state = 'complete' AND m.format = 'C' || upper(?1)
           AND {of_platform}
           AND (?2 IS NULL
                OR instr(lower(COALESCE(o.display_name, o.username)), ?2) > 0)
         ORDER BY m.started_at DESC, m.id DESC
         LIMIT ?3",
        of_platform = account_of_platform("m.account_id", "?4"),
    ))?;
    let rows = stmt.query_map(params![format, search, limit, platform], |row| {
        Ok(FormMatch {
            id: row.get(0)?,
            token: row.get(1)?,
            opponent: row.get(2)?,
            started_at: row.get(3)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// A match's opponent cards for "Scan match" (0.x
/// `ScanMatchOpponentCards`): as detection reads them, summed across games,
/// capped at 4, all main. Empty for a match without any.
pub fn match_opponent_cards(
    conn: &Connection,
    match_id: i64,
) -> Result<Vec<ObservedCard>, StoreError> {
    opponent_cards(conn, match_id)
}

/// The deck a match was played with, through its version.
pub fn match_deck_id(conn: &Connection, match_id: i64) -> Result<Option<i64>, StoreError> {
    let deck: Option<Option<i64>> = conn
        .prepare_cached(
            "SELECT v.deck_id FROM matches m
             LEFT JOIN deck_versions v ON v.id = m.deck_version_id WHERE m.id = ?1",
        )?
        .query_row([match_id], |row| row.get(0))
        .optional()?;
    Ok(deck.flatten())
}

/// A match's MTGO format code (`CMODERN`).
pub fn match_format(conn: &Connection, match_id: i64) -> Result<Option<String>, StoreError> {
    let format: Option<Option<String>> = conn
        .prepare_cached("SELECT format FROM matches WHERE id = ?1")?
        .query_row([match_id], |row| row.get(0))
        .optional()?;
    Ok(format.flatten())
}

/// One card of an archetype list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListRow {
    pub catalog_id: i64,
    pub oracle_id: Option<String>,
    pub quantity: u32,
    pub sideboard: bool,
}

/// One list of an archetype (a manual variant, a downloaded key list).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchetypeList {
    pub list: i64,
    pub cards: Vec<ListRow>,
}

/// An archetype's lists in list order, cards by catalog id then board.
pub fn archetype_lists(conn: &Connection, id: i64) -> Result<Vec<ArchetypeList>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT list, catalog_id, oracle_id, quantity, sideboard FROM archetype_cards
         WHERE archetype_id = ?1 ORDER BY list, catalog_id, sideboard",
    )?;
    let mut rows = stmt.query([id])?;
    let mut out: Vec<ArchetypeList> = Vec::new();
    while let Some(row) = rows.next()? {
        let list: i64 = row.get(0)?;
        let card = ListRow {
            catalog_id: row.get(1)?,
            oracle_id: row.get(2)?,
            quantity: u32::try_from(row.get::<_, i64>(3)?).unwrap_or(0),
            sideboard: row.get(4)?,
        };
        match out.last_mut() {
            Some(last) if last.list == list => last.cards.push(card),
            _ => out.push(ArchetypeList {
                list,
                cards: vec![card],
            }),
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::open_writer;
    use crate::test_support::number;
    use archetypes::{ConditionKind, Source};
    use fixture_tools::TestDir;
    use rusqlite::{Connection, params};

    pub(crate) fn db(label: &str) -> (TestDir, Connection) {
        let dir = TestDir::new(label);
        let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        conn.execute_batch(
            "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);
             INSERT INTO opponents (id, username, first_seen_at) VALUES (1, 'Qopponent', 0);",
        )
        .expect("registries");
        (dir, conn)
    }

    /// A complete Modern match won against Qopponent.
    pub(crate) fn insert_match(conn: &Connection, id: i64, source: &str, started_at: i64) {
        conn.execute(
            "INSERT INTO matches (id, token, account_id, opponent_id, source, format,
                 match_type, state, outcome, started_at, last_activity_at, revision,
                 derived_version)
             VALUES (?1, 'Q-match-' || ?1, 1, 1, ?2, 'CMODERN', 'Modern', 'complete', 'win',
                 ?3, ?3, 1, 0)",
            params![id, source, started_at],
        )
        .expect("match");
    }

    pub(crate) fn insert_archetype(
        conn: &Connection,
        uuid: &str,
        name: &str,
        format: Option<&str>,
        source: &str,
    ) -> i64 {
        conn.execute(
            "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 1, 1)",
            params![uuid, name, format, source],
        )
        .expect("archetype");
        conn.last_insert_rowid()
    }

    pub(crate) fn insert_deck(conn: &Connection, id: i64, format: &str) {
        conn.execute(
            "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source)
             VALUES (?1, 'Q-deck-' || ?1, 'Qdeck', ?2, 1, 1, 'file')",
            params![id, format],
        )
        .expect("deck");
        conn.execute(
            "INSERT INTO deck_versions (id, deck_id, signature, modified_at)
             VALUES (?1 * 10, ?1, 'Q-sig-' || ?1, 1)",
            [id],
        )
        .expect("deck version");
    }

    pub(crate) fn seat(conn: &Connection, match_id: i64, is_opponent: bool, archetype_id: i64) {
        conn.execute(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
             VALUES (?1, ?2, ?3, 0.9, 0)",
            params![match_id, is_opponent, archetype_id],
        )
        .expect("seat");
    }

    fn columns(conn: &Connection, table: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info(?1) ORDER BY cid")
            .expect("prepare");
        stmt.query_map([table], |row| row.get(0))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect()
    }

    /// Review I2: "Run detection" waits for the offline download, so a
    /// catalog met online is not enough.
    #[test]
    fn has_offline_held_needs_the_offline_download() {
        let (_dir, conn) = db("archetypes-offline-held");
        let held = insert_archetype(&conn, "q-b", "Qb", Some("modern"), "downloaded");
        conn.execute(
            "UPDATE archetypes SET definition = '{}' WHERE id = ?1",
            [held],
        )
        .expect("definition");
        assert!(!has_offline_held(&conn).expect("online catalog only"));
    }

    /// 0.x `ArchetypeDeckSchemaTest` as the v1 shape: one list table keyed
    /// by archetype and list (no sample decks), cascades and unique keys.
    #[test]
    fn the_archetype_tables_have_the_v1_shape() {
        let (_dir, conn) = db("archetypes-schema");
        assert_eq!(
            columns(&conn, "archetypes"),
            [
                "id",
                "uuid",
                "name",
                "format",
                "color_identity",
                "source",
                "definition",
                "definition_version",
                "retired_at",
                "merged_into_id",
                "source_match_id",
                "incomplete",
                "created_at",
                "updated_at",
                "offline_held"
            ]
        );
        assert_eq!(
            columns(&conn, "archetype_cards"),
            [
                "archetype_id",
                "list",
                "catalog_id",
                "oracle_id",
                "quantity",
                "sideboard"
            ]
        );
        assert_eq!(
            columns(&conn, "match_archetypes"),
            [
                "match_id",
                "is_opponent",
                "archetype_id",
                "confidence",
                "manual"
            ],
            "no archetype_deck_id (ruling 9)"
        );
        assert!(columns(&conn, "decks").contains(&"archetype_id".to_string()));
        let matches = columns(&conn, "matches");
        assert!(matches.contains(&"archetypes_detected_at".to_string()));
        assert!(matches.contains(&"games_synced_at".to_string()));
        for table in [
            "archetype_decks",
            "archetype_deck_cards",
            "archetype_match_attempts",
        ] {
            assert!(columns(&conn, table).is_empty(), "{table} is not ported");
        }

        let id = insert_archetype(&conn, "q-uuid-1", "Qtron", Some("modern"), "manual");
        let card = |list: i64, quantity: i64| {
            conn.execute(
                "INSERT INTO archetype_cards (archetype_id, list, catalog_id, quantity, sideboard)
                 VALUES (?1, ?2, 501, ?3, 0)",
                params![id, list, quantity],
            )
        };
        card(1, 4).expect("list 1");
        card(2, 4).expect("the same card on another list");
        assert!(card(1, 2).is_err(), "one row per list, card and board");
        assert!(card(0, 1).is_err(), "lists start at 1");
        conn.execute(
            "INSERT INTO archetype_cards (archetype_id, list, catalog_id, quantity, sideboard)
             VALUES (?1, 1, 501, 1, 1)",
            [id],
        )
        .expect("the same card in the sideboard");

        insert_match(&conn, 1, "log", 100);
        seat(&conn, 1, true, id);
        let again = conn.execute(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
             VALUES (1, 1, ?1, 0.5)",
            [id],
        );
        assert!(again.is_err(), "one row per match and seat");
        conn.execute("DELETE FROM matches WHERE id = 1", [])
            .expect("delete the match");
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM match_archetypes"), 0);
        conn.execute("DELETE FROM archetypes WHERE id = ?1", [id])
            .expect("delete");
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM archetype_cards"),
            0,
            "lists go with their archetype"
        );
    }

    /// 0.x `FallbackSeedMigrationTest`.
    #[test]
    fn the_baseline_seeds_homebrew_and_rogue() {
        let (_dir, conn) = db("archetypes-seeds");
        let mut stmt = conn
            .prepare(
                "SELECT uuid, name, format, source, created_at, updated_at FROM archetypes
                 ORDER BY uuid",
            )
            .expect("prepare");
        let rows: Vec<(String, String, Option<String>, String, i64, i64)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })
            .expect("query")
            .map(|row| row.expect("row"))
            .collect();
        assert_eq!(
            rows,
            [
                (
                    archetypes::HOMEBREW_UUID.to_string(),
                    "Homebrew".to_string(),
                    None,
                    "fallback".to_string(),
                    0,
                    0
                ),
                (
                    archetypes::ROGUE_UUID.to_string(),
                    "Rogue".to_string(),
                    None,
                    "fallback".to_string(),
                    0,
                    0
                ),
            ]
        );
    }

    #[test]
    fn a_fallback_has_no_format_and_every_other_archetype_has_one() {
        let (_dir, conn) = db("archetypes-check-format");
        let insert = |uuid: &str, format: Option<&str>, source: &str| {
            conn.execute(
                "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
                 VALUES (?1, 'Qname', ?2, ?3, 1, 1)",
                params![uuid, format, source],
            )
        };
        assert!(insert("q-1", Some("modern"), "fallback").is_err());
        assert!(insert("q-2", None, "manual").is_err());
        assert!(insert("q-3", None, "downloaded").is_err());
        assert!(insert("q-4", Some("modern"), "other").is_err());
        insert("q-5", None, "fallback").expect("a fallback without a format");
        insert("q-6", Some("modern"), "manual").expect("a manual archetype");
    }

    #[test]
    fn only_a_downloaded_archetype_has_a_definition() {
        let (_dir, conn) = db("archetypes-check-definition");
        let insert = |uuid: &str, format: Option<&str>, source: &str, definition: &str| {
            conn.execute(
                "INSERT INTO archetypes (uuid, name, format, source, definition, created_at,
                     updated_at)
                 VALUES (?1, 'Qname', ?2, ?3, ?4, 1, 1)",
                params![uuid, format, source, definition],
            )
        };
        assert!(insert("q-1", Some("modern"), "manual", "{}").is_err());
        assert!(insert("q-2", None, "fallback", "{}").is_err());
        assert!(
            insert("q-3", Some("modern"), "downloaded", "not json").is_err(),
            "the definition is JSON"
        );
        insert("q-4", Some("modern"), "downloaded", "{\"cards\": []}").expect("downloaded");
    }

    #[test]
    fn an_archetype_is_never_merged_into_itself() {
        let (_dir, conn) = db("archetypes-check-merge");
        let id = insert_archetype(&conn, "q-1", "Qone", Some("modern"), "manual");
        let self_merge = conn.execute(
            "UPDATE archetypes SET merged_into_id = id WHERE id = ?1",
            [id],
        );
        assert!(self_merge.is_err());
    }

    /// 0.x `ManualArchetypeTest` "cascades delete to match_archetypes".
    #[test]
    fn deleting_an_archetype_deletes_its_match_rows() {
        let (_dir, conn) = db("archetypes-delete-cascades");
        let id = insert_archetype(&conn, "q-1", "Qone", Some("modern"), "manual");
        insert_match(&conn, 1, "log", 100);
        seat(&conn, 1, true, id);
        conn.execute("DELETE FROM archetypes WHERE id = ?1", [id])
            .expect("delete");
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM match_archetypes"), 0);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), 1);
    }

    /// 0.x `DeckArchetypeTest` "nullifies archetype_id when archetype is
    /// deleted".
    #[test]
    fn deleting_an_archetype_clears_the_deck_link() {
        let (_dir, conn) = db("archetypes-delete-deck-link");
        let id = insert_archetype(&conn, "q-1", "Qone", Some("modern"), "manual");
        insert_deck(&conn, 1, "CMODERN");
        conn.execute("UPDATE decks SET archetype_id = ?1", [id])
            .expect("link");
        conn.execute("DELETE FROM archetypes WHERE id = ?1", [id])
            .expect("delete");
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM decks WHERE archetype_id IS NULL"
            ),
            1
        );
    }

    fn uuids(options: &[ArchetypeOption]) -> Vec<String> {
        options.iter().map(|option| option.uuid.clone()).collect()
    }

    /// 0.x `FallbackForFormatScopeTest` "forFormat scope returns
    /// format-specific archetypes plus fallbacks", with retired ones hidden.
    #[test]
    fn the_options_of_a_format_are_its_archetypes_and_the_fallbacks() {
        let (_dir, conn) = db("archetypes-options");
        insert_archetype(&conn, "q-modern", "Qmodern", Some("modern"), "downloaded");
        insert_archetype(&conn, "q-pauper", "Qpauper", Some("pauper"), "downloaded");
        insert_archetype(&conn, "q-manual", "Qmanual", Some("modern"), "manual");
        let retired = insert_archetype(&conn, "q-old", "Qold", Some("modern"), "downloaded");
        conn.execute(
            "UPDATE archetypes SET retired_at = 5 WHERE id = ?1",
            [retired],
        )
        .expect("retire");
        let options = archetype_options(&conn, Some("modern")).expect("options");
        assert_eq!(
            uuids(&options),
            [
                "q-manual",
                "q-modern",
                archetypes::HOMEBREW_UUID,
                archetypes::ROGUE_UUID
            ],
            "by name, fallbacks last"
        );
        assert_eq!(options[0].name, "Qmanual");
        assert_eq!(options[0].source, Source::Manual);
        assert_eq!(options[2].source, Source::Fallback);
    }

    /// 0.x `FallbackForFormatScopeTest` "forFormat scope with null format only
    /// returns fallbacks".
    #[test]
    fn the_options_without_a_format_are_the_fallbacks() {
        let (_dir, conn) = db("archetypes-options-no-format");
        insert_archetype(&conn, "q-modern", "Qmodern", Some("modern"), "downloaded");
        let options = archetype_options(&conn, None).expect("options");
        assert_eq!(
            uuids(&options),
            [archetypes::HOMEBREW_UUID, archetypes::ROGUE_UUID]
        );
    }

    /// 0.x `Unit/Models/ArchetypeMergeRelationshipsTest`, as reads.
    #[test]
    fn a_merged_archetype_reads_its_parent_and_a_parent_its_children() {
        let (_dir, conn) = db("archetypes-merge-reads");
        let parent = insert_archetype(&conn, "q-parent", "Qparent", Some("modern"), "manual");
        let first = insert_archetype(&conn, "q-first", "Qfirst", Some("modern"), "manual");
        let second = insert_archetype(&conn, "q-second", "Qsecond", Some("modern"), "manual");
        conn.execute(
            "UPDATE archetypes SET merged_into_id = ?1 WHERE id IN (?2, ?3)",
            params![parent, first, second],
        )
        .expect("merge");

        let source = archetype(&conn, first).expect("read").expect("found");
        assert_eq!(source.uuid, "q-first");
        assert_eq!(
            source.merged_into.as_ref().map(|into| into.id),
            Some(parent)
        );
        assert!(source.merged_from.is_empty());

        let parent_row = archetype(&conn, parent).expect("read").expect("found");
        assert_eq!(parent_row.merged_into, None);
        let children: Vec<i64> = parent_row.merged_from.iter().map(|from| from.id).collect();
        assert_eq!(children, [first, second]);
        assert_eq!(archetype(&conn, 999).expect("read"), None);
    }

    #[test]
    fn the_downloaded_versions_are_every_downloaded_row() {
        let (_dir, conn) = db("archetypes-versions");
        let held = insert_archetype(&conn, "q-b", "Qb", Some("modern"), "downloaded");
        conn.execute(
            "UPDATE archetypes SET definition = '{}', definition_version = 'v7',
                 retired_at = 3 WHERE id = ?1",
            [held],
        )
        .expect("definition");
        insert_archetype(&conn, "q-a", "Qa", Some("modern"), "downloaded");
        insert_archetype(&conn, "q-c", "Qc", Some("modern"), "manual");
        assert_eq!(
            downloaded_versions(&conn).expect("versions"),
            [
                ("q-a".to_string(), None),
                ("q-b".to_string(), Some("v7".to_string()))
            ]
        );
        assert!(has_downloaded_definition(&conn).expect("held"));
        conn.execute("UPDATE archetypes SET definition = NULL", [])
            .expect("none");
        conn.execute(
            "UPDATE archetypes SET source = 'manual', offline_held = 1 WHERE uuid = 'q-c'",
            [],
        )
        .expect("held offline");
        assert!(
            has_downloaded_definition(&conn).expect("offline-held"),
            "an offline-held Custom row counts as a downloaded catalog"
        );
        assert!(has_offline_held(&conn).expect("held offline"));
        conn.execute("UPDATE archetypes SET offline_held = 0", [])
            .expect("not held");
        assert!(
            !has_downloaded_definition(&conn).expect("none"),
            "rows known only by uuid do not count"
        );
        // 0.x ManualArchetypeTest "does not download decklists for manual
        // archetypes": the check sees downloaded rows only.
        assert_eq!(
            held_definitions(&conn).expect("held"),
            [
                archetypes::HeldDefinition {
                    uuid: "q-a".to_string(),
                    format: Some("modern".to_string()),
                    version: None,
                    retired: false,
                },
                archetypes::HeldDefinition {
                    uuid: "q-b".to_string(),
                    format: Some("modern".to_string()),
                    version: Some("v7".to_string()),
                    retired: true,
                },
            ]
        );
    }

    fn insert_card(conn: &Connection, catalog_id: i64, oracle: &str, name: &str, kind: &str) {
        conn.execute(
            "INSERT INTO cards (catalog_id, oracle_id, name, type, color_identity)
             VALUES (?1, ?2, ?3, ?4, 'G')",
            params![catalog_id, oracle, name, kind],
        )
        .expect("card");
    }

    #[test]
    fn the_held_candidates_are_the_formats_usable_archetypes_with_their_lists() {
        let (_dir, conn) = db("archetypes-candidates");
        insert_card(&conn, 501, "q-oracle-tron", "Qtron Tower", "Land");
        let definition = archetypes::Definition {
            uuid: "q-down".to_string(),
            name: "Qdown".to_string(),
            conditions: vec![archetypes::Condition {
                kind: ConditionKind::InMainboard,
                cards: vec!["Qtron Tower".to_string()],
            }],
            ..archetypes::Definition::default()
        };
        let down = insert_archetype(&conn, "q-down", "Qdown", Some("modern"), "downloaded");
        conn.execute(
            "UPDATE archetypes SET definition = ?2, color_identity = 'G' WHERE id = ?1",
            params![down, serde_json::to_string(&definition).expect("json")],
        )
        .expect("definition");
        conn.execute(
            "INSERT INTO archetype_cards (archetype_id, list, catalog_id, oracle_id, quantity,
                 sideboard)
             VALUES (?1, 1, 501, NULL, 4, 0)",
            [down],
        )
        .expect("list 1");
        let manual = insert_archetype(&conn, "q-manual", "Qmanual", Some("modern"), "manual");
        conn.execute(
            "INSERT INTO archetype_cards (archetype_id, list, catalog_id, oracle_id, quantity,
                 sideboard)
             VALUES (?1, 2, 502, 'q-oracle-two', 2, 1), (?1, 1, 501, 'q-oracle-x', 3, 0)",
            [manual],
        )
        .expect("two variants");
        let merged = insert_archetype(&conn, "q-merged", "Qmerged", Some("modern"), "manual");
        conn.execute(
            "UPDATE archetypes SET merged_into_id = ?1 WHERE id = ?2",
            params![manual, merged],
        )
        .expect("merge");
        // Left out: no definition yet, retired, another format.
        insert_archetype(&conn, "q-bare", "Qbare", Some("modern"), "downloaded");
        let retired = insert_archetype(&conn, "q-old", "Qold", Some("modern"), "manual");
        conn.execute(
            "UPDATE archetypes SET retired_at = 1 WHERE id = ?1",
            [retired],
        )
        .expect("retire");
        insert_archetype(&conn, "q-pauper", "Qpauper", Some("pauper"), "manual");

        let candidates = held_candidates(&conn, "modern").expect("candidates");
        let ids: Vec<i64> = candidates.iter().map(|c| c.archetype_id).collect();
        assert_eq!(ids, [down, manual, merged], "merged ones resolve after");

        let first = &candidates[0];
        assert_eq!(first.uuid, "q-down");
        assert_eq!(first.source, Source::Downloaded);
        assert_eq!(first.color_identity.as_deref(), Some("G"));
        assert_eq!(first.definition.as_ref(), Some(&definition));
        assert_eq!(
            first.lists,
            [vec![archetypes::ListCard {
                catalog_id: 501,
                oracle_id: Some("q-oracle-tron".to_string()),
                card_type: Some("Land".to_string()),
                quantity: 4,
                sideboard: false,
            }]],
            "the card row's oracle id fills a missing one"
        );
        let second = &candidates[1];
        assert_eq!(second.definition, None);
        assert_eq!(second.lists.len(), 2);
        assert_eq!(second.lists[0][0].oracle_id.as_deref(), Some("q-oracle-x"));
        assert_eq!(second.lists[1][0].catalog_id, 502);
        assert!(second.lists[1][0].sideboard);
        assert!(candidates[2].lists.is_empty());
    }

    fn game_with_cards(conn: &Connection, match_id: i64, number: i64, cards: &str) {
        conn.execute(
            "INSERT INTO games (id, match_id, game_number, result, end_reason)
             VALUES (?1 * 10 + ?2, ?1, ?2, 'win', 'win')",
            params![match_id, number],
        )
        .expect("game");
        conn.execute_batch(&format!(
            "INSERT INTO game_cards (game_id, side, catalog_id, main_qty, side_qty, seen)
             VALUES {cards};"
        ))
        .expect("cards");
    }

    #[test]
    fn a_log_match_is_read_for_detection_once_its_game_sync_has_written() {
        let (_dir, conn) = db("archetypes-detection-input");
        insert_card(&conn, 501, "q-oracle-a", "Qcard A", "Creature");
        insert_match(&conn, 1, "log", 100);
        game_with_cards(
            &conn,
            1,
            1,
            "(11, 'local', 501, 4, 0, 0), (11, 'local', 502, 0, 2, 0),
             (11, 'local', 503, 1, 1, 0), (11, 'opponent', 601, 0, 0, 3),
             (11, 'opponent', 602, 0, 0, 1)",
        );
        game_with_cards(
            &conn,
            1,
            2,
            "(12, 'local', 504, 4, 0, 0), (12, 'opponent', 601, 0, 0, 2),
             (12, 'opponent', 603, 0, 0, 0)",
        );
        assert_eq!(
            detection_input(&conn, 1).expect("read"),
            None,
            "not before the game sync"
        );
        conn.execute("UPDATE matches SET games_synced_at = 5 WHERE id = 1", [])
            .expect("synced");
        let manual = insert_archetype(&conn, "q-manual", "Qmanual", Some("modern"), "manual");
        conn.execute(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
             VALUES (1, 1, ?1, 1.0, 1)",
            [manual],
        )
        .expect("a manual opponent");
        insert_deck(&conn, 7, "CMODERN");
        conn.execute("UPDATE decks SET archetype_id = ?1 WHERE id = 7", [manual])
            .expect("deck archetype");
        conn.execute("UPDATE matches SET deck_version_id = 70 WHERE id = 1", [])
            .expect("deck link");

        let input = detection_input(&conn, 1).expect("read").expect("ready");
        assert_eq!(input.match_id, 1);
        assert_eq!(input.format.as_deref(), Some("CMODERN"));
        assert_eq!(input.deck_archetype_id, Some(manual));
        assert!(!input.player_manual);
        assert!(input.opponent_manual);
        let observed = |catalog_id: i64, quantity: u32, sideboard: bool| archetypes::ObservedCard {
            catalog_id,
            quantity,
            sideboard,
            oracle_id: None,
            name: None,
            card_type: None,
            color_identity: None,
            mana_cost: None,
        };
        let mut known = observed(501, 4, false);
        known.oracle_id = Some("q-oracle-a".to_string());
        known.name = Some("Qcard A".to_string());
        known.card_type = Some("Creature".to_string());
        known.color_identity = Some("G".to_string());
        assert_eq!(
            input.player_cards,
            [
                known,
                observed(502, 2, true),
                observed(503, 1, false),
                observed(503, 1, true)
            ],
            "game 1's deck, main and side"
        );
        assert_eq!(
            input.opponent_cards,
            [observed(601, 4, false), observed(602, 1, false)],
            "seen summed across games, capped at 4, all main"
        );
        assert_eq!(detection_input(&conn, 99).expect("read"), None);
    }

    #[test]
    fn a_match_from_elsewhere_is_ready_without_a_game_sync() {
        let (_dir, conn) = db("archetypes-detection-input-imported");
        insert_match(&conn, 1, "imported", 100);
        let input = detection_input(&conn, 1).expect("read").expect("ready");
        assert!(input.player_cards.is_empty());
        assert!(input.opponent_cards.is_empty());
        assert_eq!(input.deck_archetype_id, None);
    }

    #[test]
    fn the_due_matches_are_complete_listed_synced_and_undetected_newest_first() {
        let (_dir, conn) = db("archetypes-due");
        for (id, source, started_at) in [
            (1, "log", 100),
            (2, "imported", 300),
            (3, "manual", 200),
            (4, "log", 400),
            (5, "history", 50),
            (6, "log", 500),
            (7, "imported", 600),
            (8, "imported", 700),
        ] {
            insert_match(&conn, id, source, started_at);
        }
        conn.execute_batch(
            "UPDATE matches SET games_synced_at = 1 WHERE id IN (1, 6);
             UPDATE matches SET archetypes_detected_at = 9 WHERE id = 6;
             UPDATE matches SET hidden_at = 9 WHERE id = 7;
             UPDATE matches SET state = 'ended' WHERE id = 8;",
        )
        .expect("states");
        assert_eq!(
            due_matches(&conn, 10).expect("due"),
            [2, 3, 1, 5],
            "4 waits for its game sync"
        );
        assert_eq!(due_matches(&conn, 2).expect("due"), [2, 3]);
    }

    #[test]
    fn arena_matches_are_due_once_synced() {
        let (_dir, conn) = db("archetypes-arena");
        insert_match(&conn, 2, "log", 200);
        conn.execute_batch(
            "INSERT INTO accounts (id, username, first_seen_at, platform, arena_user_id)
                 VALUES (2, 'Qarena#12345', 0, 'arena', 'Qarena-id');
             UPDATE matches SET account_id = 2 WHERE id = 2;",
        )
        .expect("arena match");
        game_with_cards(&conn, 2, 1, "(21, 'opponent', 9001, 0, 0, 2)");
        assert!(due_matches(&conn, 10).expect("due").is_empty());
        assert!(due_log_matches(&conn, 10).expect("due").is_empty());
        assert!(detection_input(&conn, 2).expect("read").is_none());
        conn.execute("UPDATE matches SET games_synced_at = 1 WHERE id = 2", [])
            .expect("stamp");
        assert_eq!(due_matches(&conn, 10).expect("due"), [2]);
        assert_eq!(due_log_matches(&conn, 10).expect("due"), [2]);
        let input = detection_input(&conn, 2).expect("read").expect("input");
        assert_eq!(input.opponent_cards.len(), 1);
        assert_eq!(input.opponent_cards[0].catalog_id, 9001);
    }

    /// A commander card the API does not know (`api_unknown_at`) is
    /// settled: the match is due and its other seat is read; a card still
    /// waiting for the cards worker keeps the match out of the queue.
    #[test]
    fn an_unknown_commander_card_leaves_the_other_seat_detectable() {
        let (_dir, conn) = db("archetypes-commander-unknown");
        insert_match(&conn, 2, "imported", 200);
        conn.execute_batch(
            "UPDATE matches SET format = 'ARENA_HISTORIC_BRAWL' WHERE id = 2;
             INSERT INTO cards (catalog_id, oracle_id, name, color_identity)
                 VALUES (5001, 'q-cmd-a', 'Qalpha', 'B,G');
             INSERT INTO cards (catalog_id) VALUES (5002);
             INSERT INTO match_commanders VALUES (2, 'player', 0, 5001), (2, 'opponent', 0, 5002);",
        )
        .expect("brawl match");
        assert!(
            due_matches(&conn, 10).expect("due").is_empty(),
            "5002 is pending"
        );
        assert!(detection_input(&conn, 2).expect("read").is_none());

        conn.execute(
            "UPDATE cards SET api_unknown_at = 7 WHERE catalog_id = 5002",
            [],
        )
        .expect("unknown");
        assert_eq!(due_matches(&conn, 10).expect("due"), [2]);
        let input = detection_input(&conn, 2).expect("read").expect("ready");
        let seats = input.commanders.expect("a commander format");
        assert_eq!(
            seats.player.map(|set| set.names),
            Some(vec!["Qalpha".to_string()])
        );
        assert_eq!(seats.opponent, None, "the unknown seat keeps no archetype");
    }

    #[test]
    fn the_due_matches_walk_the_partial_index() {
        let (_dir, conn) = db("archetypes-due-plan");
        let mut stmt = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {DUE_MATCHES_SQL}"))
            .expect("prepare");
        let plan: Vec<String> = stmt
            .query_map([10], |row| row.get::<_, String>(3))
            .expect("plan")
            .map(|row| row.expect("row"))
            .collect();
        let plan = plan.join(" | ");
        assert!(plan.contains("matches_detect_due"), "{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "{plan}");
    }

    #[test]
    fn a_held_uuid_is_found_with_its_retirement_and_merge() {
        let (_dir, conn) = db("archetypes-by-uuid");
        let parent = insert_archetype(&conn, "q-parent", "Qparent", Some("modern"), "downloaded");
        let source = insert_archetype(&conn, "q-source", "Qsource", Some("modern"), "downloaded");
        conn.execute(
            "UPDATE archetypes SET definition = '{}' WHERE id = ?1",
            [parent],
        )
        .expect("defined");
        conn.execute(
            "UPDATE archetypes SET merged_into_id = ?1, retired_at = 5 WHERE id = ?2",
            params![parent, source],
        )
        .expect("merged and retired");
        assert_eq!(
            held_uuid(&conn, "q-source").expect("read"),
            Some(HeldUuid {
                id: source,
                retired: true,
                merged_into_id: Some(parent),
                needs_definition: true,
            })
        );
        assert_eq!(
            held_uuid(&conn, "q-parent").expect("read"),
            Some(HeldUuid {
                id: parent,
                retired: false,
                merged_into_id: None,
                needs_definition: false,
            })
        );
        assert_eq!(held_uuid(&conn, "q-unknown").expect("read"), None);
        assert_eq!(merged_into(&conn, source).expect("read"), Some(parent));
        assert_eq!(merged_into(&conn, parent).expect("read"), None);
        assert_eq!(merged_into(&conn, 999).expect("read"), None);
    }

    /// 0.x `SyncDecks::prefillArchetype` reads the latest version's cards;
    /// v1 reads its maindeck by catalog id (spec 6.3).
    #[test]
    fn the_deck_prefill_input_is_the_latest_versions_maindeck() {
        let (_dir, conn) = db("archetypes-prefill-input");
        insert_card(&conn, 501, "q-oracle-a", "Qcard A", "Creature");
        insert_deck(&conn, 1, "CMODERN");
        let older = decks::signature(&[decks::DeckCard {
            catalog_id: 900,
            quantity: 4,
            sideboard: false,
        }]);
        let newer = decks::signature(&[
            decks::DeckCard {
                catalog_id: 501,
                quantity: 4,
                sideboard: false,
            },
            decks::DeckCard {
                catalog_id: 502,
                quantity: 2,
                sideboard: true,
            },
        ]);
        conn.execute_batch(&format!(
            "DELETE FROM deck_versions;
             INSERT INTO deck_versions (id, deck_id, signature, modified_at) VALUES
                 (10, 1, '{older}', 1), (11, 1, '{newer}', 2);"
        ))
        .expect("versions");
        let input = deck_prefill_input(&conn, 1).expect("read").expect("deck");
        assert_eq!(input.format, "CMODERN");
        assert_eq!(input.archetype_id, None);
        assert_eq!(
            input.cards,
            [ObservedCard {
                catalog_id: 501,
                quantity: 4,
                sideboard: false,
                oracle_id: Some("q-oracle-a".to_string()),
                name: Some("Qcard A".to_string()),
                card_type: Some("Creature".to_string()),
                color_identity: Some("G".to_string()),
                mana_cost: None,
            }],
            "the maindeck only"
        );
        assert_eq!(deck_prefill_input(&conn, 99).expect("read"), None);
    }

    /// 0.x `TriggerArchetypeDetectionTest` "queues detection only for
    /// matches with no opponent archetype when filter is none", "... whose
    /// opponent has the given archetype id", "returns 0 when no matches
    /// match the filter".
    #[test]
    fn a_decks_detection_matches_follow_the_opponent_archetype_filter() {
        let (_dir, conn) = db("archetypes-deck-detect");
        let target = insert_archetype(&conn, "q-target", "Qtarget", Some("modern"), "downloaded");
        let other = insert_archetype(&conn, "q-other", "Qother", Some("modern"), "downloaded");
        insert_deck(&conn, 1, "CMODERN");
        insert_deck(&conn, 2, "CMODERN");
        for id in 1..=5 {
            insert_match(&conn, id, "log", id * 100);
        }
        conn.execute_batch(
            "UPDATE matches SET deck_version_id = 10;
             UPDATE matches SET deck_version_id = 20 WHERE id = 5;
             UPDATE matches SET state = 'ended' WHERE id = 4;",
        )
        .expect("links");
        seat(&conn, 2, true, target);
        seat(&conn, 3, true, other);
        seat(&conn, 1, false, target);
        assert_eq!(
            deck_detection_matches(&conn, 1, DeckFilter::NoOpponentArchetype).expect("read"),
            [1]
        );
        assert_eq!(
            deck_detection_matches(&conn, 1, DeckFilter::Opponent(target)).expect("read"),
            [2]
        );
        assert!(
            deck_detection_matches(&conn, 2, DeckFilter::Opponent(target))
                .expect("read")
                .is_empty()
        );
        assert!(
            deck_detection_matches(&conn, 99, DeckFilter::NoOpponentArchetype)
                .expect("read")
                .is_empty()
        );
    }

    /// 0.x `TriggerArchetypeDetectionTest` "exposes pendingArchetypeCount
    /// as a page prop scoped to the deck": v1's pending matches are the
    /// deck's complete matches waiting for detection.
    #[test]
    fn the_pending_count_is_the_decks_undetected_complete_matches() {
        let (_dir, conn) = db("archetypes-pending");
        insert_deck(&conn, 1, "CMODERN");
        insert_deck(&conn, 2, "CMODERN");
        for id in 1..=4 {
            insert_match(&conn, id, "imported", id * 100);
        }
        conn.execute_batch(
            "UPDATE matches SET deck_version_id = 10;
             UPDATE matches SET archetypes_detected_at = 5 WHERE id = 3;
             UPDATE matches SET deck_version_id = 20 WHERE id = 4;",
        )
        .expect("links");
        assert_eq!(pending_archetype_count(&conn, 1).expect("count"), 2);
        assert_eq!(pending_archetype_count(&conn, 2).expect("count"), 1);
        assert_eq!(pending_archetype_count(&conn, 3).expect("count"), 0);
    }

    #[test]
    fn decks_without_an_archetype_are_found_by_mtgo_id() {
        let (_dir, conn) = db("archetypes-decks-without");
        let a = insert_archetype(&conn, "q-a", "Qa", Some("modern"), "manual");
        insert_deck(&conn, 1, "CMODERN");
        insert_deck(&conn, 2, "CMODERN");
        insert_deck(&conn, 3, "CMODERN");
        conn.execute("UPDATE decks SET archetype_id = ?1 WHERE id = 2", [a])
            .expect("set");
        let ids = [
            "Q-deck-1".to_string(),
            "Q-deck-2".to_string(),
            "Q-deck-missing".to_string(),
        ];
        assert_eq!(decks_without_archetype(&conn, &ids).expect("read"), [1]);
    }

    #[test]
    fn the_downloaded_rows_without_a_definition_are_listed() {
        let (_dir, conn) = db("archetypes-undownloaded");
        insert_archetype(&conn, "q-b", "Qb", Some("modern"), "downloaded");
        insert_archetype(&conn, "q-a", "Qa", Some("modern"), "downloaded");
        insert_archetype(&conn, "q-m", "Qm", Some("modern"), "manual");
        conn.execute(
            "UPDATE archetypes SET definition = '{}', definition_version = 'v' WHERE uuid = 'q-b'",
            [],
        )
        .expect("defined");
        // T5 review m3: a retired stub is not asked about on every round;
        // the full change check still sends it (it may come back).
        insert_archetype(&conn, "q-r", "Qr", Some("modern"), "downloaded");
        conn.execute(
            "UPDATE archetypes SET retired_at = 5 WHERE uuid = 'q-r'",
            [],
        )
        .expect("retired");
        assert_eq!(undefined_uuids(&conn).expect("read"), ["q-a"]);
    }

    // ---- The manage screen (T7): 0.x IndexTest and StickyFiltersTest ----

    fn manage(
        conn: &Connection,
        search: Option<&str>,
        format: Option<&str>,
        page: u32,
    ) -> ManagePage {
        manage_archetypes(
            conn,
            &ManageQuery {
                search: search.map(str::to_string),
                format: format.map(str::to_string),
                source: None,
                page,
            },
        )
        .expect("manage")
    }

    fn names(page: &ManagePage) -> Vec<String> {
        page.rows
            .iter()
            .map(|row| row.archetype.name.clone())
            .collect()
    }

    #[test]
    fn the_manage_list_shows_25_a_page_with_the_fallbacks_last() {
        let (_dir, conn) = db("archetypes-manage-pages");
        for index in 0..30 {
            insert_archetype(
                &conn,
                &format!("q-{index:02}"),
                &format!("Qarch {index:02}"),
                Some("modern"),
                "manual",
            );
        }
        let first = manage(&conn, None, None, 1);
        assert_eq!(
            (first.rows.len(), first.total, first.per_page, first.page),
            (25, 32, 25, 1)
        );
        assert_eq!(first.rows[0].archetype.name, "Qarch 00");
        let second = manage(&conn, None, None, 2);
        assert_eq!(
            names(&second),
            [
                "Qarch 25", "Qarch 26", "Qarch 27", "Qarch 28", "Qarch 29", "Homebrew", "Rogue"
            ]
        );
    }

    #[test]
    fn the_manage_list_filters_by_format() {
        let (_dir, conn) = db("archetypes-manage-format");
        for index in 0..5 {
            insert_archetype(
                &conn,
                &format!("q-m{index}"),
                "Qmodern",
                Some("modern"),
                "manual",
            );
        }
        for index in 0..3 {
            insert_archetype(
                &conn,
                &format!("q-l{index}"),
                "Qlegacy",
                Some("legacy"),
                "downloaded",
            );
        }
        let page = manage(&conn, None, Some("modern"), 1);
        assert_eq!(page.total, 5);
        assert!(
            page.rows
                .iter()
                .all(|row| row.archetype.format.as_deref() == Some("modern"))
        );
    }

    #[test]
    fn the_manage_list_filters_by_source() {
        let (_dir, conn) = db("archetypes-manage-source");
        insert_archetype(&conn, "q-m1", "Qcustom", Some("modern"), "manual");
        insert_archetype(&conn, "q-m2", "Qcustom two", Some("modern"), "manual");
        insert_archetype(&conn, "q-d1", "Qserver", Some("modern"), "downloaded");
        let by_source = |source: &str| {
            manage_archetypes(
                &conn,
                &ManageQuery {
                    search: None,
                    format: None,
                    source: Some(source.to_string()),
                    page: 1,
                },
            )
            .expect("manage")
        };
        assert_eq!(names(&by_source("manual")), ["Qcustom", "Qcustom two"]);
        assert_eq!(by_source("manual").total, 2);
        assert_eq!(names(&by_source("downloaded")), ["Qserver"]);
        // Homebrew and Rogue are the fallbacks; no source shows everything.
        assert_eq!(manage(&conn, None, None, 1).total, 5);
    }

    /// A server archetype held as Custom by the offline download shows no
    /// Custom badge, so the rows and the detail say which rows are held.
    #[test]
    fn the_manage_list_and_the_detail_say_which_rows_are_held_offline() {
        let (_dir, conn) = db("archetypes-manage-held");
        let own = insert_archetype(&conn, "q-own", "Qown", Some("modern"), "manual");
        let held = insert_archetype(&conn, "q-held", "Qheld", Some("modern"), "manual");
        conn.execute(
            "UPDATE archetypes SET offline_held = 1 WHERE id = ?1",
            [held],
        )
        .expect("held offline");
        let page = manage(&conn, Some("q"), None, 1);
        let flags: Vec<(String, bool)> = page
            .rows
            .iter()
            .map(|row| (row.archetype.name.clone(), row.offline_held))
            .collect();
        assert_eq!(
            flags,
            [("Qheld".to_string(), true), ("Qown".to_string(), false)]
        );
        assert!(
            archetype(&conn, held)
                .expect("read")
                .expect("row")
                .offline_held
        );
        assert!(
            !archetype(&conn, own)
                .expect("read")
                .expect("row")
                .offline_held
        );
    }

    #[test]
    fn the_manage_list_searches_names() {
        let (_dir, conn) = db("archetypes-manage-search");
        insert_archetype(&conn, "q-1", "Mono Red Aggro", Some("modern"), "manual");
        insert_archetype(&conn, "q-2", "Azorius Control", Some("modern"), "manual");
        insert_archetype(&conn, "q-3", "100% Burn_", Some("modern"), "manual");
        assert_eq!(
            names(&manage(&conn, Some("mono"), None, 1)),
            ["Mono Red Aggro"]
        );
        // Wildcards are text, not patterns.
        assert_eq!(names(&manage(&conn, Some("%"), None, 1)), ["100% Burn_"]);
        assert_eq!(names(&manage(&conn, Some("_"), None, 1)), ["100% Burn_"]);
        // Search and format together (StickyFiltersTest's first case).
        insert_archetype(&conn, "q-4", "Mono Red Prowess", Some("modern"), "manual");
        insert_archetype(&conn, "q-5", "Mono Blue Tron", Some("legacy"), "manual");
        assert_eq!(
            names(&manage(&conn, Some("mono"), Some("modern"), 1)),
            ["Mono Red Aggro", "Mono Red Prowess"]
        );
    }

    #[test]
    fn the_manage_list_offers_the_held_formats() {
        let (_dir, conn) = db("archetypes-manage-formats");
        insert_archetype(&conn, "q-1", "Qa", Some("modern"), "manual");
        insert_archetype(&conn, "q-2", "Qb", Some("legacy"), "downloaded");
        insert_archetype(&conn, "q-3", "Qc", Some("modern"), "downloaded");
        assert_eq!(manage(&conn, None, None, 1).formats, ["legacy", "modern"]);
    }

    #[test]
    fn a_manage_row_counts_matches_faced_and_variants_and_names_its_parent() {
        let (_dir, conn) = db("archetypes-manage-row");
        let parent = insert_archetype(&conn, "q-p", "Qparent", Some("modern"), "downloaded");
        let child = insert_archetype(&conn, "q-c", "Qchild", Some("modern"), "manual");
        conn.execute_batch(&format!(
            "UPDATE archetypes SET merged_into_id = {parent}, incomplete = 1 WHERE id = {child};
             UPDATE archetypes SET retired_at = 9 WHERE id = {parent};
             INSERT INTO archetype_cards (archetype_id, list, catalog_id, quantity, sideboard)
                 VALUES ({child}, 1, 5, 4, 0), ({child}, 1, 6, 1, 1), ({child}, 2, 5, 3, 0);"
        ))
        .expect("rows");
        for match_id in 1..=3 {
            insert_match(&conn, match_id, "log", 100 + match_id);
        }
        seat(&conn, 1, true, child);
        seat(&conn, 2, true, child);
        // The player's seat and a hidden match do not count as faced.
        seat(&conn, 3, false, child);
        insert_match(&conn, 4, "log", 200);
        seat(&conn, 4, true, child);
        conn.execute("UPDATE matches SET hidden_at = 1 WHERE id = 4", [])
            .expect("hidden");
        let page = manage(&conn, None, None, 1);
        let row = page
            .rows
            .iter()
            .find(|row| row.archetype.id == child)
            .expect("child row");
        assert_eq!(
            (row.matches_faced, row.variants, row.incomplete),
            (2, 2, true)
        );
        assert_eq!(
            row.merged_into.as_ref().map(|parent| parent.name.as_str()),
            Some("Qparent")
        );
        let parent_row = page
            .rows
            .iter()
            .find(|row| row.archetype.id == parent)
            .expect("parent");
        assert!(parent_row.retired);
        assert_eq!(parent_row.merged_into, None);
    }

    // ---- 0.x MergeCandidatesControllerTest ----

    #[test]
    fn merge_candidates_are_same_format_unmerged_non_fallbacks_but_self() {
        let (_dir, conn) = db("archetypes-merge-candidates");
        let source = insert_archetype(&conn, "q-s", "Source", Some("modern"), "manual");
        insert_archetype(&conn, "q-v", "Valid", Some("modern"), "downloaded");
        insert_archetype(&conn, "q-w", "WrongFormat", Some("pioneer"), "manual");
        let parent = insert_archetype(&conn, "q-p", "Parent", Some("modern"), "manual");
        let merged = insert_archetype(&conn, "q-m", "Merged", Some("modern"), "manual");
        let retired = insert_archetype(&conn, "q-r", "Retired", Some("modern"), "downloaded");
        conn.execute_batch(&format!(
            "UPDATE archetypes SET merged_into_id = {parent} WHERE id = {merged};
             UPDATE archetypes SET retired_at = 1 WHERE id = {retired};"
        ))
        .expect("merge");
        let shown: Vec<String> = merge_candidates(&conn, source)
            .expect("candidates")
            .into_iter()
            .map(|option| option.name)
            .collect();
        assert_eq!(shown, ["Parent", "Valid"]);
        // A fallback has no candidates (its format is none).
        assert!(merge_candidates(&conn, 1).expect("fallback").is_empty());
    }

    #[test]
    fn merge_candidates_are_ordered_by_name() {
        let (_dir, conn) = db("archetypes-merge-candidates-order");
        let source = insert_archetype(&conn, "q-s", "Qsource", Some("modern"), "manual");
        insert_archetype(&conn, "q-c", "Charlie", Some("modern"), "manual");
        insert_archetype(&conn, "q-a", "Alpha", Some("modern"), "manual");
        insert_archetype(&conn, "q-b", "Bravo", Some("modern"), "manual");
        let shown: Vec<String> = merge_candidates(&conn, source)
            .expect("candidates")
            .into_iter()
            .map(|option| option.name)
            .collect();
        assert_eq!(shown, ["Alpha", "Bravo", "Charlie"]);
    }

    /// T7 review I1: a variant moves only to what `reassign_variant`
    /// accepts: manual archetypes of the format, not the source, not merged,
    /// not retired. Downloaded ones stay merge candidates only.
    #[test]
    fn reassign_candidates_are_the_formats_other_manual_archetypes() {
        let (_dir, conn) = db("archetypes-reassign-candidates");
        let source = insert_archetype(&conn, "q-s", "Source", Some("modern"), "manual");
        insert_archetype(&conn, "q-b", "Bravo", Some("modern"), "manual");
        insert_archetype(&conn, "q-a", "Alpha", Some("modern"), "manual");
        insert_archetype(&conn, "q-d", "Downloaded", Some("modern"), "downloaded");
        insert_archetype(&conn, "q-w", "WrongFormat", Some("pioneer"), "manual");
        let parent = insert_archetype(&conn, "q-p", "Parent", Some("modern"), "manual");
        let merged = insert_archetype(&conn, "q-m", "Merged", Some("modern"), "manual");
        let retired = insert_archetype(&conn, "q-r", "Retired", Some("modern"), "manual");
        conn.execute_batch(&format!(
            "UPDATE archetypes SET merged_into_id = {parent} WHERE id = {merged};
             UPDATE archetypes SET retired_at = 1 WHERE id = {retired};"
        ))
        .expect("states");
        let shown: Vec<String> = reassign_candidates(&conn, source)
            .expect("candidates")
            .into_iter()
            .map(|option| option.name)
            .collect();
        assert_eq!(shown, ["Alpha", "Bravo", "Parent"]);
        let merge: Vec<String> = merge_candidates(&conn, source)
            .expect("merge")
            .into_iter()
            .map(|option| option.name)
            .collect();
        assert!(merge.contains(&"Downloaded".to_string()));
    }

    // ---- 0.x CreateControllerMatchesTest (the match select) ----

    fn named_opponent(conn: &Connection, match_id: i64, opponent_id: i64, name: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO opponents (id, username, first_seen_at) VALUES (?1, ?2, 0)",
            params![opponent_id, name],
        )
        .expect("opponent");
        conn.execute(
            "UPDATE matches SET opponent_id = ?2 WHERE id = ?1",
            params![match_id, opponent_id],
        )
        .expect("set opponent");
    }

    #[test]
    fn the_form_offers_the_10_newest_matches_of_the_format() {
        let (_dir, conn) = db("archetypes-form-matches");
        for index in 1..=12 {
            insert_match(&conn, index, "log", 1_000 - index);
            named_opponent(&conn, index, 10 + index, &format!("Qopp{index}"));
        }
        insert_match(&conn, 50, "log", 5_000);
        conn.execute("UPDATE matches SET format = 'CPIONEER' WHERE id = 50", [])
            .expect("pioneer");
        let shown = form_matches(&conn, Platform::Mtgo, Some("modern"), None).expect("matches");
        assert_eq!(shown.len(), 10);
        assert_eq!((shown[0].id, shown[0].opponent.as_str()), (1, "Qopp1"));
        assert_eq!(shown[0].started_at, 999);
    }

    #[test]
    fn the_form_offers_no_matches_without_a_format() {
        let (_dir, conn) = db("archetypes-form-no-format");
        insert_match(&conn, 1, "log", 100);
        assert!(
            form_matches(&conn, Platform::Mtgo, None, None)
                .expect("matches")
                .is_empty()
        );
        assert!(
            form_matches(&conn, Platform::Mtgo, Some(""), None)
                .expect("matches")
                .is_empty()
        );
    }

    #[test]
    fn the_form_searches_opponent_names() {
        let (_dir, conn) = db("archetypes-form-search");
        insert_match(&conn, 1, "log", 200);
        named_opponent(&conn, 1, 21, "QalphaPlayer");
        insert_match(&conn, 2, "log", 100);
        named_opponent(&conn, 2, 22, "QbetaPlayer");
        let shown =
            form_matches(&conn, Platform::Mtgo, Some("modern"), Some("ALPH")).expect("matches");
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].opponent, "QalphaPlayer");
    }

    #[test]
    fn the_form_reads_one_platform() {
        let (_dir, conn) = db("archetypes-form-arena");
        insert_match(&conn, 1, "log", 200);
        insert_match(&conn, 2, "log", 100);
        let token: String = conn
            .query_row("SELECT token FROM matches WHERE id = 2", [], |row| {
                row.get(0)
            })
            .expect("token");
        crate::test_support::move_match_to_arena(&conn, &token);
        let shown = form_matches(&conn, Platform::Mtgo, Some("modern"), None).expect("matches");
        assert_eq!(shown.iter().map(|row| row.id).collect::<Vec<_>>(), [1]);
        let shown = form_matches(&conn, Platform::Arena, Some("modern"), None).expect("matches");
        assert_eq!(shown.iter().map(|row| row.id).collect::<Vec<_>>(), [2]);
    }

    #[test]
    fn the_form_offers_only_complete_matches() {
        let (_dir, conn) = db("archetypes-form-complete");
        insert_match(&conn, 1, "log", 200);
        insert_match(&conn, 2, "log", 100);
        conn.execute("UPDATE matches SET state = 'in_progress' WHERE id = 2", [])
            .expect("started");
        let shown = form_matches(&conn, Platform::Mtgo, Some("modern"), None).expect("matches");
        assert_eq!(shown.iter().map(|row| row.id).collect::<Vec<_>>(), [1]);
    }

    // ---- 0.x ScanMatchControllerTest (the store part) ----

    #[test]
    fn a_scan_sums_the_opponents_cards_across_games_capped_at_4() {
        let (_dir, conn) = db("archetypes-scan");
        insert_match(&conn, 1, "log", 100);
        game_with_cards(
            &conn,
            1,
            1,
            "(11, 'opponent', 501, 0, 0, 3), (11, 'local', 900, 4, 0, 4)",
        );
        game_with_cards(
            &conn,
            1,
            2,
            "(12, 'opponent', 501, 0, 0, 2), (12, 'opponent', 502, 0, 0, 1)",
        );
        let scanned: Vec<(i64, u32, bool)> = match_opponent_cards(&conn, 1)
            .expect("scan")
            .into_iter()
            .map(|card| (card.catalog_id, card.quantity, card.sideboard))
            .collect();
        // The local player's 900 is not the opponent's.
        assert_eq!(scanned, [(501, 4, false), (502, 1, false)]);
    }

    #[test]
    fn a_scan_of_a_match_without_opponent_cards_finds_nothing() {
        let (_dir, conn) = db("archetypes-scan-empty");
        insert_match(&conn, 1, "log", 100);
        game_with_cards(&conn, 1, 1, "(11, 'local', 900, 4, 0, 4)");
        assert!(match_opponent_cards(&conn, 1).expect("scan").is_empty());
        assert!(match_opponent_cards(&conn, 99).expect("missing").is_empty());
    }

    // ---- 0.x StoreControllerRedirectTest (the store part) ----

    #[test]
    fn a_matchs_deck_is_found_through_its_version() {
        let (_dir, conn) = db("archetypes-match-deck");
        insert_deck(&conn, 3, "CMODERN");
        insert_match(&conn, 1, "log", 100);
        insert_match(&conn, 2, "log", 100);
        conn.execute("UPDATE matches SET deck_version_id = 30 WHERE id = 1", [])
            .expect("deck");
        assert_eq!(match_deck_id(&conn, 1).expect("deck"), Some(3));
        assert_eq!(match_deck_id(&conn, 2).expect("no deck"), None);
        assert_eq!(match_deck_id(&conn, 9).expect("no match"), None);
        assert_eq!(
            match_format(&conn, 1).expect("format").as_deref(),
            Some("CMODERN")
        );
    }

    #[test]
    fn an_archetypes_lists_read_in_list_order() {
        let (_dir, conn) = db("archetypes-lists");
        let id = insert_archetype(&conn, "q-m", "Qm", Some("modern"), "manual");
        conn.execute_batch(&format!(
            "INSERT INTO archetype_cards (archetype_id, list, catalog_id, quantity, sideboard)
                 VALUES ({id}, 3, 7, 1, 0), ({id}, 1, 9, 4, 1), ({id}, 1, 5, 4, 0);"
        ))
        .expect("lists");
        let read = archetype_lists(&conn, id).expect("lists");
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].list, 1);
        assert_eq!(
            read[0]
                .cards
                .iter()
                .map(|card| (card.catalog_id, card.quantity, card.sideboard))
                .collect::<Vec<_>>(),
            [(5, 4, false), (9, 4, true)]
        );
        assert_eq!(read[1].list, 3);
    }
}
