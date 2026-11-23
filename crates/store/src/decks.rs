//! Decks (decks spec sections 3 to 6): decks read from MTGO's deck files, their
//! versions, card data, and the deck version each match used.
//!
//! Two kinds of write:
//! - `DeckWrite` (a deck file sync, a match link) travels in a `WriteBatch`,
//!   like every other tracker write. Nothing waits for it.
//! - `DeckEdit` (rename, colour identity, cover, archive, restore, overlay
//!   label) is a user
//!   action whose caller needs an answer ("done" or "refused: archived"), so it
//!   goes through `WriterHandle::edit_deck`, which commits it in its own
//!   transaction and replies with an `EditOutcome` (like `WriterHandle::import`).

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::write::{decode_raw, registry_id};
use crate::{Platform, StoreError};
use decks::DeckCard;

/// A tracker write about decks, applied inside the batch transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeckWrite {
    /// One parsed deck file (spec 3, per file). `signature` is the file's card
    /// list signature, `account` the local account to give a deck that has
    /// none, `identity` what to do with the stored colour identity.
    SyncFile {
        file: decks::DeckFile,
        signature: String,
        account: Option<String>,
        /// The account's platform: a name not stored is made on it (#862).
        platform: Platform,
        synced_at_ms: i64,
        identity: IdentityUpdate,
    },
    /// The deck list a platform match was played with (Endstep), which can
    /// be folded after a newer match of the same deck. Like `SyncFile`, but
    /// a list at or before the deck's last write still becomes a version
    /// (only the version: the deck's columns and its other versions stay),
    /// so every match can link to the list it was played with. Arena reads
    /// its log in order and keeps `SyncFile`.
    SyncUse {
        file: decks::DeckFile,
        signature: String,
        account: Option<String>,
        platform: Platform,
        synced_at_ms: i64,
        identity: IdentityUpdate,
    },
    /// An unchanged deck file whose deck has no account yet: give it the local
    /// account (0.x `SyncDecks` backfills `account_id` on every synced deck).
    /// A deck that has an account keeps it.
    Adopt {
        mtgo_id: String,
        account: String,
        at_ms: i64,
    },
    /// A local account was seen for the first time: when it is the only
    /// account, every deck without one (archived included) becomes its (0.x
    /// `AdoptOrphanDecks`). With two accounts an orphan could be either's, so
    /// nothing changes.
    AdoptOrphans { account: String, at_ms: i64 },
    /// The deck version a match used (`matches.deck_version_id`, set or
    /// replaced). Sent after the match upsert in the same batch. Skipped when
    /// the version no longer exists (a later sync pruned it): the next relink
    /// retries, and a foreign key failure would stop the writer.
    Link {
        match_token: String,
        deck_version_id: i64,
    },
    /// Archive the deck `mtgo_id` (#867) when no match uses any of its
    /// versions and it holds nothing of the user's or of the sync. Sent by
    /// a source after the link that moved a match off a throwaway deck.
    ArchiveUnused { mtgo_id: String, at_ms: i64 },
}

/// What a deck file sync does to the stored colour identity (0.x
/// `ComputeDeckIdentity`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityUpdate {
    /// Leave it as stored: the file has no cards, and 0.x returns early then.
    Keep,
    /// Store this identity; `None` stores NULL (no colour reached 4 cards).
    Set(Option<String>),
}

/// A user edit of one deck (spec 6). Every edit except `Restore` and
/// `SetOverlayLabel` is refused on an archived deck.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeckEdit {
    Rename {
        deck_id: i64,
        name: String,
    },
    /// A manual colour identity, already normalised by
    /// `decks::normalize_manual_identity` (`None` for no colours).
    SetIdentity {
        deck_id: i64,
        identity: Option<String>,
    },
    SetCover {
        deck_id: i64,
        catalog_id: Option<u32>,
    },
    /// The deck's own cover image: the name of a file v1 wrote in the deck
    /// covers folder, or `None` to go back to the cover card. Local only.
    SetCoverFile {
        deck_id: i64,
        file: Option<String>,
    },
    Archive {
        deck_id: i64,
        at_ms: i64,
    },
    Restore {
        deck_id: i64,
    },
    /// The streamer's name for the deck on the league card (overlay spec
    /// 9.2, 0.x `AppSettings::setOverlayDeckLabel`): trimmed, the first
    /// `MAX_OVERLAY_LABEL_CHARS` characters kept, blank or `None` clears it.
    /// Allowed on an archived deck (a display setting, ruling C4).
    SetOverlayLabel {
        deck_id: i64,
        label: Option<String>,
    },
}

/// 0.x's refusal for any change to a deck deleted on MTGO (v1: archived),
/// shared by every command that refuses one.
pub const ARCHIVED_DECK: &str = "This deck has been deleted on MTGO and is read-only.";

/// What happened to a `DeckEdit`. Only `Done` changed anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOutcome {
    Done,
    /// Refused: the deck is archived and read-only.
    Archived,
    /// Refused: there is no deck with this id.
    NotFound,
    /// Refused: a name must have 1 to 255 characters after trimming.
    InvalidName,
}

impl EditOutcome {
    /// The message to show for a refusal, `None` for `Done`.
    pub fn refusal(self) -> Option<&'static str> {
        match self {
            EditOutcome::Done => None,
            EditOutcome::Archived => Some(ARCHIVED_DECK),
            EditOutcome::NotFound => Some("This deck no longer exists."),
            EditOutcome::InvalidName => Some("A deck name needs 1 to 255 characters."),
        }
    }
}

/// The longest deck name, in characters (0.x `max:255`).
pub const MAX_NAME_CHARS: usize = 255;

/// The longest overlay deck label kept, in characters (0.x `mb_substr(.., 0, 40)`).
pub const MAX_OVERLAY_LABEL_CHARS: usize = 40;

/// Apply one tracker write inside the batch transaction (`write::commit`).
pub(crate) fn apply_deck_write(tx: &Transaction, write: &DeckWrite) -> Result<(), StoreError> {
    match write {
        DeckWrite::SyncFile {
            file,
            signature,
            account,
            platform,
            synced_at_ms,
            identity,
        } => sync_file(
            tx,
            file,
            signature,
            account.as_deref().map(|name| (name, *platform)),
            *synced_at_ms,
            identity,
            OlderList::Skip,
        ),
        DeckWrite::SyncUse {
            file,
            signature,
            account,
            platform,
            synced_at_ms,
            identity,
        } => sync_file(
            tx,
            file,
            signature,
            account.as_deref().map(|name| (name, *platform)),
            *synced_at_ms,
            identity,
            OlderList::AddVersion,
        ),
        DeckWrite::Adopt {
            mtgo_id,
            account,
            at_ms,
        } => adopt(tx, mtgo_id, account, *at_ms),
        DeckWrite::AdoptOrphans { account, at_ms } => adopt_orphans(tx, account, *at_ms),
        DeckWrite::Link {
            match_token,
            deck_version_id,
        } => {
            tx.execute(LINK, params![match_token, deck_version_id])?;
            Ok(())
        }
        DeckWrite::ArchiveUnused { mtgo_id, at_ms } => archive_unused(tx, mtgo_id, *at_ms),
    }
}

/// Set or replace a match's link, only while its version exists.
const LINK: &str = "UPDATE matches SET deck_version_id = ?2
     WHERE token = ?1 AND EXISTS (SELECT 1 FROM deck_versions WHERE id = ?2)";

/// Archive a deck (#867) only when no match uses any of its versions and it
/// holds nothing of the user's: no rename (`original_name`), cover card or
/// cover file, overlay label or hand-set colours, no sideboard guide or
/// note, no league on it or on a version. The archetype is not checked: the
/// prefill and detection write `decks.archetype_id` too (manual archetypes
/// included), so a hand-set one cannot be told apart, and archiving keeps
/// it (Restore brings the deck back as it was). Nor anything of
/// the sync: no slot, no `synced_rows` row, not pulled or imported. Never
/// deleted (a delete would cascade guides, notes and versions and orphan a
/// synced copy), and an archived deck keeps its archive time.
const ARCHIVE_UNUSED: &str = "UPDATE decks SET archived_at = ?2
     WHERE id = ?1 AND archived_at IS NULL
       AND original_name IS NULL AND cover_catalog_id IS NULL AND cover_file IS NULL
       AND overlay_label IS NULL AND color_identity_manual = 0
       AND cloud_sync_enabled = 0 AND pulled_platform IS NULL AND source = 'file'
       AND NOT EXISTS (SELECT 1 FROM matches m JOIN deck_versions v ON v.id = m.deck_version_id
                       WHERE v.deck_id = ?1)
       AND NOT EXISTS (SELECT 1 FROM sideboard_guides WHERE deck_id = ?1)
       AND NOT EXISTS (SELECT 1 FROM deck_archetype_notes WHERE deck_id = ?1)
       AND NOT EXISTS (SELECT 1 FROM leagues WHERE deck_id = ?1)
       AND NOT EXISTS (SELECT 1 FROM leagues l JOIN deck_versions v ON v.id = l.deck_version_id
                       WHERE v.deck_id = ?1)
       AND NOT EXISTS (SELECT 1 FROM synced_rows WHERE type = 'deck' AND client_id = ?3)";

/// `DeckWrite::ArchiveUnused`: nothing when the deck does not exist or does
/// not qualify (`ARCHIVE_UNUSED`).
fn archive_unused(tx: &Transaction, mtgo_id: &str, at_ms: i64) -> Result<(), StoreError> {
    let deck_id: Option<i64> = tx
        .query_row(
            "SELECT id FROM decks WHERE mtgo_id = ?1",
            [mtgo_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(deck_id) = deck_id else {
        return Ok(());
    };
    let client_id = sync_bundles::deck_client_id(mtgo_id);
    tx.execute(ARCHIVE_UNUSED, params![deck_id, at_ms, client_id])?;
    Ok(())
}

/// What a file sync needs to know about a stored deck.
struct StoredDeck {
    id: i64,
    account_id: Option<i64>,
    file_synced_at: Option<i64>,
}

/// A deck file changed: update the deck from it. The name is left alone once
/// the user renamed the deck (`original_name` set). A deck imported from 0.x
/// becomes a file deck, so a later import keeps the file's data. The colour
/// identity is written apart (`IdentityUpdate`).
const UPDATE_FROM_FILE: &str = "UPDATE decks SET format = ?3, account_id = ?4,
         updated_at = ?5, file_synced_at = ?5,
         name = CASE WHEN original_name IS NULL OR original_name = '' THEN ?2 ELSE name END,
         source = 'file'
     WHERE id = ?1";

const INSERT_FROM_FILE: &str = "INSERT INTO decks
         (mtgo_id, account_id, name, format, created_at, updated_at, file_synced_at, source)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, 'file')
     RETURNING id";

/// A deck's other versions that no match uses (0.x prunes them on every
/// changed file: the player was editing the list).
pub(crate) const PRUNE_VERSIONS: &str = "DELETE FROM deck_versions
     WHERE deck_id = ?1 AND id <> ?2
       AND NOT EXISTS (SELECT 1 FROM matches m WHERE m.deck_version_id = deck_versions.id)";

/// What a deck sync does with a list at or before the deck's last write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OlderList {
    /// Nothing: a deck file read again (`SyncFile`).
    Skip,
    /// Add it as a version only (`SyncUse`).
    AddVersion,
}

/// One deck file (decks spec 3, per file; 0.x `SyncDecks`).
fn sync_file(
    tx: &Transaction,
    file: &decks::DeckFile,
    signature: &str,
    account: Option<(&str, Platform)>,
    synced_at_ms: i64,
    identity: &IdentityUpdate,
    older: OlderList,
) -> Result<(), StoreError> {
    let stored = tx
        .query_row(
            "SELECT id, account_id, file_synced_at FROM decks WHERE mtgo_id = ?1",
            [&file.net_deck_id],
            |row| {
                Ok(StoredDeck {
                    id: row.get(0)?,
                    account_id: row.get(1)?,
                    file_synced_at: row.get(2)?,
                })
            },
        )
        .optional()?;

    // Already read at this file timestamp or a newer one. Like 0.x, the stored
    // value is the file's own Timestamp, so the comparison is like with like.
    if let Some(deck) = &stored
        && let Some(synced) = deck.file_synced_at
        && synced >= file.timestamp_ms
    {
        if older == OlderList::AddVersion {
            add_version(tx, deck.id, signature, file)?;
        }
        return Ok(());
    }

    // The deck's own account wins; otherwise the local account, if known.
    let account_id = match stored.as_ref().and_then(|deck| deck.account_id) {
        Some(id) => Some(id),
        None => match account {
            Some((name, platform)) => Some(crate::arena::local_account_id(
                tx,
                name,
                platform,
                synced_at_ms,
            )?),
            None => None,
        },
    };

    let deck_id = match stored {
        Some(deck) => {
            tx.execute(
                UPDATE_FROM_FILE,
                params![
                    deck.id,
                    file.name,
                    file.format,
                    account_id,
                    file.timestamp_ms
                ],
            )?;
            deck.id
        }
        None => tx.query_row(
            INSERT_FROM_FILE,
            params![
                file.net_deck_id,
                account_id,
                file.name,
                file.format,
                synced_at_ms,
                file.timestamp_ms
            ],
            |row| row.get(0),
        )?,
    };

    // A list the deck had before (a revert) reuses that version and keeps its
    // modified_at.
    tx.execute(
        "INSERT INTO deck_versions (deck_id, signature, modified_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (deck_id, signature) DO NOTHING",
        params![deck_id, signature, file.timestamp_ms],
    )?;
    let version_id: i64 = tx.query_row(
        "SELECT id FROM deck_versions WHERE deck_id = ?1 AND signature = ?2",
        params![deck_id, signature],
        |row| row.get(0),
    )?;
    tx.execute(PRUNE_VERSIONS, params![deck_id, version_id])?;
    // Every card of the file gets a `cards` row (0.x `GenerateDeckSignature`).
    let catalog_ids: Vec<u32> = file.cards.iter().map(|card| card.catalog_id).collect();
    crate::cards::ensure_id_stubs(tx, &catalog_ids)?;

    // A computed identity replaces a manual one (0.x recomputes on every
    // changed file); a file without cards keeps both the value and the flag.
    if let IdentityUpdate::Set(identity) = identity {
        tx.execute(
            "UPDATE decks SET color_identity = ?2, color_identity_manual = 0 WHERE id = ?1",
            params![deck_id, identity],
        )?;
    }
    Ok(())
}

/// Add `signature` as a version of the deck `deck_id` (kept when the deck
/// has it already), with stubs for its cards, and nothing else.
fn add_version(
    tx: &Transaction,
    deck_id: i64,
    signature: &str,
    file: &decks::DeckFile,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO deck_versions (deck_id, signature, modified_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (deck_id, signature) DO NOTHING",
        params![deck_id, signature, file.timestamp_ms],
    )?;
    let catalog_ids: Vec<u32> = file.cards.iter().map(|card| card.catalog_id).collect();
    crate::cards::ensure_id_stubs(tx, &catalog_ids)?;
    Ok(())
}

/// The first MTGO account takes every MTGO deck without one
/// (`DeckWrite::AdoptOrphans`). Arena and Endstep accounts are not counted.
/// A deck without an account is MTGO's unless `pulled_platform` names its
/// platform: an Arena or Endstep deck pulled before this device had an
/// account of its platform waits for one (cross-device sync spec 7).
fn adopt_orphans(tx: &Transaction, account: &str, at_ms: i64) -> Result<(), StoreError> {
    let account_id = registry_id(tx, "accounts", account, at_ms)?;
    let accounts: i64 = tx.query_row(
        "SELECT COUNT(*) FROM accounts WHERE platform = 'mtgo'",
        [],
        |row| row.get(0),
    )?;
    if accounts > 1 {
        return Ok(());
    }
    tx.execute(
        "UPDATE decks SET account_id = ?1 WHERE account_id IS NULL AND pulled_platform IS NULL",
        [account_id],
    )?;
    Ok(())
}

/// Give a deck without an account the local account (`DeckWrite::Adopt`).
fn adopt(tx: &Transaction, mtgo_id: &str, account: &str, at_ms: i64) -> Result<(), StoreError> {
    let stored: Option<Option<i64>> = tx
        .query_row(
            "SELECT account_id FROM decks WHERE mtgo_id = ?1",
            [mtgo_id],
            |row| row.get(0),
        )
        .optional()?;
    // Only a deck that exists and has no account; the account row is only
    // registered when it is used.
    if stored != Some(None) {
        return Ok(());
    }
    let account_id = registry_id(tx, "accounts", account, at_ms)?;
    tx.execute(
        "UPDATE decks SET account_id = ?2
         WHERE mtgo_id = ?1 AND account_id IS NULL AND pulled_platform IS NULL",
        params![mtgo_id, account_id],
    )?;
    Ok(())
}

impl DeckEdit {
    pub fn deck_id(&self) -> i64 {
        match self {
            DeckEdit::Rename { deck_id, .. }
            | DeckEdit::SetIdentity { deck_id, .. }
            | DeckEdit::SetCover { deck_id, .. }
            | DeckEdit::SetCoverFile { deck_id, .. }
            | DeckEdit::Archive { deck_id, .. }
            | DeckEdit::Restore { deck_id }
            | DeckEdit::SetOverlayLabel { deck_id, .. } => *deck_id,
        }
    }
}

/// Apply one user edit in its own transaction (on the writer thread). A
/// refused edit changes nothing.
pub fn edit_deck(conn: &mut Connection, edit: &DeckEdit) -> Result<EditOutcome, StoreError> {
    let tx = conn.transaction()?;
    let outcome = apply_edit(&tx, edit)?;
    tx.commit()?;
    Ok(outcome)
}

fn apply_edit(tx: &Transaction, edit: &DeckEdit) -> Result<EditOutcome, StoreError> {
    let archived_at: Option<Option<i64>> = tx
        .query_row(
            "SELECT archived_at FROM decks WHERE id = ?1",
            [edit.deck_id()],
            |row| row.get(0),
        )
        .optional()?;
    let Some(archived_at) = archived_at else {
        return Ok(EditOutcome::NotFound);
    };
    let allowed_when_archived = matches!(
        edit,
        DeckEdit::Restore { .. } | DeckEdit::SetOverlayLabel { .. }
    );
    if archived_at.is_some() && !allowed_when_archived {
        return Ok(EditOutcome::Archived);
    }

    match edit {
        DeckEdit::Rename { deck_id, name } => return rename(tx, *deck_id, name),
        DeckEdit::SetIdentity { deck_id, identity } => {
            tx.execute(
                "UPDATE decks SET color_identity = ?2, color_identity_manual = 1 WHERE id = ?1",
                params![deck_id, identity],
            )?;
        }
        DeckEdit::SetCover {
            deck_id,
            catalog_id,
        } => {
            tx.execute(
                "UPDATE decks SET cover_catalog_id = ?2 WHERE id = ?1",
                params![deck_id, catalog_id],
            )?;
        }
        DeckEdit::SetCoverFile { deck_id, file } => {
            tx.execute(
                "UPDATE decks SET cover_file = ?2 WHERE id = ?1",
                params![deck_id, file],
            )?;
        }
        DeckEdit::Archive { deck_id, at_ms } => {
            tx.execute(
                "UPDATE decks SET archived_at = ?2 WHERE id = ?1",
                params![deck_id, at_ms],
            )?;
        }
        DeckEdit::Restore { deck_id } => {
            tx.execute(
                "UPDATE decks SET archived_at = NULL WHERE id = ?1",
                [deck_id],
            )?;
        }
        DeckEdit::SetOverlayLabel { deck_id, label } => {
            tx.execute(
                "UPDATE decks SET overlay_label = ?2 WHERE id = ?1",
                params![deck_id, overlay_label(label.as_deref())],
            )?;
        }
    }
    Ok(EditOutcome::Done)
}

/// A label as stored: trimmed, at most `MAX_OVERLAY_LABEL_CHARS`
/// characters, `None` when blank.
pub(crate) fn overlay_label(label: Option<&str>) -> Option<String> {
    let kept: String = label
        .unwrap_or("")
        .trim()
        .chars()
        .take(MAX_OVERLAY_LABEL_CHARS)
        .collect();
    if kept.is_empty() { None } else { Some(kept) }
}

/// 0.x `UpdateNameController`: the first rename keeps the file's name in
/// `original_name` (so a sync stops overwriting it); renaming back to it
/// clears it (the file drives the name again).
fn rename(tx: &Transaction, deck_id: i64, name: &str) -> Result<EditOutcome, StoreError> {
    let new_name = name.trim();
    let length = new_name.chars().count();
    if length == 0 || length > MAX_NAME_CHARS {
        return Ok(EditOutcome::InvalidName);
    }
    let (current, original): (String, Option<String>) = tx.query_row(
        "SELECT name, original_name FROM decks WHERE id = ?1",
        [deck_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if new_name == current {
        return Ok(EditOutcome::Done);
    }
    let original = match original {
        Some(text) if !text.is_empty() => text,
        _ => current,
    };
    let original = if original == new_name {
        None
    } else {
        Some(original)
    };
    tx.execute(
        "UPDATE decks SET name = ?2, original_name = ?3 WHERE id = ?1",
        params![deck_id, new_name, original],
    )?;
    Ok(EditOutcome::Done)
}

/// One `decks` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckRow {
    pub id: i64,
    pub mtgo_id: String,
    pub account_id: Option<i64>,
    pub name: String,
    pub original_name: Option<String>,
    pub format: String,
    pub color_identity: Option<String>,
    pub color_identity_manual: bool,
    pub cover_catalog_id: Option<u32>,
    /// The deck's own cover image, a file name in the deck covers folder.
    pub cover_file: Option<String>,
    pub archived_at: Option<i64>,
    pub file_synced_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    /// "file" | "imported"
    pub source: String,
}

/// One `deck_versions` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionRow {
    pub id: i64,
    pub deck_id: i64,
    pub signature: String,
    pub modified_at: i64,
}

/// One `cards` row.
#[derive(Debug, Clone, PartialEq)]
pub struct CardRow {
    pub catalog_id: u32,
    pub oracle_id: Option<String>,
    pub name: Option<String>,
    /// The `type` column.
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
    /// The downloaded files in v1's images folder (`{id}.jpg`,
    /// `{id}_art.jpg`), when local images are on.
    pub local_image: Option<String>,
    pub local_art_crop: Option<String>,
}

/// A match with no deck link yet, and the Game IDs of its games.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnlinkedMatch {
    pub token: String,
    /// The local account's username.
    pub account: String,
    /// One entry per game in game order, so the first is always game 1, even
    /// when game 1 has no Game ID (`None`, a synthetic forfeit game).
    pub game_ids: Vec<Option<u64>>,
    pub tournament_event_id: Option<u64>,
}

const DECK_COLUMNS: &str = "id, mtgo_id, account_id, name, original_name, format, color_identity,
     color_identity_manual, cover_catalog_id, archived_at, file_synced_at, created_at, updated_at,
     source, cover_file";

fn deck_row(row: &rusqlite::Row) -> rusqlite::Result<DeckRow> {
    Ok(DeckRow {
        id: row.get(0)?,
        mtgo_id: row.get(1)?,
        account_id: row.get(2)?,
        name: row.get(3)?,
        original_name: row.get(4)?,
        format: row.get(5)?,
        color_identity: row.get(6)?,
        color_identity_manual: row.get(7)?,
        cover_catalog_id: row.get(8)?,
        archived_at: row.get(9)?,
        file_synced_at: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
        source: row.get(13)?,
        cover_file: row.get(14)?,
    })
}

/// The deck with this MTGO id (`NetDeckId`), archived or not.
pub fn deck_by_mtgo_id(conn: &Connection, mtgo_id: &str) -> Result<Option<DeckRow>, StoreError> {
    Ok(conn
        .query_row(
            &format!("SELECT {DECK_COLUMNS} FROM decks WHERE mtgo_id = ?1"),
            [mtgo_id],
            deck_row,
        )
        .optional()?)
}

/// The deck with this id, archived or not.
pub fn deck_by_id(conn: &Connection, id: i64) -> Result<Option<DeckRow>, StoreError> {
    Ok(conn
        .query_row(
            &format!("SELECT {DECK_COLUMNS} FROM decks WHERE id = ?1"),
            [id],
            deck_row,
        )
        .optional()?)
}

pub(crate) const VERSIONS_OF: &str = "SELECT id, deck_id, signature, modified_at FROM deck_versions
     WHERE deck_id = ?1 ORDER BY modified_at, id";

/// A commander format deck's default cover card: its latest version's first
/// commander (the first sideboard row; the first of partners). `None` for
/// any other format and for a deck without versions or commanders.
pub fn commander_cover(
    conn: &Connection,
    deck_id: i64,
    format: &str,
) -> Result<Option<u32>, StoreError> {
    if !crate::match_commanders::is_commander_format(Some(format)) {
        return Ok(None);
    }
    let Some(latest) = versions_of(conn, deck_id)?.pop() else {
        return Ok(None);
    };
    let rows = decks::decode_signature(&latest.signature);
    Ok(rows
        .iter()
        .find(|row| row.sideboard)
        .map(|row| row.catalog_id))
}

/// A deck's versions, oldest first.
pub fn versions_of(conn: &Connection, deck_id: i64) -> Result<Vec<VersionRow>, StoreError> {
    let mut stmt = conn.prepare_cached(VERSIONS_OF)?;
    let rows = stmt.query_map([deck_id], version_row)?;
    let mut versions = Vec::new();
    for row in rows {
        versions.push(row?);
    }
    Ok(versions)
}

fn version_row(row: &rusqlite::Row) -> rusqlite::Result<VersionRow> {
    Ok(VersionRow {
        id: row.get(0)?,
        deck_id: row.get(1)?,
        signature: row.get(2)?,
        modified_at: row.get(3)?,
    })
}

/// The card rows of these catalog ids. An id with no row is left out, and so
/// is a stub without a name (`store::cards`): it is an unknown card, shown as
/// "Card {id}", until the cards worker fills it.
pub fn card_infos(
    conn: &Connection,
    catalog_ids: &[u32],
) -> Result<HashMap<u32, CardRow>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT catalog_id, oracle_id, name, type, sub_type, rarity, color_identity, colors, cmc,
             mana_cost, set_name, set_code, image, art_crop, local_image, local_art_crop
         FROM cards WHERE catalog_id = ?1 AND name IS NOT NULL",
    )?;
    let mut cards = HashMap::new();
    for &catalog_id in catalog_ids {
        if cards.contains_key(&catalog_id) {
            continue;
        }
        let card = stmt
            .query_row([catalog_id], |row| {
                Ok(CardRow {
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
                    local_image: row.get(14)?,
                    local_art_crop: row.get(15)?,
                })
            })
            .optional()?;
        if let Some(card) = card {
            cards.insert(catalog_id, card);
        }
    }
    Ok(cards)
}

/// The oracle id of each catalog id that has one (not empty).
pub fn oracle_ids(
    conn: &Connection,
    catalog_ids: &[u32],
) -> Result<HashMap<u32, String>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT oracle_id FROM cards
         WHERE catalog_id = ?1 AND oracle_id IS NOT NULL AND oracle_id <> ''",
    )?;
    let mut oracles = HashMap::new();
    for &catalog_id in catalog_ids {
        let oracle: Option<String> = stmt.query_row([catalog_id], |row| row.get(0)).optional()?;
        if let Some(oracle) = oracle {
            oracles.insert(catalog_id, oracle);
        }
    }
    Ok(oracles)
}

/// A card a name search found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardSearchRow {
    pub catalog_id: u32,
    pub oracle_id: Option<String>,
    pub name: String,
    pub image: Option<String>,
    pub local_image: Option<String>,
    /// The `type` column.
    pub card_type: Option<String>,
}

/// 0.x `SearchCards` (the manual reveals picker): cards whose name contains
/// `term` (trimmed, any case, `%` and `_` taken literally), one printing per
/// oracle id (the highest catalog id; a card with no oracle id is its own),
/// by name, at most `limit`. A stub without an oracle id whose name a
/// resolved card has is left out: v1 names stubs from the game log, which
/// 0.x never did, and the resolved card stands for it.
pub fn search_cards(
    conn: &Connection,
    term: &str,
    limit: u32,
) -> Result<Vec<CardSearchRow>, StoreError> {
    let escaped = term
        .trim()
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let pattern = format!("%{escaped}%");
    let mut stmt = conn.prepare_cached(
        "SELECT catalog_id, oracle_id, name, image, type, local_image FROM cards
         WHERE catalog_id IN (
             SELECT MAX(catalog_id) FROM cards c
             WHERE name LIKE ?1 ESCAPE '\\'
                 AND (oracle_id IS NOT NULL OR NOT EXISTS (
                     SELECT 1 FROM cards r WHERE r.name = c.name AND r.oracle_id IS NOT NULL))
             GROUP BY COALESCE(oracle_id, 'mtgo:' || catalog_id))
         ORDER BY name, catalog_id
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![pattern, limit], |row| {
        Ok(CardSearchRow {
            catalog_id: row.get(0)?,
            oracle_id: row.get(1)?,
            name: row.get(2)?,
            image: row.get(3)?,
            local_image: row.get(5)?,
            card_type: row.get(4)?,
        })
    })?;
    let mut found = Vec::new();
    for row in rows {
        found.push(row?);
    }
    Ok(found)
}

pub(crate) const ACCOUNT_VERSIONS: &str = "SELECT v.id, v.deck_id, v.signature, v.modified_at
     FROM accounts a
     JOIN decks d ON d.account_id = a.id
     JOIN deck_versions v ON v.deck_id = d.id
     WHERE a.username = ?1";

/// Every version of every deck of this account (an untagged Arena name is
/// the account holding its user id), archived decks included, in id order
/// (the order 0.x's unordered `first()` sees).
pub fn account_versions(conn: &Connection, account: &str) -> Result<Vec<VersionRow>, StoreError> {
    let mut stmt = conn.prepare_cached(ACCOUNT_VERSIONS)?;
    let account = crate::arena::platform_account_username(conn, account)?;
    let rows = stmt.query_map([account], version_row)?;
    let mut versions = Vec::new();
    for row in rows {
        versions.push(row?);
    }
    versions.sort_by_key(|version| version.id);
    Ok(versions)
}

/// 0.x `RelinkOrphanMatches`: MTGO matches in progress, ended or complete,
/// started at or after `?1`, with no link and at least one game, newest
/// first. Arena matches link through their own source.
pub(crate) const UNLINKED_MATCHES_SINCE: &str = "
SELECT m.id, m.token, a.username, m.tournament_event_id
FROM listed_matches m JOIN accounts a ON a.id = m.account_id AND a.platform = 'mtgo'
WHERE m.started_at >= ?1
  AND m.state IN ('in_progress', 'ended', 'complete')
  AND m.deck_version_id IS NULL
  AND EXISTS (SELECT 1 FROM games g WHERE g.match_id = m.id)
ORDER BY m.started_at DESC, m.id DESC
LIMIT ?2";

/// The Game IDs of a match's games, in game order.
pub(crate) const GAME_IDS_OF: &str =
    "SELECT mtgo_game_id FROM games WHERE match_id = ?1 ORDER BY game_number, id";

/// At most `limit` unlinked matches (see `UNLINKED_MATCHES_SINCE`), leaving
/// out the tokens in `skip` before the limit applies, so matches already
/// known not to link cannot crowd out older ones that might.
pub fn unlinked_matches_since(
    conn: &Connection,
    since_ms: i64,
    limit: u32,
    skip: &HashSet<String>,
) -> Result<Vec<UnlinkedMatch>, StoreError> {
    let rows_wanted = i64::from(limit) + skip.len() as i64;
    let mut stmt = conn.prepare_cached(UNLINKED_MATCHES_SINCE)?;
    let rows = stmt.query_map(params![since_ms, rows_wanted], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<i64>>(3)?,
        ))
    })?;
    let mut found = Vec::new();
    for row in rows {
        let row = row?;
        if skip.contains(&row.1) {
            continue;
        }
        found.push(row);
        if found.len() >= limit as usize {
            break;
        }
    }

    let mut games = conn.prepare_cached(GAME_IDS_OF)?;
    let mut matches = Vec::new();
    for (match_id, token, account, tournament_event_id) in found {
        let ids = games.query_map([match_id], |row| row.get::<_, Option<i64>>(0))?;
        let mut game_ids = Vec::new();
        for id in ids {
            game_ids.push(id?.map(|id| id as u64));
        }
        matches.push(UnlinkedMatch {
            token,
            account,
            game_ids,
            tournament_event_id: tournament_event_id.map(|id| id as u64),
        });
    }
    Ok(matches)
}

/// An unlinked tournament round and the one deck version its account's
/// other rounds of the same tournament are linked to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiblingVersion {
    pub token: String,
    pub deck_version_id: i64,
}

/// 0.x `RelinkOrphanMatches::inheritTournamentDecks`: the newest unlinked
/// tournament matches (at most `?1`, any state, no date window), each with
/// how many distinct versions its account's rounds of the same tournament
/// are linked to, and the smallest of them. Only tournament matches are
/// walked (the partial index holds nothing else), not every match by start.
pub(crate) const TOURNAMENT_SIBLING_VERSIONS: &str = "
SELECT m.token, COUNT(DISTINCT s.deck_version_id), MIN(s.deck_version_id)
FROM (SELECT id, token, account_id, tournament_event_id, started_at
      FROM matches INDEXED BY matches_account_tournament
      WHERE tournament_event_id IS NOT NULL
        AND deck_version_id IS NULL
      ORDER BY started_at DESC, id DESC
      LIMIT ?1) m
LEFT JOIN matches s
    ON s.account_id = m.account_id AND s.tournament_event_id = m.tournament_event_id
GROUP BY m.id
ORDER BY m.started_at DESC, m.id DESC";

/// Unlinked tournament rounds that can take a version from their rounds:
/// those whose account's linked rounds of the same tournament all use one
/// version (MTGO locks the registered list for the whole event). Of the
/// newest `limit` unlinked rounds only, as 0.x.
pub fn tournament_sibling_versions(
    conn: &Connection,
    limit: u32,
) -> Result<Vec<SiblingVersion>, StoreError> {
    let mut stmt = conn.prepare_cached(TOURNAMENT_SIBLING_VERSIONS)?;
    let rows = stmt.query_map([limit], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, Option<i64>>(2)?,
        ))
    })?;
    let mut found = Vec::new();
    for row in rows {
        let (token, versions, version) = row?;
        if versions == 1
            && let Some(deck_version_id) = version
        {
            found.push(SiblingVersion {
                token,
                deck_version_id,
            });
        }
    }
    Ok(found)
}

pub(crate) const DECK_USED_RAW: &str =
    "SELECT log_file_id, byte_offset, raw, compressed FROM log_events
     WHERE game_id = ?1 AND kind = 'deck_used' AND quarantined = 0";

/// The raw text of the first `deck_used` event (in log order) for this Game
/// ID still in `log_events`, decoded. 0.x takes the first one too. A
/// compacted match's events are in its archive, not here.
pub fn deck_used_raw(conn: &Connection, game_id: u64) -> Result<Option<String>, StoreError> {
    let mut stmt = conn.prepare_cached(DECK_USED_RAW)?;
    let rows = stmt.query_map([game_id as i64], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, Vec<u8>>(2)?,
            row.get::<_, bool>(3)?,
        ))
    })?;
    // A game has few such rows (MTGO logs one per start); `first_raw` keeps
    // only the earliest, so a hostile log with many costs no more memory.
    first_raw(rows)
}

pub(crate) const REGISTERED_DECK_RAW: &str = "SELECT log_file_id, byte_offset, raw, compressed
     FROM log_events
     WHERE match_token = ?1 AND kind = 'match_deck_registered' AND quarantined = 0";

/// The raw text of the first registered deck event (`MatchDeckRegistered`,
/// in log order) for this match token still in `log_events`, decoded. MTGO
/// logs it before each sideboarding, so the first one carries the list game 1
/// was played with.
pub fn registered_deck_raw(conn: &Connection, token: &str) -> Result<Option<String>, StoreError> {
    let mut stmt = conn.prepare_cached(REGISTERED_DECK_RAW)?;
    let rows = stmt.query_map([token], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, Vec<u8>>(2)?,
            row.get::<_, bool>(3)?,
        ))
    })?;
    first_raw(rows)
}

/// The tournament form of `match_deck_registered`: the only rows of the kind
/// with no match token, one per login during a tournament.
pub(crate) const TOURNAMENT_DECK_RAWS: &str = "SELECT log_file_id, byte_offset, raw, compressed
     FROM log_events
     WHERE match_token IS NULL AND kind = 'match_deck_registered' AND quarantined = 0";

/// The list of the first tournament deck line (in log order) still in
/// `log_events` for this tournament (`matches.tournament_event_id`) that
/// carries cards (`decks::parse_tournament_deck`).
pub fn tournament_deck(
    conn: &Connection,
    tournament_event_id: u64,
) -> Result<Option<Vec<DeckCard>>, StoreError> {
    let mut stmt = conn.prepare_cached(TOURNAMENT_DECK_RAWS)?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, Vec<u8>>(2)?,
            row.get::<_, bool>(3)?,
        ))
    })?;
    let mut first: Option<((i64, i64), Vec<DeckCard>)> = None;
    for row in rows {
        let (file, offset, raw, compressed) = row?;
        let earlier = first.as_ref().is_none_or(|(at, _)| (file, offset) < *at);
        if !earlier {
            continue;
        }
        if let Some((id, cards)) = decks::parse_tournament_deck(&decode_raw(raw, compressed)?)
            && id == tournament_event_id
        {
            first = Some(((file, offset), cards));
        }
    }
    Ok(first.map(|(_, cards)| cards))
}

/// The first row (by log file, then byte offset) of `(file, offset, raw,
/// compressed)` rows, decoded. A few rows at most, so they are compared here
/// rather than sorted by SQLite (which could then prefer walking the whole
/// table).
fn first_raw(
    rows: impl Iterator<Item = rusqlite::Result<(i64, i64, Vec<u8>, bool)>>,
) -> Result<Option<String>, StoreError> {
    let mut first: Option<(i64, i64, Vec<u8>, bool)> = None;
    for row in rows {
        let row = row?;
        let earlier = match &first {
            Some(best) => (row.0, row.1) < (best.0, best.1),
            None => true,
        };
        if earlier {
            first = Some(row);
        }
    }
    match first {
        Some((_, _, raw, compressed)) => Ok(Some(decode_raw(raw, compressed)?)),
        None => Ok(None),
    }
}

/// The most `deck_used` events read for one Game ID.
const MAX_DECK_USED_RAWS: usize = 8;

/// The raw text of every `deck_used` event for this Game ID still in
/// `log_events`, decoded, in log order, at most `MAX_DECK_USED_RAWS` (a
/// hostile log can repeat a Game ID without end). A later one is the
/// fallback when the first does not parse.
pub fn deck_used_raws(conn: &Connection, game_id: u64) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare_cached(DECK_USED_RAW)?;
    let rows = stmt.query_map([game_id as i64], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, Vec<u8>>(2)?,
            row.get::<_, bool>(3)?,
        ))
    })?;
    let mut found = Vec::new();
    for row in rows {
        found.push(row?);
        // Keep only the earliest rows, so the vector stays small.
        if found.len() >= 2 * MAX_DECK_USED_RAWS {
            found.sort_by_key(|row| (row.0, row.1));
            found.truncate(MAX_DECK_USED_RAWS);
        }
    }
    found.sort_by_key(|row| (row.0, row.1));
    found.truncate(MAX_DECK_USED_RAWS);
    let mut raws = Vec::new();
    for (_, _, raw, compressed) in found {
        raws.push(decode_raw(raw, compressed)?);
    }
    Ok(raws)
}

/// The deck version linked to this match, if any.
pub fn link_of(conn: &Connection, token: &str) -> Result<Option<i64>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT deck_version_id FROM matches WHERE token = ?1",
            [token],
            |row| row.get(0),
        )
        .optional()?
        .flatten())
}

/// A printing a deck cover can use (0.x `CoverArtOptionsController`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverOption {
    pub catalog_id: u32,
    pub set_name: Option<String>,
    pub set_code: Option<String>,
    pub art_crop: String,
    pub local_art_crop: Option<String>,
}

/// Every printing of the card named `card_name` that has art (an `art_crop`
/// that is not empty), in catalog id order. A user action on a small table,
/// so it reads the cards without an index on the name.
pub fn cover_options(conn: &Connection, card_name: &str) -> Result<Vec<CoverOption>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT catalog_id, set_name, set_code, art_crop, local_art_crop FROM cards
         WHERE name = ?1 AND art_crop IS NOT NULL AND art_crop <> ''
         ORDER BY catalog_id",
    )?;
    let rows = stmt.query_map([card_name], |row| {
        Ok(CoverOption {
            catalog_id: row.get(0)?,
            set_name: row.get(1)?,
            set_code: row.get(2)?,
            art_crop: row.get(3)?,
            local_art_crop: row.get(4)?,
        })
    })?;
    let mut options = Vec::new();
    for row in rows {
        options.push(row?);
    }
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open_reader;
    use crate::test_support::*;
    use crate::write::{COMPRESS_OVER, MatchWrite, WriteBatch, commit, insert_log_file};
    use crate::writer::spawn_writer;
    use decks::DeckFile;
    use fixture_tools::TestDir;
    use mtgo_match::MatchState;
    use mtgo_protocol::{EventKind, Subjects};

    const DECK: &str = "6f0c3b9e-1d2a-4c5b-9e8f-7a6b5c4d3e2f";
    const OTHER_DECK: &str = "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d";
    const T1: i64 = 1_777_000_000_000;
    const T2: i64 = T1 + 60_000;
    const T3: i64 = T2 + 60_000;
    const SYNCED: i64 = 1_800_000_000_000;

    /// A stored match for `token`, so a link has a row to point from.
    fn stored_match(conn: &mut Connection, token: &str) {
        commit(
            conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(view(token, "Qopponent", T1)),
                    revision: 1,
                }],
                now_ms: SYNCED,
                ..WriteBatch::default()
            },
        )
        .expect("a match");
    }

    fn deck_writes(conn: &mut Connection, writes: Vec<DeckWrite>) {
        commit(
            conn,
            &WriteBatch {
                decks: writes,
                now_ms: SYNCED,
                ..WriteBatch::default()
            },
        )
        .expect("commit");
    }

    fn card(catalog_id: u32, quantity: u32, sideboard: bool) -> DeckCard {
        DeckCard {
            catalog_id,
            quantity,
            sideboard,
        }
    }

    fn file(mtgo_id: &str, name: &str, timestamp_ms: i64) -> DeckFile {
        DeckFile {
            name: name.to_string(),
            net_deck_id: mtgo_id.to_string(),
            format: "CMODERN".to_string(),
            timestamp_ms,
            cards: vec![card(1001, 4, false), card(1002, 2, true)],
        }
    }

    /// A sync of `file` with signature `signature`, by Qlocal. `identity`
    /// `None` keeps the stored identity, `Some` sets it.
    fn sync(file: DeckFile, signature: &str, identity: Option<&str>) -> DeckWrite {
        let identity = match identity {
            Some(colors) => IdentityUpdate::Set(Some(colors.to_string())),
            None => IdentityUpdate::Keep,
        };
        DeckWrite::SyncFile {
            file,
            signature: signature.to_string(),
            account: Some("Qlocal".to_string()),
            platform: Platform::Mtgo,
            synced_at_ms: SYNCED,
            identity,
        }
    }

    fn link(token: &str, version: i64) -> DeckWrite {
        DeckWrite::Link {
            match_token: token.to_string(),
            deck_version_id: version,
        }
    }

    fn deck(conn: &Connection, mtgo_id: &str) -> DeckRow {
        deck_by_mtgo_id(conn, mtgo_id)
            .expect("read")
            .expect("the deck exists")
    }

    /// `(signature, modified_at)` of a deck's versions, oldest first.
    fn versions(conn: &Connection, mtgo_id: &str) -> Vec<(String, i64)> {
        let id = deck(conn, mtgo_id).id;
        versions_of(conn, id)
            .expect("versions")
            .into_iter()
            .map(|v| (v.signature, v.modified_at))
            .collect()
    }

    fn version_id(conn: &Connection, mtgo_id: &str, signature: &str) -> i64 {
        let id = deck(conn, mtgo_id).id;
        versions_of(conn, id)
            .expect("versions")
            .into_iter()
            .find(|v| v.signature == signature)
            .expect("the version exists")
            .id
    }

    fn plan(conn: &Connection, sql: &str, params: impl rusqlite::Params) -> String {
        let mut stmt = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .expect("explain");
        let details: Vec<String> = stmt
            .query_map(params, |row| row.get::<_, String>(3))
            .expect("rows")
            .map(|row| row.expect("row"))
            .collect();
        details.join("\n")
    }

    #[test]
    fn deck_foreign_keys_state_their_delete_rules() {
        let (_dir, conn) = db("decks-fk-rules");
        let mut found = Vec::new();
        for table in ["decks", "deck_versions", "matches"] {
            // A match's link to the deck version played is the deck table's.
            let mut stmt = conn
                .prepare(
                    "SELECT \"table\", \"from\", on_delete FROM pragma_foreign_key_list(?1)
                     WHERE ?1 <> 'matches' OR \"table\" = 'deck_versions'",
                )
                .expect("prepare");
            let rows = stmt
                .query_map([table], |row| {
                    Ok(format!(
                        "{table}.{} -> {} {}",
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(2)?
                    ))
                })
                .expect("query");
            for row in rows {
                found.push(row.expect("row"));
            }
        }
        found.sort();
        assert_eq!(
            found,
            vec![
                "deck_versions.deck_id -> decks CASCADE",
                "decks.account_id -> accounts RESTRICT",
                "decks.archetype_id -> archetypes SET NULL",
                "matches.deck_version_id -> deck_versions SET NULL",
            ]
        );
    }

    #[test]
    fn a_batch_with_only_deck_writes_is_not_empty() {
        let batch = WriteBatch {
            decks: vec![link(TOKEN, 1)],
            ..WriteBatch::default()
        };
        assert!(!batch.is_empty());
    }

    // Deck file sync

    #[test]
    fn a_new_deck_file_creates_the_deck_and_its_version() {
        let (_dir, mut conn) = db("decks-sync-new");
        deck_writes(
            &mut conn,
            vec![sync(file(DECK, "Qdeck", T1), "SIG-A", Some("U,R"))],
        );
        let row = deck(&conn, DECK);
        assert_eq!(row.name, "Qdeck");
        assert_eq!(row.original_name, None);
        assert_eq!(row.format, "CMODERN");
        assert_eq!(row.source, "file");
        assert_eq!(row.created_at, SYNCED);
        assert_eq!(row.updated_at, T1);
        assert_eq!(
            row.file_synced_at,
            Some(T1),
            "the file's own timestamp, so the skip compares like with like (0.x)"
        );
        assert_eq!(row.color_identity.as_deref(), Some("U,R"));
        assert!(!row.color_identity_manual);
        assert_eq!(row.archived_at, None);
        assert_eq!(row.cover_catalog_id, None);
        let account: String = conn
            .query_row(
                "SELECT username FROM accounts WHERE id = ?1",
                [row.account_id.expect("an account")],
                |r| r.get(0),
            )
            .expect("account");
        assert_eq!(account, "Qlocal");
        assert_eq!(versions(&conn, DECK), vec![("SIG-A".to_string(), T1)]);
    }

    #[test]
    fn a_deck_sync_adds_stubs_in_its_own_transaction() {
        let (_dir, mut conn) = db("decks-sync-stubs");
        conn.execute(
            "INSERT INTO cards (catalog_id, name) VALUES (1001, 'Qbolt')",
            [],
        )
        .expect("a known card");
        // A batch that fails (an event of a log file that does not exist)
        // stores neither the deck nor its stubs.
        let failed = commit(
            &mut conn,
            &WriteBatch {
                events: vec![token_event(999, 0, "q-token")],
                decks: vec![sync(file(DECK, "Qdeck", T1), "SIG-A", None)],
                now_ms: SYNCED,
                ..WriteBatch::default()
            },
        );
        assert!(failed.is_err());
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM decks"), 0);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM cards"), 1);

        // Every card of the file gets a row; the known one is left alone.
        deck_writes(
            &mut conn,
            vec![sync(file(DECK, "Qdeck", T1), "SIG-A", None)],
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM cards"), 2);
        assert_eq!(
            number(&conn, "SELECT catalog_id FROM cards WHERE name IS NULL"),
            1002
        );
        assert_eq!(crate::cards::take_stubs_added(&conn).expect("count"), 1);
    }

    #[test]
    fn a_deck_file_without_an_account_leaves_the_account_empty() {
        let (_dir, mut conn) = db("decks-sync-no-account");
        deck_writes(
            &mut conn,
            vec![DeckWrite::SyncFile {
                file: file(DECK, "Qdeck", T1),
                signature: "SIG-A".to_string(),
                account: None,
                platform: Platform::Mtgo,
                synced_at_ms: SYNCED,
                identity: IdentityUpdate::Keep,
            }],
        );
        assert_eq!(deck(&conn, DECK).account_id, None);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM accounts"), 0);
        deck_writes(
            &mut conn,
            vec![sync(file(DECK, "Qdeck", T2), "SIG-A", None)],
        );
        assert!(
            deck(&conn, DECK).account_id.is_some(),
            "a later sync gives it the local account"
        );
    }

    #[test]
    fn adopting_gives_a_deck_without_an_account_the_local_one_only() {
        let (_dir, mut conn) = db("decks-adopt");
        deck_writes(
            &mut conn,
            vec![
                DeckWrite::SyncFile {
                    file: file(DECK, "Qdeck", T1),
                    signature: "SIG-A".to_string(),
                    account: None,
                    platform: Platform::Mtgo,
                    synced_at_ms: SYNCED,
                    identity: IdentityUpdate::Keep,
                },
                sync(file(OTHER_DECK, "Qother", T1), "SIG-X", None),
            ],
        );
        let other_account = deck(&conn, OTHER_DECK).account_id;
        assert!(other_account.is_some());

        let adopt = |mtgo_id: &str| DeckWrite::Adopt {
            mtgo_id: mtgo_id.to_string(),
            account: "Qsecond".to_string(),
            at_ms: SYNCED,
        };
        deck_writes(&mut conn, vec![adopt(DECK), adopt(OTHER_DECK)]);

        let adopted = deck(&conn, DECK);
        let second: i64 = conn
            .query_row(
                "SELECT id FROM accounts WHERE username = 'Qsecond'",
                [],
                |row| row.get(0),
            )
            .expect("the account was registered");
        assert_eq!(adopted.account_id, Some(second));
        assert_eq!(adopted.file_synced_at, Some(T1), "nothing else changes");
        assert_eq!(
            deck(&conn, OTHER_DECK).account_id,
            other_account,
            "a deck keeps the account it has"
        );
    }

    #[test]
    fn the_first_account_adopts_every_deck_without_one_archived_included() {
        let (_dir, mut conn) = db("decks-adopt-orphans");
        let orphan = |mtgo_id: &str, name: &str| DeckWrite::SyncFile {
            file: file(mtgo_id, name, T1),
            signature: "SIG-A".to_string(),
            account: None,
            platform: Platform::Mtgo,
            synced_at_ms: SYNCED,
            identity: IdentityUpdate::Keep,
        };
        deck_writes(
            &mut conn,
            vec![orphan(DECK, "Qdeck"), orphan(OTHER_DECK, "Qother")],
        );
        let archived = deck(&conn, OTHER_DECK).id;
        edit_deck(
            &mut conn,
            &DeckEdit::Archive {
                deck_id: archived,
                at_ms: 5,
            },
        )
        .expect("archive");

        deck_writes(
            &mut conn,
            vec![DeckWrite::AdoptOrphans {
                account: "Qfirst".to_string(),
                at_ms: SYNCED,
            }],
        );
        let first = deck(&conn, DECK).account_id;
        assert!(first.is_some());
        assert_eq!(deck(&conn, OTHER_DECK).account_id, first);
    }

    #[test]
    fn orphan_decks_are_adopted_when_an_arena_account_exists() {
        let (_dir, mut conn) = db("decks-adopt-orphans-arena");
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at, platform) VALUES ('Qarena#12345', 0, 'arena')",
            [],
        )
        .expect("arena account");
        deck_writes(
            &mut conn,
            vec![DeckWrite::SyncFile {
                file: file(DECK, "Qdeck", T1),
                signature: "SIG-A".to_string(),
                account: None,
                platform: Platform::Mtgo,
                synced_at_ms: SYNCED,
                identity: IdentityUpdate::Keep,
            }],
        );
        deck_writes(
            &mut conn,
            vec![DeckWrite::AdoptOrphans {
                account: "Qfirst".to_string(),
                at_ms: SYNCED,
            }],
        );
        let first: i64 = conn
            .query_row(
                "SELECT id FROM accounts WHERE username = 'Qfirst'",
                [],
                |row| row.get(0),
            )
            .expect("account");
        assert_eq!(
            deck(&conn, DECK).account_id,
            Some(first),
            "an Arena account is not a second MTGO account"
        );
    }

    #[test]
    fn a_second_account_adopts_no_orphans_and_never_takes_a_deck() {
        let (_dir, mut conn) = db("decks-adopt-orphans-second");
        deck_writes(
            &mut conn,
            vec![
                sync(file(DECK, "Qdeck", T1), "SIG-A", None),
                DeckWrite::SyncFile {
                    file: file(OTHER_DECK, "Qother", T1),
                    signature: "SIG-X".to_string(),
                    account: None,
                    platform: Platform::Mtgo,
                    synced_at_ms: SYNCED,
                    identity: IdentityUpdate::Keep,
                },
            ],
        );
        let owned = deck(&conn, DECK).account_id;
        assert!(owned.is_some(), "Qlocal is the first account");

        deck_writes(
            &mut conn,
            vec![DeckWrite::AdoptOrphans {
                account: "Qsecond".to_string(),
                at_ms: SYNCED,
            }],
        );
        assert_eq!(
            deck(&conn, OTHER_DECK).account_id,
            None,
            "with two accounts an orphan could be either's (0.x)"
        );
        assert_eq!(deck(&conn, DECK).account_id, owned);
    }

    #[test]
    fn a_deck_keeps_its_account_when_another_account_syncs_it() {
        let (_dir, mut conn) = db("decks-sync-account");
        deck_writes(
            &mut conn,
            vec![sync(file(DECK, "Qdeck", T1), "SIG-A", None)],
        );
        let first = deck(&conn, DECK).account_id;
        deck_writes(
            &mut conn,
            vec![DeckWrite::SyncFile {
                file: file(DECK, "Qdeck", T2),
                signature: "SIG-A".to_string(),
                account: Some("Qsecond".to_string()),
                platform: Platform::Mtgo,
                synced_at_ms: SYNCED,
                identity: IdentityUpdate::Keep,
            }],
        );
        assert_eq!(deck(&conn, DECK).account_id, first);
    }

    #[test]
    fn a_file_at_or_before_the_last_synced_timestamp_is_skipped() {
        let (_dir, mut conn) = db("decks-sync-skip");
        deck_writes(
            &mut conn,
            vec![sync(file(DECK, "Qdeck", T2), "SIG-A", Some("U"))],
        );
        let mut same_time = file(DECK, "Qrenamed", T2);
        same_time.format = "CPAUPER".to_string();
        deck_writes(
            &mut conn,
            vec![
                sync(same_time, "SIG-B", Some("G")),
                sync(file(DECK, "Qolder", T1), "SIG-C", Some("B")),
            ],
        );
        let row = deck(&conn, DECK);
        assert_eq!(row.name, "Qdeck");
        assert_eq!(row.format, "CMODERN");
        assert_eq!(row.color_identity.as_deref(), Some("U"));
        assert_eq!(versions(&conn, DECK), vec![("SIG-A".to_string(), T2)]);
    }

    /// A platform match's deck list (Endstep) folded after a newer one of
    /// the same deck: its version is added, nothing else changes.
    #[test]
    fn an_older_deck_use_adds_only_its_version() {
        let (_dir, mut conn) = db("decks-use-older");
        deck_writes(
            &mut conn,
            vec![
                sync(file(DECK, "Qdeck", T2), "SIG-A", Some("U")),
                sync(file(DECK, "Qdeck", T3), "SIG-B", Some("U")),
            ],
        );
        let before = deck(&conn, DECK);
        let mut older = file(DECK, "Qolder", T1);
        older.format = "CPAUPER".to_string();
        let DeckWrite::SyncFile {
            file: used_file,
            signature,
            account,
            platform,
            synced_at_ms,
            identity,
        } = sync(older, "SIG-C", Some("B"))
        else {
            unreachable!("a file sync")
        };
        let used = DeckWrite::SyncUse {
            file: used_file,
            signature,
            account,
            platform,
            synced_at_ms,
            identity,
        };
        deck_writes(&mut conn, vec![used.clone()]);
        assert_eq!(deck(&conn, DECK), before, "the deck's own columns stay");
        assert_eq!(
            versions(&conn, DECK),
            vec![("SIG-C".to_string(), T1), ("SIG-B".to_string(), T3)],
            "the older list is a version; nothing is pruned"
        );
        // Again: the version is not added twice.
        deck_writes(&mut conn, vec![used]);
        assert_eq!(versions(&conn, DECK).len(), 2);
    }

    /// A deck use at or after the deck's last write is a file sync.
    #[test]
    fn a_newer_deck_use_syncs_like_a_file() {
        let (_dir, mut conn) = db("decks-use-newer");
        let DeckWrite::SyncFile {
            file: used_file,
            signature,
            account,
            platform,
            synced_at_ms,
            identity,
        } = sync(file(DECK, "Qdeck", T2), "SIG-A", Some("U"))
        else {
            unreachable!("a file sync")
        };
        deck_writes(
            &mut conn,
            vec![
                sync(file(DECK, "Qdeck", T1), "SIG-0", None),
                DeckWrite::SyncUse {
                    file: used_file,
                    signature,
                    account,
                    platform,
                    synced_at_ms,
                    identity,
                },
            ],
        );
        let row = deck(&conn, DECK);
        assert_eq!(row.file_synced_at, Some(T2));
        assert_eq!(row.color_identity.as_deref(), Some("U"));
        assert_eq!(versions(&conn, DECK), vec![("SIG-A".to_string(), T2)]);
    }

    #[test]
    fn a_newer_file_updates_the_deck_and_prunes_unlinked_versions() {
        let (_dir, mut conn) = db("decks-sync-newer");
        deck_writes(
            &mut conn,
            vec![sync(file(DECK, "Qdeck", T1), "SIG-A", Some("U"))],
        );
        let mut newer = file(DECK, "Qdeck two", T2);
        newer.format = "CPIONEER".to_string();
        deck_writes(&mut conn, vec![sync(newer, "SIG-B", Some("U,B"))]);
        let row = deck(&conn, DECK);
        assert_eq!(row.name, "Qdeck two");
        assert_eq!(row.format, "CPIONEER");
        assert_eq!(row.updated_at, T2);
        assert_eq!(row.file_synced_at, Some(T2));
        assert_eq!(row.created_at, SYNCED);
        assert_eq!(row.color_identity.as_deref(), Some("U,B"));
        assert_eq!(
            versions(&conn, DECK),
            vec![("SIG-B".to_string(), T2)],
            "the old version had no match, so it goes"
        );
    }

    #[test]
    fn pruning_keeps_versions_a_match_used_and_leaves_other_decks_alone() {
        let (_dir, mut conn) = db("decks-sync-prune-links");
        deck_writes(
            &mut conn,
            vec![
                sync(file(DECK, "Qdeck", T1), "SIG-A", None),
                sync(file(OTHER_DECK, "Qother", T1), "SIG-X", None),
            ],
        );
        let used = version_id(&conn, DECK, "SIG-A");
        stored_match(&mut conn, TOKEN);
        deck_writes(&mut conn, vec![link(TOKEN, used)]);
        deck_writes(
            &mut conn,
            vec![sync(file(DECK, "Qdeck", T2), "SIG-B", None)],
        );
        deck_writes(
            &mut conn,
            vec![sync(file(DECK, "Qdeck", T3), "SIG-C", None)],
        );
        assert_eq!(
            versions(&conn, DECK),
            vec![("SIG-A".to_string(), T1), ("SIG-C".to_string(), T3)],
            "A has a match; B had none and is replaced by C"
        );
        assert_eq!(versions(&conn, OTHER_DECK), vec![("SIG-X".to_string(), T1)]);
        assert_eq!(link_of(&conn, TOKEN).expect("link"), Some(used));
    }

    fn archive_unused(mtgo_id: &str, at_ms: i64) -> DeckWrite {
        DeckWrite::ArchiveUnused {
            mtgo_id: mtgo_id.to_string(),
            at_ms,
        }
    }

    /// DECK and OTHER_DECK synced and the match TOKEN linked to DECK.
    fn linked_to_deck(label: &str) -> (TestDir, Connection) {
        let (dir, mut conn) = db(label);
        deck_writes(
            &mut conn,
            vec![
                sync(file(DECK, "Qdeck", T1), "SIG-A", None),
                sync(file(OTHER_DECK, "Qother", T1), "SIG-X", None),
            ],
        );
        stored_match(&mut conn, TOKEN);
        let used = version_id(&conn, DECK, "SIG-A");
        deck_writes(&mut conn, vec![link(TOKEN, used)]);
        (dir, conn)
    }

    /// #867: a relink moves the match to OTHER_DECK and, in the same batch,
    /// DECK (now used by no match) is archived; a later ask keeps the first
    /// archive time.
    #[test]
    fn a_deck_left_without_a_match_is_archived() {
        let (_dir, mut conn) = linked_to_deck("decks-archive-unused");
        let other = version_id(&conn, OTHER_DECK, "SIG-X");
        deck_writes(
            &mut conn,
            vec![link(TOKEN, other), archive_unused(DECK, T2)],
        );
        assert_eq!(deck(&conn, DECK).archived_at, Some(T2));
        assert_eq!(deck(&conn, OTHER_DECK).archived_at, None);
        assert_eq!(
            versions(&conn, DECK),
            vec![("SIG-A".to_string(), T1)],
            "nothing is deleted"
        );
        deck_writes(&mut conn, vec![archive_unused(DECK, T3)]);
        assert_eq!(deck(&conn, DECK).archived_at, Some(T2));
    }

    #[test]
    fn a_deck_a_match_uses_is_not_archived() {
        let (_dir, mut conn) = linked_to_deck("decks-archive-used");
        deck_writes(&mut conn, vec![archive_unused(DECK, T2)]);
        assert_eq!(deck(&conn, DECK).archived_at, None);
    }

    #[test]
    fn archiving_a_deck_that_does_not_exist_changes_nothing() {
        let (_dir, mut conn) = db("decks-archive-missing");
        deck_writes(&mut conn, vec![archive_unused(DECK, T2)]);
        assert!(deck_by_mtgo_id(&conn, DECK).expect("read").is_none());
    }

    /// #867: a deck left without a match stays when it holds anything of
    /// the user's or of the sync: each setup is applied alone.
    #[test]
    fn a_deck_with_anything_of_the_users_is_not_archived() {
        let setups = [
            "UPDATE decks SET original_name = name, name = 'Qrenamed' WHERE id = ?1",
            "UPDATE decks SET cover_catalog_id = 1001 WHERE id = ?1",
            "UPDATE decks SET cover_file = '1-5.png' WHERE id = ?1",
            "UPDATE decks SET overlay_label = 'Qlabel' WHERE id = ?1",
            "UPDATE decks SET color_identity_manual = 1 WHERE id = ?1",
            "INSERT INTO sideboard_guides (deck_id, archetype_id, created_at, updated_at)
             VALUES (?1, (SELECT MIN(id) FROM archetypes), 0, 0)",
            "INSERT INTO deck_archetype_notes (deck_id, archetype_id, body, created_at)
             VALUES (?1, (SELECT MIN(id) FROM archetypes), 'Qnote', 0)",
            "INSERT INTO leagues (run_key, source, kind, state, token, name, deck_id,
                 started_at, created_at)
             VALUES ('log:1', 'log', 'constructed', 'complete', 'Qtoken', 'Qleague', ?1, 0, 0)",
            "INSERT INTO leagues (run_key, source, kind, state, token, name, deck_version_id,
                 started_at, created_at)
             VALUES ('log:1', 'log', 'constructed', 'complete', 'Qtoken', 'Qleague',
                 (SELECT MIN(id) FROM deck_versions WHERE deck_id = ?1), 0, 0)",
            "UPDATE decks SET cloud_sync_enabled = 1 WHERE id = ?1",
            "INSERT INTO synced_rows (type, client_id)
             SELECT 'deck', mtgo_id FROM decks WHERE id = ?1",
            "UPDATE decks SET pulled_platform = 'endstep' WHERE id = ?1",
            "UPDATE decks SET source = 'imported' WHERE id = ?1",
        ];
        for setup in setups {
            let (_dir, mut conn) = linked_to_deck("decks-archive-kept");
            let deck_id = deck(&conn, DECK).id;
            conn.execute(setup, [deck_id]).expect(setup);
            let other = version_id(&conn, OTHER_DECK, "SIG-X");
            deck_writes(
                &mut conn,
                vec![link(TOKEN, other), archive_unused(DECK, T2)],
            );
            assert_eq!(deck(&conn, DECK).archived_at, None, "{setup}");
        }
    }

    /// A deck with an archetype is archived and keeps it: the prefill's and
    /// detection's cannot be told from a hand-set one (#867).
    #[test]
    fn a_deck_with_an_archetype_is_archived_and_keeps_it() {
        let (_dir, mut conn) = linked_to_deck("decks-archive-archetype");
        conn.execute(
            "UPDATE decks SET archetype_id = (SELECT MIN(id) FROM archetypes) WHERE mtgo_id = ?1",
            [DECK],
        )
        .expect("an archetype");
        let other = version_id(&conn, OTHER_DECK, "SIG-X");
        deck_writes(
            &mut conn,
            vec![link(TOKEN, other), archive_unused(DECK, T2)],
        );
        let row = deck(&conn, DECK);
        assert_eq!(row.archived_at, Some(T2));
        let archetype: Option<i64> = conn
            .query_row(
                "SELECT archetype_id FROM decks WHERE id = ?1",
                [row.id],
                |r| r.get(0),
            )
            .expect("read");
        assert!(archetype.is_some(), "the archetype stays");
    }

    /// A deck archived by hand keeps its own archive time.
    #[test]
    fn an_archived_deck_keeps_its_archive_time() {
        let (_dir, mut conn) = linked_to_deck("decks-archive-already");
        let other = version_id(&conn, OTHER_DECK, "SIG-X");
        deck_writes(&mut conn, vec![link(TOKEN, other)]);
        conn.execute(
            "UPDATE decks SET archived_at = 5 WHERE mtgo_id = ?1",
            [DECK],
        )
        .expect("archive by hand");
        deck_writes(&mut conn, vec![archive_unused(DECK, T2)]);
        assert_eq!(deck(&conn, DECK).archived_at, Some(5));
    }

    #[test]
    fn a_revert_reuses_the_old_version_and_keeps_its_modified_at() {
        let (_dir, mut conn) = db("decks-sync-revert");
        deck_writes(
            &mut conn,
            vec![sync(file(DECK, "Qdeck", T1), "SIG-A", None)],
        );
        let first = version_id(&conn, DECK, "SIG-A");
        stored_match(&mut conn, TOKEN);
        deck_writes(&mut conn, vec![link(TOKEN, first)]);
        deck_writes(
            &mut conn,
            vec![sync(file(DECK, "Qdeck", T2), "SIG-B", None)],
        );
        deck_writes(
            &mut conn,
            vec![sync(file(DECK, "Qdeck", T3), "SIG-A", None)],
        );
        assert_eq!(versions(&conn, DECK), vec![("SIG-A".to_string(), T1)]);
        assert_eq!(version_id(&conn, DECK, "SIG-A"), first);
        assert_eq!(deck(&conn, DECK).updated_at, T3);
    }

    #[test]
    fn a_renamed_deck_keeps_its_name_when_its_file_changes() {
        let (_dir, mut conn) = db("decks-sync-renamed");
        deck_writes(
            &mut conn,
            vec![sync(file(DECK, "Qdeck", T1), "SIG-A", None)],
        );
        let id = deck(&conn, DECK).id;
        let outcome = edit_deck(
            &mut conn,
            &DeckEdit::Rename {
                deck_id: id,
                name: "Qmine".to_string(),
            },
        )
        .expect("rename");
        assert_eq!(outcome, EditOutcome::Done);
        deck_writes(
            &mut conn,
            vec![sync(file(DECK, "Qfile", T2), "SIG-A", None)],
        );
        let row = deck(&conn, DECK);
        assert_eq!(row.name, "Qmine");
        assert_eq!(row.original_name.as_deref(), Some("Qdeck"));
    }

    #[test]
    fn an_empty_original_name_lets_the_file_name_through() {
        let (_dir, mut conn) = db("decks-sync-empty-original");
        deck_writes(
            &mut conn,
            vec![sync(file(DECK, "Qdeck", T1), "SIG-A", None)],
        );
        conn.execute("UPDATE decks SET original_name = ''", [])
            .expect("an imported empty original name");
        deck_writes(
            &mut conn,
            vec![sync(file(DECK, "Qfile", T2), "SIG-A", None)],
        );
        assert_eq!(deck(&conn, DECK).name, "Qfile");
    }

    #[test]
    fn a_file_with_cards_sets_the_identity_and_clears_the_manual_flag() {
        let (_dir, mut conn) = db("decks-sync-identity");
        deck_writes(
            &mut conn,
            vec![sync(file(DECK, "Qdeck", T1), "SIG-A", Some("U"))],
        );
        let id = deck(&conn, DECK).id;
        edit_deck(
            &mut conn,
            &DeckEdit::SetIdentity {
                deck_id: id,
                identity: Some("W,B".to_string()),
            },
        )
        .expect("manual");
        let manual = deck(&conn, DECK);
        assert_eq!(manual.color_identity.as_deref(), Some("W,B"));
        assert!(manual.color_identity_manual);

        deck_writes(
            &mut conn,
            vec![sync(file(DECK, "Qdeck", T2), "SIG-A", None)],
        );
        let kept = deck(&conn, DECK);
        assert_eq!(
            kept.color_identity.as_deref(),
            Some("W,B"),
            "a file without cards keeps the stored identity (0.x)"
        );
        assert!(
            kept.color_identity_manual,
            "and keeps it manual, so a 0.x import cannot replace it"
        );

        deck_writes(
            &mut conn,
            vec![sync(file(DECK, "Qdeck", T3), "SIG-A", Some("G"))],
        );
        let computed = deck(&conn, DECK);
        assert_eq!(computed.color_identity.as_deref(), Some("G"));
        assert!(
            !computed.color_identity_manual,
            "a computed identity clears the flag"
        );
    }

    #[test]
    fn a_file_whose_cards_reach_no_colour_clears_the_identity() {
        let (_dir, mut conn) = db("decks-sync-identity-null");
        deck_writes(
            &mut conn,
            vec![sync(file(DECK, "Qdeck", T1), "SIG-A", Some("U"))],
        );
        assert_eq!(deck(&conn, DECK).color_identity.as_deref(), Some("U"));

        // 0.x writes NULL when the latest version has cards but no colour
        // reaches 4.
        deck_writes(
            &mut conn,
            vec![DeckWrite::SyncFile {
                file: file(DECK, "Qdeck", T2),
                signature: "SIG-B".to_string(),
                account: Some("Qlocal".to_string()),
                platform: Platform::Mtgo,
                synced_at_ms: SYNCED,
                identity: IdentityUpdate::Set(None),
            }],
        );
        let row = deck(&conn, DECK);
        assert_eq!(row.color_identity, None);
        assert!(!row.color_identity_manual);
    }

    #[test]
    fn an_archived_deck_still_syncs_from_its_file_and_stays_archived() {
        let (_dir, mut conn) = db("decks-sync-archived");
        deck_writes(
            &mut conn,
            vec![sync(file(DECK, "Qdeck", T1), "SIG-A", None)],
        );
        let id = deck(&conn, DECK).id;
        edit_deck(
            &mut conn,
            &DeckEdit::Archive {
                deck_id: id,
                at_ms: 77,
            },
        )
        .expect("archive");
        deck_writes(
            &mut conn,
            vec![sync(file(DECK, "Qfile", T2), "SIG-B", None)],
        );
        let row = deck(&conn, DECK);
        assert_eq!(row.id, id, "found by its MTGO id, archived included");
        assert_eq!(row.name, "Qfile");
        assert_eq!(row.archived_at, Some(77));
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM decks"), 1);
    }

    #[test]
    fn an_imported_deck_read_from_its_file_becomes_a_file_deck() {
        let (_dir, mut conn) = db("decks-sync-imported");
        conn.execute(
            "INSERT INTO decks (mtgo_id, name, format, created_at, updated_at, source)
             VALUES (?1, 'Qimported', 'CMODERN', 5, 5, 'imported')",
            [DECK],
        )
        .expect("an imported deck");
        deck_writes(
            &mut conn,
            vec![sync(file(DECK, "Qfile", T1), "SIG-A", None)],
        );
        let row = deck(&conn, DECK);
        assert_eq!(row.source, "file");
        assert_eq!(row.created_at, 5);
        assert_eq!(row.name, "Qfile");
    }

    // Links

    #[test]
    fn a_link_is_inserted_or_replaced() {
        let (_dir, mut conn) = db("decks-link");
        deck_writes(
            &mut conn,
            vec![
                sync(file(DECK, "Qdeck", T1), "SIG-A", None),
                sync(file(OTHER_DECK, "Qother", T1), "SIG-X", None),
            ],
        );
        let first = version_id(&conn, DECK, "SIG-A");
        let second = version_id(&conn, OTHER_DECK, "SIG-X");
        assert_eq!(link_of(&conn, TOKEN).expect("none yet"), None);
        stored_match(&mut conn, TOKEN);
        deck_writes(&mut conn, vec![link(TOKEN, first)]);
        assert_eq!(link_of(&conn, TOKEN).expect("link"), Some(first));
        deck_writes(&mut conn, vec![link(TOKEN, second)]);
        assert_eq!(link_of(&conn, TOKEN).expect("link"), Some(second));
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE deck_version_id IS NOT NULL"
            ),
            1
        );
    }

    #[test]
    fn a_link_to_a_pruned_version_is_skipped_without_failing_the_batch() {
        let (_dir, mut conn) = db("decks-link-missing");
        deck_writes(
            &mut conn,
            vec![
                link(TOKEN, 999),
                sync(file(DECK, "Qdeck", T1), "SIG-A", None),
            ],
        );
        assert_eq!(link_of(&conn, TOKEN).expect("read"), None);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM decks"), 1);
    }

    // Edits

    fn new_deck(conn: &mut Connection) -> i64 {
        deck_writes(conn, vec![sync(file(DECK, "Qdeck", T1), "SIG-A", None)]);
        deck(conn, DECK).id
    }

    fn rename(conn: &mut Connection, id: i64, name: &str) -> EditOutcome {
        edit_deck(
            conn,
            &DeckEdit::Rename {
                deck_id: id,
                name: name.to_string(),
            },
        )
        .expect("rename")
    }

    #[test]
    fn rename_trims_keeps_the_original_name_and_reverting_clears_it() {
        let (_dir, mut conn) = db("decks-rename");
        let id = new_deck(&mut conn);
        assert_eq!(rename(&mut conn, id, "  Qfirst  "), EditOutcome::Done);
        let row = deck(&conn, DECK);
        assert_eq!(row.name, "Qfirst");
        assert_eq!(row.original_name.as_deref(), Some("Qdeck"));

        assert_eq!(rename(&mut conn, id, "Qsecond"), EditOutcome::Done);
        assert_eq!(
            deck(&conn, DECK).original_name.as_deref(),
            Some("Qdeck"),
            "the original name is captured once"
        );

        assert_eq!(rename(&mut conn, id, "Qdeck"), EditOutcome::Done);
        let reverted = deck(&conn, DECK);
        assert_eq!(reverted.name, "Qdeck");
        assert_eq!(reverted.original_name, None);
    }

    #[test]
    fn renaming_to_the_same_name_changes_nothing() {
        let (_dir, mut conn) = db("decks-rename-same");
        let id = new_deck(&mut conn);
        assert_eq!(rename(&mut conn, id, " Qdeck "), EditOutcome::Done);
        assert_eq!(deck(&conn, DECK).original_name, None);
    }

    #[test]
    fn a_name_must_have_1_to_255_characters() {
        let (_dir, mut conn) = db("decks-rename-invalid");
        let id = new_deck(&mut conn);
        assert_eq!(rename(&mut conn, id, "   "), EditOutcome::InvalidName);
        let too_long = "Q".repeat(256);
        assert_eq!(rename(&mut conn, id, &too_long), EditOutcome::InvalidName);
        let longest = "\u{e9}".repeat(255);
        assert_eq!(
            rename(&mut conn, id, &longest),
            EditOutcome::Done,
            "characters, not bytes"
        );
        assert_eq!(deck(&conn, DECK).name, longest);
    }

    #[test]
    fn identity_and_cover_edits_are_stored() {
        let (_dir, mut conn) = db("decks-identity-cover");
        let id = new_deck(&mut conn);
        let set = |conn: &mut Connection, edit: DeckEdit| edit_deck(conn, &edit).expect("edit");
        assert_eq!(
            set(
                &mut conn,
                DeckEdit::SetIdentity {
                    deck_id: id,
                    identity: None
                }
            ),
            EditOutcome::Done
        );
        let row = deck(&conn, DECK);
        assert_eq!(row.color_identity, None);
        assert!(row.color_identity_manual);
        assert_eq!(
            set(
                &mut conn,
                DeckEdit::SetCover {
                    deck_id: id,
                    catalog_id: Some(1001)
                }
            ),
            EditOutcome::Done
        );
        assert_eq!(deck(&conn, DECK).cover_catalog_id, Some(1001));
        set(
            &mut conn,
            DeckEdit::SetCover {
                deck_id: id,
                catalog_id: None,
            },
        );
        assert_eq!(deck(&conn, DECK).cover_catalog_id, None);
    }

    #[test]
    fn the_cover_file_edit_is_stored_apart_from_the_cover_card() {
        let (_dir, mut conn) = db("decks-cover-file");
        let id = new_deck(&mut conn);
        let set = |conn: &mut Connection, edit: DeckEdit| edit_deck(conn, &edit).expect("edit");
        set(
            &mut conn,
            DeckEdit::SetCover {
                deck_id: id,
                catalog_id: Some(1001),
            },
        );
        assert_eq!(
            set(
                &mut conn,
                DeckEdit::SetCoverFile {
                    deck_id: id,
                    file: Some("1-5.png".to_string())
                }
            ),
            EditOutcome::Done
        );
        let row = deck(&conn, DECK);
        assert_eq!(row.cover_file.as_deref(), Some("1-5.png"));
        assert_eq!(row.cover_catalog_id, Some(1001), "the printing stays");
        set(
            &mut conn,
            DeckEdit::SetCoverFile {
                deck_id: id,
                file: None,
            },
        );
        assert_eq!(deck(&conn, DECK).cover_file, None);
        assert_eq!(
            set(
                &mut conn,
                DeckEdit::SetCoverFile {
                    deck_id: 9999,
                    file: None
                }
            ),
            EditOutcome::NotFound
        );
    }

    #[test]
    fn an_archived_deck_refuses_every_edit_but_restore() {
        let (_dir, mut conn) = db("decks-archived-refusals");
        let id = new_deck(&mut conn);
        assert_eq!(
            edit_deck(
                &mut conn,
                &DeckEdit::Archive {
                    deck_id: id,
                    at_ms: 50
                }
            )
            .expect("archive"),
            EditOutcome::Done
        );
        let before = deck(&conn, DECK);
        assert_eq!(before.archived_at, Some(50));
        let refused = [
            DeckEdit::Rename {
                deck_id: id,
                name: "Qother".to_string(),
            },
            DeckEdit::SetIdentity {
                deck_id: id,
                identity: Some("G".to_string()),
            },
            DeckEdit::SetCover {
                deck_id: id,
                catalog_id: Some(1001),
            },
            DeckEdit::SetCoverFile {
                deck_id: id,
                file: Some("1-5.png".to_string()),
            },
            DeckEdit::Archive {
                deck_id: id,
                at_ms: 60,
            },
        ];
        for edit in &refused {
            assert_eq!(
                edit_deck(&mut conn, edit).expect("edit"),
                EditOutcome::Archived,
                "{edit:?}"
            );
        }
        assert_eq!(deck(&conn, DECK), before, "nothing changed");
        assert_eq!(EditOutcome::Archived.refusal(), Some(ARCHIVED_DECK));

        assert_eq!(
            edit_deck(&mut conn, &DeckEdit::Restore { deck_id: id }).expect("restore"),
            EditOutcome::Done
        );
        assert_eq!(deck(&conn, DECK).archived_at, None);
        assert_eq!(rename(&mut conn, id, "Qother"), EditOutcome::Done);
        assert_eq!(
            versions(&conn, DECK).len(),
            1,
            "archive and restore keep the versions"
        );
    }

    #[test]
    fn an_edit_of_a_missing_deck_is_refused() {
        let (_dir, mut conn) = db("decks-edit-missing");
        assert_eq!(rename(&mut conn, 42, "Qname"), EditOutcome::NotFound);
        assert_eq!(
            edit_deck(&mut conn, &DeckEdit::Restore { deck_id: 42 }).expect("restore"),
            EditOutcome::NotFound
        );
    }

    // Overlay deck labels (overlay spec 9.2)

    fn set_label(conn: &mut Connection, id: i64, label: Option<&str>) -> EditOutcome {
        edit_deck(
            conn,
            &DeckEdit::SetOverlayLabel {
                deck_id: id,
                label: label.map(str::to_string),
            },
        )
        .expect("label")
    }

    fn label_of(conn: &Connection, id: i64) -> Option<String> {
        conn.query_row(
            "SELECT overlay_label FROM decks WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .expect("label")
    }

    /// 0.x `UpdateOverlayDeckLabelTest` "saves the label and pushes an
    /// overlay reload" (the reload is the tracker's `DecksChanged`).
    #[test]
    fn a_deck_label_is_saved_trimmed() {
        let (_dir, mut conn) = db("decks-label-saved");
        let id = new_deck(&mut conn);
        assert_eq!(
            set_label(&mut conn, id, Some(" Big Mana ")),
            EditOutcome::Done
        );
        assert_eq!(label_of(&conn, id).as_deref(), Some("Big Mana"));
    }

    /// 0.x `UpdateOverlayDeckLabelTest` "removes the label when blank".
    #[test]
    fn a_blank_deck_label_removes_it() {
        let (_dir, mut conn) = db("decks-label-blank");
        let id = new_deck(&mut conn);
        set_label(&mut conn, id, Some("Old"));
        assert_eq!(set_label(&mut conn, id, None), EditOutcome::Done);
        assert_eq!(label_of(&conn, id), None);
    }

    /// 0.x `UpdateOverlayDeckLabelTest` "rejects an unknown deck".
    #[test]
    fn a_label_for_an_unknown_deck_is_refused() {
        let (_dir, mut conn) = db("decks-label-unknown");
        assert_eq!(
            set_label(&mut conn, 999_999, Some("x")),
            EditOutcome::NotFound
        );
    }

    /// 0.x `OverlayPublishSettingsTest` "stores deck labels trimmed, capped
    /// at 40 and removed when blank" (characters, not bytes: `mb_substr`).
    #[test]
    fn deck_labels_are_stored_trimmed_capped_at_40_and_removed_when_blank() {
        let (_dir, mut conn) = db("decks-label-rules");
        let seven = new_deck(&mut conn);
        deck_writes(
            &mut conn,
            vec![sync(file(OTHER_DECK, "Qother", T1), "SIG-X", None)],
        );
        let nine = deck(&conn, OTHER_DECK).id;
        set_label(&mut conn, seven, Some("  Tron, again  "));
        set_label(&mut conn, nine, Some(&"x".repeat(60)));
        assert_eq!(label_of(&conn, seven).as_deref(), Some("Tron, again"));
        assert_eq!(label_of(&conn, nine), Some("x".repeat(40)));

        set_label(&mut conn, seven, Some("   "));
        set_label(&mut conn, nine, None);
        assert_eq!(label_of(&conn, seven), None);
        assert_eq!(label_of(&conn, nine), None);

        set_label(&mut conn, seven, Some(&"é".repeat(45)));
        assert_eq!(label_of(&conn, seven), Some("é".repeat(40)));
    }

    /// Ruling C4: the label is a streaming display setting, so an archived
    /// deck (a finished league run's, say) takes one too.
    #[test]
    fn an_archived_deck_takes_a_label() {
        let (_dir, mut conn) = db("decks-label-archived");
        let id = new_deck(&mut conn);
        edit_deck(
            &mut conn,
            &DeckEdit::Archive {
                deck_id: id,
                at_ms: 50,
            },
        )
        .expect("archive");
        assert_eq!(set_label(&mut conn, id, Some("Qlabel")), EditOutcome::Done);
        assert_eq!(label_of(&conn, id).as_deref(), Some("Qlabel"));
        assert_eq!(deck(&conn, DECK).archived_at, Some(50), "still archived");
    }

    /// Spec ruling 9: a label is not in the deck bundle and never moves the
    /// deck's `updated_at`, so it never makes the deck dirty for the sync.
    #[test]
    fn a_label_never_moves_the_deck_touch_key() {
        let (_dir, mut conn) = db("decks-label-touch-key");
        let id = new_deck(&mut conn);
        let touch_key = |conn: &Connection| {
            crate::sync::deck_candidates(conn, mymtgo_api::Tier::Supporter)
                .expect("candidates")
                .into_iter()
                .find(|candidate| candidate.deck_id == id)
                .expect("the deck")
                .touch_key
        };
        let before_key = touch_key(&conn);
        let before = deck(&conn, DECK);
        set_label(&mut conn, id, Some("Qlabel"));
        set_label(&mut conn, id, None);
        set_label(&mut conn, id, Some("Qagain"));
        assert_eq!(touch_key(&conn), before_key);
        assert_eq!(
            deck(&conn, DECK),
            before,
            "updated_at and every column kept"
        );
    }

    #[test]
    fn the_writer_applies_an_edit_and_answers_with_its_outcome() {
        let dir = TestDir::new("decks-writer-edit");
        let path = dir.join("mymtgo.sqlite");
        let writer = spawn_writer(&path).expect("spawn");
        writer
            .write(WriteBatch {
                decks: vec![sync(file(DECK, "Qdeck", T1), "SIG-A", None)],
                now_ms: SYNCED,
                ..WriteBatch::default()
            })
            .expect("queue");
        writer.flush().expect("flush");
        let reader = open_reader(&path).expect("reader");
        let id = deck(&reader, DECK).id;
        let archive = DeckEdit::Archive {
            deck_id: id,
            at_ms: 9,
        };
        assert_eq!(
            writer.edit_deck(archive.clone()).expect("edit"),
            EditOutcome::Done
        );
        assert_eq!(
            writer.edit_deck(archive).expect("edit"),
            EditOutcome::Archived
        );
        assert_eq!(deck(&reader, DECK).archived_at, Some(9));
    }

    // Reads

    #[test]
    fn versions_are_read_oldest_first_and_walk_the_unique_index() {
        let (_dir, mut conn) = db("decks-versions-of");
        deck_writes(
            &mut conn,
            vec![sync(file(DECK, "Qdeck", T2), "SIG-B", None)],
        );
        let id = deck(&conn, DECK).id;
        let b = version_id(&conn, DECK, "SIG-B");
        stored_match(&mut conn, TOKEN);
        deck_writes(&mut conn, vec![link(TOKEN, b)]);
        deck_writes(
            &mut conn,
            vec![sync(file(DECK, "Qdeck", T3), "SIG-A", None)],
        );
        conn.execute(
            "UPDATE deck_versions SET modified_at = ?1 WHERE signature = 'SIG-A'",
            [T1],
        )
        .expect("an older modified_at");
        let read = versions_of(&conn, id).expect("versions");
        assert_eq!(
            read.iter()
                .map(|v| v.signature.as_str())
                .collect::<Vec<_>>(),
            vec!["SIG-A", "SIG-B"]
        );
        assert!(read.iter().all(|v| v.deck_id == id));
        let query = plan(&conn, VERSIONS_OF, [id]);
        assert!(
            query.contains("sqlite_autoindex_deck_versions_1"),
            "plan:\n{query}"
        );
        assert!(!query.contains("SCAN deck_versions"), "plan:\n{query}");
    }

    #[test]
    fn a_missing_deck_reads_as_none() {
        let (_dir, conn) = db("decks-missing");
        assert_eq!(deck_by_mtgo_id(&conn, DECK).expect("read"), None);
    }

    #[test]
    fn a_deck_reads_by_its_id() {
        let (_dir, conn) = db("decks-by-id");
        conn.execute_batch(
            "INSERT INTO decks (id, mtgo_id, name, format, archived_at, created_at, updated_at, source)
             VALUES (7, 'q-7', 'Qdeck', 'CMODERN', 5, 1, 2, 'file');",
        )
        .expect("deck");
        let deck = deck_by_id(&conn, 7).expect("read").expect("found");
        assert_eq!(
            (deck.mtgo_id.as_str(), deck.name.as_str()),
            ("q-7", "Qdeck")
        );
        assert_eq!(deck.archived_at, Some(5));
        assert_eq!(deck_by_id(&conn, 8).expect("read"), None);
    }

    fn insert_card(conn: &Connection, catalog_id: u32, oracle_id: Option<&str>) {
        conn.execute(
            "INSERT INTO cards (catalog_id, oracle_id, name, type, sub_type, rarity, color_identity,
                 colors, cmc, mana_cost, set_name, set_code, image, art_crop)
             VALUES (?1, ?2, 'Qcard', 'Creature', 'Elf', 'common', 'G', 'G', 1.0, '{G}',
                 'Qset', 'QST', 'https://cards.scryfall.io/q.jpg', NULL)",
            params![catalog_id, oracle_id],
        )
        .expect("card");
    }

    #[test]
    fn card_infos_and_oracle_ids_return_only_known_cards() {
        let (_dir, conn) = db("decks-cards");
        insert_card(&conn, 1001, Some("oracle-1"));
        insert_card(&conn, 1002, None);
        let infos = card_infos(&conn, &[1001, 1002, 1003, 1001]).expect("infos");
        assert_eq!(infos.len(), 2);
        let first = &infos[&1001];
        assert_eq!(first.catalog_id, 1001);
        assert_eq!(first.oracle_id.as_deref(), Some("oracle-1"));
        assert_eq!(first.name.as_deref(), Some("Qcard"));
        assert_eq!(first.card_type.as_deref(), Some("Creature"));
        assert_eq!(first.sub_type.as_deref(), Some("Elf"));
        assert_eq!(first.rarity.as_deref(), Some("common"));
        assert_eq!(first.color_identity.as_deref(), Some("G"));
        assert_eq!(first.colors.as_deref(), Some("G"));
        assert_eq!(first.cmc, Some(1.0));
        assert_eq!(first.mana_cost.as_deref(), Some("{G}"));
        assert_eq!(first.set_name.as_deref(), Some("Qset"));
        assert_eq!(first.set_code.as_deref(), Some("QST"));
        assert_eq!(
            first.image.as_deref(),
            Some("https://cards.scryfall.io/q.jpg")
        );
        assert_eq!(first.art_crop, None);

        let oracles = oracle_ids(&conn, &[1001, 1002, 1003]).expect("oracles");
        assert_eq!(oracles.len(), 1);
        assert_eq!(oracles[&1001], "oracle-1");
        assert!(card_infos(&conn, &[]).expect("none").is_empty());
    }

    /// Every read that returns a remote image also returns its local file
    /// (cards spec section 7).
    #[test]
    fn card_reads_return_the_local_files() {
        let (_dir, conn) = db("decks-local-images");
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, oracle_id, name, image, art_crop, local_image,
                 local_art_crop, set_name, set_code)
             VALUES (7, 'o-q', 'Qbolt', 'https://cards.scryfall.io/7.jpg',
                 'https://cards.scryfall.io/7a.jpg', '7.jpg', '7_art.jpg', 'Qset', 'Q1');",
        )
        .expect("card");
        let infos = card_infos(&conn, &[7]).expect("infos");
        assert_eq!(infos[&7].local_image.as_deref(), Some("7.jpg"));
        assert_eq!(infos[&7].local_art_crop.as_deref(), Some("7_art.jpg"));
        let found = search_cards(&conn, "qbolt", 5).expect("search");
        assert_eq!(found[0].local_image.as_deref(), Some("7.jpg"));
        let options = cover_options(&conn, "Qbolt").expect("options");
        assert_eq!(options[0].local_art_crop.as_deref(), Some("7_art.jpg"));
    }

    /// 0.x `SearchCards`: name contains the term (any case, `%` and `_`
    /// literal), one printing per oracle id (the highest), by name, limited.
    #[test]
    fn a_card_search_finds_one_printing_per_card_by_name() {
        let (_dir, conn) = db("decks-card-search");
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, oracle_id, name, type, image) VALUES
                 (10, 'o-bolt', 'Qbolt', 'Instant', 'https://cards.scryfall.io/10.jpg'),
                 (12, 'o-bolt', 'Qbolt', 'Instant', 'https://cards.scryfall.io/12.jpg'),
                 (11, 'o-arc', 'Arc Qbolt', 'Sorcery', NULL),
                 (20, NULL, 'Qbolt Token', 'Token', NULL),
                 (21, NULL, 'Qbolt Token', 'Token', NULL),
                 (30, 'o-pct', '100% Qbolt_', NULL, NULL),
                 (40, 'o-other', 'Qmountain', 'Land', NULL);",
        )
        .expect("cards");
        let names = |found: &[CardSearchRow]| -> Vec<(u32, String)> {
            found
                .iter()
                .map(|card| (card.catalog_id, card.name.clone()))
                .collect()
        };
        let found = search_cards(&conn, "qBOLT", 20).expect("search");
        assert_eq!(
            names(&found),
            vec![
                (30, "100% Qbolt_".to_string()),
                (11, "Arc Qbolt".to_string()),
                (12, "Qbolt".to_string()),
                (20, "Qbolt Token".to_string()),
                (21, "Qbolt Token".to_string()),
            ]
        );
        assert_eq!(found[2].oracle_id.as_deref(), Some("o-bolt"));
        assert_eq!(found[2].card_type.as_deref(), Some("Instant"));
        assert_eq!(
            found[2].image.as_deref(),
            Some("https://cards.scryfall.io/12.jpg")
        );
        assert_eq!(
            names(&search_cards(&conn, "% Q", 20).expect("search")),
            vec![(30, "100% Qbolt_".to_string())]
        );
        assert_eq!(
            names(&search_cards(&conn, "t_", 20).expect("search")),
            vec![(30, "100% Qbolt_".to_string())]
        );
        assert_eq!(search_cards(&conn, "qbolt", 2).expect("search").len(), 2);
    }

    /// S1-6: a stub named from the game log (no oracle id, no image) is
    /// not listed beside the resolved card of the same name; a stub whose
    /// name no resolved card has still is.
    #[test]
    fn a_card_search_leaves_out_log_named_stubs_of_a_resolved_card() {
        let (_dir, conn) = db("decks-card-search-stubs");
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, oracle_id, name, type, image) VALUES
                 (10, 'o-bolt', 'Qbolt', 'Instant', 'https://cards.scryfall.io/10.jpg'),
                 (99, NULL, 'Qbolt', NULL, NULL),
                 (50, NULL, 'Qunknown', NULL, NULL);",
        )
        .expect("cards");
        let found: Vec<u32> = search_cards(&conn, "q", 20)
            .expect("search")
            .iter()
            .map(|card| card.catalog_id)
            .collect();
        assert_eq!(found, vec![10, 50]);
    }

    #[test]
    fn cover_options_are_the_printings_of_a_name_with_art() {
        let (_dir, conn) = db("decks-cover-options");
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, name, set_name, set_code, art_crop) VALUES
                 (3003, 'Qbolt', 'Qset three', 'Q3', 'https://cards.scryfall.io/art/3.jpg'),
                 (1001, 'Qbolt', 'Qset one', 'Q1', 'https://cards.scryfall.io/art/1.jpg'),
                 (2002, 'Qbolt', 'Qset two', 'Q2', NULL),
                 (4004, 'Qbolt', NULL, NULL, ''),
                 (5005, 'Qother', 'Qset one', 'Q1', 'https://cards.scryfall.io/art/5.jpg');",
        )
        .expect("cards");
        let options = cover_options(&conn, "Qbolt").expect("options");
        assert_eq!(
            options,
            vec![
                CoverOption {
                    catalog_id: 1001,
                    set_name: Some("Qset one".to_string()),
                    set_code: Some("Q1".to_string()),
                    art_crop: "https://cards.scryfall.io/art/1.jpg".to_string(),
                    local_art_crop: None,
                },
                CoverOption {
                    catalog_id: 3003,
                    set_name: Some("Qset three".to_string()),
                    set_code: Some("Q3".to_string()),
                    art_crop: "https://cards.scryfall.io/art/3.jpg".to_string(),
                    local_art_crop: None,
                },
            ]
        );
        assert!(cover_options(&conn, "Qnone").expect("none").is_empty());
    }

    #[test]
    fn account_versions_include_archived_decks_and_skip_other_accounts() {
        let (_dir, mut conn) = db("decks-account-versions");
        deck_writes(
            &mut conn,
            vec![
                sync(file(DECK, "Qdeck", T1), "SIG-A", None),
                DeckWrite::SyncFile {
                    file: file(OTHER_DECK, "Qother", T1),
                    signature: "SIG-X".to_string(),
                    account: Some("Qsecond".to_string()),
                    platform: Platform::Mtgo,
                    synced_at_ms: SYNCED,
                    identity: IdentityUpdate::Keep,
                },
            ],
        );
        let id = deck(&conn, DECK).id;
        edit_deck(
            &mut conn,
            &DeckEdit::Archive {
                deck_id: id,
                at_ms: 5,
            },
        )
        .expect("archive");
        let read = account_versions(&conn, "Qlocal").expect("versions");
        assert_eq!(
            read,
            vec![VersionRow {
                id: version_id(&conn, DECK, "SIG-A"),
                deck_id: id,
                signature: "SIG-A".to_string(),
                modified_at: T1,
            }]
        );
        assert!(account_versions(&conn, "Qnobody").expect("none").is_empty());
        let query = plan(&conn, ACCOUNT_VERSIONS, ["Qlocal"]);
        assert!(query.contains("decks_account_id"), "plan:\n{query}");
        assert!(!query.contains("SCAN"), "plan:\n{query}");
    }

    const THIRD: &str = "9d8c7b6a-5f4e-4d3c-8b2a-1f0e9d8c7b6a";
    const FOURTH: &str = "2b3c4d5e-6f70-4812-9a3b-4c5d6e7f8091";

    fn upsert(conn: &mut Connection, token: &str, started_at_ms: i64, state: MatchState) {
        let mut view = view(token, "Qopponent", started_at_ms);
        view.state = state;
        // Game IDs unique per match; the first game is game 1.
        let base = (started_at_ms / 1000) as u64;
        for (index, game) in view.games.iter_mut().enumerate() {
            game.mtgo_game_id = Some(base + index as u64);
        }
        view.games.reverse();
        commit(
            conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(view),
                    revision: 1,
                }],
                now_ms: SYNCED,
                ..WriteBatch::default()
            },
        )
        .expect("match");
    }

    #[test]
    fn unlinked_matches_are_recent_unlinked_and_newest_first() {
        let (_dir, mut conn) = db("decks-unlinked");
        upsert(&mut conn, TOKEN, 10_000, MatchState::Complete);
        upsert(&mut conn, OTHER, 20_000, MatchState::InProgress);
        upsert(&mut conn, THIRD, 30_000, MatchState::Ended);
        upsert(&mut conn, FOURTH, 40_000, MatchState::Complete);
        upsert(
            &mut conn,
            "5c6d7e8f-9012-4a3b-8c4d-5e6f70819203",
            45_000,
            MatchState::Abandoned,
        );
        let pending = "6d7e8f90-1234-4b5c-9d6e-7f8091a2b3c4";
        upsert(&mut conn, pending, 46_000, MatchState::Complete);
        conn.execute(
            "UPDATE matches SET state = 'pending' WHERE token = ?1",
            [pending],
        )
        .expect("a pending (imported) match");
        upsert(
            &mut conn,
            "7e8f9012-3456-4c6d-8e7f-8091a2b3c4d5",
            5_000,
            MatchState::Complete,
        );
        deck_writes(
            &mut conn,
            vec![sync(file(DECK, "Qdeck", T1), "SIG-A", None)],
        );
        let version = version_id(&conn, DECK, "SIG-A");
        deck_writes(&mut conn, vec![link(THIRD, version)]);

        let read = unlinked_matches_since(&conn, 10_000, 10, &HashSet::new()).expect("unlinked");
        assert_eq!(
            read,
            vec![
                UnlinkedMatch {
                    token: FOURTH.to_string(),
                    account: "Qlocal".to_string(),
                    game_ids: vec![Some(40), Some(41), Some(42)],
                    tournament_event_id: None,
                },
                UnlinkedMatch {
                    token: OTHER.to_string(),
                    account: "Qlocal".to_string(),
                    game_ids: vec![Some(20), Some(21), Some(22)],
                    tournament_event_id: None,
                },
                UnlinkedMatch {
                    token: TOKEN.to_string(),
                    account: "Qlocal".to_string(),
                    game_ids: vec![Some(10), Some(11), Some(12)],
                    tournament_event_id: None,
                },
            ],
            "linked, pending, abandoned and older matches are left out"
        );
        assert_eq!(
            unlinked_matches_since(&conn, 10_000, 1, &HashSet::new())
                .expect("limited")
                .len(),
            1
        );
    }

    #[test]
    fn skipped_matches_do_not_count_towards_the_limit() {
        let (_dir, mut conn) = db("decks-unlinked-skip");
        upsert(&mut conn, TOKEN, 10_000, MatchState::Complete);
        upsert(&mut conn, OTHER, 20_000, MatchState::Complete);
        upsert(&mut conn, THIRD, 30_000, MatchState::Complete);
        let skip: HashSet<String> = [THIRD.to_string(), OTHER.to_string()].into();
        let read = unlinked_matches_since(&conn, 0, 1, &skip).expect("unlinked");
        let tokens: Vec<&str> = read.iter().map(|found| found.token.as_str()).collect();
        assert_eq!(tokens, vec![TOKEN], "the newest match not skipped");
    }

    #[test]
    fn an_arena_match_is_not_a_relink_candidate() {
        let (_dir, mut conn) = db("decks-unlinked-arena");
        upsert(&mut conn, TOKEN, 10_000, MatchState::Complete);
        upsert(&mut conn, OTHER, 20_000, MatchState::Complete);
        crate::test_support::move_match_to_arena(&conn, OTHER);
        let read = unlinked_matches_since(&conn, 0, 10, &HashSet::new()).expect("unlinked");
        let tokens: Vec<&str> = read.iter().map(|found| found.token.as_str()).collect();
        assert_eq!(tokens, vec![TOKEN]);
    }

    #[test]
    fn a_hidden_match_is_not_a_relink_candidate() {
        let (_dir, mut conn) = db("decks-unlinked-hidden");
        upsert(&mut conn, TOKEN, 10_000, MatchState::Complete);
        upsert(&mut conn, OTHER, 20_000, MatchState::Complete);
        hide(&conn, OTHER);
        let read = unlinked_matches_since(&conn, 0, 1, &HashSet::new()).expect("unlinked");
        let tokens: Vec<&str> = read.iter().map(|found| found.token.as_str()).collect();
        assert_eq!(tokens, vec![TOKEN]);
    }

    #[test]
    fn a_match_without_games_is_not_unlinked() {
        let (_dir, mut conn) = db("decks-unlinked-no-games");
        let mut view = view(TOKEN, "Qopponent", 10_000);
        view.games.clear();
        commit(
            &mut conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(view),
                    revision: 1,
                }],
                now_ms: SYNCED,
                ..WriteBatch::default()
            },
        )
        .expect("match");
        assert!(
            unlinked_matches_since(&conn, 0, 10, &HashSet::new())
                .expect("read")
                .is_empty()
        );
    }

    #[test]
    fn game_one_without_a_game_id_stays_first_and_game_two_never_takes_its_place() {
        let (_dir, mut conn) = db("decks-unlinked-game-one");
        let mut view = view(TOKEN, "Qopponent", 10_000);
        view.games[0].mtgo_game_id = None;
        view.games.reverse();
        let mut no_ids = view.clone();
        no_ids.token = OTHER.to_string();
        no_ids.mtgo_match_id = Some(98);
        for game in &mut no_ids.games {
            game.mtgo_game_id = None;
        }
        commit(
            &mut conn,
            &WriteBatch {
                matches: vec![
                    MatchWrite::Upsert {
                        view: Box::new(view),
                        revision: 1,
                    },
                    MatchWrite::Upsert {
                        view: Box::new(no_ids),
                        revision: 1,
                    },
                ],
                now_ms: SYNCED,
                ..WriteBatch::default()
            },
        )
        .expect("match");
        let mut read = unlinked_matches_since(&conn, 0, 10, &HashSet::new()).expect("read");
        read.sort_by(|a, b| a.token.cmp(&b.token));
        assert_eq!(
            read,
            vec![
                UnlinkedMatch {
                    token: OTHER.to_string(),
                    account: "Qlocal".to_string(),
                    game_ids: vec![None, None, None],
                    tournament_event_id: None,
                },
                UnlinkedMatch {
                    token: TOKEN.to_string(),
                    account: "Qlocal".to_string(),
                    game_ids: vec![None, Some(8), Some(9)],
                    tournament_event_id: None,
                },
            ],
            "games in game order; a registered list can still link a match without Game IDs"
        );
    }

    #[test]
    fn unlinked_matches_walk_the_started_at_index() {
        let (_dir, conn) = db("decks-unlinked-plan");
        let query = plan(&conn, UNLINKED_MATCHES_SINCE, params![0, 200]);
        assert!(query.contains("matches_started_at"), "plan:\n{query}");
        assert!(!query.contains("SCAN m"), "plan:\n{query}");
        assert!(!query.contains("SCAN g"), "plan:\n{query}");
        let games = plan(&conn, GAME_IDS_OF, [1]);
        assert!(games.contains("sqlite_autoindex_games_1"), "plan:\n{games}");
        assert!(!games.contains("SCAN games"), "plan:\n{games}");
    }

    fn deck_used(file: i64, offset: u64, game_id: u64, raw: &str) -> crate::write::NewEvent {
        event_with(
            file,
            offset,
            EventKind::DeckUsed,
            Subjects {
                game_id: Some(game_id),
                ..Subjects::default()
            },
            raw,
        )
    }

    #[test]
    fn deck_used_raws_are_every_event_of_the_game_in_log_order() {
        let (_dir, mut conn) = db("decks-deck-used-all");
        let log = insert_log_file(&conn, &new_log_file("C:/mtgo.log")).expect("file");
        commit(
            &mut conn,
            &WriteBatch {
                events: vec![
                    deck_used(log, 9, 7, "second"),
                    deck_used(log, 5, 7, "first"),
                    deck_used(log, 7, 8, "other game"),
                ],
                ..WriteBatch::default()
            },
        )
        .expect("commit");
        assert_eq!(
            deck_used_raws(&conn, 7).expect("raws"),
            vec!["first".to_string(), "second".to_string()]
        );
        assert!(deck_used_raws(&conn, 99).expect("raws").is_empty());
    }

    #[test]
    fn deck_used_raws_keep_only_the_earliest_few_of_a_flooded_game() {
        let (_dir, mut conn) = db("decks-deck-used-flood");
        let log = insert_log_file(&conn, &new_log_file("C:/mtgo.log")).expect("file");
        let events = (0..40)
            .rev()
            .map(|offset| deck_used(log, offset, 7, &format!("line {offset:02}")))
            .collect();
        commit(
            &mut conn,
            &WriteBatch {
                events,
                ..WriteBatch::default()
            },
        )
        .expect("commit");
        let raws = deck_used_raws(&conn, 7).expect("raws");
        let want: Vec<String> = (0..MAX_DECK_USED_RAWS)
            .map(|offset| format!("line {offset:02}"))
            .collect();
        assert_eq!(raws, want);
    }

    #[test]
    fn deck_used_raw_is_the_first_deck_used_event_of_the_game_decoded() {
        let (_dir, mut conn) = db("decks-deck-used");
        let log = insert_log_file(&conn, &new_log_file("C:/mtgo.log")).expect("file");
        let long = format!(
            "Deck Used in Game ID: 7 [{}]",
            "{\"CatalogId\":1001,\"Quantity\":4,\"Annotation\":\"NotSet\",\"InSideboard\":false},"
                .repeat(COMPRESS_OVER / 60)
                .trim_end_matches(',')
        );
        assert!(long.len() > COMPRESS_OVER, "stored compressed");
        let mut quarantined = deck_used(log, 5, 7, "quarantined");
        quarantined.quarantined = true;
        let other_kind = event_with(
            log,
            8,
            EventKind::GameStateUpdate,
            Subjects {
                game_id: Some(7),
                ..Subjects::default()
            },
            "a game state",
        );
        commit(
            &mut conn,
            &WriteBatch {
                events: vec![
                    deck_used(log, 30, 7, "Deck Used in Game ID: 7 later"),
                    deck_used(log, 10, 7, &long),
                    deck_used(log, 20, 8, "Deck Used in Game ID: 8"),
                    quarantined,
                    other_kind,
                ],
                now_ms: SYNCED,
                ..WriteBatch::default()
            },
        )
        .expect("events");
        assert_eq!(deck_used_raw(&conn, 7).expect("read"), Some(long));
        assert_eq!(
            deck_used_raw(&conn, 8).expect("read").as_deref(),
            Some("Deck Used in Game ID: 8")
        );
        assert_eq!(deck_used_raw(&conn, 9).expect("read"), None);
        let query = plan(&conn, DECK_USED_RAW, [7]);
        assert!(query.contains("log_events_game_id"), "plan:\n{query}");
        assert!(!query.contains("SCAN log_events"), "plan:\n{query}");
    }

    fn registered(file: i64, offset: u64, token: &str, raw: &str) -> crate::write::NewEvent {
        event_with(
            file,
            offset,
            EventKind::MatchDeckRegistered,
            Subjects {
                match_token: Some(token.to_string()),
                match_id: Some(99),
                ..Subjects::default()
            },
            raw,
        )
    }

    #[test]
    fn registered_deck_raw_is_the_first_registered_event_of_the_match_decoded() {
        let (_dir, mut conn) = db("decks-registered");
        let log = insert_log_file(&conn, &new_log_file("C:/mtgo.log")).expect("file");
        let long = format!(
            "FlsMatchDeckGetRespMessage {{\"Cards\":[{}]}}",
            "{\"CatalogID\":1001,\"Quantity\":4,\"InSideboard\":false},"
                .repeat(COMPRESS_OVER / 50)
                .trim_end_matches(',')
        );
        assert!(long.len() > COMPRESS_OVER, "stored compressed");
        let mut quarantined = registered(log, 5, TOKEN, "quarantined");
        quarantined.quarantined = true;
        commit(
            &mut conn,
            &WriteBatch {
                events: vec![
                    registered(log, 30, TOKEN, "after game 2"),
                    registered(log, 10, TOKEN, &long),
                    registered(log, 20, OTHER, "another match"),
                    quarantined,
                    token_event(log, 8, TOKEN),
                ],
                now_ms: SYNCED,
                ..WriteBatch::default()
            },
        )
        .expect("events");
        assert_eq!(registered_deck_raw(&conn, TOKEN).expect("read"), Some(long));
        assert_eq!(
            registered_deck_raw(&conn, OTHER).expect("read").as_deref(),
            Some("another match")
        );
        assert_eq!(
            registered_deck_raw(&conn, "9d8c7b6a-5f4e-4d3c-8b2a-1f0e9d8c7b6a").expect("read"),
            None
        );
        let query = plan(&conn, REGISTERED_DECK_RAW, [TOKEN]);
        // Migration 10's `log_events_registered` (built for the Limited
        // pages) serves this lookup too.
        assert!(
            query.contains("log_events_match_token") || query.contains("log_events_registered"),
            "plan:\n{query}"
        );
        assert!(!query.contains("SCAN log_events"), "plan:\n{query}");
    }

    fn tournament_deck_line(
        file: i64,
        offset: u64,
        tournament: u64,
        cards: &str,
    ) -> crate::write::NewEvent {
        event_with(
            file,
            offset,
            EventKind::MatchDeckRegistered,
            Subjects {
                tournament_token: Some("c8eed54f-8182-44d5-adb0-dc8cb4ac95f7".to_string()),
                ..Subjects::default()
            },
            &format!(
                "(Game Management|Processing Registered Handler for FlsTournamentDeckRespMessage in X) \
                 Message: {{\"TournamentID\":{tournament},\"DeckCards\":{cards},\"ReturnCode\":0}}"
            ),
        )
    }

    fn one_card(catalog_id: u32) -> String {
        format!("[{{\"CatalogID\":{catalog_id},\"Quantity\":4,\"InSideboard\":false}}]")
    }

    #[test]
    fn tournament_deck_is_the_first_list_logged_for_the_tournament() {
        let (_dir, mut conn) = db("decks-tournament-deck");
        let log = insert_log_file(&conn, &new_log_file("C:/mtgo.log")).expect("file");
        let mut quarantined = tournament_deck_line(log, 5, 500, &one_card(1));
        quarantined.quarantined = true;
        commit(
            &mut conn,
            &WriteBatch {
                events: vec![
                    tournament_deck_line(log, 30, 500, &one_card(3)),
                    tournament_deck_line(log, 10, 500, "[]"),
                    tournament_deck_line(log, 20, 500, &one_card(2)),
                    tournament_deck_line(log, 15, 600, &one_card(6)),
                    quarantined,
                    registered(log, 12, TOKEN, &format!(
                        "FlsMatchDeckGetRespMessage {{\"MatchToken\":\"{TOKEN}\",\"Cards\":{}}}",
                        one_card(9)
                    )),
                ],
                now_ms: SYNCED,
                ..WriteBatch::default()
            },
        )
        .expect("events");
        let card = |catalog_id| {
            Some(vec![DeckCard {
                catalog_id,
                quantity: 4,
                sideboard: false,
            }])
        };
        assert_eq!(
            tournament_deck(&conn, 500).expect("read"),
            card(2),
            "an empty (finished) answer is skipped"
        );
        assert_eq!(tournament_deck(&conn, 600).expect("read"), card(6));
        assert_eq!(tournament_deck(&conn, 700).expect("read"), None);
        let query = plan(&conn, TOURNAMENT_DECK_RAWS, []);
        assert!(query.contains("log_events_registered"), "plan:\n{query}");
        assert!(!query.contains("SCAN log_events"), "plan:\n{query}");
    }

    #[test]
    fn pruning_finds_links_through_the_version_index() {
        let (_dir, conn) = db("decks-prune-plan");
        let query = plan(&conn, PRUNE_VERSIONS, [1, 2]);
        assert!(query.contains("matches_deck_version_id"), "plan:\n{query}");
        assert!(!query.contains("SCAN"), "plan:\n{query}");
    }

    // Tournament siblings

    /// A tournament round: `(token, account id, event id, minutes after T1,
    /// linked version)`.
    type Round = (&'static str, i64, Option<i64>, i64, Option<i64>);

    /// Qlocal's tournament rounds, plus one Qother round in event 700.
    fn tournament_rounds(conn: &Connection) {
        conn.execute_batch(
            "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0), (2, 'Qother', 0);
             INSERT INTO decks (id, mtgo_id, account_id, name, format, created_at, updated_at, source)
                 VALUES (1, 'Q-deck', 1, 'Qdeck', 'CMODERN', 1, 1, 'file');
             INSERT INTO deck_versions (id, deck_id, signature, modified_at)
                 VALUES (10, 1, 'Q-one', 1), (11, 1, 'Q-two', 2);",
        )
        .expect("decks");
        let rounds: [Round; 10] = [
            ("Q-a", 1, Some(500), 0, Some(10)),
            ("Q-b", 1, Some(500), 1, None),
            ("Q-c", 1, Some(600), 2, Some(10)),
            ("Q-d", 1, Some(600), 3, Some(11)),
            ("Q-e", 1, Some(600), 4, None),
            ("Q-f", 1, Some(700), 5, None),
            ("Q-g", 2, Some(700), 6, Some(10)),
            ("Q-h", 1, None, 7, None),
            ("Q-i", 1, Some(800), 8, None),
            ("Q-j", 1, Some(800), 9, None),
        ];
        for (token, account, event, minutes, version) in rounds {
            conn.execute(
                "INSERT INTO matches (token, account_id, source, tournament_event_id, state,
                     outcome, started_at, last_activity_at, revision, derived_version)
                 VALUES (?1, ?2, 'log', ?3, 'complete', 'win', ?4, ?4, 1, 4)",
                params![token, account, event, T1 + minutes * 60_000],
            )
            .expect("match");
            if let Some(version) = version {
                conn.execute(
                    "UPDATE matches SET deck_version_id = ?2 WHERE token = ?1",
                    params![token, version],
                )
                .expect("link");
            }
        }
    }

    #[test]
    fn an_unlinked_tournament_round_takes_the_one_version_its_rounds_use() {
        let (_dir, conn) = db("decks-tournament-siblings");
        tournament_rounds(&conn);
        assert_eq!(
            tournament_sibling_versions(&conn, 50).expect("read"),
            vec![SiblingVersion {
                token: "Q-b".to_string(),
                deck_version_id: 10,
            }],
            "two versions (600), another account's round (700), no linked round (800) and \
             a match outside a tournament give nothing"
        );
        // 0.x looks at the newest unlinked rounds only, whether or not they can
        // inherit.
        assert!(
            tournament_sibling_versions(&conn, 3)
                .expect("read")
                .is_empty()
        );
    }

    #[test]
    fn tournament_siblings_walk_the_tournament_index() {
        let (_dir, conn) = db("decks-siblings-plan");
        let query = plan(&conn, TOURNAMENT_SIBLING_VERSIONS, [50]);
        assert!(
            query.contains("SCAN matches USING INDEX matches_account_tournament"),
            "plan:\n{query}"
        );
        assert!(
            query.contains("SEARCH s USING INDEX matches_account_tournament"),
            "plan:\n{query}"
        );
        assert!(!query.contains("SCAN s"), "plan:\n{query}");
    }
}
