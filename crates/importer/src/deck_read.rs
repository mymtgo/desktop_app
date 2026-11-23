//! Reading 0.x cards, decks, deck versions and match links (decks spec section
//! 9). These tables are optional: an old 0.x database without them imports its
//! matches only. Values are read loosely, as in `read`.

use std::collections::{HashMap, HashSet};

use rusqlite::Connection;
use store::import_decks::{
    ImportedCard, ImportedDeck, ImportedDecks, ImportedLink, ImportedVersion,
};
use store::import_leagues::LeagueDeck;

use crate::legacy_signature::convert_legacy;
use crate::limited_read::LimitedVersions;
use crate::read::{ImportError, read_error};
use crate::sql::{
    as_integer, as_real, as_text, column_list, has_all, has_column, parse_id, parse_utc_ms,
    select_list, table_columns, value, where_account_not_deleted,
};

/// Columns without which the deck tables cannot be read; a database missing
/// any of them has no deck data v1 can use.
const REQUIRED: [(&str, &[&str]); 3] = [
    ("cards", &CARD_KEYS),
    ("decks", &DECK_KEYS),
    ("deck_versions", &VERSION_COLUMNS),
];

/// `cards` columns every 0.x has, read before `CARD_COLUMNS`.
const CARD_KEYS: [&str; 2] = ["id", "mtgo_id"];

/// `decks` columns every 0.x has, read before `DECK_COLUMNS`.
const DECK_KEYS: [&str; 4] = ["id", "mtgo_id", "name", "format"];

/// `deck_versions` columns, in the order `read_versions` reads them.
const VERSION_COLUMNS: [&str; 4] = ["id", "deck_id", "signature", "modified_at"];

/// `accounts` columns read (the account id and name of a deck's account).
const ACCOUNT_COLUMNS: [&str; 2] = ["id", "username"];

/// `matches` columns of the deck links, in the order `read_links` reads them.
const LINK_COLUMNS: [&str; 3] = ["token", "mtgo_id", "deck_version_id"];

/// Every `(table, columns)` this reader selects. The SQL is built from the
/// same lists, so the coverage ledger cannot drift from it.
pub const READ_COLUMNS: [(&str, &[&str]); 7] = [
    ("cards", &CARD_KEYS),
    ("cards", &CARD_COLUMNS),
    ("decks", &DECK_KEYS),
    ("decks", &DECK_COLUMNS),
    ("deck_versions", &VERSION_COLUMNS),
    ("accounts", &ACCOUNT_COLUMNS),
    ("matches", &LINK_COLUMNS),
];

const CARD_COLUMNS: [&str; 14] = [
    "oracle_id",
    "name",
    "type",
    "sub_type",
    "rarity",
    "color_identity",
    "colors",
    "cmc",
    "mana_cost",
    "set_name",
    "set_code",
    "image",
    "art_crop",
    "scryfall_id",
];

const DECK_COLUMNS: [&str; 7] = [
    "account_id",
    "original_name",
    "color_identity",
    "cover_id",
    "deleted_at",
    "created_at",
    "updated_at",
];

/// The deck data of a 0.x database and what was left out.
#[derive(Debug, Clone, PartialEq)]
pub struct DeckRead {
    pub data: ImportedDecks,
    /// Limited decks (format `Limited`, or an MTGO id starting `limited:`).
    pub skipped_limited: u32,
    /// Versions v1 cannot read: signatures that do not decode, legacy
    /// oracle-anchored ones with an oracle id no imported card has, and
    /// unreadable times.
    pub skipped_versions: u32,
    /// Decks whose MTGO id an earlier deck (by 0.x id) already has: the
    /// first wins, and these, with their versions and match links, are left
    /// out (issue #122 M13; 0.x itself avoids such duplicates).
    pub skipped_duplicates: u32,
    /// The MTGO id of each imported deck, by its 0.x `decks.id`: where a
    /// kept 0.x Deck stats card finds its v1 deck (issue #152). A duplicate
    /// left out maps to the MTGO id it shares with the deck that won.
    pub deck_mtgo_ids: HashMap<i64, String>,
}

/// Each imported deck version by its 0.x id: the deck's MTGO id and the
/// version's signature, as v1 finds it.
pub(crate) type ImportedVersions = HashMap<i64, LeagueDeck>;

/// The deck data, or `None` when the 0.x database has no deck tables. Also
/// gives the imported versions, for the leagues, and the versions of the
/// limited decks left out, for the limited registered decks.
pub(crate) fn read_decks(
    conn: &Connection,
) -> Result<Option<(DeckRead, ImportedVersions, LimitedVersions)>, ImportError> {
    for (table, required) in REQUIRED {
        if !has_all(&table_columns(conn, table)?, required) {
            return Ok(None);
        }
    }
    let (cards, catalog_by_card_id) = read_cards(conn)?;
    let accounts = read_accounts(conn)?;
    let catalog_by_oracle = catalog_by_oracle(&cards);
    let mut skipped_versions = 0;
    let versions = read_versions(conn, &catalog_by_oracle, &mut skipped_versions)?;
    let mut skipped_limited = 0;
    let mut skipped_duplicates = 0;
    // 0.x ids of the duplicates left out, with the MTGO id of the deck that won.
    let mut duplicate_mtgo_ids: HashMap<i64, String> = HashMap::new();
    let rows = read_deck_rows(conn)?;

    let mut versions_by_deck: HashMap<i64, Vec<ImportedVersion>> = HashMap::new();
    for version in &versions {
        versions_by_deck
            .entry(version.deck_id)
            .or_default()
            .push(ImportedVersion {
                signature: version.signature.clone(),
                modified_at_ms: version.modified_at_ms,
            });
    }

    let mut decks = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    // 0.x deck id to its MTGO id, for the links.
    let mut mtgo_by_deck_id: HashMap<i64, String> = HashMap::new();
    // 0.x ids of the limited decks left out.
    let mut limited_deck_ids: HashSet<i64> = HashSet::new();
    for row in rows {
        if row.mtgo_id.is_empty() {
            continue;
        }
        if seen.contains(&row.mtgo_id) {
            skipped_duplicates += 1;
            duplicate_mtgo_ids.insert(row.id, row.mtgo_id.clone());
            continue;
        }
        if decks::is_limited(&row.format) || row.mtgo_id.starts_with("limited:") {
            skipped_limited += 1;
            limited_deck_ids.insert(row.id);
            continue;
        }
        seen.insert(row.mtgo_id.clone());
        mtgo_by_deck_id.insert(row.id, row.mtgo_id.clone());
        let deck_versions = versions_by_deck.remove(&row.id).unwrap_or_default();
        decks.push(ImportedDeck {
            mtgo_id: row.mtgo_id,
            account: row.account_id.and_then(|id| accounts.get(&id).cloned()),
            name: row.name,
            original_name: row.original_name,
            format: row.format,
            color_identity: row.color_identity,
            cover_catalog_id: row
                .cover_id
                .and_then(|id| catalog_by_card_id.get(&id).copied()),
            archived_at_ms: row.deleted_at.as_deref().and_then(parse_utc_ms),
            created_at_ms: row.created_at.as_deref().and_then(parse_utc_ms),
            updated_at_ms: row.updated_at.as_deref().and_then(parse_utc_ms),
            versions: deck_versions,
        });
    }

    let by_version = imported_versions(&versions, &mtgo_by_deck_id);
    let mut limited_versions = LimitedVersions::new();
    for version in &versions {
        if limited_deck_ids.contains(&version.deck_id) {
            limited_versions.insert(version.id, version.signature.clone());
        }
    }
    let links = read_links(conn, &by_version)?;
    let mut deck_mtgo_ids = mtgo_by_deck_id;
    deck_mtgo_ids.extend(duplicate_mtgo_ids);
    let read = DeckRead {
        data: ImportedDecks {
            cards,
            decks,
            links,
        },
        skipped_limited,
        skipped_versions,
        skipped_duplicates,
        deck_mtgo_ids,
    };
    Ok(Some((read, by_version, limited_versions)))
}

/// Each version of an imported deck, by its 0.x id.
fn imported_versions(
    versions: &[VersionRow],
    mtgo_by_deck_id: &HashMap<i64, String>,
) -> ImportedVersions {
    let mut by_version = HashMap::new();
    for version in versions {
        if let Some(mtgo_id) = mtgo_by_deck_id.get(&version.deck_id) {
            by_version.insert(
                version.id,
                LeagueDeck {
                    deck_mtgo_id: mtgo_id.clone(),
                    signature: version.signature.clone(),
                },
            );
        }
    }
    by_version
}

/// One card per numeric catalog id, and every 0.x card id's catalog id (for
/// deck covers). 0.x can hold the same printing twice: a later row wins unless
/// it has no name and the earlier one has.
fn read_cards(conn: &Connection) -> Result<(Vec<ImportedCard>, HashMap<i64, i64>), ImportError> {
    let present = table_columns(conn, "cards")?;
    let sql = format!(
        "SELECT {}, {} FROM cards ORDER BY id",
        column_list("", &CARD_KEYS),
        select_list(&present, &CARD_COLUMNS)
    );
    let mut stmt = conn.prepare(&sql).map_err(read_error)?;
    let rows = stmt
        .query_map([], |row| {
            let id: i64 = row.get(0)?;
            let catalog_id = as_text(value(row, 1)?).as_deref().and_then(parse_id);
            let card = ImportedCard {
                catalog_id: catalog_id.unwrap_or(0),
                oracle_id: as_text(value(row, 2)?),
                name: as_text(value(row, 3)?),
                card_type: as_text(value(row, 4)?),
                sub_type: as_text(value(row, 5)?),
                rarity: as_text(value(row, 6)?),
                color_identity: as_text(value(row, 7)?),
                colors: as_text(value(row, 8)?),
                cmc: as_real(value(row, 9)?),
                mana_cost: as_text(value(row, 10)?),
                set_name: as_text(value(row, 11)?),
                set_code: as_text(value(row, 12)?),
                image: as_text(value(row, 13)?),
                art_crop: as_text(value(row, 14)?),
                scryfall_id: as_text(value(row, 15)?),
            };
            Ok((id, catalog_id, card))
        })
        .map_err(read_error)?;

    let mut order: Vec<i64> = Vec::new();
    let mut by_catalog: HashMap<i64, ImportedCard> = HashMap::new();
    let mut catalog_by_card_id: HashMap<i64, i64> = HashMap::new();
    for row in rows {
        let (id, catalog_id, card) = row.map_err(read_error)?;
        let Some(catalog_id) = catalog_id else {
            continue;
        };
        catalog_by_card_id.insert(id, catalog_id);
        match by_catalog.get(&catalog_id) {
            None => {
                order.push(catalog_id);
                by_catalog.insert(catalog_id, card);
            }
            Some(kept) => {
                if card.name.is_some() || kept.name.is_none() {
                    by_catalog.insert(catalog_id, card);
                }
            }
        }
    }
    let mut cards = Vec::new();
    for catalog_id in order {
        if let Some(card) = by_catalog.remove(&catalog_id) {
            cards.push(card);
        }
    }
    Ok((cards, catalog_by_card_id))
}

/// 0.x account id to username, deleted accounts left out (empty when the
/// table is missing). 0.x's soft delete leaves a deleted account out of a
/// deck's `account` and of every account list, so its deck imports with no
/// account rather than registering an account 0.x does not show (issue
/// #337).
fn read_accounts(conn: &Connection) -> Result<HashMap<i64, String>, ImportError> {
    let present = table_columns(conn, "accounts")?;
    let mut accounts = HashMap::new();
    if !has_all(&present, &ACCOUNT_COLUMNS) {
        return Ok(accounts);
    }
    let not_deleted = where_account_not_deleted(&present, "");
    let sql = format!(
        "SELECT {} FROM accounts{not_deleted}",
        column_list("", &ACCOUNT_COLUMNS)
    );
    let mut stmt = conn.prepare(&sql).map_err(read_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((as_integer(value(row, 0)?), as_text(value(row, 1)?)))
        })
        .map_err(read_error)?;
    for row in rows {
        if let (Some(id), Some(username)) = row.map_err(read_error)?
            && !username.trim().is_empty()
        {
            accounts.insert(id, username);
        }
    }
    Ok(accounts)
}

/// One `decks` row.
struct DeckRow {
    id: i64,
    mtgo_id: String,
    name: String,
    format: String,
    account_id: Option<i64>,
    original_name: Option<String>,
    color_identity: Option<String>,
    cover_id: Option<i64>,
    deleted_at: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

/// Every deck, in id order (the first of duplicate MTGO ids wins).
fn read_deck_rows(conn: &Connection) -> Result<Vec<DeckRow>, ImportError> {
    let present = table_columns(conn, "decks")?;
    let sql = format!(
        "SELECT {}, {} FROM decks ORDER BY id",
        column_list("", &DECK_KEYS),
        select_list(&present, &DECK_COLUMNS)
    );
    let mut stmt = conn.prepare(&sql).map_err(read_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(DeckRow {
                id: row.get(0)?,
                mtgo_id: as_text(value(row, 1)?)
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
                name: as_text(value(row, 2)?).unwrap_or_default(),
                format: as_text(value(row, 3)?).unwrap_or_default(),
                account_id: as_integer(value(row, 4)?),
                original_name: as_text(value(row, 5)?),
                color_identity: as_text(value(row, 6)?),
                cover_id: as_integer(value(row, 7)?),
                deleted_at: as_text(value(row, 8)?),
                created_at: as_text(value(row, 9)?),
                updated_at: as_text(value(row, 10)?),
            })
        })
        .map_err(read_error)?;
    let mut decks = Vec::new();
    for row in rows {
        decks.push(row.map_err(read_error)?);
    }
    Ok(decks)
}

/// One readable `deck_versions` row.
struct VersionRow {
    id: i64,
    deck_id: i64,
    signature: String,
    modified_at_ms: i64,
}

/// Each oracle id's lowest catalog id among the imported cards, so a legacy
/// signature converts the same way every time.
fn catalog_by_oracle(cards: &[ImportedCard]) -> HashMap<String, i64> {
    let mut map: HashMap<String, i64> = HashMap::new();
    for card in cards {
        let Some(oracle_id) = &card.oracle_id else {
            continue;
        };
        let oracle_id = oracle_id.trim();
        if oracle_id.is_empty() {
            continue;
        }
        let lowest = map.entry(oracle_id.to_string()).or_insert(card.catalog_id);
        if card.catalog_id < *lowest {
            *lowest = card.catalog_id;
        }
    }
    map
}

/// Every version v1 can read, in id order. A legacy oracle-anchored signature
/// is converted to the current form through the imported cards
/// (`convert_legacy`). Signatures that cannot be read or converted (an oracle
/// id without a card, bad base64) are counted in `skipped`, as are unreadable
/// times. An empty signature (an empty list) is kept: a deck file without
/// cards gives the same signature.
fn read_versions(
    conn: &Connection,
    catalog_by_oracle: &HashMap<String, i64>,
    skipped: &mut u32,
) -> Result<Vec<VersionRow>, ImportError> {
    let sql = format!(
        "SELECT {} FROM deck_versions ORDER BY id",
        column_list("", &VERSION_COLUMNS)
    );
    let mut stmt = conn.prepare(&sql).map_err(read_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                as_integer(value(row, 1)?),
                as_text(value(row, 2)?).unwrap_or_default(),
                as_text(value(row, 3)?),
            ))
        })
        .map_err(read_error)?;
    let mut versions = Vec::new();
    for row in rows {
        let (id, deck_id, signature, modified_at) = row.map_err(read_error)?;
        let Some(deck_id) = deck_id else {
            continue;
        };
        let current = if signature.is_empty() || !decks::decode_signature(&signature).is_empty() {
            Some(signature)
        } else {
            convert_legacy(&signature, catalog_by_oracle)
        };
        let modified_at_ms = modified_at.as_deref().and_then(parse_utc_ms);
        match (current, modified_at_ms) {
            (Some(signature), Some(modified_at_ms)) => versions.push(VersionRow {
                id,
                deck_id,
                signature,
                modified_at_ms,
            }),
            _ => *skipped += 1,
        }
    }
    Ok(versions)
}

/// The deck version of every 0.x match that has one, in match id order. Only
/// versions of imported decks are linked; an old 0.x without
/// `matches.deck_version_id` has no links.
fn read_links(
    conn: &Connection,
    by_version: &ImportedVersions,
) -> Result<Vec<ImportedLink>, ImportError> {
    if !has_column(&table_columns(conn, "matches")?, "deck_version_id") {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT {} FROM matches WHERE deck_version_id IS NOT NULL ORDER BY id",
        column_list("", &LINK_COLUMNS)
    );
    let mut stmt = conn.prepare(&sql).map_err(read_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                as_text(value(row, 0)?).unwrap_or_default(),
                as_text(value(row, 1)?),
                as_integer(value(row, 2)?),
            ))
        })
        .map_err(read_error)?;
    let mut links = Vec::new();
    for row in rows {
        let (token, mtgo_id, version_id) = row.map_err(read_error)?;
        let Some(version_id) = version_id else {
            continue;
        };
        if token.is_empty() {
            continue;
        }
        if let Some(deck) = by_version.get(&version_id) {
            links.push(ImportedLink {
                token,
                mtgo_match_id: mtgo_id.as_deref().and_then(parse_id),
                deck_mtgo_id: deck.deck_mtgo_id.clone(),
                signature: deck.signature.clone(),
            });
        }
    }
    Ok(links)
}
