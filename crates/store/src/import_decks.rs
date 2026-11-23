//! Cards, decks, deck versions and match links imported from the 0.x database
//! (decks spec section 9). Runs after the matches are imported, so the links
//! find their match rows.
//!
//! Live and file data win:
//! - A deck read from its MTGO file (`source = 'file'`) keeps its file data
//!   (name, format); an imported version is added to it only when its
//!   signature is new.
//! - The user's 0.x edits (rename, cover, archive, colour identity) fill what
//!   v1 has not set and never replace an edit made in v1 (`merged_fields`).
//! - An imported link never replaces a stored link (`matches.deck_version_id`).
//! - A file deck with no colour identity, not set by hand, gets one computed
//!   from the imported card data (`fill_missing_identities`).
//! - A link goes to the 0.x match's token, or to the live token when the match
//!   was skipped because v1 tracked it live (the rule notes follow).

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::StoreError;
use crate::cards::{CardRef, ensure_stubs};
use crate::import::live_row_token;
use crate::write::registry_id;

/// One printing (MTGO catalog id) with its card data.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedCard {
    pub catalog_id: i64,
    /// Set once the card is resolved (cards spec section 13): a row imported
    /// resolved is not asked about again.
    pub scryfall_id: Option<String>,
    pub oracle_id: Option<String>,
    pub name: Option<String>,
    /// The type line (`cards.type`).
    pub card_type: Option<String>,
    pub sub_type: Option<String>,
    pub rarity: Option<String>,
    pub color_identity: Option<String>,
    pub colors: Option<String>,
    pub cmc: Option<f64>,
    pub mana_cost: Option<String>,
    pub set_name: Option<String>,
    pub set_code: Option<String>,
    pub image: Option<String>,
    pub art_crop: Option<String>,
}

/// One card list of a deck, as its signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedVersion {
    pub signature: String,
    pub modified_at_ms: i64,
}

/// One 0.x deck. A Limited one is left out by the import (`import_decks`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedDeck {
    pub mtgo_id: String,
    /// The 0.x account's username, if the deck has one.
    pub account: Option<String>,
    pub name: String,
    pub original_name: Option<String>,
    pub format: String,
    pub color_identity: Option<String>,
    pub cover_catalog_id: Option<i64>,
    /// Set when 0.x archived (soft deleted) the deck.
    pub archived_at_ms: Option<i64>,
    pub created_at_ms: Option<i64>,
    pub updated_at_ms: Option<i64>,
    pub versions: Vec<ImportedVersion>,
}

/// The deck version a 0.x match was played with, by the deck's MTGO id and
/// the version's signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedLink {
    pub token: String,
    pub mtgo_match_id: Option<i64>,
    pub deck_mtgo_id: String,
    pub signature: String,
}

/// Everything deck related read from one 0.x database.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportedDecks {
    pub cards: Vec<ImportedCard>,
    pub decks: Vec<ImportedDeck>,
    pub links: Vec<ImportedLink>,
}

/// What one deck import did. A deck read from its file counts as unchanged:
/// its row is kept as it is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeckImportCounts {
    pub cards_added: u32,
    pub cards_updated: u32,
    pub cards_unchanged: u32,
    pub decks_added: u32,
    pub decks_updated: u32,
    pub decks_unchanged: u32,
    pub versions_added: u32,
    pub versions_unchanged: u32,
    pub links_added: u32,
    /// The match already had a link (from tracking, or an earlier import).
    pub links_unchanged: u32,
    /// Matches linked whose imported postboard rule could not be applied
    /// (a stored game log that cannot be read): each is logged, and its
    /// rows stay as imported.
    pub rules_failed: u32,
}

/// What happened to one row.
enum Change {
    Added,
    Updated,
    Unchanged,
}

/// Write the cards, then the decks with their versions, then the links, in
/// one transaction.
pub fn import_decks(
    conn: &mut Connection,
    data: &ImportedDecks,
    now_ms: i64,
) -> Result<DeckImportCounts, StoreError> {
    let tx = conn.transaction()?;
    let mut counts = DeckImportCounts::default();
    for card in &data.cards {
        match import_card(&tx, card)? {
            Change::Added => counts.cards_added += 1,
            Change::Updated => counts.cards_updated += 1,
            Change::Unchanged => counts.cards_unchanged += 1,
        }
    }
    for deck in &data.decks {
        // v1 keeps no Limited deck rows (limited ruling 3, issue #194): their
        // versions become registered decks in the Limited import.
        if is_limited_deck(deck) {
            continue;
        }
        import_deck(&tx, deck, now_ms, &mut counts)?;
    }
    fill_missing_identities(&tx)?;
    for link in &data.links {
        import_link(&tx, link, &mut counts)?;
    }
    tx.commit()?;
    Ok(counts)
}

/// One deck pulled from the cloud (07 spec section 10): stored as a 0.x deck
/// is (live and file data win, `merged_fields`), then filed under the current
/// account of its platform when it has none (0.x `DeckBundleImporter`: a
/// bundle carries no account; a deck already filed keeps its account;
/// cross-device sync spec section 7). An Arena or Endstep deck with no
/// account of its platform here stays unfiled, its platform in
/// `pulled_platform`, until a pulled match played with it files it
/// (`import::import_pulled_match`). Inside the caller's transaction.
pub fn import_pulled_deck(
    tx: &Transaction,
    deck: &ImportedDeck,
    platform: crate::Platform,
    now_ms: i64,
) -> Result<(), StoreError> {
    let mut counts = DeckImportCounts::default();
    import_deck(tx, deck, now_ms, &mut counts)?;
    if let Some(account_id) = crate::accounts::current_account_id(tx, platform)? {
        tx.execute(
            "UPDATE decks SET account_id = ?2
             WHERE mtgo_id = ?1 AND account_id IS NULL
               AND (pulled_platform IS NULL OR pulled_platform = ?3)",
            params![deck.mtgo_id, account_id, platform.as_str()],
        )?;
    }
    // Still unfiled: an Arena or Endstep deck records its platform, so no
    // MTGO rule adopts it while it waits for an account of its own.
    if platform != crate::Platform::Mtgo {
        tx.execute(
            "UPDATE decks SET pulled_platform = ?2 WHERE mtgo_id = ?1 AND account_id IS NULL",
            params![deck.mtgo_id, platform.as_str()],
        )?;
    }
    Ok(())
}

/// A 0.x Limited deck (0.x `Deck::isLimited`): format `Limited`, or an MTGO
/// id starting `limited:`.
fn is_limited_deck(deck: &ImportedDeck) -> bool {
    decks::is_limited(&deck.format) || deck.mtgo_id.starts_with("limited:")
}

fn import_card(tx: &Transaction, card: &ImportedCard) -> Result<Change, StoreError> {
    let stored = tx
        .prepare_cached(
            "SELECT catalog_id, oracle_id, name, type, sub_type, rarity, color_identity, colors,
                 cmc, mana_cost, set_name, set_code, image, art_crop, scryfall_id
             FROM cards WHERE catalog_id = ?1",
        )?
        .query_row([card.catalog_id], |row| {
            Ok(ImportedCard {
                catalog_id: row.get(0)?,
                oracle_id: row.get(1)?,
                name: row.get(2)?,
                card_type: row.get(3)?,
                sub_type: row.get(4)?,
                rarity: row.get(5)?,
                color_identity: row.get(6)?,
                colors: row.get(7)?,
                cmc: row.get(8)?,
                mana_cost: row.get(9)?,
                set_name: row.get(10)?,
                set_code: row.get(11)?,
                image: row.get(12)?,
                art_crop: row.get(13)?,
                scryfall_id: row.get(14)?,
            })
        })
        .optional()?;
    // A nameless 0.x row is 0.x's own unresolved stub: it never erases what
    // v1 already knows (a name from a game log, or full data), only fills
    // what is missing, and adds nothing.
    if card.name.is_none() && stored.is_some() {
        fill_unknown_columns(tx, card)?;
        return Ok(Change::Unchanged);
    }
    let change = match &stored {
        None => Change::Added,
        Some(stored) if stored == card => return Ok(Change::Unchanged),
        // A row v1 resolved itself (or an earlier import did) keeps its
        // values: v1's answer is the newer one. 0.x fills only what it lacks.
        Some(stored) if stored.scryfall_id.is_some() => {
            fill_unknown_columns(tx, card)?;
            return Ok(Change::Unchanged);
        }
        // A stub (`store::cards`) had no card data: this adds it.
        Some(stored) if is_stub(stored) => Change::Added,
        Some(_) => Change::Updated,
    };
    tx.prepare_cached(
        "INSERT INTO cards (catalog_id, oracle_id, name, type, sub_type, rarity, color_identity,
             colors, cmc, mana_cost, set_name, set_code, image, art_crop, scryfall_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
         ON CONFLICT (catalog_id) DO UPDATE SET
             scryfall_id = excluded.scryfall_id,
             oracle_id = excluded.oracle_id, name = excluded.name, type = excluded.type,
             sub_type = excluded.sub_type, rarity = excluded.rarity,
             color_identity = excluded.color_identity, colors = excluded.colors,
             cmc = excluded.cmc, mana_cost = excluded.mana_cost, set_name = excluded.set_name,
             set_code = excluded.set_code, image = excluded.image, art_crop = excluded.art_crop",
    )?
    .execute(params![
        card.catalog_id,
        card.oracle_id,
        card.name,
        card.card_type,
        card.sub_type,
        card.rarity,
        card.color_identity,
        card.colors,
        card.cmc,
        card.mana_cost,
        card.set_name,
        card.set_code,
        card.image,
        card.art_crop,
        card.scryfall_id
    ])?;
    Ok(change)
}

/// Fill only the stored row's null columns from a 0.x row.
fn fill_unknown_columns(tx: &Transaction, card: &ImportedCard) -> Result<(), StoreError> {
    tx.prepare_cached(
        "UPDATE cards SET
             oracle_id = COALESCE(oracle_id, ?2), type = COALESCE(type, ?3),
             sub_type = COALESCE(sub_type, ?4), rarity = COALESCE(rarity, ?5),
             color_identity = COALESCE(color_identity, ?6), colors = COALESCE(colors, ?7),
             cmc = COALESCE(cmc, ?8), mana_cost = COALESCE(mana_cost, ?9),
             set_name = COALESCE(set_name, ?10), set_code = COALESCE(set_code, ?11),
             image = COALESCE(image, ?12), art_crop = COALESCE(art_crop, ?13),
             scryfall_id = COALESCE(scryfall_id, ?14)
         WHERE catalog_id = ?1",
    )?
    .execute(params![
        card.catalog_id,
        card.oracle_id,
        card.card_type,
        card.sub_type,
        card.rarity,
        card.color_identity,
        card.colors,
        card.cmc,
        card.mana_cost,
        card.set_name,
        card.set_code,
        card.image,
        card.art_crop,
        card.scryfall_id
    ])?;
    Ok(())
}

/// A stored row with nothing but its catalog id and perhaps a name.
fn is_stub(card: &ImportedCard) -> bool {
    card.scryfall_id.is_none()
        && card.oracle_id.is_none()
        && card.card_type.is_none()
        && card.sub_type.is_none()
        && card.rarity.is_none()
        && card.color_identity.is_none()
        && card.colors.is_none()
        && card.cmc.is_none()
        && card.mana_cost.is_none()
        && card.set_name.is_none()
        && card.set_code.is_none()
        && card.image.is_none()
        && card.art_crop.is_none()
}

/// The columns of a stored deck the import compares and writes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DeckFields {
    account_id: Option<i64>,
    name: String,
    original_name: Option<String>,
    format: String,
    color_identity: Option<String>,
    color_identity_manual: bool,
    cover_catalog_id: Option<i64>,
    archived_at: Option<i64>,
}

fn import_deck(
    tx: &Transaction,
    deck: &ImportedDeck,
    now_ms: i64,
    counts: &mut DeckImportCounts,
) -> Result<(), StoreError> {
    let stored = tx
        .query_row(
            "SELECT id, source, account_id, name, original_name, format, color_identity,
                 color_identity_manual, cover_catalog_id, archived_at
             FROM decks WHERE mtgo_id = ?1",
            [&deck.mtgo_id],
            |row| {
                let id: i64 = row.get(0)?;
                let source: String = row.get(1)?;
                let fields = DeckFields {
                    account_id: row.get(2)?,
                    name: row.get(3)?,
                    original_name: row.get(4)?,
                    format: row.get(5)?,
                    color_identity: row.get(6)?,
                    color_identity_manual: row.get(7)?,
                    cover_catalog_id: row.get(8)?,
                    archived_at: row.get(9)?,
                };
                Ok((id, source, fields))
            },
        )
        .optional()?;

    let deck_id = match stored {
        Some((id, source, stored_fields)) => {
            let from_file = source == "file";
            // An imported deck follows its 0.x account; one without an
            // account keeps the account v1 gave it. A file deck keeps its own.
            let account_id = match &deck.account {
                Some(name) if !from_file => Some(registry_id(tx, "accounts", name, now_ms)?),
                _ => stored_fields.account_id,
            };
            let merged = merged_fields(&stored_fields, deck, from_file, account_id);
            if merged == stored_fields {
                counts.decks_unchanged += 1;
            } else {
                // A file deck's `updated_at` is its file's time: left alone.
                let updated_at = if from_file {
                    None
                } else {
                    Some(deck.updated_at_ms.unwrap_or(now_ms))
                };
                update_deck(tx, id, &merged, updated_at)?;
                counts.decks_updated += 1;
            }
            id
        }
        None => {
            let account_id = match &deck.account {
                Some(name) => Some(registry_id(tx, "accounts", name, now_ms)?),
                None => None,
            };
            counts.decks_added += 1;
            insert_deck(tx, deck, account_id, now_ms)?
        }
    };

    // An Arena course deck is made on every device that played the course,
    // each version at its own observation time: the earliest time wins
    // everywhere, so both devices build the same bundle (cross-device sync
    // spec section 8). Every other deck keeps the time it has.
    let course_deck = deck
        .mtgo_id
        .starts_with(sync_bundles::league::ARENA_COURSE_DECK_PREFIX);
    for version in &deck.versions {
        if course_deck {
            tx.prepare_cached(
                "UPDATE deck_versions SET modified_at = ?3
                 WHERE deck_id = ?1 AND signature = ?2 AND modified_at > ?3",
            )?
            .execute(params![deck_id, version.signature, version.modified_at_ms])?;
        }
        let inserted = tx
            .prepare_cached(
                "INSERT INTO deck_versions (deck_id, signature, modified_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT (deck_id, signature) DO NOTHING",
            )?
            .execute(params![deck_id, version.signature, version.modified_at_ms])?;
        if inserted == 1 {
            counts.versions_added += 1;
        } else {
            counts.versions_unchanged += 1;
        }
    }

    // Every card of the versions and the cover gets a `cards` row (0.x
    // stored its deck cards' stubs; a sync pull brings signatures only).
    let mut stubs: Vec<CardRef> = Vec::new();
    for version in &deck.versions {
        for card in decks::decode_signature(&version.signature) {
            stubs.push(CardRef::id(card.catalog_id));
        }
    }
    if let Some(cover) = deck.cover_catalog_id {
        stubs.push(CardRef {
            catalog_id: cover,
            name: None,
        });
    }
    ensure_stubs(tx, &stubs)?;
    Ok(())
}

/// The stored deck with the 0.x deck merged in (owner ruling, Plan G task 5).
/// The user's 0.x edits only fill what v1 has not set, so an edit made in v1
/// is never overwritten:
/// - name: a deck not renamed in v1 (`original_name` blank) takes the 0.x
///   rename (`name` and `original_name`);
/// - cover: taken when v1 has none;
/// - archived: set when v1's deck is not archived and 0.x's is (a restore in
///   0.x does not unarchive v1's deck);
/// - colour identity: 0.x's, unless v1's was set by hand (the flag stays 0,
///   so a later file change recomputes it, as in 0.x).
///
/// An imported deck (no MTGO file seen yet) also follows 0.x's name (until
/// renamed in v1) and format. A file deck keeps its file's name and format.
fn merged_fields(
    stored: &DeckFields,
    deck: &ImportedDeck,
    from_file: bool,
    account_id: Option<i64>,
) -> DeckFields {
    let mut merged = stored.clone();
    merged.account_id = account_id;
    if !from_file {
        merged.format = deck.format.clone();
    }
    let renamed_in_v1 = stored
        .original_name
        .as_deref()
        .is_some_and(|name| !name.is_empty());
    let renamed_in_0x = deck
        .original_name
        .as_deref()
        .is_some_and(|name| !name.is_empty());
    if !renamed_in_v1 && (renamed_in_0x || !from_file) {
        merged.name = deck.name.clone();
        merged.original_name = deck.original_name.clone();
    }
    if stored.cover_catalog_id.is_none() {
        merged.cover_catalog_id = deck.cover_catalog_id;
    }
    if stored.archived_at.is_none() && deck.archived_at_ms.is_some() {
        merged.archived_at = deck.archived_at_ms;
    }
    if !stored.color_identity_manual && deck.color_identity.is_some() {
        merged.color_identity = deck.color_identity.clone();
    }
    merged
}

fn insert_deck(
    tx: &Transaction,
    deck: &ImportedDeck,
    account_id: Option<i64>,
    now_ms: i64,
) -> Result<i64, StoreError> {
    let created_at = deck.created_at_ms.unwrap_or(now_ms);
    let updated_at = deck.updated_at_ms.unwrap_or(created_at);
    Ok(tx.query_row(
        "INSERT INTO decks (mtgo_id, account_id, name, original_name, format, color_identity,
             cover_catalog_id, archived_at, created_at, updated_at, source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'imported')
         RETURNING id",
        params![
            deck.mtgo_id,
            account_id,
            deck.name,
            deck.original_name,
            deck.format,
            deck.color_identity,
            deck.cover_catalog_id,
            deck.archived_at_ms,
            created_at,
            updated_at
        ],
        |row| row.get(0),
    )?)
}

/// Write the merged columns; `updated_at` `None` keeps the stored one.
fn update_deck(
    tx: &Transaction,
    deck_id: i64,
    fields: &DeckFields,
    updated_at: Option<i64>,
) -> Result<(), StoreError> {
    tx.execute(
        "UPDATE decks SET account_id = ?2, name = ?3, original_name = ?4, format = ?5,
             color_identity = ?6, cover_catalog_id = ?7, archived_at = ?8,
             updated_at = COALESCE(?9, updated_at)
         WHERE id = ?1",
        params![
            deck_id,
            fields.account_id,
            fields.name,
            fields.original_name,
            fields.format,
            fields.color_identity,
            fields.cover_catalog_id,
            fields.archived_at,
            updated_at
        ],
    )?;
    Ok(())
}

/// File decks synced before any card data was imported computed no colour
/// identity (NULL). Each such deck whose identity was not set by hand gets
/// the one its latest version gives now (0.x `ComputeDeckIdentity`); a deck
/// that still gives none is left alone. A stored identity is never replaced.
fn fill_missing_identities(tx: &Transaction) -> Result<(), StoreError> {
    let mut deck_ids: Vec<i64> = Vec::new();
    {
        let mut stmt = tx.prepare(
            "SELECT id FROM decks
             WHERE source = 'file' AND color_identity_manual = 0 AND color_identity IS NULL
             ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        for row in rows {
            deck_ids.push(row?);
        }
    }
    for deck_id in deck_ids {
        let versions = crate::decks::versions_of(tx, deck_id)?;
        let Some(latest) = versions.last() else {
            continue;
        };
        let cards = decks::decode_signature(&latest.signature);
        let catalog_ids: Vec<u32> = cards.iter().map(|card| card.catalog_id).collect();
        let infos = crate::decks::card_infos(tx, &catalog_ids)?;
        let info_of = |catalog_id: u32| {
            infos.get(&catalog_id).map(|card| decks::CardInfo {
                card_type: card.card_type.clone(),
                color_identity: card.color_identity.clone(),
            })
        };
        if let Some(identity) = decks::color_identity(&cards, &info_of) {
            tx.execute(
                "UPDATE decks SET color_identity = ?2 WHERE id = ?1",
                params![deck_id, identity],
            )?;
        }
    }
    Ok(())
}

/// Link the match to its v1 version. Nothing is written when the version was
/// not imported (Limited, legacy signature) or v1 has no row for the match
/// (still in progress in 0.x, or skipped for another reason).
fn import_link(
    tx: &Transaction,
    link: &ImportedLink,
    counts: &mut DeckImportCounts,
) -> Result<(), StoreError> {
    let version_id: Option<i64> = tx
        .prepare_cached(
            "SELECT v.id FROM deck_versions v JOIN decks d ON d.id = v.deck_id
             WHERE d.mtgo_id = ?1 AND v.signature = ?2",
        )?
        .query_row(params![link.deck_mtgo_id, link.signature], |row| row.get(0))
        .optional()?;
    let Some(version_id) = version_id else {
        return Ok(());
    };
    let Some(token) = link_token(tx, link)? else {
        return Ok(());
    };
    let inserted = tx
        .prepare_cached(
            "UPDATE matches SET deck_version_id = ?2
             WHERE token = ?1 AND deck_version_id IS NULL",
        )?
        .execute(params![token, version_id])?;
    if inserted == 1 {
        counts.links_added += 1;
        // The match's rows were imported before its deck: the imported
        // postboard rule needs the version.
        let match_id: i64 = tx
            .prepare_cached("SELECT id FROM matches WHERE token = ?1")?
            .query_row([&token], |row| row.get(0))?;
        reapply_rule_or_report(tx, match_id, counts)?;
    } else {
        counts.links_unchanged += 1;
    }
    Ok(())
}

/// Apply the imported postboard rule to a newly linked match. A match the
/// rule cannot be applied to (a stored game log that cannot be read; a
/// SQLite failure still fails the import) is logged and counted, its partial writes undone (a savepoint), and the rest
/// of the import goes on: its rows stay as imported.
fn reapply_rule_or_report(
    tx: &Transaction,
    match_id: i64,
    counts: &mut DeckImportCounts,
) -> Result<(), StoreError> {
    tx.execute_batch("SAVEPOINT reapply_rule")?;
    match crate::imported_detail::reapply_imported_rule(tx, match_id) {
        Ok(_) => tx.execute_batch("RELEASE reapply_rule")?,
        // A storage failure (busy, disk full) fails the import as before.
        Err(StoreError::Sqlite(error)) => {
            tx.execute_batch("ROLLBACK TO reapply_rule; RELEASE reapply_rule")?;
            return Err(StoreError::Sqlite(error));
        }
        Err(error) => {
            tx.execute_batch("ROLLBACK TO reapply_rule; RELEASE reapply_rule")?;
            tracing::warn!(%error, match_id, "the imported postboard rule was not applied");
            counts.rules_failed += 1;
        }
    }
    Ok(())
}

/// The v1 match the link belongs to: the live row with this token or MTGO
/// match ID, else the imported row with this token.
fn link_token(tx: &Transaction, link: &ImportedLink) -> Result<Option<String>, StoreError> {
    if let Some(live) = live_row_token(tx, &link.token, link.mtgo_match_id)? {
        return Ok(Some(live));
    }
    let imported: bool = tx
        .prepare_cached("SELECT EXISTS (SELECT 1 FROM matches WHERE token = ?1)")?
        .query_row([&link.token], |row| row.get(0))?;
    Ok(imported.then(|| link.token.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open_writer;
    use fixture_tools::TestDir;

    fn card(catalog_id: i64, name: &str) -> ImportedCard {
        ImportedCard {
            catalog_id,
            scryfall_id: None,
            oracle_id: Some(format!("q-oracle-{catalog_id}")),
            name: Some(name.to_string()),
            card_type: Some("Instant".to_string()),
            sub_type: None,
            rarity: None,
            color_identity: None,
            colors: None,
            cmc: None,
            mana_cost: None,
            set_name: None,
            set_code: None,
            image: None,
            art_crop: None,
        }
    }

    fn limited_deck(mtgo_id: &str, format: &str) -> ImportedDeck {
        ImportedDeck {
            mtgo_id: mtgo_id.to_string(),
            account: None,
            name: "Q Draft Deck".to_string(),
            original_name: None,
            format: format.to_string(),
            color_identity: None,
            cover_catalog_id: None,
            archived_at_ms: None,
            created_at_ms: Some(1),
            updated_at_ms: Some(1),
            versions: vec![ImportedVersion {
                signature: decks::signature(&[decks::DeckCard {
                    catalog_id: 12,
                    quantity: 40,
                    sideboard: false,
                }]),
                modified_at_ms: 1,
            }],
        }
    }

    // Issue #194: v1 keeps no deck rows for Limited (limited ruling 3), so
    // the 0.x deck import makes neither a deck nor versions for one, whatever
    // the reader hands it.
    #[test]
    fn a_limited_0x_deck_gets_no_deck_or_versions() {
        let dir = TestDir::new("import-decks-limited");
        let mut conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        let data = ImportedDecks {
            cards: Vec::new(),
            decks: vec![
                limited_deck("limited:q-draft", "DHOBHOBHOB"),
                limited_deck("q-sealed-1", "Limited"),
            ],
            links: Vec::new(),
        };
        let counts = import_decks(&mut conn, &data, 0).expect("import");
        assert_eq!((counts.decks_added, counts.versions_added), (0, 0));
        let rows: i64 = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM decks) + (SELECT COUNT(*) FROM deck_versions)",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(rows, 0);
    }

    #[test]
    fn filling_a_card_stub_counts_as_a_card_added() {
        // The 0.x import's details run before its decks, so a card may
        // already have a stub; its data still arrives with this import.
        let dir = TestDir::new("import-decks-stub");
        let mut conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        let tx = conn.transaction().expect("tx");
        crate::cards::ensure_stubs(
            &tx,
            &[
                crate::cards::CardRef::id(12),
                crate::cards::CardRef {
                    catalog_id: 13,
                    name: Some("Q Log Name".to_string()),
                },
            ],
        )
        .expect("stubs");
        tx.commit().expect("commit");
        let data = ImportedDecks {
            cards: vec![card(12, "Q Bolt"), card(13, "Q Wear")],
            decks: Vec::new(),
            links: Vec::new(),
        };
        let counts = import_decks(&mut conn, &data, 0).expect("import");
        assert_eq!((counts.cards_added, counts.cards_updated), (2, 0));
        let again = import_decks(&mut conn, &data, 0).expect("again");
        assert_eq!(again.cards_unchanged, 2);
    }

    #[test]
    fn a_nameless_0x_stub_never_erases_a_known_name_or_counts_as_added() {
        // The details import (run first) named a stub from 0.x's game log;
        // 0.x itself only had a nameless stub for the id.
        let dir = TestDir::new("import-decks-nameless-stub");
        let mut conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        let tx = conn.transaction().expect("tx");
        crate::cards::ensure_stubs(
            &tx,
            &[crate::cards::CardRef {
                catalog_id: 48556,
                name: Some("Tear".to_string()),
            }],
        )
        .expect("stub");
        tx.commit().expect("commit");
        conn.execute(
            "INSERT INTO cards (catalog_id, oracle_id, name, image)
             VALUES (7, 'q-oracle-7', 'Q Known', 'https://cards.scryfall.io/q.jpg')",
            [],
        )
        .expect("a known card");
        let nameless = |catalog_id| ImportedCard {
            name: None,
            oracle_id: None,
            card_type: None,
            ..card(catalog_id, "")
        };
        let data = ImportedDecks {
            cards: vec![nameless(48556), nameless(7)],
            decks: Vec::new(),
            links: Vec::new(),
        };
        let counts = import_decks(&mut conn, &data, 0).expect("import");
        assert_eq!(
            (
                counts.cards_added,
                counts.cards_updated,
                counts.cards_unchanged
            ),
            (0, 0, 2)
        );
        let row = |id: i64| -> (Option<String>, Option<String>, Option<String>) {
            conn.query_row(
                "SELECT name, oracle_id, image FROM cards WHERE catalog_id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("row")
        };
        assert_eq!(row(48556), (Some("Tear".to_string()), None, None));
        assert_eq!(
            row(7),
            (
                Some("Q Known".to_string()),
                Some("q-oracle-7".to_string()),
                Some("https://cards.scryfall.io/q.jpg".to_string())
            )
        );
        // A nameless 0.x stub for an id v1 has no row for is still added.
        let fresh = ImportedDecks {
            cards: vec![nameless(9)],
            decks: Vec::new(),
            links: Vec::new(),
        };
        assert_eq!(
            import_decks(&mut conn, &fresh, 0)
                .expect("fresh")
                .cards_added,
            1
        );
    }
}
