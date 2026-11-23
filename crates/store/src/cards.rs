//! Card stubs (cards spec section 5; 0.x `CreateMissingCards`,
//! `CreateMissingCardsFromLocalData` and `CreateMissingCardsFromTimelines`).
//!
//! A stub is a `cards` row with only a catalog id, perhaps a name (from the
//! game log). It is written in the same transaction as the write that first
//! references the id, so the cards worker can fill it from the API later.
//! Readers treat a stub without a name as an unknown card ("Card {id}").

use std::collections::{BTreeMap, BTreeSet};

use cards::tokens::{TOKEN_RARITY, TokenCard};
use mymtgo_api::{Resolution, Resolutions, UnresolvedCard};

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::Value;

use crate::StoreError;
use crate::platform::Platform;

/// A catalog id a write references, with the name its source gave (the game
/// log's), if any. `catalog_id` is whatever the source held: implausible ids
/// are refused, never stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardRef {
    pub catalog_id: i64,
    pub name: Option<String>,
}

impl CardRef {
    /// A reference without a name.
    pub fn id(catalog_id: u32) -> CardRef {
        CardRef {
            catalog_id: i64::from(catalog_id),
            name: None,
        }
    }
}

/// The integer entries of a stored JSON id list (a pick's
/// `cards_available`), each read on its own: one entry that is not an
/// integer drops only itself. Anything but an array gives none.
pub fn catalog_ids_in(json: &str) -> Vec<i64> {
    let Ok(Value::Array(entries)) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    entries.iter().filter_map(Value::as_i64).collect()
}

/// Insert a stub for every plausible id without a `cards` row and fill a null
/// name from a given one (a stub name never replaces a known name). Refused
/// ids are logged once per call. Returns how many rows were added.
///
/// The transaction must be on a connection from `open_writer`: the stubs
/// added are counted in its `temp.card_stubs_added` table (made there by
/// `create_stub_count`), which the writer thread reports after each command;
/// on any other connection a write that adds a stub fails.
pub fn ensure_stubs(tx: &Transaction, refs: &[CardRef]) -> Result<u32, StoreError> {
    let mut refused = Vec::new();
    let mut stored_name = tx.prepare_cached("SELECT name FROM cards WHERE catalog_id = ?1")?;
    let mut upsert = tx.prepare_cached(
        "INSERT INTO cards (catalog_id, name) VALUES (?1, ?2)
         ON CONFLICT (catalog_id) DO UPDATE SET name = COALESCE(cards.name, excluded.name)",
    )?;
    let mut added = 0;
    for card in refs {
        if !cards::storable(card.catalog_id) {
            refused.push(card.catalog_id);
            continue;
        }
        // `None`: no row yet; `Some(None)`: a stub without a name.
        let stored: Option<Option<String>> = stored_name
            .query_row([card.catalog_id], |row| row.get(0))
            .optional()?;
        match stored {
            None => {
                upsert.execute(params![card.catalog_id, card.name])?;
                added += 1;
            }
            // Only a null name is filled, so a known row is never written.
            Some(None) if card.name.is_some() => {
                upsert.execute(params![card.catalog_id, card.name])?;
            }
            Some(_) => {}
        }
    }
    if let Some(warning) = refusal_warning(&refused) {
        tracing::warn!("{warning}");
    }
    if added > 0 {
        tx.prepare_cached("UPDATE temp.card_stubs_added SET added = added + ?1")?
            .execute([added])?;
    }
    Ok(added)
}

/// A card a match's `.dat` log names differently from its MetaMessages:
/// `(catalog id, MetaMessage name, .dat name)`.
pub type NameFix = (u32, String, String);

/// Give a bare stub (a name at most: no card data, no token data) still
/// holding a match's MetaMessage name that match's `.dat` name (S1-5). Live,
/// the MetaMessages can name a card before the `.dat` file exists, and
/// `ensure_stubs` keeps the first name; a rebuild reads the `.dat` file
/// first. Only the match's own MetaMessage name gives way (review M5), so a
/// name another match gave is never flipped, and a row with card data keeps
/// its name.
pub fn replace_meta_names(tx: &Transaction, fixes: &[NameFix]) -> Result<(), StoreError> {
    let mut rename = tx.prepare_cached(
        "UPDATE cards SET name = ?3
         WHERE catalog_id = ?1 AND name = ?2
           AND scryfall_id IS NULL AND oracle_id IS NULL AND type IS NULL AND rarity IS NULL",
    )?;
    for (catalog_id, meta_name, dat_name) in fixes {
        rename.execute(params![catalog_id, meta_name, dat_name])?;
    }
    Ok(())
}

/// Stubs for catalog ids that have no name to give.
pub fn ensure_id_stubs(tx: &Transaction, catalog_ids: &[u32]) -> Result<u32, StoreError> {
    let refs: Vec<CardRef> = catalog_ids.iter().map(|&id| CardRef::id(id)).collect();
    ensure_stubs(tx, &refs)
}

/// Rows for tokens an Arena replay found no art for (`arena_replay`), so the
/// API's token-by-name fetch asks about them as it does MTGO's XML tokens:
/// a stub for each (counted as added, so the cards worker runs), then the
/// token's name, type and `token` rarity on a row still without a name.
pub fn ensure_token_stubs(tx: &Transaction, tokens: &[TokenCard]) -> Result<u32, StoreError> {
    let ids: Vec<u32> = tokens.iter().map(|token| token.catalog_id).collect();
    let added = ensure_id_stubs(tx, &ids)?;
    for token in tokens {
        apply_token(tx, token)?;
    }
    Ok(added)
}

/// The ids among `catalog_ids` that have no `cards` row at all, in id order
/// without repeats: what a list read from the API (an archetype decklist)
/// names but the user never met. Ids `ensure_stubs` would refuse (zero
/// and implausible ones) are left out, so asking for stubs never logs.
pub fn without_rows(conn: &Connection, catalog_ids: &[u32]) -> Result<Vec<u32>, StoreError> {
    let wanted: BTreeSet<u32> = catalog_ids
        .iter()
        .copied()
        .filter(|&id| id > 0 && cards::storable(i64::from(id)))
        .collect();
    let mut exists = conn.prepare_cached("SELECT 1 FROM cards WHERE catalog_id = ?1")?;
    let mut missing = Vec::new();
    for id in wanted {
        if !exists.exists([id])? {
            missing.push(id);
        }
    }
    Ok(missing)
}

/// The warning for ids `ensure_stubs` refused, `None` when it refused none
/// (0.x's text, with the ids).
pub(crate) fn refusal_warning(refused: &[i64]) -> Option<String> {
    if refused.is_empty() {
        return None;
    }
    let list: Vec<String> = refused.iter().map(i64::to_string).collect();
    Some(format!(
        "CreateMissingCards: refused ids that are not MTGO catalog ids: {}",
        list.join(", ")
    ))
}

/// Ids per `ensure_stubs` call in the full scan (0.x's chunk).
const SCAN_CHUNK: usize = 500;

/// The full scan: a stub for every id stored anywhere without a `cards` row
/// (deck version signatures, game rows, mulligans, timelines, draft picks,
/// limited deck snapshots, Arena course snapshots, deck covers and match
/// commanders), in chunks of 500. Returns how many rows were added.
pub fn scan_missing(tx: &Transaction) -> Result<u32, StoreError> {
    let mut found: BTreeSet<i64> = BTreeSet::new();

    // A signature that does not decode, or whose entries are not numeric,
    // gives no cards (`decode_signature`).
    for signature in strings(tx, "SELECT signature FROM deck_versions")? {
        for card in decks::decode_signature(&signature) {
            found.insert(i64::from(card.catalog_id));
        }
    }
    for sql in [
        "SELECT DISTINCT catalog_id FROM game_cards",
        "SELECT DISTINCT catalog_id FROM game_mulligan_cards",
        "SELECT DISTINCT picked_catalog_id FROM draft_picks WHERE picked_catalog_id IS NOT NULL",
        "SELECT DISTINCT cover_catalog_id FROM decks WHERE cover_catalog_id IS NOT NULL",
        "SELECT DISTINCT catalog_id FROM match_commanders",
    ] {
        let mut stmt = tx.prepare(sql)?;
        let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
        for row in rows {
            found.insert(row?);
        }
    }
    for game_id in integers(tx, "SELECT game_id FROM game_timelines")? {
        // A timeline that cannot be read gives nothing; the scan goes on.
        let Ok(Some(frames)) = crate::game_sync::timeline(tx, game_id) else {
            continue;
        };
        for frame in &frames {
            frame_catalog_ids(&frame.text, &mut found);
        }
    }
    for json in strings(tx, "SELECT cards_available FROM draft_picks")? {
        found.extend(catalog_ids_in(&json));
    }
    for json in strings(tx, "SELECT cards FROM arena_course_observations")? {
        found.extend(
            crate::arena_courses::stored_card_ids(&json)
                .into_iter()
                .map(i64::from),
        );
    }
    for json in strings(tx, "SELECT cards FROM limited_deck_snapshots")? {
        if let Ok(Value::Array(cards)) = serde_json::from_str::<Value>(&json) {
            for card in cards {
                if let Some(id) = card.get("catalog_id").and_then(Value::as_i64) {
                    found.insert(id);
                }
            }
        }
    }

    let mut missing = Vec::new();
    {
        let mut exists = tx.prepare_cached("SELECT 1 FROM cards WHERE catalog_id = ?1")?;
        for id in found {
            if !exists.exists([id])? {
                missing.push(id);
            }
        }
    }
    let mut added = 0;
    for chunk in missing.chunks(SCAN_CHUNK) {
        let refs: Vec<CardRef> = chunk
            .iter()
            .map(|&catalog_id| CardRef {
                catalog_id,
                name: None,
            })
            .collect();
        added += ensure_stubs(tx, &refs)?;
    }
    Ok(added)
}

/// The catalog ids of one stored frame's `Cards[].CatalogID` (0.x reads them
/// with `json_each` and drops empty ones, so 0 and missing ids are skipped).
fn frame_catalog_ids(frame: &str, found: &mut BTreeSet<i64>) {
    let Ok(json) = serde_json::from_str::<Value>(frame) else {
        return;
    };
    let Some(cards) = json.get("Cards").and_then(Value::as_array) else {
        return;
    };
    for card in cards {
        if let Some(id) = card.get("CatalogID").and_then(Value::as_i64)
            && id != 0
        {
            found.insert(id);
        }
    }
}

fn strings(conn: &Connection, sql: &str) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut found = Vec::new();
    for row in rows {
        found.push(row?);
    }
    Ok(found)
}

fn integers(conn: &Connection, sql: &str) -> Result<Vec<i64>, StoreError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
    let mut found = Vec::new();
    for row in rows {
        found.push(row?);
    }
    Ok(found)
}

/// Create the writer connection's stub count: a one-row TEMP table (it lives
/// only on this connection and is not part of the schema). `ensure_stubs`
/// adds to it inside the write's transaction, so a write that rolls back
/// counts nothing, and every write that adds stubs is counted whichever
/// command it came in.
pub(crate) fn create_stub_count(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS card_stubs_added (added INTEGER NOT NULL);
         DELETE FROM temp.card_stubs_added;
         INSERT INTO temp.card_stubs_added (added) VALUES (0);",
    )?;
    Ok(())
}

/// How many stubs this connection added since the last call, and reset the
/// count (the writer thread reports it to the tracker).
pub fn take_stubs_added(conn: &Connection) -> Result<u32, StoreError> {
    let added: u32 = conn.query_row("SELECT added FROM temp.card_stubs_added", [], |row| {
        row.get(0)
    })?;
    if added > 0 {
        conn.execute("UPDATE temp.card_stubs_added SET added = 0", [])?;
    }
    Ok(added)
}

/// What a population run learned, applied by `apply_resolutions` in one
/// transaction: names from MTGO's token XML first, then the API's answers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CardsUpdate {
    pub tokens: Vec<TokenCard>,
    pub resolutions: Resolutions,
    /// When the lookup ran (UTC ms): `resolutions.unknown` is marked
    /// `api_unknown_at` with it.
    pub unknown_at_ms: i64,
}

/// What `apply_resolutions` did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolveOutcome {
    /// Rows written (a token and an answer for the same row count twice).
    pub updated: u32,
    /// Catalog ids whose `name` or `oracle_id` changed: the game sync reads
    /// both, so the log matches using them are synced again.
    pub changed: Vec<u32>,
}

/// The `cards` table's counts for the Settings "Card data" card and the
/// Cards page.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CardDataCounts {
    pub total: u32,
    /// Rows without a name (0.x `cardsIncomplete`).
    pub incomplete: u32,
    /// Rows without a name, a Scryfall id or an image (0.x `missingCount`).
    pub missing: u32,
}

/// One card's image work (cards spec sections 7 and 13): its remote URLs
/// and whether its local files are recorded yet. The 0.x image copy and the
/// download queue fetch each file with a URL and no local file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImageJob {
    pub catalog_id: u32,
    pub image: Option<String>,
    pub art_crop: Option<String>,
    pub has_local_image: bool,
    pub has_local_art: bool,
}

/// The image work after catalog id `after`, at most `limit` rows in catalog
/// id order: rows with an `image` and no `local_image`, or an `art_crop`
/// and no `local_art_crop` (empty URLs count as none). Paging by catalog id
/// lets a run pass rows whose download failed (their columns stay null for
/// the next run).
pub fn images_to_download(
    conn: &Connection,
    after: u32,
    limit: u32,
) -> Result<Vec<ImageJob>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT catalog_id, NULLIF(image, ''), NULLIF(art_crop, ''),
             local_image IS NOT NULL, local_art_crop IS NOT NULL
         FROM cards
         WHERE catalog_id > ?1
           AND ((image <> '' AND local_image IS NULL)
             OR (art_crop <> '' AND local_art_crop IS NULL))
         ORDER BY catalog_id
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![after, limit], |row| {
        Ok(ImageJob {
            catalog_id: row.get(0)?,
            image: row.get(1)?,
            art_crop: row.get(2)?,
            has_local_image: row.get(3)?,
            has_local_art: row.get(4)?,
        })
    })?;
    let mut jobs = Vec::new();
    for row in rows {
        jobs.push(row?);
    }
    Ok(jobs)
}

/// Record a card's local files (`CardImagesStored`): only the given columns
/// are set. Returns the rows written (0 when the row is gone).
pub fn store_images(
    tx: &Transaction,
    catalog_id: u32,
    local_image: Option<&str>,
    local_art_crop: Option<&str>,
) -> Result<u32, StoreError> {
    let written = tx
        .prepare_cached(
            "UPDATE cards SET local_image = COALESCE(?2, local_image),
                 local_art_crop = COALESCE(?3, local_art_crop)
             WHERE catalog_id = ?1",
        )?
        .execute(params![catalog_id, local_image, local_art_crop])?;
    Ok(written as u32)
}

/// Forget the local files that are not in `present` (the file names now in
/// the images folder): each such column is nulled, so the download queue
/// fetches the file again. Returns the columns nulled.
pub fn keep_present_images(
    tx: &Transaction,
    present: &BTreeSet<String>,
) -> Result<u32, StoreError> {
    let mut rows: Vec<(i64, Option<String>, Option<String>)> = Vec::new();
    {
        let mut stmt = tx.prepare(
            "SELECT catalog_id, local_image, local_art_crop FROM cards
             WHERE local_image IS NOT NULL OR local_art_crop IS NOT NULL",
        )?;
        let found = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        for row in found {
            rows.push(row?);
        }
    }
    let gone = |name: &Option<String>| name.as_ref().is_some_and(|name| !present.contains(name));
    let mut nulled = 0;
    for (catalog_id, image, art) in rows {
        if gone(&image) {
            tx.execute(
                "UPDATE cards SET local_image = NULL WHERE catalog_id = ?1",
                [catalog_id],
            )?;
            nulled += 1;
        }
        if gone(&art) {
            tx.execute(
                "UPDATE cards SET local_art_crop = NULL WHERE catalog_id = ?1",
                [catalog_id],
            )?;
            nulled += 1;
        }
    }
    Ok(nulled)
}

/// How long a card the API answered without is passed over before it is
/// asked about again (#501).
pub const UNKNOWN_RECHECK_MS: i64 = 7 * 24 * 3_600_000;

/// The settings key holding the app version that last asked about every
/// unresolved card: a new version asks about them all again.
pub const UNKNOWN_CHECKED_VERSION: &str = "cards_unknown_checked_version";

/// The rows the API passes look up: every row not resolved yet
/// (`scryfall_id IS NULL`), in catalog id order, but a row the API answered
/// without (`api_unknown_at`) after `unknown_since` (#501). `None` asks
/// about every unresolved row. A row whose rarity is `token` is a token
/// (asked by id and name together). An Endstep row carries its name and
/// printing from `endstep_cards` (asked about by those); every other row
/// has an empty printing.
pub fn unresolved(
    conn: &Connection,
    unknown_since: Option<i64>,
) -> Result<Vec<UnresolvedCard>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT catalog_id, name, rarity FROM cards WHERE scryfall_id IS NULL
           AND (?1 IS NULL OR api_unknown_at IS NULL OR api_unknown_at <= ?1)
         ORDER BY catalog_id",
    )?;
    let rows = stmt.query_map([unknown_since], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut cards = Vec::new();
    for row in rows {
        let (catalog_id, name, rarity) = row?;
        // Every stored id is plausible; anything else is never asked about.
        let Ok(mtgo_id) = u32::try_from(catalog_id) else {
            continue;
        };
        let card = UnresolvedCard::new(mtgo_id, name, rarity.as_deref() == Some(TOKEN_RARITY));
        cards.push(card);
    }
    drop(stmt);
    for card in &mut cards {
        if let Some(row) = cards::endstep_row_of(i64::from(card.mtgo_id)) {
            with_endstep_printing(conn, card, row)?;
        }
    }
    Ok(cards)
}

/// Fill an Endstep row's name and printing from its `endstep_cards` row
/// (the name as Endstep sends it). A row without one keeps its own name.
fn with_endstep_printing(
    conn: &Connection,
    card: &mut UnresolvedCard,
    row: u32,
) -> Result<(), StoreError> {
    let found: Option<(String, String, String)> = conn
        .prepare_cached("SELECT name, set_code, collector_number FROM endstep_cards WHERE id = ?1")?
        .query_row([row], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .optional()?;
    if let Some((name, set_code, collector_number)) = found {
        card.name = Some(name);
        card.set_code = set_code;
        card.collector_number = collector_number;
    }
    Ok(())
}

/// The settings key set once resolved rows stored before the API sent mana
/// costs have been looked up again with costs.
pub const MANA_COSTS_FILLED: &str = "cards_mana_costs_filled";

/// The rows to look up again for their mana cost: resolved (they have a
/// Scryfall id) with no cost stored, which is every row resolved before the
/// API sent costs. Lands (no cost, stored `""` once answered) and tokens
/// (the API holds none) are left out, so they are never asked about again,
/// and so are Endstep rows (only ever resolved by an API that sends costs).
/// In catalog id order, asked as the rows they are (by id, then name).
pub fn mana_cost_refresh(conn: &Connection) -> Result<Vec<UnresolvedCard>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT catalog_id, name FROM cards
         WHERE scryfall_id IS NOT NULL AND mana_cost IS NULL
           AND (type IS NULL OR instr(type, 'Land') = 0)
           AND (rarity IS NULL OR rarity <> ?1)
           AND catalog_id < ?2
         ORDER BY catalog_id",
    )?;
    let rows = stmt.query_map(params![TOKEN_RARITY, cards::ENDSTEP_ID_OFFSET], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
    })?;
    let mut cards = Vec::new();
    for row in rows {
        let (catalog_id, name) = row?;
        let Ok(mtgo_id) = u32::try_from(catalog_id) else {
            continue;
        };
        cards.push(UnresolvedCard::new(mtgo_id, name, false));
    }
    Ok(cards)
}

/// The settings key counting the refresh runs that saw costs but had a
/// request fail; the refresh is given up at `MANA_COST_ATTEMPTS_MAX`.
pub const MANA_COST_ATTEMPTS: &str = "cards_mana_costs_attempts";

/// The failed refresh runs after which the refresh is recorded as done
/// anyway, so a request the API always fails is not sent forever.
pub const MANA_COST_ATTEMPTS_MAX: u32 = 5;

fn setting_value(conn: &Connection, key: &str) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()?)
}

/// Whether the mana cost refresh has run (`MANA_COSTS_FILLED` is set).
pub fn mana_costs_filled(conn: &Connection) -> Result<bool, StoreError> {
    Ok(setting_value(conn, MANA_COSTS_FILLED)?.is_some())
}

/// The settings row that records the mana cost refresh as done.
/// The app version that last asked about every unresolved card
/// (`UNKNOWN_CHECKED_VERSION`), `None` when unset or unreadable.
pub fn unknown_checked_version(conn: &Connection) -> Result<Option<String>, StoreError> {
    Ok(setting_value(conn, UNKNOWN_CHECKED_VERSION)?
        .and_then(|value| serde_json::from_str::<String>(&value).ok()))
}

/// The settings row recording that `version` asked about every unresolved
/// card.
pub fn unknown_checked_version_row(version: &str) -> (String, String) {
    (
        UNKNOWN_CHECKED_VERSION.to_string(),
        serde_json::Value::String(version.to_string()).to_string(),
    )
}

pub fn mana_costs_filled_row() -> (String, String) {
    (MANA_COSTS_FILLED.to_string(), "1".to_string())
}

/// The failed refresh runs so far (`MANA_COST_ATTEMPTS`, 0 when unset or
/// unreadable).
pub fn mana_cost_attempts(conn: &Connection) -> Result<u32, StoreError> {
    Ok(setting_value(conn, MANA_COST_ATTEMPTS)?
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0))
}

/// The settings row recording `attempts` failed refresh runs.
pub fn mana_cost_attempts_row(attempts: u32) -> (String, String) {
    (MANA_COST_ATTEMPTS.to_string(), attempts.to_string())
}

/// A card image's two sources: the remote URL and v1's local file name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImageSources {
    pub image: Option<String>,
    pub local_image: Option<String>,
}

/// The image of a card by oracle id (0.x `ImageBase64Controller`: the first
/// card with that oracle id). Printings share an oracle id in v1, so the
/// one with a local file comes first, then one with a remote image, each by
/// catalog id. `None` when no row has that oracle id.
pub fn oracle_image(
    conn: &Connection,
    oracle_id: &str,
) -> Result<Option<ImageSources>, StoreError> {
    let found = conn
        .prepare_cached(
            "SELECT image, local_image FROM cards WHERE oracle_id = ?1
             ORDER BY local_image IS NULL, image IS NULL, catalog_id LIMIT 1",
        )?
        .query_row([oracle_id], |row| {
            Ok(ImageSources {
                image: row.get(0)?,
                local_image: row.get(1)?,
            })
        })
        .optional()?;
    Ok(found)
}

/// The catalog ids of rows without a name (the token XML's candidates).
pub fn nameless(conn: &Connection) -> Result<Vec<u32>, StoreError> {
    let ids = integers(
        conn,
        "SELECT catalog_id FROM cards WHERE name IS NULL ORDER BY catalog_id",
    )?;
    Ok(ids
        .into_iter()
        .filter_map(|id| u32::try_from(id).ok())
        .collect())
}

/// The card data counts (total, without a name, missing data).
pub fn card_data_counts(conn: &Connection) -> Result<CardDataCounts, StoreError> {
    card_counts_between(conn, i64::MIN, i64::MAX)
}

/// The catalog id range `[low, high)` one platform's cards sit in: MTGO
/// below `cards::ARENA_ID_OFFSET`, Arena from there to
/// `cards::ENDSTEP_ID_OFFSET`, Endstep from there to twice it.
pub(crate) fn catalog_range(platform: Platform) -> (i64, i64) {
    let arena = i64::from(cards::ARENA_ID_OFFSET);
    let endstep = i64::from(cards::ENDSTEP_ID_OFFSET);
    match platform {
        Platform::Mtgo => (i64::MIN, arena),
        Platform::Arena => (arena, endstep),
        Platform::Endstep => (endstep, 2 * endstep),
    }
}

/// The card data counts of one platform's cards.
pub fn platform_card_counts(
    conn: &Connection,
    platform: Platform,
) -> Result<CardDataCounts, StoreError> {
    let (low, high) = catalog_range(platform);
    card_counts_between(conn, low, high)
}

fn card_counts_between(
    conn: &Connection,
    low: i64,
    high: i64,
) -> Result<CardDataCounts, StoreError> {
    Ok(conn.query_row(
        "SELECT COUNT(*),
                COUNT(*) FILTER (WHERE name IS NULL),
                COUNT(*) FILTER (WHERE name IS NULL OR scryfall_id IS NULL OR image IS NULL)
         FROM cards
         WHERE catalog_id >= ?1 AND catalog_id < ?2",
        params![low, high],
        |row| {
            Ok(CardDataCounts {
                total: row.get(0)?,
                incomplete: row.get(1)?,
                missing: row.get(2)?,
            })
        },
    )?)
}

/// A row's identity before an update: what the game sync reads.
type Identity = (Option<String>, Option<String>);

/// Apply a population run (0.x `PopulateTokensFromXml` then `updateCard`):
/// each token fills its row while the row still has no name; each answer
/// writes 0.x's columns (the answer's value, else the row's, for `name`,
/// `type`, `sub_type` and `rarity`; the normalised colour identity, which a
/// token keeps when the answer has none; `colors`, `cmc`, `mana_cost`,
/// `set_name`, `set_code` and `art_crop` as answered, null included;
/// `scryfall_id`, `oracle_id` and `image` as answered). The row keeps its
/// own catalog id. Rows that are gone are skipped. The Endstep matches
/// holding an Endstep card that resolved just now are detected again (their
/// card names changed); MTGO matches are synced again by the caller
/// (`log_matches_using`).
pub fn apply_resolutions(
    tx: &Transaction,
    update: &CardsUpdate,
) -> Result<ResolveOutcome, StoreError> {
    let mut before: BTreeMap<u32, Identity> = BTreeMap::new();
    let ids = update.tokens.iter().map(|token| token.catalog_id).chain(
        update
            .resolutions
            .resolved
            .iter()
            .map(|found| found.mtgo_id),
    );
    for id in ids {
        if !before.contains_key(&id)
            && let Some(identity) = identity_of(tx, id)?
        {
            before.insert(id, identity);
        }
    }
    let mut newly_endstep: Vec<u32> = Vec::new();
    for found in &update.resolutions.resolved {
        if cards::endstep_row_of(i64::from(found.mtgo_id)).is_some()
            && is_unresolved(tx, found.mtgo_id)?
        {
            newly_endstep.push(found.mtgo_id);
        }
    }
    let mut outcome = ResolveOutcome::default();
    for token in &update.tokens {
        outcome.updated += apply_token(tx, token)?;
    }
    for found in &update.resolutions.resolved {
        outcome.updated += apply_answer(tx, found)?;
    }
    let mut mark = tx.prepare_cached(
        "UPDATE cards SET api_unknown_at = ?2 WHERE catalog_id = ?1 AND scryfall_id IS NULL",
    )?;
    for id in &update.resolutions.unknown {
        mark.execute(params![id, update.unknown_at_ms])?;
    }
    for (id, old) in before {
        if identity_of(tx, id)?.is_some_and(|new| new != old) {
            outcome.changed.push(id);
        }
    }
    for id in newly_endstep {
        if !is_unresolved(tx, id)? {
            requeue_detection_using(tx, id)?;
        }
    }
    // A commander that resolves may fill a seat detection left empty (a
    // card the API did not know): its detected Brawl matches go back to
    // detection.
    let mut redetect = tx.prepare_cached(
        "UPDATE matches SET archetypes_detected_at = NULL
         WHERE archetypes_detected_at IS NOT NULL
           AND id IN (SELECT match_id FROM match_commanders WHERE catalog_id = ?1)",
    )?;
    for id in &outcome.changed {
        redetect.execute([id])?;
    }
    Ok(outcome)
}

/// Whether the row exists without a Scryfall id.
fn is_unresolved(tx: &Transaction, catalog_id: u32) -> Result<bool, StoreError> {
    Ok(tx
        .prepare_cached("SELECT 1 FROM cards WHERE catalog_id = ?1 AND scryfall_id IS NULL")?
        .query_row([catalog_id], |_| Ok(()))
        .optional()?
        .is_some())
}

/// Clear the detection stamp of the matches with a game row for this card.
fn requeue_detection_using(tx: &Transaction, catalog_id: u32) -> Result<(), StoreError> {
    tx.prepare_cached(
        "UPDATE matches SET archetypes_detected_at = NULL
         WHERE id IN (SELECT games.match_id FROM game_cards
                      JOIN games ON games.id = game_cards.game_id
                      WHERE game_cards.catalog_id = ?1)",
    )?
    .execute([catalog_id])?;
    Ok(())
}

fn identity_of(tx: &Transaction, catalog_id: u32) -> Result<Option<Identity>, StoreError> {
    Ok(tx
        .prepare_cached("SELECT name, oracle_id FROM cards WHERE catalog_id = ?1")?
        .query_row([catalog_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?)
}

fn apply_token(tx: &Transaction, token: &TokenCard) -> Result<u32, StoreError> {
    let written = tx
        .prepare_cached(
            "UPDATE cards SET name = ?2, type = ?3, sub_type = ?4, rarity = ?5,
                 color_identity = ?6
             WHERE catalog_id = ?1 AND name IS NULL",
        )?
        .execute(params![
            token.catalog_id,
            token.name,
            token.card_type,
            token.sub_type,
            TOKEN_RARITY,
            token.color_identity,
        ])?;
    Ok(written as u32)
}

fn apply_answer(tx: &Transaction, found: &Resolution) -> Result<u32, StoreError> {
    let answer = &found.card;
    // A token keeps its stored identity when the answer has none (0.x);
    // the answer's identity is normalised already (`CardAnswer`).
    let written = tx
        .prepare_cached(
            "UPDATE cards SET
                 scryfall_id = ?2,
                 oracle_id = ?3,
                 name = COALESCE(?4, name),
                 type = COALESCE(?5, type),
                 sub_type = COALESCE(?6, sub_type),
                 rarity = COALESCE(?7, rarity),
                 color_identity = CASE WHEN ?8 IS NOT NULL THEN ?8
                                       WHEN ?9 THEN color_identity
                                       ELSE NULL END,
                 colors = ?10,
                 cmc = ?11,
                 mana_cost = ?12,
                 set_name = ?13,
                 set_code = ?14,
                 art_crop = ?15,
                 image = ?16,
                 api_unknown_at = NULL
             WHERE catalog_id = ?1",
        )?
        .execute(params![
            found.mtgo_id,
            answer.scryfall_id,
            answer.oracle_id,
            answer.name,
            answer.card_type,
            answer.sub_type,
            answer.rarity,
            answer.color_identity,
            found.is_token,
            answer.colors,
            answer.cmc,
            answer.mana_cost,
            answer.set_name,
            answer.set_code,
            answer.art_crop,
            answer.image,
        ])?;
    Ok(written as u32)
}

/// The tokens of the MTGO log matches with a `game_cards` row for any of
/// these catalog ids (`game_cards_catalog`), oldest first: the matches whose
/// game rows read those cards. An Arena match's rows never read card data,
/// so it is never synced again for it.
pub fn log_matches_using(
    conn: &Connection,
    catalog_ids: &[u32],
) -> Result<Vec<String>, StoreError> {
    let mut match_ids: BTreeSet<i64> = BTreeSet::new();
    {
        let mut stmt = conn.prepare_cached(
            "SELECT DISTINCT games.match_id FROM game_cards
             JOIN games ON games.id = game_cards.game_id
             WHERE game_cards.catalog_id = ?1",
        )?;
        for id in catalog_ids {
            let rows = stmt.query_map([id], |row| row.get::<_, i64>(0))?;
            for row in rows {
                match_ids.insert(row?);
            }
        }
    }
    let mut tokens = Vec::new();
    let mut stmt = conn.prepare_cached(
        "SELECT m.token FROM matches m JOIN accounts a ON a.id = m.account_id
             WHERE m.id = ?1 AND m.source = 'log' AND a.platform = 'mtgo'",
    )?;
    for id in match_ids {
        if let Some(token) = stmt.query_row([id], |row| row.get(0)).optional()? {
            tokens.push(token);
        }
    }
    Ok(tokens)
}

// ---- The Cards page (0.x `Cards\IndexController`, `ArchetypesController`) ------

/// Cards per page (0.x `paginate(80)`).
pub const CARDS_PER_PAGE: u32 = 80;

/// What the Cards page asks for. `hidden_types` are category names; names
/// that are not a category are ignored, and `Other` is never hidden.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardsPageQuery {
    pub search: Option<String>,
    /// An archetype format key (`modern`): only cards a held archetype of
    /// the format holds, with popularity in that format.
    pub format: Option<String>,
    pub hidden_types: Vec<String>,
    pub group_printings: bool,
    /// 1-based.
    pub page: u32,
    /// Whose cards the page lists (`catalog_range`): MTGO catalog ids sit
    /// below `cards::ARENA_ID_OFFSET`, Arena ones from there to
    /// `cards::ENDSTEP_ID_OFFSET`, Endstep ones above that.
    pub platform: Platform,
}

impl CardsPageQuery {
    /// The page's defaults: no filters, printings grouped, page one.
    pub fn new() -> CardsPageQuery {
        CardsPageQuery {
            search: None,
            format: None,
            hidden_types: Vec::new(),
            group_printings: true,
            page: 1,
            platform: Platform::Mtgo,
        }
    }
}

impl Default for CardsPageQuery {
    fn default() -> CardsPageQuery {
        CardsPageQuery::new()
    }
}

/// One card of the grid.
#[derive(Debug, Clone, PartialEq)]
pub struct CardTile {
    pub catalog_id: i64,
    pub name: Option<String>,
    pub set_code: Option<String>,
    pub type_line: Option<String>,
    pub sub_type: Option<String>,
    pub oracle_id: Option<String>,
    /// The remote image URL.
    pub image: Option<String>,
    /// The downloaded file's name in v1's images folder.
    pub local_image: Option<String>,
    /// Held archetypes that hold the card (any printing when grouped).
    pub popularity: u32,
}

impl CardTile {
    /// The key the archetype sheet asks for: the oracle id, or `id:{catalog_id}`.
    pub fn group_key(&self) -> String {
        match &self.oracle_id {
            Some(oracle) => oracle.clone(),
            None => format!("id:{}", self.catalog_id),
        }
    }
}

/// One page of the grid with what the toolbar needs.
#[derive(Debug, Clone, PartialEq)]
pub struct CardsPage {
    pub cards: Vec<CardTile>,
    pub page: u32,
    pub last_page: u32,
    /// Rows matching the filters (grouped when grouping is on).
    pub total: u32,
    /// `(format key, label)`: the archetypes' formats, by label.
    pub formats: Vec<(String, String)>,
    /// Rows with no name, `scryfall_id` or `image`.
    pub missing_count: u32,
    /// Every row in `cards`.
    pub total_count: u32,
}

/// An archetype that holds a card (the sheet's rows).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardArchetype {
    pub id: i64,
    pub name: String,
    pub format: Option<String>,
    pub format_label: Option<String>,
    pub color_identity: Option<String>,
    pub maindeck: bool,
    pub sideboard: bool,
    pub deck_count: u32,
}

/// The group key of a catalog id's printing: its oracle id, or `id:{id}`.
const GROUP_KEY_SQL: &str = "coalesce(oracle_id, 'id:' || catalog_id)";

/// The SQL that names a row's category (0.x's first-match `LIKE` order).
fn category_sql() -> String {
    let mut sql = String::from("CASE");
    for category in cards::stats::TYPE_CATEGORIES {
        sql.push_str(&format!(" WHEN type LIKE '%{category}%' THEN '{category}'"));
    }
    sql.push_str(&format!(" ELSE '{}' END", cards::stats::OTHER_CATEGORY));
    sql
}

/// Whether list row `ac`'s archetype `a` is held for the Cards page
/// (archetypes spec section 12, ruling 8): not retired, of the format `?2`
/// when it is not NULL. The one copy of the predicate (#300 F13).
const HELD_ARCHETYPE: &str = "a.retired_at IS NULL AND (?2 IS NULL OR a.format = ?2)";

/// The list rows the Cards page counts: rows of held archetypes (a
/// downloaded archetype's list 1 or a manual archetype's variants). `ac` is
/// the list row, `a` the archetype.
fn held_rows() -> String {
    format!(
        "archetype_cards ac
         JOIN archetypes a ON a.id = ac.archetype_id
         WHERE {HELD_ARCHETYPE}"
    )
}

/// Popularity per printing: the distinct held archetypes holding it (0.x
/// `ungroupedQuery`).
fn printing_popularity() -> String {
    format!(
        "SELECT ac.catalog_id AS gk, COUNT(DISTINCT ac.archetype_id) AS popularity
         FROM {}
         GROUP BY 1",
        held_rows()
    )
}

/// Popularity per group of printings: the distinct held archetypes holding
/// any printing of it (0.x `groupedQuery`). A list card's group is its card
/// row's.
fn group_popularity() -> String {
    format!(
        "SELECT coalesce(c.oracle_id, 'id:' || ac.catalog_id) AS gk,
                COUNT(DISTINCT ac.archetype_id) AS popularity
         FROM archetype_cards ac
         JOIN archetypes a ON a.id = ac.archetype_id
         LEFT JOIN cards c ON c.catalog_id = ac.catalog_id
         WHERE {HELD_ARCHETYPE}
         GROUP BY 1"
    )
}

/// The formats the select offers (0.x `Archetype::distinct('format')`):
/// the formats of archetypes that are not retired, by label.
fn page_formats(conn: &Connection) -> Result<Vec<(String, String)>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT format FROM archetypes
         WHERE format IS NOT NULL AND retired_at IS NULL",
    )?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut formats = Vec::new();
    for row in rows {
        let key = row?;
        let label = archetypes::format_label(&key);
        formats.push((key, label));
    }
    formats.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    Ok(formats)
}

/// One page of the Cards page (0.x `Cards\IndexController`): filtered,
/// grouped, ordered by popularity then name, 80 per page. With a format,
/// only cards some held archetype of that format holds are listed, and
/// their popularity counts that format only. Popularity is joined in SQL so
/// the order holds across pages.
pub fn cards_page(conn: &Connection, query: &CardsPageQuery) -> Result<CardsPage, StoreError> {
    let search = query
        .search
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|search| {
            // `%` and `_` in the search match only themselves.
            let escaped = search
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            format!("%{escaped}%")
        });
    let format = query.format.as_deref().filter(|f| !f.is_empty());
    let (low, high) = catalog_range(query.platform);
    let mut conditions: Vec<String> = vec![
        "(?1 IS NULL OR name LIKE ?1 ESCAPE '\\')".to_string(),
        format!("catalog_id >= {low} AND catalog_id < {high}"),
    ];
    let hidden: Vec<&str> = cards::stats::TYPE_CATEGORIES
        .iter()
        .copied()
        .filter(|category| query.hidden_types.iter().any(|h| h == category))
        .collect();
    if !hidden.is_empty() {
        let list: Vec<String> = hidden.iter().map(|h| format!("'{h}'")).collect();
        conditions.push(format!("({}) NOT IN ({})", category_sql(), list.join(", ")));
    }
    let filtered = format!("SELECT * FROM cards WHERE {}", conditions.join(" AND "));
    let (source, key, popularity) = if query.group_printings {
        (
            format!(
                "SELECT * FROM cards WHERE catalog_id IN (
                     SELECT MAX(catalog_id) FROM ({filtered}) GROUP BY {GROUP_KEY_SQL})"
            ),
            "coalesce(s.oracle_id, 'id:' || s.catalog_id)",
            group_popularity(),
        )
    } else {
        (filtered, "s.catalog_id", printing_popularity())
    };
    // 0.x: a format inner-joins the popularity, so unplayed cards drop out.
    let join = if format.is_some() {
        "JOIN"
    } else {
        "LEFT JOIN"
    };
    let rows = format!(
        "SELECT s.catalog_id, s.name, s.set_code, s.type, s.sub_type, s.oracle_id, s.image,
                s.local_image, coalesce(pop.popularity, 0) AS popularity
         FROM ({source}) s
         {join} ({popularity}) pop ON pop.gk = {key}"
    );

    // The page and the total in one query (`COUNT(*) OVER ()`), so the
    // popularity aggregate runs once (#300 F13). Only a page past the last
    // one (no rows) counts again and reads the last page.
    let requested = query.page.max(1);
    let (mut cards, mut total) = page_rows(conn, &rows, &search, format, requested)?;
    let mut page = requested;
    if cards.is_empty() && requested > 1 {
        total = conn.query_row(
            &format!("SELECT COUNT(*) FROM ({rows})"),
            params![search, format],
            |row| row.get(0),
        )?;
        page = total.div_ceil(CARDS_PER_PAGE).max(1);
        (cards, _) = page_rows(conn, &rows, &search, format, page)?;
    }
    let last_page = total.div_ceil(CARDS_PER_PAGE).max(1);

    let counts = platform_card_counts(conn, query.platform)?;
    Ok(CardsPage {
        cards,
        page,
        last_page,
        total,
        formats: page_formats(conn)?,
        missing_count: counts.missing,
        total_count: counts.total,
    })
}

/// One page of the Cards page's rows, by popularity then name, with the
/// count of every row (0 when the page is empty).
fn page_rows(
    conn: &Connection,
    rows: &str,
    search: &Option<String>,
    format: Option<&str>,
    page: u32,
) -> Result<(Vec<CardTile>, u32), StoreError> {
    let offset = i64::from(page - 1) * i64::from(CARDS_PER_PAGE);
    let mut stmt = conn.prepare(&format!(
        "SELECT r.*, COUNT(*) OVER () FROM ({rows}) r
         ORDER BY r.popularity DESC, r.name, r.catalog_id
         LIMIT {CARDS_PER_PAGE} OFFSET {offset}"
    ))?;
    let found = stmt.query_map(params![search, format], |row| {
        Ok((
            CardTile {
                catalog_id: row.get(0)?,
                name: row.get(1)?,
                set_code: row.get(2)?,
                type_line: row.get(3)?,
                sub_type: row.get(4)?,
                oracle_id: row.get(5)?,
                image: row.get(6)?,
                local_image: row.get(7)?,
                popularity: row.get(8)?,
            },
            row.get::<_, u32>(9)?,
        ))
    })?;
    let mut cards = Vec::new();
    let mut total = 0;
    for row in found {
        let (card, count) = row?;
        total = count;
        cards.push(card);
    }
    Ok((cards, total))
}

/// The archetypes that hold any printing of a group (`oracle_id` or
/// `id:{catalog_id}`), optionally inside a format (0.x
/// `Cards\ArchetypesController`): held, not retired, whether a list holds
/// it in the maindeck or the sideboard, and how many lists hold it, by
/// that count then name.
pub fn card_archetypes(
    conn: &Connection,
    group: &str,
    format: Option<&str>,
) -> Result<Vec<CardArchetype>, StoreError> {
    let catalog_ids: Vec<i64> = match group.strip_prefix("id:") {
        Some(id) => match id.parse::<i64>() {
            Ok(id) => vec![id],
            Err(_) => return Ok(Vec::new()),
        },
        None => {
            let mut stmt =
                conn.prepare_cached("SELECT catalog_id FROM cards WHERE oracle_id = ?1")?;
            let rows = stmt.query_map([group], |row| row.get(0))?;
            let mut ids = Vec::new();
            for row in rows {
                ids.push(row?);
            }
            ids
        }
    };
    if catalog_ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<String> = catalog_ids.iter().map(i64::to_string).collect();
    let held = held_rows();
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT a.id, a.name, a.format, a.color_identity,
                MAX(ac.sideboard = 0), MAX(ac.sideboard = 1), COUNT(DISTINCT ac.list)
         FROM {held}
           AND ac.catalog_id IN (SELECT value FROM json_each(?1))
         GROUP BY a.id
         ORDER BY 7 DESC, a.name, a.id"
    ))?;
    let format = format.filter(|f| !f.is_empty());
    let rows = stmt.query_map(params![format!("[{}]", ids.join(",")), format], |row| {
        let format: Option<String> = row.get(2)?;
        let colors: Option<String> = row.get(3)?;
        Ok(CardArchetype {
            id: row.get(0)?,
            name: row.get(1)?,
            format_label: format.as_deref().map(archetypes::format_label),
            format,
            color_identity: archetypes::normalize_colors(colors.as_deref()),
            maindeck: row.get(4)?,
            sideboard: row.get(5)?,
            deck_count: row.get(6)?,
        })
    })?;
    let mut found = Vec::new();
    for row in rows {
        found.push(row?);
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{TOKEN, db, insert_manual, number};
    use decks::{DeckCard, signature};

    /// S1-7: one bad entry in a stored id list drops only itself.
    #[test]
    fn an_id_list_is_read_entry_by_entry() {
        assert_eq!(catalog_ids_in("[1, \"q\", 2, null, 3.5, 4]"), vec![1, 2, 4]);
        assert_eq!(catalog_ids_in("[]"), Vec::<i64>::new());
        assert_eq!(catalog_ids_in("not json"), Vec::<i64>::new());
        assert_eq!(catalog_ids_in("{\"a\": 1}"), Vec::<i64>::new());
    }

    fn stub(conn: &mut Connection, refs: &[CardRef]) -> u32 {
        let tx = conn.transaction().expect("tx");
        let added = ensure_stubs(&tx, refs).expect("stubs");
        tx.commit().expect("commit");
        added
    }

    fn scan(conn: &mut Connection) -> u32 {
        let tx = conn.transaction().expect("tx");
        let added = scan_missing(&tx).expect("scan");
        tx.commit().expect("commit");
        added
    }

    fn ids(values: &[i64]) -> Vec<CardRef> {
        values
            .iter()
            .map(|&catalog_id| CardRef {
                catalog_id,
                name: None,
            })
            .collect()
    }

    fn catalog_ids(conn: &Connection) -> Vec<i64> {
        let mut stmt = conn
            .prepare("SELECT catalog_id FROM cards ORDER BY catalog_id")
            .expect("prepare");
        let rows = stmt.query_map([], |row| row.get(0)).expect("query");
        rows.map(|row| row.expect("row")).collect()
    }

    fn name_of(conn: &Connection, catalog_id: i64) -> Option<String> {
        conn.query_row(
            "SELECT name FROM cards WHERE catalog_id = ?1",
            [catalog_id],
            |row| row.get(0),
        )
        .expect("a card row")
    }

    /// A deck version whose signature lists these catalog ids, four each,
    /// with no `cards` rows: the state a sync pull or a 0.x import leaves.
    fn version_with(conn: &Connection, deck: i64, ids: &[u32]) {
        let cards: Vec<DeckCard> = ids
            .iter()
            .map(|&catalog_id| DeckCard {
                catalog_id,
                quantity: 4,
                sideboard: false,
            })
            .collect();
        raw_version(conn, deck, &signature(&cards));
    }

    fn raw_version(conn: &Connection, deck: i64, signature: &str) {
        conn.execute(
            "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source)
             VALUES (?1, 'q-deck-' || ?1, 'Q deck', 'CMODERN', 0, 0, 'file')
             ON CONFLICT (id) DO NOTHING",
            [deck],
        )
        .expect("deck");
        conn.execute(
            "INSERT INTO deck_versions (deck_id, signature, modified_at) VALUES (?1, ?2, 0)",
            params![deck, signature],
        )
        .expect("version");
    }

    fn first_game(conn: &Connection) -> i64 {
        number(
            conn,
            "SELECT id FROM games WHERE game_number = 1 ORDER BY id LIMIT 1",
        )
    }

    // ---- CreateMissingCardsTest ----------------------------------------------

    // Issue #326: an archetype decklist from the API names cards the user
    // may never have met; the ones without a row are looked up.
    #[test]
    fn ids_without_a_card_row_are_the_plausible_ones_never_stored() {
        let (_dir, mut conn) = db("cards-without-rows");
        conn.execute(
            "INSERT INTO cards (catalog_id, name) VALUES (101, 'Qknown')",
            [],
        )
        .expect("known card");
        assert_eq!(stub(&mut conn, &ids(&[102])), 1);
        let missing = without_rows(&conn, &[303, 101, 102, 0, 303, 2_000_000, 202]).expect("read");
        assert_eq!(missing, vec![202, 303]);
    }

    #[test]
    fn creates_a_stub_for_a_plausible_catalog_id() {
        let (_dir, mut conn) = db("cards-stub-plausible");
        assert_eq!(stub(&mut conn, &ids(&[126_519])), 1);
        assert_eq!(catalog_ids(&conn), vec![126_519]);
        assert_eq!(name_of(&conn, 126_519), None);
    }

    #[test]
    fn arena_cards_never_store_a_raw_grp_id() {
        let (_dir, mut conn) = db("cards-stub-arena");
        assert_eq!(
            without_rows(&conn, &[100_091_538]).expect("read"),
            vec![100_091_538]
        );
        assert_eq!(stub(&mut conn, &ids(&[100_091_538, 91_538])), 2);
        assert!(catalog_ids(&conn).contains(&100_091_538));
        assert!(
            without_rows(&conn, &[100_091_538])
                .expect("read")
                .is_empty()
        );
    }

    #[test]
    fn refuses_catalog_ids_far_beyond_anything_mtgo_has_issued() {
        // Real catalog ids are five- to six-digit. Values like these came
        // from misread data, and a stub for one can never resolve.
        let (_dir, mut conn) = db("cards-stub-beyond");
        let refs = ids(&[89_215_000_000, 590_000_000, 9_496_421, 126_519]);
        assert_eq!(stub(&mut conn, &refs), 1);
        assert_eq!(catalog_ids(&conn), vec![126_519]);
    }

    #[test]
    fn refuses_ids_that_are_not_whole_numbers() {
        // 0.x's '', 'abc' and null cannot be an i64; the signature and frame
        // readers drop them before they get here. A negative id still can.
        let (_dir, mut conn) = db("cards-stub-negative");
        assert_eq!(stub(&mut conn, &ids(&[-5, 126_519])), 1);
        assert_eq!(catalog_ids(&conn), vec![126_519]);
    }

    #[test]
    fn creates_no_duplicate_when_run_twice_with_the_same_ids() {
        let (_dir, mut conn) = db("cards-stub-twice");
        assert_eq!(stub(&mut conn, &ids(&[126_519, 126_519])), 1);
        assert_eq!(stub(&mut conn, &ids(&[126_519])), 0);
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM cards WHERE catalog_id = 126519"
            ),
            1
        );
    }

    #[test]
    fn logs_the_ids_it_refused_so_a_source_that_feeds_it_non_card_ids_can_be_found() {
        let (_accepted, refused) = cards::split_plausible(&[89_215_000_000, 590_000_000, 126_519]);
        assert_eq!(
            refusal_warning(&refused).as_deref(),
            Some(
                "CreateMissingCards: refused ids that are not MTGO catalog ids: \
                 89215000000, 590000000"
            )
        );
    }

    #[test]
    fn stays_quiet_when_every_id_is_plausible() {
        let (_accepted, refused) = cards::split_plausible(&[126_519]);
        assert_eq!(refusal_warning(&refused), None);
    }

    // ---- PruneImpossibleCardStubsTest ------------------------------------------

    #[test]
    fn impossible_catalog_ids_never_become_stubs() {
        let (_dir, mut conn) = db("cards-stub-impossible");
        assert_eq!(stub(&mut conn, &ids(&[89_215_000_000, 590_000_000])), 0);
        // Nor from the full scan: a game row, a mulligan, a frame, a pick, a
        // snapshot or a signature holding one.
        insert_manual(&conn, TOKEN);
        let game = first_game(&conn);
        conn.execute(
            "INSERT INTO game_cards (game_id, side, catalog_id) VALUES (?1, 'local', 89215000000)",
            [game],
        )
        .expect("row");
        conn.execute(
            "INSERT INTO game_mulligan_cards (game_id, hand, catalog_id, quantity)
             VALUES (?1, 1, 590000000, 1)",
            [game],
        )
        .expect("mulligan");
        let tx = conn.transaction().expect("tx");
        crate::game_sync::store_timeline(
            &tx,
            game,
            &[crate::game_sync::FrameText {
                time: "10:00:01".to_string(),
                text: "{\"Cards\":[{\"Id\":1,\"CatalogID\":89215000000}]}".to_string(),
            }],
        )
        .expect("timeline");
        tx.commit().expect("commit");
        version_with(&conn, 1, &[590_000_000]);
        assert_eq!(scan(&mut conn), 0);
        assert!(catalog_ids(&conn).is_empty());
    }

    // ---- Names ---------------------------------------------------------------

    #[test]
    fn a_stub_name_never_overwrites_a_known_name() {
        let (_dir, mut conn) = db("cards-stub-names");
        conn.execute(
            "INSERT INTO cards (catalog_id, name) VALUES (101, 'Already Known')",
            [],
        )
        .expect("known card");
        let named = |catalog_id: i64, name: &str| CardRef {
            catalog_id,
            name: Some(name.to_string()),
        };
        // 202 arrives nameless, then with the log's name, then another one.
        assert_eq!(stub(&mut conn, &ids(&[202])), 1);
        assert_eq!(
            stub(&mut conn, &[named(101, "Log Name"), named(202, "Tear")]),
            0
        );
        assert_eq!(stub(&mut conn, &[named(202, "Wear")]), 0);
        assert_eq!(name_of(&conn, 101).as_deref(), Some("Already Known"));
        assert_eq!(name_of(&conn, 202).as_deref(), Some("Tear"));
        // A new id with a name keeps it.
        assert_eq!(stub(&mut conn, &[named(303, "Damnation")]), 1);
        assert_eq!(name_of(&conn, 303).as_deref(), Some("Damnation"));
    }

    #[test]
    fn the_writer_connection_counts_the_stubs_it_added() {
        let (_dir, mut conn) = db("cards-stub-count");
        assert_eq!(take_stubs_added(&conn).expect("count"), 0);
        stub(&mut conn, &ids(&[1, 2, 3]));
        // A rolled back write added nothing.
        {
            let tx = conn.transaction().expect("tx");
            assert_eq!(ensure_stubs(&tx, &ids(&[4])).expect("stub"), 1);
            tx.rollback().expect("rollback");
        }
        stub(&mut conn, &ids(&[3, 5]));
        assert_eq!(take_stubs_added(&conn).expect("count"), 4);
        assert_eq!(take_stubs_added(&conn).expect("reset"), 0);
    }

    // ---- CreateMissingCardsFromLocalDataTest -----------------------------------

    #[test]
    fn creates_a_stub_for_every_catalog_id_in_a_deck_version_signature() {
        let (_dir, mut conn) = db("cards-scan-signature");
        version_with(&conn, 1, &[101, 202, 303]);
        assert!(catalog_ids(&conn).is_empty());
        assert_eq!(scan(&mut conn), 3);
        assert_eq!(catalog_ids(&conn), vec![101, 202, 303]);
    }

    #[test]
    fn creates_stubs_for_cards_only_present_in_a_registered_game_deck_list() {
        // 0.x's `game_player.deck_json` is v1's local `game_cards` rows.
        let (_dir, mut conn) = db("cards-scan-game-deck");
        insert_manual(&conn, TOKEN);
        let game = first_game(&conn);
        conn.execute(
            "INSERT INTO game_cards (game_id, side, catalog_id, main_qty, side_qty)
             VALUES (?1, 'local', 555, 2, 0), (?1, 'local', 666, 0, 1)",
            [game],
        )
        .expect("rows");
        assert_eq!(scan(&mut conn), 2);
        assert_eq!(catalog_ids(&conn), vec![555, 666]);
    }

    #[test]
    fn creates_stubs_for_tokens_that_only_ever_appear_in_a_game_timeline() {
        let (_dir, mut conn) = db("cards-scan-timeline");
        insert_manual(&conn, TOKEN);
        let game = first_game(&conn);
        let tx = conn.transaction().expect("tx");
        crate::game_sync::store_timeline(
            &tx,
            game,
            &[
                crate::game_sync::FrameText {
                    time: "10:00:01".to_string(),
                    text: "{\"Cards\":[{\"Id\":1,\"CatalogID\":777,\"Zone\":\"Battlefield\"}]}"
                        .to_string(),
                },
                crate::game_sync::FrameText {
                    time: "10:00:01".to_string(),
                    text: "not json".to_string(),
                },
                crate::game_sync::FrameText {
                    time: "10:00:01".to_string(),
                    text: "{\"Cards\":[{\"Id\":2}]}".to_string(),
                },
            ],
        )
        .expect("timeline");
        tx.commit().expect("commit");
        assert_eq!(scan(&mut conn), 1);
        assert_eq!(catalog_ids(&conn), vec![777]);
    }

    #[test]
    fn leaves_existing_cards_alone_and_is_idempotent() {
        let (_dir, mut conn) = db("cards-scan-idempotent");
        version_with(&conn, 1, &[101, 202]);
        conn.execute(
            "INSERT INTO cards (catalog_id, name) VALUES (101, 'Already Known')",
            [],
        )
        .expect("known card");
        assert_eq!(scan(&mut conn), 1);
        assert_eq!(scan(&mut conn), 0);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM cards"), 2);
        assert_eq!(name_of(&conn, 101).as_deref(), Some("Already Known"));
    }

    #[test]
    fn survives_a_malformed_signature_rather_than_aborting_the_scan() {
        let (_dir, mut conn) = db("cards-scan-malformed");
        raw_version(&conn, 1, "not-valid-base64-!!!");
        version_with(&conn, 2, &[404]);
        assert_eq!(scan(&mut conn), 1);
        assert_eq!(catalog_ids(&conn), vec![404]);
    }

    #[test]
    fn ignores_signature_entries_whose_catalog_id_is_not_numeric() {
        // Base64 of anything decodes to something; casting the first segment
        // of this one would invent catalog id 3.
        let (_dir, mut conn) = db("cards-scan-not-numeric");
        // base64 of "3f2ab9c1-dead-beef-cafe-000000000000:4:false".
        raw_version(
            &conn,
            1,
            "M2YyYWI5YzEtZGVhZC1iZWVmLWNhZmUtMDAwMDAwMDAwMDAwOjQ6ZmFsc2U=",
        );
        assert_eq!(scan(&mut conn), 0);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM cards"), 0);
    }

    #[test]
    fn the_scan_reads_mulligans_picks_snapshots_and_covers() {
        let (_dir, mut conn) = db("cards-scan-sources");
        insert_manual(&conn, TOKEN);
        let game = first_game(&conn);
        conn.execute_batch(&format!(
            "INSERT INTO game_mulligan_cards (game_id, hand, catalog_id, quantity)
                 VALUES ({game}, 1, 11, 1);
             INSERT INTO drafts (id, draft_token, source, state, imported_from)
                 VALUES (3, 'q-draft', 'imported', 'finished', 3);
             INSERT INTO draft_picks (draft_id, ordinal, pack_number, pick_number,
                 cards_available, picked_catalog_id, picked_card_id, reservations)
                 VALUES (3, 1, 1, 1, '[21,22]', 23, 4000000000, '[]');
             INSERT INTO limited_deck_snapshots (match_id, source, cards, signature, captured_at)
                 SELECT id, 'imported',
                     '[{{\"catalog_id\":31,\"quantity\":1,\"sideboard\":false}}]', '', 0
                 FROM matches WHERE token = '{TOKEN}';
             INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source,
                 cover_catalog_id)
                 VALUES (9, 'q-cover', 'Q deck', 'CMODERN', 0, 0, 'file', 41);
             INSERT INTO match_commanders (match_id, seat, position, catalog_id)
                 SELECT id, 'player', 0, 51 FROM matches WHERE token = '{TOKEN}';"
        ))
        .expect("sources");
        assert_eq!(scan(&mut conn), 7);
        // `picked_card_id` is MTGO's card instance id, not a catalog id.
        assert_eq!(catalog_ids(&conn), vec![11, 21, 22, 23, 31, 41, 51]);
    }

    // ---- population (Plan S task 2) -------------------------------------------

    fn answer(name: Option<&str>) -> mymtgo_api::CardAnswer {
        mymtgo_api::CardAnswer {
            scryfall_id: Some("q-scryfall".to_string()),
            oracle_id: Some("q-oracle".to_string()),
            name: name.map(str::to_string),
            image: Some("https://cards.scryfall.io/q.jpg".to_string()),
            ..mymtgo_api::CardAnswer::default()
        }
    }

    fn resolved(mtgo_id: u32, card: mymtgo_api::CardAnswer, is_token: bool) -> Resolution {
        Resolution {
            mtgo_id,
            card,
            is_token,
        }
    }

    fn apply(conn: &mut Connection, update: &CardsUpdate) -> ResolveOutcome {
        let tx = conn.transaction().expect("tx");
        let outcome = apply_resolutions(&tx, update).expect("apply");
        tx.commit().expect("commit");
        outcome
    }

    fn row(conn: &Connection, catalog_id: i64) -> Vec<Option<String>> {
        conn.query_row(
            "SELECT scryfall_id, oracle_id, name, type, sub_type, rarity, color_identity,
                    colors, CAST(cmc AS TEXT), mana_cost, set_name, set_code, art_crop, image
             FROM cards WHERE catalog_id = ?1",
            [catalog_id],
            |row| (0..14).map(|index| row.get(index)).collect(),
        )
        .expect("a card row")
    }

    #[test]
    fn unresolved_rows_are_those_without_a_scryfall_id_even_when_named() {
        let (_dir, conn) = db("cards-unresolved");
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, name) VALUES (3, 'Qlogged');
             INSERT INTO cards (catalog_id) VALUES (1);
             INSERT INTO cards (catalog_id, name, rarity) VALUES (2, 'Qcat', 'token');
             INSERT INTO cards (catalog_id, scryfall_id, name) VALUES (4, 'q-done', 'Qdone');",
        )
        .expect("rows");
        let card = |mtgo_id, name: Option<&str>, is_token| {
            UnresolvedCard::new(mtgo_id, name.map(str::to_string), is_token)
        };
        assert_eq!(
            unresolved(&conn, None).expect("unresolved"),
            vec![
                card(1, None, false),
                card(2, Some("Qcat"), true),
                card(3, Some("Qlogged"), false),
            ]
        );
        assert_eq!(nameless(&conn).expect("nameless"), vec![1]);
    }

    // #501: an id the API answered without is passed over until the
    // recheck window has passed; `None` asks about every row.
    #[test]
    fn an_id_the_api_does_not_know_is_passed_over_for_a_while() {
        let (_dir, mut conn) = db("cards-api-unknown");
        conn.execute_batch("INSERT INTO cards (catalog_id) VALUES (1), (2), (3);")
            .expect("rows");
        let now = 1_800_000_000_000;
        apply(
            &mut conn,
            &CardsUpdate {
                tokens: Vec::new(),
                resolutions: Resolutions {
                    resolved: vec![resolved(3, answer(None), false)],
                    unknown: vec![1, 3],
                },
                unknown_at_ms: now,
            },
        );
        let ids = |conn: &Connection, since: Option<i64>| -> Vec<u32> {
            unresolved(conn, since)
                .expect("unresolved")
                .into_iter()
                .map(|card| card.mtgo_id)
                .collect()
        };
        assert_eq!(ids(&conn, Some(now - UNKNOWN_RECHECK_MS)), vec![2]);
        assert_eq!(ids(&conn, Some(now)), vec![1, 2], "the window has passed");
        assert_eq!(ids(&conn, None), vec![1, 2]);
        let marked: Vec<Option<i64>> = conn
            .prepare("SELECT api_unknown_at FROM cards ORDER BY catalog_id")
            .expect("query")
            .query_map([], |row| row.get(0))
            .expect("rows")
            .map(|row| row.expect("row"))
            .collect();
        assert_eq!(
            marked,
            vec![Some(now), None, None],
            "a resolved row is not marked"
        );
        assert_eq!(UNKNOWN_RECHECK_MS, 7 * 24 * 3_600_000);
    }

    #[test]
    fn the_unknown_checked_version_round_trips() {
        let (_dir, conn) = db("cards-unknown-version");
        assert_eq!(unknown_checked_version(&conn).expect("read"), None);
        let (key, value) = unknown_checked_version_row("1.0.4");
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)",
            [&key, &value],
        )
        .expect("save");
        assert_eq!(
            unknown_checked_version(&conn).expect("read"),
            Some("1.0.4".to_string())
        );
    }

    #[test]
    fn the_mana_cost_refresh_is_resolved_spells_stored_without_a_cost() {
        let (_dir, conn) = db("cards-mana-cost-refresh");
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, scryfall_id, name, type) VALUES
                 (5, 'q-5', 'Qspell', 'Instant'),
                 (6, 'q-6', 'Qmountain', 'Basic Land'),
                 (7, 'q-7', 'Qdryad', 'Land Creature'),
                 (8, 'q-8', 'Qnotype', NULL);
             INSERT INTO cards (catalog_id, scryfall_id, name, type, mana_cost) VALUES
                 (9, 'q-9', 'Qcosted', 'Instant', '{R}'),
                 (10, 'q-10', 'Qempty', 'Artifact', '');
             INSERT INTO cards (catalog_id, scryfall_id, name, type, rarity) VALUES
                 (11, 'q-11', 'Qclue', 'Token Artifact', 'token');
             INSERT INTO cards (catalog_id, name, type) VALUES (12, 'Qstub', 'Instant');",
        )
        .expect("rows");
        // An Endstep row resolved without a cost is never asked again.
        conn.execute(
            "INSERT INTO cards (catalog_id, scryfall_id, name, type) VALUES (?1, 'q-e', 'Qendstep', 'Instant')",
            [i64::from(cards::ENDSTEP_ID_OFFSET) + 5],
        )
        .expect("endstep row");
        let card =
            |mtgo_id, name: &str| UnresolvedCard::new(mtgo_id, Some(name.to_string()), false);
        assert_eq!(
            mana_cost_refresh(&conn).expect("refresh"),
            vec![card(5, "Qspell"), card(8, "Qnotype")]
        );
    }

    #[test]
    fn the_mana_cost_refresh_asks_about_an_arena_row_by_its_catalog_id() {
        let (_dir, conn) = db("cards-mana-cost-refresh-arena");
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, scryfall_id, name, type) VALUES
                 (5, 'q-5', 'Qspell', 'Instant');",
        )
        .expect("mtgo row");
        conn.execute(
            "INSERT INTO cards (catalog_id, scryfall_id, name, type) VALUES (?1, 'q-a', 'Qarena', 'Instant')",
            [i64::from(cards::ARENA_ID_OFFSET) + 5],
        )
        .expect("arena row");
        let asked: Vec<u32> = mana_cost_refresh(&conn)
            .expect("refresh")
            .into_iter()
            .map(|card| card.mtgo_id)
            .collect();
        // As `unresolved` does: the stored id, never the raw grpId (which
        // would be another card's MTGO id).
        assert_eq!(asked, vec![5, cards::ARENA_ID_OFFSET + 5]);
    }

    #[test]
    fn an_mtgo_answer_never_fills_an_arena_row_by_its_raw_grp_id() {
        let (_dir, mut conn) = db("cards-answer-raw-grp");
        let arena_id = i64::from(cards::ARENA_ID_OFFSET) + 91_538;
        conn.execute("INSERT INTO cards (catalog_id) VALUES (?1)", [arena_id])
            .expect("arena row");
        let outcome = apply(
            &mut conn,
            &CardsUpdate {
                tokens: Vec::new(),
                resolutions: Resolutions {
                    resolved: vec![resolved(91_538, answer(Some("Qmtgo")), false)],
                    unknown: Vec::new(),
                },
                unknown_at_ms: 0,
            },
        );
        assert_eq!(outcome.updated, 0, "no row has the raw grpId");
        assert_eq!(row(&conn, arena_id)[0], None, "the Arena row is untouched");
    }

    #[test]
    fn the_mana_cost_refresh_marker_is_a_setting() {
        let (_dir, conn) = db("cards-mana-cost-marker");
        assert!(!mana_costs_filled(&conn).expect("read"));
        let (key, value) = mana_costs_filled_row();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)",
            [key, value],
        )
        .expect("marker");
        assert!(mana_costs_filled(&conn).expect("read"));

        assert_eq!(mana_cost_attempts(&conn).expect("read"), 0);
        let (key, value) = mana_cost_attempts_row(3);
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)",
            [key, value],
        )
        .expect("attempts");
        assert_eq!(mana_cost_attempts(&conn).expect("read"), 3);
    }

    // 0.x IndexTest: "reports the number of cards missing data".
    #[test]
    fn reports_the_number_of_cards_missing_data() {
        let (_dir, conn) = db("cards-counts");
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, name, scryfall_id, image)
                 VALUES (1, 'Complete', 'sf-1', 'http://img');
             INSERT INTO cards (catalog_id) VALUES (2), (3);
             INSERT INTO cards (catalog_id, name) VALUES (4, 'Qnamed');",
        )
        .expect("rows");
        assert_eq!(
            card_data_counts(&conn).expect("counts"),
            CardDataCounts {
                total: 4,
                incomplete: 2,
                missing: 3,
            }
        );
    }

    #[test]
    fn an_answer_is_applied_with_0x_update_card_rules() {
        let (_dir, mut conn) = db("cards-apply-rules");
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, name, type, sub_type, rarity, color_identity,
                 colors, cmc, mana_cost, set_name, set_code, art_crop)
             VALUES (7, 'Qold', 'Qtype', 'Qsub', 'common', 'G', 'G', 3, '{G}', 'Qset', 'QS', 'q-art')",
        )
        .expect("row");
        let mut card = answer(None);
        card.color_identity = Some("W,C".to_string());
        let outcome = apply(
            &mut conn,
            &CardsUpdate {
                tokens: Vec::new(),
                resolutions: Resolutions {
                    resolved: vec![resolved(7, card, false)],
                    unknown: Vec::new(),
                },
                unknown_at_ms: 0,
            },
        );
        let text = |value: &str| Some(value.to_string());
        // Name, type, sub type and rarity stay when the answer has none;
        // colors, cmc, mana cost, set and art crop follow the answer, null
        // included.
        assert_eq!(
            row(&conn, 7),
            vec![
                text("q-scryfall"),
                text("q-oracle"),
                text("Qold"),
                text("Qtype"),
                text("Qsub"),
                text("common"),
                text("W,C"),
                None,
                None,
                None,
                None,
                None,
                None,
                text("https://cards.scryfall.io/q.jpg"),
            ]
        );
        assert_eq!(outcome.updated, 1);
        assert_eq!(outcome.changed, vec![7], "the oracle id changed");
    }

    #[test]
    fn a_non_token_without_an_answered_identity_loses_its_identity() {
        let (_dir, mut conn) = db("cards-apply-identity");
        conn.execute(
            "INSERT INTO cards (catalog_id, color_identity) VALUES (8, 'R')",
            [],
        )
        .expect("row");
        apply(
            &mut conn,
            &CardsUpdate {
                tokens: Vec::new(),
                resolutions: Resolutions {
                    resolved: vec![resolved(8, answer(Some("Qbolt")), false)],
                    unknown: Vec::new(),
                },
                unknown_at_ms: 0,
            },
        );
        assert_eq!(row(&conn, 8)[6], None);
        assert_eq!(row(&conn, 8)[2].as_deref(), Some("Qbolt"));
    }

    #[test]
    fn a_token_keeps_its_xml_identity_when_the_answer_has_none() {
        let (_dir, mut conn) = db("cards-apply-token");
        conn.execute("INSERT INTO cards (catalog_id) VALUES (1006)", [])
            .expect("stub");
        let token = TokenCard {
            catalog_id: 1006,
            name: "Food".to_string(),
            card_type: "Token Enchantment Creature".to_string(),
            sub_type: Some("Goblin".to_string()),
            color_identity: "W,U".to_string(),
        };
        let outcome = apply(
            &mut conn,
            &CardsUpdate {
                tokens: vec![token],
                resolutions: Resolutions {
                    resolved: vec![resolved(1006, answer(Some("Food")), true)],
                    unknown: Vec::new(),
                },
                unknown_at_ms: 0,
            },
        );
        let stored = row(&conn, 1006);
        assert_eq!(stored[0].as_deref(), Some("q-scryfall"));
        assert_eq!(stored[2].as_deref(), Some("Food"));
        assert_eq!(stored[3].as_deref(), Some("Token Enchantment Creature"));
        assert_eq!(stored[5].as_deref(), Some("token"));
        assert_eq!(stored[6].as_deref(), Some("W,U"));
        assert_eq!(outcome.updated, 2);
        assert_eq!(outcome.changed, vec![1006]);
    }

    #[test]
    fn a_token_only_fills_a_row_without_a_name() {
        let (_dir, mut conn) = db("cards-apply-token-named");
        conn.execute(
            "INSERT INTO cards (catalog_id, name) VALUES (1001, 'Qlogged')",
            [],
        )
        .expect("row");
        let token = TokenCard {
            catalog_id: 1001,
            name: "Goblin".to_string(),
            card_type: "Token Creature".to_string(),
            sub_type: None,
            color_identity: "R".to_string(),
        };
        let outcome = apply(
            &mut conn,
            &CardsUpdate {
                tokens: vec![token],
                resolutions: Resolutions::default(),
                unknown_at_ms: 0,
            },
        );
        assert_eq!(name_of(&conn, 1001).as_deref(), Some("Qlogged"));
        assert_eq!(outcome, ResolveOutcome::default());
    }

    /// A commander card that resolves sends its detected Brawl matches
    /// back to detection, so the seat it left empty is filled.
    #[test]
    fn a_resolved_commander_card_detects_its_matches_again() {
        let (_dir, mut conn) = db("cards-apply-commander");
        insert_manual(&conn, TOKEN);
        let match_id = number(&conn, "SELECT id FROM matches");
        conn.execute_batch(&format!(
            "INSERT INTO cards (catalog_id, api_unknown_at) VALUES (5002, 7), (5003, 7);
             INSERT INTO match_commanders VALUES ({match_id}, 'opponent', 0, 5002);
             UPDATE matches SET archetypes_detected_at = 9;"
        ))
        .expect("detected brawl match");
        let detected = |conn: &Connection| -> Option<i64> {
            conn.query_row("SELECT archetypes_detected_at FROM matches", [], |row| {
                row.get(0)
            })
            .expect("marker")
        };
        // Another card resolving leaves the match alone.
        apply(
            &mut conn,
            &CardsUpdate {
                tokens: Vec::new(),
                resolutions: Resolutions {
                    resolved: vec![resolved(5003, answer(Some("Qother")), false)],
                    unknown: Vec::new(),
                },
                unknown_at_ms: 0,
            },
        );
        assert_eq!(detected(&conn), Some(9));
        apply(
            &mut conn,
            &CardsUpdate {
                tokens: Vec::new(),
                resolutions: Resolutions {
                    resolved: vec![resolved(5002, answer(Some("Qomnath")), false)],
                    unknown: Vec::new(),
                },
                unknown_at_ms: 0,
            },
        );
        assert_eq!(detected(&conn), None);
    }

    #[test]
    fn an_answer_that_changes_nothing_the_game_sync_reads_is_not_changed() {
        let (_dir, mut conn) = db("cards-apply-unchanged");
        conn.execute(
            "INSERT INTO cards (catalog_id, name, oracle_id) VALUES (9, 'Qsame', 'q-oracle')",
            [],
        )
        .expect("row");
        let outcome = apply(
            &mut conn,
            &CardsUpdate {
                tokens: Vec::new(),
                resolutions: Resolutions {
                    // An answer for a row that is gone is skipped.
                    resolved: vec![
                        resolved(9, answer(Some("Qsame")), false),
                        resolved(10, answer(None), false),
                    ],
                    unknown: Vec::new(),
                },
                unknown_at_ms: 0,
            },
        );
        assert_eq!(outcome.updated, 1);
        assert!(outcome.changed.is_empty());
    }

    #[test]
    fn the_log_matches_using_a_card_are_found_by_catalog_id() {
        let (_dir, conn) = db("cards-matches-using");
        insert_manual(&conn, TOKEN);
        let game = first_game(&conn);
        conn.execute(
            "INSERT INTO game_cards (game_id, side, catalog_id) VALUES (?1, 'opponent', 77)",
            [game],
        )
        .expect("row");
        assert!(
            log_matches_using(&conn, &[77]).expect("manual").is_empty(),
            "only log matches are synced"
        );
        conn.execute("UPDATE matches SET source = 'log'", [])
            .expect("log");
        assert_eq!(
            log_matches_using(&conn, &[77, 78]).expect("log"),
            vec![TOKEN.to_string()]
        );
        assert!(log_matches_using(&conn, &[78]).expect("none").is_empty());
    }

    fn local(conn: &Connection, catalog_id: i64) -> (Option<String>, Option<String>) {
        conn.query_row(
            "SELECT local_image, local_art_crop FROM cards WHERE catalog_id = ?1",
            [catalog_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("row")
    }

    fn image_rows(conn: &Connection) {
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, image, art_crop, local_image, local_art_crop) VALUES
                 (10, 'https://q/10.jpg', 'https://q/10a.jpg', NULL, NULL),
                 (11, 'https://q/11.jpg', 'https://q/11a.jpg', '11.jpg', '11_art.jpg'),
                 (12, 'https://q/12.jpg', 'https://q/12a.jpg', '12.jpg', NULL),
                 (13, NULL, NULL, NULL, NULL),
                 (14, '', '', NULL, NULL),
                 (15, 'https://q/15.jpg', NULL, NULL, NULL),
                 (16, NULL, 'https://q/16a.jpg', NULL, NULL);",
        )
        .expect("rows");
    }

    #[test]
    fn the_image_queue_is_rows_with_a_url_and_no_local_file() {
        let (_dir, conn) = db("cards-image-queue");
        image_rows(&conn);
        let jobs = images_to_download(&conn, 0, 50).expect("jobs");
        let ids: Vec<u32> = jobs.iter().map(|job| job.catalog_id).collect();
        assert_eq!(ids, vec![10, 12, 15, 16]);
        assert_eq!(
            jobs[1],
            ImageJob {
                catalog_id: 12,
                image: Some("https://q/12.jpg".to_string()),
                art_crop: Some("https://q/12a.jpg".to_string()),
                has_local_image: true,
                has_local_art: false,
            }
        );
        let ids = |after, limit| -> Vec<u32> {
            images_to_download(&conn, after, limit)
                .expect("jobs")
                .iter()
                .map(|job| job.catalog_id)
                .collect()
        };
        assert_eq!(ids(10, 2), vec![12, 15], "pages by catalog id");
        assert_eq!(ids(16, 50), Vec::<u32>::new());
    }

    #[test]
    fn stored_images_set_only_the_given_columns_and_clearing_nulls_both() {
        let (_dir, mut conn) = db("cards-image-store");
        image_rows(&conn);
        let tx = conn.transaction().expect("tx");
        assert_eq!(
            store_images(&tx, 10, Some("10.jpg"), None).expect("store"),
            1
        );
        assert_eq!(
            store_images(&tx, 12, None, Some("12_art.jpg")).expect("store"),
            1
        );
        assert_eq!(
            store_images(&tx, 999, Some("999.jpg"), None).expect("store"),
            0
        );
        tx.commit().expect("commit");
        assert_eq!(local(&conn, 10), (Some("10.jpg".to_string()), None));
        assert_eq!(
            local(&conn, 12),
            (Some("12.jpg".to_string()), Some("12_art.jpg".to_string()))
        );
    }

    #[test]
    fn rows_whose_files_are_gone_are_queued_again() {
        let (_dir, mut conn) = db("cards-image-present");
        image_rows(&conn);
        let present: BTreeSet<String> =
            ["11.jpg", "12.jpg"].iter().map(|n| n.to_string()).collect();
        let tx = conn.transaction().expect("tx");
        assert_eq!(keep_present_images(&tx, &present).expect("keep"), 1);
        tx.commit().expect("commit");
        assert_eq!(local(&conn, 11), (Some("11.jpg".to_string()), None));
        assert_eq!(local(&conn, 12), (Some("12.jpg".to_string()), None));
        let queued: Vec<u32> = images_to_download(&conn, 0, 50)
            .expect("jobs")
            .iter()
            .map(|job| job.catalog_id)
            .collect();
        assert_eq!(
            queued,
            vec![10, 11, 12, 15, 16],
            "11's art is fetched again"
        );
    }

    // ---- Cards page (0.x IndexTest, ArchetypesTest) -------------------------

    fn card(conn: &Connection, id: i64, name: &str, oracle: Option<&str>, type_line: Option<&str>) {
        conn.execute(
            "INSERT INTO cards (catalog_id, name, oracle_id, type, set_code, scryfall_id, image)
             VALUES (?1, ?2, ?3, ?4, 'ABC', 'sf', 'http://img')",
            params![id, name, oracle, type_line],
        )
        .expect("card");
    }

    fn names(page: &CardsPage) -> Vec<String> {
        page.cards
            .iter()
            .map(|c| c.name.clone().unwrap_or_default())
            .collect()
    }

    fn with(f: impl FnOnce(&mut CardsPageQuery)) -> CardsPageQuery {
        let mut query = CardsPageQuery::new();
        f(&mut query);
        query
    }

    #[test]
    fn the_cards_page_lists_only_the_platforms_cards() {
        let (_dir, conn) = db("cards-page-platform");
        let arena_id = i64::from(cards::ARENA_ID_OFFSET) + 7;
        card(&conn, 1, "Lightning Bolt", None, None);
        card(&conn, arena_id, "Arena Bolt", None, None);
        let mtgo = cards_page(&conn, &CardsPageQuery::new()).expect("mtgo");
        assert_eq!(names(&mtgo), vec!["Lightning Bolt"]);
        assert_eq!(mtgo.total, 1);
        let arena = cards_page(&conn, &with(|q| q.platform = Platform::Arena)).expect("arena");
        assert_eq!(names(&arena), vec!["Arena Bolt"]);
        assert_eq!(arena.total, 1);
    }

    #[test]
    fn catalog_ranges_do_not_overlap() {
        let mtgo = catalog_range(Platform::Mtgo);
        let arena = catalog_range(Platform::Arena);
        let endstep = catalog_range(Platform::Endstep);
        // Each `[low, high)` ends where the next starts.
        assert_eq!(mtgo.1, arena.0);
        assert_eq!(arena.1, endstep.0);
        assert_eq!(arena.1, i64::from(cards::ENDSTEP_ID_OFFSET));
        for (low, high) in [mtgo, arena, endstep] {
            assert!(low < high, "{low}..{high} is not empty");
        }
        let endstep_id = i64::from(cards::endstep_catalog_id(1));
        assert!(endstep_id >= endstep.0 && endstep_id < endstep.1);
        assert!(endstep_id >= arena.1, "an Endstep id is not Arena's");
    }

    #[test]
    fn the_cards_page_lists_endstep_cards_on_their_own() {
        let (_dir, conn) = db("cards-page-endstep");
        let endstep_id = i64::from(cards::endstep_catalog_id(3));
        card(
            &conn,
            i64::from(cards::ARENA_ID_OFFSET) + 7,
            "Arena Bolt",
            None,
            None,
        );
        card(&conn, endstep_id, "Endstep Bolt", None, None);
        let arena = cards_page(&conn, &with(|q| q.platform = Platform::Arena)).expect("arena");
        assert_eq!(names(&arena), vec!["Arena Bolt"]);
        let endstep =
            cards_page(&conn, &with(|q| q.platform = Platform::Endstep)).expect("endstep");
        assert_eq!(names(&endstep), vec!["Endstep Bolt"]);
    }

    /// Endstep stubs are asked about by name and printing (`endstep_cards`);
    /// the other rows carry an empty printing.
    #[test]
    fn unresolved_lists_endstep_printings() {
        let (_dir, conn) = db("cards-unresolved-endstep");
        let printed = cards::endstep_catalog_id(1);
        let plain = cards::endstep_catalog_id(2);
        conn.execute_batch(&format!(
            "INSERT INTO endstep_cards (id, name, set_code, collector_number, first_seen_at)
                 VALUES (1, 'Qend', 'mh3', '12', 0), (2, 'Qopp', '', '', 0);
             INSERT INTO cards (catalog_id, name) VALUES
                 (5, 'Qmtgo'), ({printed}, 'Qend'), ({plain}, 'Qopp');"
        ))
        .expect("stubs");
        assert_eq!(
            unresolved(&conn, None).expect("unresolved"),
            vec![
                UnresolvedCard::new(5, Some("Qmtgo".to_string()), false),
                UnresolvedCard::new(printed, Some("Qend".to_string()), false)
                    .with_printing("mh3".to_string(), "12".to_string()),
                UnresolvedCard::new(plain, Some("Qopp".to_string()), false),
            ]
        );
    }

    /// An Endstep card resolving changes the names detection reads, so the
    /// Endstep matches holding it are detected again; MTGO matches are
    /// synced again instead (`log_matches_using`) and Arena ones keep theirs.
    #[test]
    fn resolving_an_endstep_card_requeues_detection() {
        let (_dir, mut conn) = db("cards-endstep-requeue");
        let arena = cards::ARENA_ID_OFFSET + 7;
        let endstep = cards::endstep_catalog_id(1);
        for (token, catalog_id) in [("q-mtgo", 77), ("q-arena", arena), ("q-end", endstep)] {
            insert_manual(&conn, token);
            conn.execute(
                "INSERT INTO game_cards (game_id, side, catalog_id)
                 SELECT g.id, 'opponent', ?2 FROM games g JOIN matches m ON m.id = g.match_id
                 WHERE m.token = ?1 AND g.game_number = 1",
                params![token, catalog_id],
            )
            .expect("game card");
            conn.execute("INSERT INTO cards (catalog_id) VALUES (?1)", [catalog_id])
                .expect("stub");
        }
        let stamp_all = |conn: &Connection| {
            conn.execute("UPDATE matches SET archetypes_detected_at = 9", [])
                .expect("stamp");
        };
        let stamps = |conn: &Connection| -> Vec<(String, Option<i64>)> {
            conn.prepare("SELECT token, archetypes_detected_at FROM matches ORDER BY token")
                .expect("query")
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .expect("rows")
                .map(|row| row.expect("row"))
                .collect()
        };
        let update = CardsUpdate {
            tokens: Vec::new(),
            resolutions: Resolutions {
                resolved: [77, arena, endstep]
                    .into_iter()
                    .map(|id| resolved(id, answer(Some("Qcard")), false))
                    .collect(),
                unknown: Vec::new(),
            },
            unknown_at_ms: 0,
        };
        stamp_all(&conn);
        apply(&mut conn, &update);
        assert_eq!(
            stamps(&conn),
            vec![
                ("q-arena".to_string(), Some(9)),
                ("q-end".to_string(), None),
                ("q-mtgo".to_string(), Some(9)),
            ]
        );
        // Answered again once resolved: nothing new, nothing requeued.
        stamp_all(&conn);
        apply(&mut conn, &update);
        assert!(stamps(&conn).iter().all(|(_, stamp)| *stamp == Some(9)));
    }

    #[test]
    fn missing_counts_follow_the_platform() {
        let (_dir, conn) = db("cards-page-platform-counts");
        let arena_id = i64::from(cards::ARENA_ID_OFFSET) + 7;
        card(&conn, 1, "Lightning Bolt", None, None);
        card(&conn, 2, "Counterspell", None, None);
        card(&conn, arena_id, "Arena Bolt", None, None);
        conn.execute(
            "INSERT INTO cards (catalog_id) VALUES (?1)",
            params![arena_id + 1],
        )
        .expect("stub");
        let mtgo = cards_page(&conn, &CardsPageQuery::new()).expect("mtgo");
        assert_eq!((mtgo.missing_count, mtgo.total_count), (0, 2));
        let arena = cards_page(&conn, &with(|q| q.platform = Platform::Arena)).expect("arena");
        assert_eq!((arena.missing_count, arena.total_count), (1, 2));
    }

    #[test]
    fn renders_the_cards_index_page() {
        let (_dir, conn) = db("cards-page-renders");
        card(&conn, 1, "Lightning Bolt", None, None);
        let page = cards_page(&conn, &CardsPageQuery::new()).expect("page");
        assert_eq!(page.cards.len(), 1);
        assert_eq!((page.page, page.last_page, page.total), (1, 1, 1));
        assert_eq!((page.missing_count, page.total_count), (0, 1));
        assert!(page.formats.is_empty());
        assert_eq!(page.cards[0].popularity, 0);
    }

    /// 0.x IndexTest "groups printings by oracle_id and counts distinct
    /// archetypes across printings": one list holding both printings counts
    /// once; a second archetype holding only the new one makes two.
    #[test]
    fn groups_printings_by_oracle_id_showing_the_highest_catalog_id() {
        let (_dir, conn) = db("cards-page-group");
        card(&conn, 10, "Brainstorm", Some("o-bs"), None);
        card(&conn, 20, "Brainstorm", Some("o-bs"), None);
        let both = held(&conn, "Qtempo", "modern");
        list_card(&conn, both, 1, 10, false);
        list_card(&conn, both, 1, 20, false);
        let other = held(&conn, "Qcontrol", "modern");
        list_card(&conn, other, 1, 20, false);
        let page = cards_page(&conn, &CardsPageQuery::new()).expect("page");
        assert_eq!(page.cards.len(), 1);
        assert_eq!(page.cards[0].catalog_id, 20);
        assert_eq!(page.cards[0].group_key(), "o-bs");
        assert_eq!(page.cards[0].popularity, 2);
    }

    /// Without grouping each printing counts its own archetypes.
    #[test]
    fn ungrouped_printings_count_their_own_archetypes() {
        let (_dir, conn) = db("cards-page-ungrouped-popularity");
        card(&conn, 10, "Brainstorm", Some("o-bs"), None);
        card(&conn, 20, "Brainstorm", Some("o-bs"), None);
        let one = held(&conn, "Qtempo", "modern");
        list_card(&conn, one, 1, 10, false);
        list_card(&conn, one, 1, 20, false);
        let two = held(&conn, "Qcontrol", "modern");
        list_card(&conn, two, 1, 20, false);
        let page = cards_page(&conn, &with(|q| q.group_printings = false)).expect("page");
        let found: Vec<(i64, u32)> = page
            .cards
            .iter()
            .map(|c| (c.catalog_id, c.popularity))
            .collect();
        assert_eq!(found, vec![(20, 2), (10, 1)]);
        let legacy = cards_page(
            &conn,
            &with(|q| {
                q.group_printings = false;
                q.format = Some("legacy".into());
            }),
        )
        .expect("legacy");
        assert!(legacy.cards.is_empty(), "no legacy archetype holds them");
    }

    // ---- Popularity and formats (0.x IndexTest cases Plan S deferred) ----

    /// An archetype v1 holds (archetypes spec section 12): a downloaded one
    /// with its list 1, or a manual one with its variants.
    fn held(conn: &Connection, name: &str, format: &str) -> i64 {
        conn.query_row(
            "INSERT INTO archetypes (uuid, name, format, color_identity, source, created_at,
                 updated_at)
             VALUES ('q-' || ?1, ?1, ?2, 'r,u', 'manual', 0, 0) RETURNING id",
            params![name, format],
            |row| row.get(0),
        )
        .expect("archetype")
    }

    fn list_card(
        conn: &Connection,
        archetype_id: i64,
        list: i64,
        catalog_id: i64,
        sideboard: bool,
    ) {
        conn.execute(
            "INSERT INTO archetype_cards (archetype_id, list, catalog_id, oracle_id, quantity,
                 sideboard)
             VALUES (?1, ?2, ?3, (SELECT oracle_id FROM cards WHERE catalog_id = ?3), 4, ?4)",
            params![archetype_id, list, catalog_id, sideboard],
        )
        .expect("list card");
    }

    #[test]
    fn lists_the_available_formats_from_archetypes() {
        let (_dir, conn) = db("cards-page-formats");
        card(&conn, 1, "Bolt", Some("o-b"), None);
        list_card(&conn, held(&conn, "Qburn", "modern"), 1, 1, false);
        list_card(&conn, held(&conn, "Qsligh", "legacy"), 1, 1, false);
        let retired = held(&conn, "Qold", "pauper");
        conn.execute(
            "UPDATE archetypes SET retired_at = 5 WHERE id = ?1",
            [retired],
        )
        .expect("retire");
        let page = cards_page(&conn, &CardsPageQuery::new()).expect("page");
        assert_eq!(
            page.formats,
            vec![
                ("legacy".to_string(), "Legacy".to_string()),
                ("modern".to_string(), "Modern".to_string()),
            ]
        );
    }

    #[test]
    fn filters_cards_by_the_format_of_archetypes_they_appear_in() {
        let (_dir, conn) = db("cards-page-format-filter");
        card(&conn, 1, "Modern Card", Some("o-m"), None);
        card(&conn, 2, "Legacy Card", Some("o-l"), None);
        card(&conn, 3, "Unplayed", Some("o-u"), None);
        list_card(&conn, held(&conn, "Qmodern", "modern"), 1, 1, false);
        list_card(&conn, held(&conn, "Qlegacy", "legacy"), 1, 2, false);
        let page = cards_page(&conn, &with(|q| q.format = Some("modern".into()))).expect("page");
        assert_eq!(names(&page), vec!["Modern Card"]);
        assert_eq!((page.total, page.last_page), (1, 1));
        let all = cards_page(&conn, &CardsPageQuery::new()).expect("all");
        assert_eq!(all.total, 3, "every card without a format");
    }

    #[test]
    fn scopes_popularity_to_the_selected_format() {
        let (_dir, conn) = db("cards-page-format-popularity");
        card(&conn, 1, "Splashable", Some("o-s"), None);
        for name in ["Qa", "Qb", "Qc"] {
            list_card(&conn, held(&conn, name, "modern"), 1, 1, false);
        }
        list_card(&conn, held(&conn, "Qd", "legacy"), 1, 1, false);
        let modern = cards_page(&conn, &with(|q| q.format = Some("modern".into()))).expect("m");
        assert_eq!(modern.cards[0].popularity, 3);
        let all = cards_page(&conn, &CardsPageQuery::new()).expect("all");
        assert_eq!(all.cards[0].popularity, 4);
    }

    /// Ordered by popularity before paging: the most popular card leads
    /// page one even when its name sorts last.
    #[test]
    fn orders_cards_by_popularity_descending_by_default() {
        let (_dir, conn) = db("cards-page-popularity-order");
        card(&conn, 1, "Popular", Some("o-popular"), None);
        card(&conn, 2, "Rare", Some("o-rare"), None);
        for name in ["Qa", "Qb", "Qc"] {
            list_card(&conn, held(&conn, name, "modern"), 1, 1, false);
        }
        list_card(&conn, held(&conn, "Qd", "modern"), 1, 2, false);
        for id in 3..=82 {
            card(
                &conn,
                id,
                &format!("Card {id:03}"),
                Some(&format!("o-{id}")),
                None,
            );
        }
        card(&conn, 90, "Zzz Staple", Some("o-zzz"), None);
        let staple = held(&conn, "Qe", "modern");
        for (index, name) in ["Qf", "Qg", "Qh", "Qi"].iter().enumerate() {
            let id = held(&conn, name, "modern");
            list_card(&conn, id, 1, 90, index % 2 == 0);
        }
        list_card(&conn, staple, 1, 90, false);

        let page = cards_page(&conn, &CardsPageQuery::new()).expect("page");
        let top: Vec<(String, u32)> = page
            .cards
            .iter()
            .take(3)
            .map(|c| (c.name.clone().unwrap_or_default(), c.popularity))
            .collect();
        assert_eq!(
            top,
            vec![
                ("Zzz Staple".to_string(), 5),
                ("Popular".to_string(), 3),
                ("Rare".to_string(), 1),
            ]
        );
        assert_eq!(page.cards[3].name.as_deref(), Some("Card 003"));
        assert_eq!((page.total, page.last_page), (83, 2));
        // #300 F13: the page and its total come from one query; a page past
        // the last reads the last one.
        for asked in [2, 9] {
            let mut query = CardsPageQuery::new();
            query.page = asked;
            let later = cards_page(&conn, &query).expect("page");
            assert_eq!(
                (later.page, later.last_page, later.total, later.cards.len()),
                (2, 2, 83, 3),
                "page {asked}"
            );
        }
    }

    #[test]
    fn counts_popularity_as_the_number_of_distinct_archetypes_the_card_is_used_in() {
        let (_dir, conn) = db("cards-page-popularity-count");
        card(&conn, 1, "Counterspell", Some("o-cs"), None);
        for name in ["Qa", "Qb", "Qc", "Qd"] {
            list_card(&conn, held(&conn, name, "pauper"), 1, 1, false);
        }
        let page = cards_page(&conn, &CardsPageQuery::new()).expect("page");
        assert_eq!(page.cards[0].popularity, 4);
    }

    /// Three variants of one manual archetype count once; retired
    /// archetypes are not held (archetypes spec section 12).
    #[test]
    fn counts_popularity_by_archetype_not_by_list() {
        let (_dir, conn) = db("cards-page-popularity-archetype");
        card(&conn, 1, "Staple", Some("o-stp"), None);
        let variants = held(&conn, "Qvariants", "modern");
        for list in 1..=3 {
            list_card(&conn, variants, list, 1, false);
        }
        list_card(&conn, held(&conn, "Qother", "modern"), 1, 1, true);
        let retired = held(&conn, "Qretired", "modern");
        list_card(&conn, retired, 1, 1, false);
        conn.execute(
            "UPDATE archetypes SET retired_at = 5 WHERE id = ?1",
            [retired],
        )
        .expect("retire");
        let page = cards_page(&conn, &CardsPageQuery::new()).expect("page");
        assert_eq!(page.cards[0].popularity, 2);
    }

    // ---- The archetype sheet (0.x Cards/ArchetypesTest) ----

    #[test]
    fn lists_the_archetypes_a_card_appears_in_with_maindeck_and_sideboard_placement() {
        let (_dir, conn) = db("cards-sheet-placement");
        card(&conn, 1, "Bolt", Some("o-bolt"), None);
        list_card(&conn, held(&conn, "Burn", "modern"), 1, 1, false);
        list_card(&conn, held(&conn, "Control", "modern"), 1, 1, true);
        let rows = card_archetypes(&conn, "o-bolt", None).expect("sheet");
        let placed: Vec<(&str, bool, bool, u32)> = rows
            .iter()
            .map(|r| (r.name.as_str(), r.maindeck, r.sideboard, r.deck_count))
            .collect();
        assert_eq!(
            placed,
            vec![("Burn", true, false, 1), ("Control", false, true, 1)]
        );
        assert_eq!(rows[0].format.as_deref(), Some("modern"));
        assert_eq!(rows[0].format_label.as_deref(), Some("Modern"));
        assert_eq!(rows[0].color_identity.as_deref(), Some("U,R"));
    }

    #[test]
    fn flags_an_archetype_that_runs_the_card_both_main_and_side() {
        let (_dir, conn) = db("cards-sheet-both");
        card(&conn, 1, "Mix", Some("o-mix"), None);
        let midrange = held(&conn, "Midrange", "modern");
        list_card(&conn, midrange, 1, 1, false);
        list_card(&conn, midrange, 2, 1, true);
        let rows = card_archetypes(&conn, "o-mix", None).expect("sheet");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].maindeck, rows[0].sideboard, rows[0].deck_count),
            (true, true, 2)
        );
    }

    /// Two lists holding one printing each are two lists of one archetype;
    /// a deck count of 2 sorts above a deck count of 1.
    #[test]
    fn aggregates_archetypes_across_every_printing_of_an_oracle_id() {
        let (_dir, conn) = db("cards-sheet-printings");
        card(&conn, 1, "Brainstorm", Some("o-bs"), None);
        card(&conn, 2, "Brainstorm", Some("o-bs"), None);
        let tempo = held(&conn, "Tempo", "legacy");
        list_card(&conn, tempo, 1, 1, false);
        list_card(&conn, tempo, 2, 2, false);
        list_card(&conn, held(&conn, "Arcane", "legacy"), 1, 2, false);
        let rows = card_archetypes(&conn, "o-bs", None).expect("sheet");
        let counts: Vec<(&str, u32)> = rows
            .iter()
            .map(|r| (r.name.as_str(), r.deck_count))
            .collect();
        assert_eq!(counts, vec![("Tempo", 2), ("Arcane", 1)]);
    }

    #[test]
    fn scopes_the_archetype_list_to_a_given_format() {
        let (_dir, conn) = db("cards-sheet-format");
        card(&conn, 1, "Get Lost", Some("o-getlost"), None);
        list_card(&conn, held(&conn, "Modern Deck", "modern"), 1, 1, false);
        list_card(&conn, held(&conn, "Legacy Deck", "legacy"), 1, 1, false);
        let rows = card_archetypes(&conn, "o-getlost", Some("modern")).expect("sheet");
        let found: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(found, vec!["Modern Deck"]);
        let retired = rows[0].id;
        conn.execute(
            "UPDATE archetypes SET retired_at = 5 WHERE id = ?1",
            [retired],
        )
        .expect("retire");
        assert!(
            card_archetypes(&conn, "o-getlost", Some("modern"))
                .expect("sheet")
                .is_empty(),
            "a retired archetype is not held"
        );
    }

    #[test]
    fn expands_printings_into_separate_rows_when_grouping_is_disabled() {
        let (_dir, conn) = db("cards-page-ungrouped");
        card(&conn, 10, "Brainstorm", Some("o-bs"), None);
        card(&conn, 20, "Brainstorm", Some("o-bs"), None);
        let page = cards_page(&conn, &with(|q| q.group_printings = false)).expect("page");
        assert_eq!(page.cards.len(), 2);
    }

    // 0.x ArchetypesTest: "resolves a printing with no oracle_id by its id".
    #[test]
    fn a_printing_with_no_oracle_id_is_its_own_group_keyed_by_its_id() {
        let (_dir, conn) = db("cards-page-no-oracle");
        card(&conn, 7, "Token", None, None);
        card(&conn, 8, "Token", None, None);
        let page = cards_page(&conn, &CardsPageQuery::new()).expect("page");
        assert_eq!(page.cards.len(), 2, "no oracle id: never merged");
        assert_eq!(page.cards[0].group_key(), "id:7");
        let rogue = held(&conn, "Qrogue", "modern");
        list_card(&conn, rogue, 1, 7, false);
        let rows = card_archetypes(&conn, "id:7", None).expect("archetypes");
        let found: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(found, vec!["Qrogue"]);
        assert!(
            card_archetypes(&conn, "id:8", None)
                .expect("archetypes")
                .is_empty()
        );
        assert!(
            card_archetypes(&conn, "id:x", None)
                .expect("archetypes")
                .is_empty()
        );
    }

    #[test]
    fn search_wildcards_match_only_themselves() {
        let (_dir, conn) = db("cards-page-search-escape");
        card(&conn, 1, "Goblin", Some("o-g"), Some("Creature - Goblin"));
        card(&conn, 2, "Fifty%Off", Some("o-f"), Some("Sorcery"));
        card(&conn, 3, "Snake_Eyes", Some("o-s"), Some("Sorcery"));
        card(&conn, 4, "Back\\Slash", Some("o-b"), Some("Sorcery"));
        let search = |text: &str| {
            let page =
                cards_page(&conn, &with(|q| q.search = Some(text.to_string()))).expect("page");
            names(&page)
        };
        assert_eq!(search("%"), vec!["Fifty%Off"]);
        assert_eq!(search("_"), vec!["Snake_Eyes"]);
        assert_eq!(search("\\"), vec!["Back\\Slash"]);
        assert_eq!(search("gob"), vec!["Goblin"]);
    }

    #[test]
    fn hides_cards_whose_canonical_type_is_hidden() {
        let (_dir, conn) = db("cards-page-hidden");
        card(&conn, 1, "Goblin", Some("o-g"), Some("Creature - Goblin"));
        card(&conn, 2, "Ponder", Some("o-p"), Some("Sorcery"));
        card(&conn, 3, "Island", Some("o-i"), Some("Basic Land - Island"));
        let page = cards_page(
            &conn,
            &with(|q| q.hidden_types = vec!["Land".into(), "Creature".into()]),
        )
        .expect("page");
        assert_eq!(names(&page), vec!["Ponder"]);
    }

    #[test]
    fn resolves_multi_type_cards_by_canonical_precedence() {
        let (_dir, conn) = db("cards-page-precedence");
        card(
            &conn,
            1,
            "Construct",
            Some("o-c"),
            Some("Artifact Creature - Construct"),
        );
        let hide = |t: &str| with(|q| q.hidden_types = vec![t.to_string()]);
        assert_eq!(
            cards_page(&conn, &hide("Artifact")).expect("a").cards.len(),
            1
        );
        assert_eq!(
            cards_page(&conn, &hide("Creature")).expect("c").cards.len(),
            0
        );
    }

    #[test]
    fn hides_battle_cards_when_the_battle_type_is_hidden() {
        let (_dir, conn) = db("cards-page-battle");
        card(&conn, 1, "Invasion", Some("o-bat"), Some("Battle - Siege"));
        card(&conn, 2, "Ponder", Some("o-pon"), Some("Sorcery"));
        let page = cards_page(&conn, &with(|q| q.hidden_types = vec!["Battle".into()])).expect("p");
        assert_eq!(names(&page), vec!["Ponder"]);
    }

    #[test]
    fn keeps_cards_with_no_recognised_type_even_when_every_type_is_hidden() {
        let (_dir, conn) = db("cards-page-other");
        card(&conn, 1, "Stub", Some("o-s"), None);
        let all: Vec<String> = cards::stats::TYPE_CATEGORIES
            .iter()
            .map(|c| c.to_string())
            .collect();
        let page = cards_page(&conn, &with(|q| q.hidden_types = all)).expect("page");
        assert_eq!(page.cards.len(), 1);
    }

    #[test]
    fn filters_cards_by_name_search() {
        let (_dir, conn) = db("cards-page-search");
        card(&conn, 1, "Lightning Bolt", Some("o-bolt"), None);
        card(&conn, 2, "Counterspell", Some("o-cs"), None);
        let page = cards_page(&conn, &with(|q| q.search = Some("bolt".into()))).expect("page");
        assert_eq!(names(&page), vec!["Lightning Bolt"]);
        assert_eq!(page.total, 1);
    }

    #[test]
    fn the_page_reports_the_number_of_cards_missing_data() {
        let (_dir, conn) = db("cards-page-missing");
        card(&conn, 1, "Complete", Some("o-1"), None);
        conn.execute("INSERT INTO cards (catalog_id) VALUES (2), (3)", [])
            .expect("stubs");
        let page = cards_page(&conn, &CardsPageQuery::new()).expect("page");
        assert_eq!(page.missing_count, 2);
        assert_eq!(page.total_count, 3);
    }

    #[test]
    fn pages_are_eighty_cards_and_a_page_past_the_end_shows_the_last() {
        let (_dir, conn) = db("cards-page-paging");
        for id in 1..=81 {
            card(
                &conn,
                id,
                &format!("Card {id:03}"),
                Some(&format!("o-{id}")),
                None,
            );
        }
        let first = cards_page(&conn, &CardsPageQuery::new()).expect("first");
        assert_eq!(
            (first.cards.len(), first.last_page, first.total),
            (80, 2, 81)
        );
        let beyond = cards_page(&conn, &with(|q| q.page = 9)).expect("beyond");
        assert_eq!((beyond.page, beyond.cards.len()), (2, 1));
    }

    #[test]
    #[ignore = "measured, not a gate: run with --release -- --ignored --nocapture"]
    fn cards_page_meets_the_budget() {
        let (_dir, mut conn) = db("cards-page-budget");
        let tx = conn.transaction().expect("tx");
        for id in 1..=20_000_i64 {
            tx.execute(
                "INSERT INTO cards (catalog_id, name, oracle_id, type, set_code, scryfall_id, image)
                 VALUES (?1, ?2, ?3, 'Creature - Elf', 'ABC', 'sf', 'http://img')",
                params![id, format!("Card {id:05}"), format!("o-{}", id / 2)],
            )
            .expect("card");
        }
        // The archetype seed of archetypes spec section 17: 500 held
        // archetypes with 60-card lists (45 main, 15 sideboard) and 50
        // manual variants, over the whole catalog.
        for n in 0..550_i64 {
            let (format, list) = if n < 500 {
                ("modern", 1)
            } else {
                ("legacy", n - 499)
            };
            // 500 archetypes with one list each, then one with 50
            // variant lists.
            let archetype = if n <= 500 {
                tx.query_row(
                    "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
                     VALUES (?1, ?1, ?2, 'manual', 0, 0) RETURNING id",
                    params![format!("q-{n}"), format],
                    |row| row.get::<_, i64>(0),
                )
                .expect("archetype")
            } else {
                tx.query_row("SELECT MAX(id) FROM archetypes", [], |row| row.get(0))
                    .expect("variant owner")
            };
            for slot in 0..60_i64 {
                let catalog_id = 1 + (n * 37 + slot * 331) % 20_000;
                tx.execute(
                    "INSERT OR IGNORE INTO archetype_cards
                         (archetype_id, list, catalog_id, quantity, sideboard)
                     VALUES (?1, ?2, ?3, 4, ?4)",
                    params![archetype, list, catalog_id, slot >= 45],
                )
                .expect("list card");
            }
        }
        tx.commit().expect("commit");
        for (label, query) in [
            ("grouped", with(|q| q.hidden_types = vec!["Land".into()])),
            (
                "grouped, modern",
                with(|q| q.format = Some("modern".into())),
            ),
            ("printings", with(|q| q.group_printings = false)),
            ("grouped, last page", with(|q| q.page = 126)),
        ] {
            let mut times = Vec::new();
            let mut page = None;
            for _ in 0..20 {
                let started = std::time::Instant::now();
                page = Some(cards_page(&conn, &query).expect("page"));
                times.push(started.elapsed());
            }
            times.sort();
            let page = page.expect("a page");
            println!(
                "cards_page 20,000 cards {label}: p50 {:?}, p95 {:?}, total {}, top popularity {}",
                times[10], times[18], page.total, page.cards[0].popularity
            );
            // The p95 (19th of 20), as the speed table reports it.
            assert!(
                times[18].as_millis() < 100,
                "{label} over the 100 ms budget: {times:?}"
            );
        }
        let page = cards_page(&conn, &CardsPageQuery::new()).expect("page");
        assert_eq!(page.total, 10_001, "ids 2k and 2k+1 share an oracle id");
        let started = std::time::Instant::now();
        let sheet = card_archetypes(&conn, &page.cards[0].group_key(), None).expect("sheet");
        println!(
            "card_archetypes of the most popular card: {:?} ({} archetypes)",
            started.elapsed(),
            sheet.len()
        );
    }
}
