//! Cross-device sync state (07 spec sections 6 and 10, survey section 12):
//! what the server holds of each synced row (`synced_rows`), each type's
//! run state (`sync_state`), the rows the server refused
//! (`sync_rejections`), the deck slot ledger's mirror
//! (`decks.cloud_sync_enabled` and the settings row `sync_slots`) and the
//! last run's error (`sync_last_error`).
//!
//! The API worker reads with a read-only connection and writes through
//! tracker commands (`SyncEdit`, applied on the writer thread), like the
//! outbox. Like 0.x's `DirtyRows`, `ApplyDeckSyncSlots` and `SyncState`,
//! with the sync columns kept off the domain rows.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use chrono_tz::Tz;
use mtgo_protocol::local_to_utc_ms;
use mymtgo_api::{Slots, SyncType, Tier, slots_from_json, slots_json};
use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::Value;
use sync_bundles::endstep_cards::{EndstepDeckCard, endstep_signature};
use sync_bundles::league::{
    ArenaRunBundle, DraftInput, LeagueBundleInput, PickInput, PulledLeagueFields, RunSnapshot,
    SnapshotInput, arena_run_bundle,
};
use sync_bundles::match_bundle::{
    ArchetypeSide, CardRowInput, CastInput, DeckRow, FrameInput, MatchBundleInput, MatchGameInput,
};
use sync_bundles::{
    ArenaPlayerFields, BundlePlatform, Carry, DeckParts, EndstepCardRef, EndstepRankedFields,
    GuideCardInput, GuideInput, LEAGUE_CARRY, NoteInput, UNPLACED_ARCHETYPE_UUID,
    UNPLACED_ARCHETYPES, UNPLACED_GUIDES, UNPLACED_NOTES, bundle_hash, canonical_json,
    deck_client_id, guides_section, notes_section, read_guides, read_notes,
};

use crate::api_reads::held_until;
use crate::game_sync::Side;
use crate::import::{PulledMatch, import_pulled_match};
use crate::import_decks::{ImportedDeck, import_pulled_deck};
use crate::import_leagues::{ImportedLeague, import_pulled_league};
use crate::import_limited::{ImportedLimited, LegacyDraft, LegacyLimitedDeck, import_limited_rows};
use crate::platform::{account_of_mtgo, account_of_platform};
use crate::{Connection, StoreError};

/// Settings row: the server's deck slot ledger as 0.x stored it.
pub const SYNC_SLOTS: &str = "sync_slots";
/// Settings row: the last sync run's error, or null.
pub const SYNC_LAST_ERROR: &str = "sync_last_error";

/// The rejection the server gives a match or league whose deck holds no
/// slot (cleared once the deck is enabled).
pub const DECK_NOT_SYNCED: &str = "deck_not_synced";

/// What this device knows of one synced row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncedRow {
    pub client_id: String,
    /// What the server holds (0.x `synced_hash`).
    pub hash: Option<String>,
    pub synced_at: Option<i64>,
    /// The hash of the last rebuild.
    pub built_hash: Option<String>,
    /// The cheap key the last rebuild was made at.
    pub touch_key: Option<String>,
    /// The manifest's `updated_at` (ms).
    pub changed_at: Option<i64>,
    /// The JSON of the bundle parts v1 has no home for (`Carry::to_json`).
    pub carry: Option<String>,
    /// The touch key a manual match or league had when it arrived from the
    /// server; while its key still equals it, the row is the server's copy.
    pub pulled_key: Option<String>,
}

/// One type's run state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncStateRow {
    pub last_synced_at: Option<i64>,
    pub canonical_version: i64,
    /// The last pushed client id of an unfinished full run.
    pub full_cursor: Option<String>,
    /// The server's download cursor from the last finished run, exactly as
    /// the server sent it.
    pub download_cursor: Option<String>,
}

/// A deck the sync can send: its bundle columns, whether v1 owns it (a file
/// deck, ruling 5) and its touch key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckCandidate {
    pub deck_id: i64,
    pub client_id: String,
    pub mtgo_id: String,
    pub name: String,
    pub original_name: Option<String>,
    pub format: String,
    pub color_identity: Option<String>,
    pub cover_catalog_id: Option<i64>,
    /// The game of the deck's account; with no account, its
    /// `pulled_platform`, else MTGO.
    pub platform: BundlePlatform,
    /// `source = 'file'`: only these are pushed.
    pub owned: bool,
    pub updated_at: i64,
    /// `updated_at`, version count, newest `modified_at`, `cloud_sync_enabled`,
    /// the guide and note parts and the archetype uuid (`deck_touch_key`).
    pub touch_key: String,
}

/// A rebuilt row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltRow {
    pub client_id: String,
    pub built_hash: String,
    pub touch_key: String,
    pub changed_at: i64,
}

/// A row the server (or the local size check) refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub client_id: String,
    pub reason: String,
    pub hash: String,
}

/// One row of the Account page's deck sync list (0.x `BuildDeckSyncList`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckSyncRow {
    pub id: i64,
    pub name: String,
    pub format: String,
    pub client_id: String,
    /// The newest match played with the deck (ms).
    pub last_played_at: Option<i64>,
    pub cloud_sync_enabled: bool,
    /// The deck's platform: its account's, else the one it was pulled for
    /// (`pulled_platform`), else MTGO.
    pub platform: crate::Platform,
}

/// A change to the sync state, applied in its own transaction on the
/// writer thread.
#[derive(Debug, Clone, PartialEq)]
pub enum SyncEdit {
    /// Rows rebuilt during the manifest phase.
    Built {
        sync_type: SyncType,
        rows: Vec<BuiltRow>,
    },
    /// Carries fetched from the server: `(client_id, carry JSON)`. A row
    /// with a new carry is built again.
    Carried {
        sync_type: SyncType,
        rows: Vec<(String, String)>,
    },
    /// The server did not give these rows' carries back at all (a deck
    /// switched off there, a blob missing on its disk; the API says ask
    /// again later). Each row's carry stays unset, so it is asked for at the
    /// next run, until it has been asked `max_asks` times and `min_age_ms`
    /// has passed since the first unanswered ask: then it settles as an
    /// empty carry (runs come in bursts, so a count alone is not time).
    CarryWithheld {
        sync_type: SyncType,
        client_ids: Vec<String>,
        max_asks: u32,
        min_age_ms: i64,
        now_ms: i64,
    },
    /// The server stored these rows: `(client_id, hash)`. Their rejection
    /// goes; a row without a carry gets an empty one (nothing to fetch).
    Stored {
        sync_type: SyncType,
        rows: Vec<(String, String)>,
        now_ms: i64,
    },
    /// Refused rows, replacing any earlier rejection.
    Rejected {
        sync_type: SyncType,
        rows: Vec<Rejection>,
    },
    /// A full run pushed a batch up to this client id.
    Cursor {
        sync_type: SyncType,
        full_cursor: String,
        canonical_version: i64,
    },
    /// The type's run finished: the time it started, the bundle version,
    /// no full cursor, and the final manifest call's download cursor
    /// (`None`, from an older server, keeps the stored one).
    Finished {
        sync_type: SyncType,
        last_synced_at: i64,
        canonical_version: i64,
        download_cursor: Option<String>,
    },
    /// The server's slot ledger (only one that lists its decks). Counts the
    /// deck flags it changed.
    ApplySlots { slots: Slots },
    /// A deck pulled from the cloud, through the deck import path, with its
    /// archetype, guides and notes, its server hash and carry.
    PulledDeck {
        client_id: String,
        deck: Box<ImportedDeck>,
        parts: Box<PulledDeckParts>,
        hash: String,
        carry: String,
        now_ms: i64,
    },
    /// A match pulled from the cloud, through the 0.x match import path,
    /// with its server hash and carry.
    PulledMatch {
        client_id: String,
        pulled: Box<PulledMatch>,
        hash: String,
        carry: String,
        /// The OS timezone, for the league sync a pulled player edit runs.
        timezone: Tz,
        now_ms: i64,
    },
    /// A league pulled from the cloud, through the 0.x league and limited
    /// import paths, with its server hash and carry.
    PulledLeague {
        client_id: String,
        pulled: Box<PulledLeague>,
        hash: String,
        carry: String,
        /// The OS timezone, for the league sync a pulled player edit runs.
        timezone: Tz,
        now_ms: i64,
    },
    /// An Arena course run pulled from the cloud (cross-device sync spec
    /// section 8): kept in `synced_arena_runs` as a run fold input, never
    /// through the league import, then the account's runs folded again.
    PulledArenaRun {
        /// The server's `updated_at` of the pulled copy (ms), when the API
        /// sends it: kept as the row's `changed_at`, so a merge sent later
        /// is newer than it.
        server_updated_at_ms: Option<i64>,
        client_id: String,
        run: Box<ArenaRunBundle>,
        hash: String,
        carry: String,
        /// The OS timezone, for the league sync a pulled player edit runs.
        timezone: Tz,
        now_ms: i64,
    },
    /// The Arena runs folded once after a pull's matches are in (cross-device
    /// sync spec section 8): pulled limited matches join their course's run.
    ArenaRunsAfterPull { now_ms: i64 },
    /// Pulled leagues' registered decks again, once their matches are here
    /// (a league is pulled before its matches; 0.x re-linked its snapshot
    /// when the match arrived).
    PulledSnapshots {
        decks: Vec<LegacyLimitedDeck>,
        now_ms: i64,
    },
    /// Matches deleted on the website (the manifest's `tombstones`, league
    /// self-management spec 6.4), listed since `since_ms` (every one when
    /// `None`): hidden here without an edit. Counts the matches hidden.
    Tombstones {
        client_ids: Vec<String>,
        since_ms: Option<i64>,
        now_ms: i64,
    },
    /// The last run's error (`None` clears it).
    LastError { error: Option<String> },
    /// Sign-out: no account, no slots (0.x `SyncController::unlink`).
    SignedOut,
}

/// Apply a sync edit in its own transaction and say how many rows it
/// changed.
pub fn edit_sync(conn: &mut Connection, edit: &SyncEdit) -> Result<u32, StoreError> {
    let tx = conn.transaction()?;
    let changed = match edit {
        SyncEdit::Built { sync_type, rows } => built(&tx, *sync_type, rows)?,
        SyncEdit::Carried { sync_type, rows } => carried(&tx, *sync_type, rows)?,
        SyncEdit::CarryWithheld {
            sync_type,
            client_ids,
            max_asks,
            min_age_ms,
            now_ms,
        } => carry_withheld(&tx, *sync_type, client_ids, *max_asks, *min_age_ms, *now_ms)?,
        SyncEdit::Stored {
            sync_type,
            rows,
            now_ms,
        } => stored(&tx, *sync_type, rows, *now_ms)?,
        SyncEdit::Rejected { sync_type, rows } => rejected(&tx, *sync_type, rows)?,
        SyncEdit::Cursor {
            sync_type,
            full_cursor,
            canonical_version,
        } => tx.execute(
            "INSERT INTO sync_state (type, canonical_version, full_cursor) VALUES (?1, ?2, ?3)
             ON CONFLICT (type) DO UPDATE SET full_cursor = excluded.full_cursor",
            params![sync_type.as_str(), canonical_version, full_cursor],
        )? as u32,
        SyncEdit::Finished {
            sync_type,
            last_synced_at,
            canonical_version,
            download_cursor,
        } => tx.execute(
            "INSERT INTO sync_state
                 (type, last_synced_at, canonical_version, full_cursor, download_cursor)
             VALUES (?1, ?2, ?3, NULL, ?4)
             ON CONFLICT (type) DO UPDATE SET last_synced_at = excluded.last_synced_at,
                 canonical_version = excluded.canonical_version, full_cursor = NULL,
                 download_cursor = COALESCE(excluded.download_cursor,
                     sync_state.download_cursor)",
            params![
                sync_type.as_str(),
                last_synced_at,
                canonical_version,
                download_cursor
            ],
        )? as u32,
        SyncEdit::ApplySlots { slots } => apply_slots(&tx, slots)?,
        SyncEdit::PulledDeck {
            client_id,
            deck,
            parts,
            hash,
            carry,
            now_ms,
        } => pulled_deck(&tx, client_id, deck, parts, hash, carry, *now_ms)?,
        SyncEdit::PulledMatch {
            client_id,
            pulled,
            hash,
            carry,
            timezone,
            now_ms,
        } => pulled_match(&tx, client_id, pulled, hash, carry, *timezone, *now_ms)?,
        SyncEdit::PulledLeague {
            client_id,
            pulled,
            hash,
            carry,
            timezone,
            now_ms,
        } => pulled_league(&tx, client_id, pulled, hash, carry, *timezone, *now_ms)?,
        SyncEdit::PulledArenaRun {
            server_updated_at_ms,
            client_id,
            run,
            hash,
            carry,
            timezone,
            now_ms,
        } => pulled_arena_run(
            &tx,
            client_id,
            run,
            &PulledRow {
                hash,
                carry,
                server_updated_at_ms: *server_updated_at_ms,
            },
            *timezone,
            *now_ms,
        )?,
        SyncEdit::ArenaRunsAfterPull { now_ms } => {
            crate::arena_run_sync::sync_all_arena_runs_guarded(&tx, *now_ms)?;
            0
        }
        SyncEdit::Tombstones {
            client_ids,
            since_ms,
            now_ms,
        } => {
            forget_parked(&tx, SyncType::Match, client_ids)?;
            crate::sync_player::apply_tombstones(&tx, client_ids, *since_ms, *now_ms)?
        }
        SyncEdit::PulledSnapshots { decks, now_ms } => {
            let mut league_ids = Vec::new();
            for deck in decks {
                league_ids.extend(league_of_match(&tx, &deck.match_token)?);
            }
            let unedited = unedited_leagues(&tx, &league_ids)?;
            let limited = ImportedLimited {
                drafts: Vec::new(),
                decks: decks.clone(),
                league_keys: None,
            };
            let counts = import_limited_rows(&tx, &limited, *now_ms)?;
            refresh_pulled_keys(&tx, &unedited)?;
            counts.decks_added + counts.decks_updated
        }
        SyncEdit::LastError { error } => {
            let value = serde_json::to_string(error).unwrap_or_else(|_| "null".to_string());
            set_setting(&tx, SYNC_LAST_ERROR, &value)?
        }
        SyncEdit::SignedOut => {
            let slots = tx.execute("DELETE FROM settings WHERE key = ?1", [SYNC_SLOTS])?;
            let flags = tx.execute(
                "UPDATE decks SET cloud_sync_enabled = 0 WHERE cloud_sync_enabled = 1",
                [],
            )?;
            (slots + flags) as u32
        }
    };
    tx.commit()?;
    Ok(changed)
}

fn set_setting(tx: &Transaction, key: &str, value: &str) -> Result<u32, StoreError> {
    Ok(tx.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )? as u32)
}

fn built(tx: &Transaction, sync_type: SyncType, rows: &[BuiltRow]) -> Result<u32, StoreError> {
    let mut statement = tx.prepare_cached(
        "INSERT INTO synced_rows (type, client_id, built_hash, touch_key, changed_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (type, client_id) DO UPDATE SET built_hash = excluded.built_hash,
             touch_key = excluded.touch_key, changed_at = excluded.changed_at",
    )?;
    let mut changed = 0;
    for row in rows {
        changed += statement.execute(params![
            sync_type.as_str(),
            row.client_id,
            row.built_hash,
            row.touch_key,
            row.changed_at
        ])? as u32;
    }
    Ok(changed)
}

fn carried(
    tx: &Transaction,
    sync_type: SyncType,
    rows: &[(String, String)],
) -> Result<u32, StoreError> {
    let mut statement = tx.prepare_cached(
        "INSERT INTO synced_rows (type, client_id, carry) VALUES (?1, ?2, ?3)
         ON CONFLICT (type, client_id) DO UPDATE SET carry = excluded.carry,
             built_hash = NULL",
    )?;
    let mut changed = 0;
    for (client_id, carry) in rows {
        changed += statement.execute(params![sync_type.as_str(), client_id, carry])? as u32;
    }
    Ok(changed)
}

/// Count one more unanswered ask for each row's carry (noting when the
/// first one was); a row asked `max_asks` times, the first at least
/// `min_age_ms` ago, takes an empty carry (nothing to fetch any more).
fn carry_withheld(
    tx: &Transaction,
    sync_type: SyncType,
    client_ids: &[String],
    max_asks: u32,
    min_age_ms: i64,
    now_ms: i64,
) -> Result<u32, StoreError> {
    let mut count = tx.prepare_cached(
        "UPDATE synced_rows SET carry_asks = carry_asks + 1,
             carry_first_ask_at = COALESCE(carry_first_ask_at, ?3)
         WHERE type = ?1 AND client_id = ?2 AND carry IS NULL",
    )?;
    let mut settle = tx.prepare_cached(
        "UPDATE synced_rows SET carry = '{}', built_hash = NULL
         WHERE type = ?1 AND client_id = ?2 AND carry IS NULL AND carry_asks >= ?3
             AND ?4 - carry_first_ask_at >= ?5",
    )?;
    let mut changed = 0;
    for client_id in client_ids {
        changed += count.execute(params![sync_type.as_str(), client_id, now_ms])? as u32;
        settle.execute(params![
            sync_type.as_str(),
            client_id,
            max_asks,
            now_ms,
            min_age_ms
        ])?;
    }
    Ok(changed)
}

fn stored(
    tx: &Transaction,
    sync_type: SyncType,
    rows: &[(String, String)],
    now_ms: i64,
) -> Result<u32, StoreError> {
    let mut statement = tx.prepare_cached(
        "INSERT INTO synced_rows (type, client_id, hash, synced_at, carry)
         VALUES (?1, ?2, ?3, ?4, '{}')
         ON CONFLICT (type, client_id) DO UPDATE SET hash = excluded.hash,
             synced_at = excluded.synced_at, carry = COALESCE(synced_rows.carry, '{}')",
    )?;
    let mut forget =
        tx.prepare_cached("DELETE FROM sync_rejections WHERE type = ?1 AND client_id = ?2")?;
    let mut changed = 0;
    for (client_id, hash) in rows {
        changed += statement.execute(params![sync_type.as_str(), client_id, hash, now_ms])? as u32;
        forget.execute(params![sync_type.as_str(), client_id])?;
    }
    Ok(changed)
}

fn rejected(tx: &Transaction, sync_type: SyncType, rows: &[Rejection]) -> Result<u32, StoreError> {
    let mut statement = tx.prepare_cached(
        "INSERT INTO sync_rejections (type, client_id, reason, hash) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (type, client_id) DO UPDATE SET reason = excluded.reason,
             hash = excluded.hash",
    )?;
    let mut changed = 0;
    for row in rows {
        changed += statement.execute(params![
            sync_type.as_str(),
            row.client_id,
            row.reason,
            row.hash
        ])? as u32;
    }
    Ok(changed)
}

/// 0.x `ApplyDeckSyncSlots`: store the ledger, then set each deck's flag
/// from it without touching `updated_at`. An archived deck is only ever
/// lowered (its slot is released by the run). A deck that becomes enabled
/// forgets the hash of its matches and leagues, so they are offered again.
/// Parked `deck_not_synced` rejections of every enabled deck's matches and
/// leagues go, keyed off the flag (a deck re-enabled elsewhere between two
/// runs arrives already enabled).
fn apply_slots(tx: &Transaction, slots: &Slots) -> Result<u32, StoreError> {
    set_setting(tx, SYNC_SLOTS, &slots_json(slots).to_string())?;
    let mut changed = 0;
    let enabled_ids: BTreeSet<String> = slots
        .decks
        .iter()
        .flatten()
        .filter(|deck| deck.disabled_at.is_none())
        .map(|deck| deck.client_id.clone())
        .collect();
    let mut decks: Vec<(i64, String, bool, bool)> = Vec::new();
    {
        let mut statement = tx.prepare(
            "SELECT id, mtgo_id, cloud_sync_enabled, archived_at IS NOT NULL FROM decks",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
        for row in rows {
            decks.push(row?);
        }
    }
    let mut set_flag =
        tx.prepare_cached("UPDATE decks SET cloud_sync_enabled = ?2 WHERE id = ?1")?;
    for (deck_id, mtgo_id, was_enabled, archived) in decks {
        let enabled = !archived && enabled_ids.contains(&deck_client_id(&mtgo_id));
        if enabled == was_enabled {
            continue;
        }
        changed += set_flag.execute(params![deck_id, enabled])? as u32;
        if enabled {
            forget_hashes(tx, deck_id)?;
        }
    }
    clear_parked_rejections(tx)?;
    Ok(changed)
}

/// The client ids of a deck's matches and leagues.
fn deck_rows(tx: &Transaction, deck_id: i64) -> Result<(Vec<String>, Vec<String>), StoreError> {
    let mut matches = Vec::new();
    {
        let mut statement = tx.prepare_cached(
            "SELECT m.token FROM matches m JOIN deck_versions v ON v.id = m.deck_version_id
             WHERE v.deck_id = ?1",
        )?;
        let rows = statement.query_map([deck_id], |row| row.get(0))?;
        for row in rows {
            matches.push(row?);
        }
    }
    let mut leagues = Vec::new();
    {
        let mut statement = tx.prepare_cached(
            "SELECT sync_client_id FROM leagues
             WHERE deck_id = ?1 AND token IS NOT NULL AND sync_client_id IS NOT NULL",
        )?;
        let rows = statement.query_map([deck_id], |row| row.get(0))?;
        for row in rows {
            leagues.push(row?);
        }
    }
    Ok((matches, leagues))
}

fn forget_hashes(tx: &Transaction, deck_id: i64) -> Result<(), StoreError> {
    let (matches, leagues) = deck_rows(tx, deck_id)?;
    let mut forget = tx.prepare_cached(
        "UPDATE synced_rows SET hash = NULL, synced_at = NULL WHERE type = ?1 AND client_id = ?2",
    )?;
    for token in &matches {
        forget.execute(params![SyncType::Match.as_str(), token])?;
    }
    for client_id in &leagues {
        forget.execute(params![SyncType::League.as_str(), client_id])?;
    }
    Ok(())
}

fn clear_parked_rejections(tx: &Transaction) -> Result<(), StoreError> {
    let parked: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM sync_rejections WHERE reason = ?1)",
        [DECK_NOT_SYNCED],
        |row| row.get(0),
    )?;
    if !parked {
        return Ok(());
    }
    let mut enabled = Vec::new();
    {
        let mut statement = tx.prepare("SELECT id FROM decks WHERE cloud_sync_enabled = 1")?;
        let rows = statement.query_map([], |row| row.get::<_, i64>(0))?;
        for row in rows {
            enabled.push(row?);
        }
    }
    let mut delete = tx.prepare_cached(
        "DELETE FROM sync_rejections WHERE type = ?1 AND client_id = ?2 AND reason = ?3",
    )?;
    for deck_id in enabled {
        let (matches, leagues) = deck_rows(tx, deck_id)?;
        for token in &matches {
            delete.execute(params![SyncType::Match.as_str(), token, DECK_NOT_SYNCED])?;
        }
        for client_id in &leagues {
            delete.execute(params![
                SyncType::League.as_str(),
                client_id,
                DECK_NOT_SYNCED
            ])?;
        }
    }
    Ok(())
}

/// A pulled deck: imported (live and file data win), its archetype, guides
/// and notes applied (`apply_pulled_deck_parts`), then its row records the
/// server's hash and the carry; it is rebuilt at the next run.
fn pulled_deck(
    tx: &Transaction,
    client_id: &str,
    deck: &ImportedDeck,
    parts: &PulledDeckParts,
    hash: &str,
    carry: &str,
    now_ms: i64,
) -> Result<u32, StoreError> {
    let platform = crate::import::store_platform(parts.platform);
    let local = local_deck(tx, deck, parts, now_ms)?;
    import_pulled_deck(tx, &local, platform, now_ms)?;
    let unplaced = apply_pulled_deck_parts(tx, &deck.mtgo_id, parts, now_ms)?;
    let carry = carry_with_unplaced(carry, &unplaced);
    tx.execute(
        "INSERT INTO synced_rows (type, client_id, hash, synced_at, carry)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (type, client_id) DO UPDATE SET hash = excluded.hash,
             synced_at = excluded.synced_at, carry = excluded.carry, built_hash = NULL,
             touch_key = NULL, changed_at = NULL",
        params![SyncType::Deck.as_str(), client_id, hash, now_ms, carry],
    )?;
    Ok(1)
}

/// A pulled deck with its cards at this device's catalog ids: an Endstep
/// deck's portable signatures (one that does not read is left out) and its
/// cover ref are mapped to local Endstep rows, made when new (spec section
/// 5.3). Any other deck is taken as it came.
fn local_deck(
    tx: &Transaction,
    deck: &ImportedDeck,
    parts: &PulledDeckParts,
    now_ms: i64,
) -> Result<ImportedDeck, StoreError> {
    let mut local = deck.clone();
    if parts.platform != BundlePlatform::Endstep {
        return Ok(local);
    }
    local.versions.clear();
    for version in &deck.versions {
        if let Some(signature) = crate::import::local_signature(
            tx,
            crate::Platform::Endstep,
            &version.signature,
            now_ms,
        )? {
            local.versions.push(crate::import_decks::ImportedVersion {
                signature,
                modified_at_ms: version.modified_at_ms,
            });
        }
    }
    local.cover_catalog_id = match &parts.endstep_cover {
        Some(card) => Some(i64::from(crate::endstep_cards::catalog_for_ref(
            tx, card, now_ms,
        )?)),
        None => None,
    };
    Ok(local)
}

/// A pulled deck's archetype, guides and notes this device could not place
/// as rows (an unknown uuid on a deck without a constructed format, final
/// review F17). They stay in the deck's carry and go back into its bundle.
#[derive(Debug, Default)]
struct Unplaced {
    archetype_uuid: Option<String>,
    guides: Vec<GuideInput>,
    notes: Vec<NoteInput>,
}

/// The pulled carry with the unplaced guides and notes under their keys
/// (`UNPLACED_GUIDES`, `UNPLACED_NOTES`), in the bundle's own form.
fn carry_with_unplaced(carry: &str, unplaced: &Unplaced) -> String {
    let mut map = match serde_json::from_str::<Value>(carry) {
        Ok(Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    map.remove(UNPLACED_ARCHETYPE_UUID);
    map.remove(UNPLACED_GUIDES);
    map.remove(UNPLACED_NOTES);
    if let Some(uuid) = &unplaced.archetype_uuid {
        map.insert(
            UNPLACED_ARCHETYPE_UUID.to_string(),
            Value::String(uuid.clone()),
        );
    }
    if !unplaced.guides.is_empty() {
        map.insert(
            UNPLACED_GUIDES.to_string(),
            guides_section(&unplaced.guides),
        );
    }
    if !unplaced.notes.is_empty() {
        map.insert(UNPLACED_NOTES.to_string(), notes_section(&unplaced.notes));
    }
    Value::Object(map).to_string()
}

/// The unplaced guides and notes kept in a deck's carry (see `Unplaced`).
fn carried_unplaced(conn: &Connection, deck_id: i64) -> Result<Unplaced, StoreError> {
    let mtgo_id: Option<String> = conn
        .prepare_cached("SELECT mtgo_id FROM decks WHERE id = ?1")?
        .query_row([deck_id], |row| row.get(0))
        .optional()?;
    let Some(mtgo_id) = mtgo_id else {
        return Ok(Unplaced::default());
    };
    let Some(value) = stored_carry(conn, SyncType::Deck, &deck_client_id(&mtgo_id))? else {
        return Ok(Unplaced::default());
    };
    Ok(Unplaced {
        archetype_uuid: value[UNPLACED_ARCHETYPE_UUID].as_str().map(str::to_string),
        guides: read_guides(&value[UNPLACED_GUIDES]),
        notes: read_notes(&value[UNPLACED_NOTES]),
    })
}

/// A row's stored carry as JSON, `None` when there is none or it does not
/// read.
fn stored_carry(
    conn: &Connection,
    sync_type: SyncType,
    client_id: &str,
) -> Result<Option<Value>, StoreError> {
    let carry: Option<Option<String>> = conn
        .prepare_cached("SELECT carry FROM synced_rows WHERE type = ?1 AND client_id = ?2")?
        .query_row(params![sync_type.as_str(), client_id], |row| row.get(0))
        .optional()?;
    Ok(carry
        .flatten()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok()))
}

/// The pulled match carry with the archetype rows this device could not
/// place (`UNPLACED_ARCHETYPES`: a limited match's unknown uuid has no
/// format to stub in, final review F17).
fn match_carry_with_unplaced(carry: &str, sides: &[ArchetypeSide]) -> String {
    let mut map = match serde_json::from_str::<Value>(carry) {
        Ok(Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    map.remove(UNPLACED_ARCHETYPES);
    if !sides.is_empty() {
        let listed: Vec<Value> = sides
            .iter()
            .map(|side| {
                serde_json::json!({
                    "uuid": side.uuid,
                    "confidence": side.confidence,
                    "manual": side.manual,
                    "opponent": side.opponent,
                })
            })
            .collect();
        map.insert(UNPLACED_ARCHETYPES.to_string(), Value::Array(listed));
    }
    Value::Object(map).to_string()
}

/// The archetype rows kept in a match's carry (see
/// `match_carry_with_unplaced`).
fn carried_match_sides(conn: &Connection, token: &str) -> Result<Vec<ArchetypeSide>, StoreError> {
    let Some(value) = stored_carry(conn, SyncType::Match, token)? else {
        return Ok(Vec::new());
    };
    let mut sides = Vec::new();
    for side in value[UNPLACED_ARCHETYPES]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let (Some(uuid), Some(confidence), Some(manual), Some(opponent)) = (
            side["uuid"].as_str(),
            side["confidence"].as_f64(),
            side["manual"].as_bool(),
            side["opponent"].as_bool(),
        ) else {
            continue;
        };
        sides.push(ArchetypeSide {
            uuid: uuid.to_string(),
            confidence,
            manual,
            opponent,
        });
    }
    Ok(sides)
}

/// A pulled match that landed on a live match under another token leaves
/// the sides it could not place with the live match too (#300 R4), so the
/// live match's push sends them: added to the live row's carry for every
/// seat it does not carry already, and the row rebuilt at the next run.
fn carry_unplaced_on_live_match(
    tx: &Transaction,
    live_token: &str,
    unplaced: &[ArchetypeSide],
) -> Result<(), StoreError> {
    if unplaced.is_empty() {
        return Ok(());
    }
    let mut sides = carried_match_sides(tx, live_token)?;
    for side in unplaced {
        if !sides.iter().any(|held| held.opponent == side.opponent) {
            sides.push(side.clone());
        }
    }
    let stored: Option<Option<String>> = tx
        .prepare_cached("SELECT carry FROM synced_rows WHERE type = ?1 AND client_id = ?2")?
        .query_row(params![SyncType::Match.as_str(), live_token], |row| {
            row.get(0)
        })
        .optional()?;
    let carry = match_carry_with_unplaced(stored.flatten().as_deref().unwrap_or("{}"), &sides);
    tx.execute(
        "INSERT INTO synced_rows (type, client_id, carry) VALUES (?1, ?2, ?3)
         ON CONFLICT (type, client_id) DO UPDATE SET carry = excluded.carry, built_hash = NULL",
        params![SyncType::Match.as_str(), live_token, carry],
    )?;
    Ok(())
}

/// A pulled row records the server's hash and carry; it is rebuilt at the
/// next run. A row parked before is applied now: no longer parked.
fn record_pulled(
    tx: &Transaction,
    sync_type: SyncType,
    client_id: &str,
    hash: &str,
    carry: &str,
    now_ms: i64,
) -> Result<u32, StoreError> {
    tx.execute(
        "INSERT INTO synced_rows (type, client_id, hash, synced_at, carry)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (type, client_id) DO UPDATE SET hash = excluded.hash,
             synced_at = excluded.synced_at, carry = excluded.carry, built_hash = NULL,
             touch_key = NULL, changed_at = NULL, parked_hash = NULL, parked_reason = NULL",
        params![sync_type.as_str(), client_id, hash, now_ms, carry],
    )?;
    Ok(1)
}

/// Why a pulled row could not be applied yet (#830).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParkedReason {
    /// An Arena run with no Arena account here to file it under: fetched
    /// again as soon as one exists.
    NoArenaAccount,
    /// An Arena or Endstep match that came without its source: fetched again
    /// only once the server's copy changes.
    NoSources,
}

impl ParkedReason {
    fn as_str(self) -> &'static str {
        match self {
            ParkedReason::NoArenaAccount => "no_arena_account",
            ParkedReason::NoSources => "no_sources",
        }
    }
}

/// Park a pulled row this device cannot apply yet (#830): its server hash
/// and why are kept, so the next run names it as known and the server
/// offers it again only once its copy changes. Nothing else of the row
/// changes.
fn park_pulled(
    tx: &Transaction,
    sync_type: SyncType,
    client_id: &str,
    hash: &str,
    reason: ParkedReason,
) -> Result<(), StoreError> {
    tx.prepare_cached(
        "INSERT INTO synced_rows (type, client_id, parked_hash, parked_reason)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (type, client_id) DO UPDATE SET parked_hash = excluded.parked_hash,
             parked_reason = excluded.parked_reason",
    )?
    .execute(params![
        sync_type.as_str(),
        client_id,
        hash,
        reason.as_str()
    ])?;
    Ok(())
}

/// Forget parked pulls deleted on the website (#830): a row that only
/// marks a refused copy goes; a row the server held a copy of stays.
fn forget_parked(
    tx: &Transaction,
    sync_type: SyncType,
    client_ids: &[String],
) -> Result<(), StoreError> {
    let mut statement = tx.prepare_cached(
        "DELETE FROM synced_rows WHERE type = ?1 AND client_id = ?2
             AND hash IS NULL AND parked_reason IS NOT NULL",
    )?;
    for client_id in client_ids {
        statement.execute(params![sync_type.as_str(), client_id])?;
    }
    Ok(())
}

/// A type's parked pulls (#830), by client id order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParkedPulls {
    /// Still waiting: sent as known, so the server offers them again only
    /// once their copy changes.
    pub waiting: Vec<String>,
    /// What held them back is gone (an Arena run whose Arena account is
    /// here now): left out of known, so the server offers them again while
    /// it still holds and serves them.
    pub ready: Vec<String>,
}

/// The parked pulls of a type, split into waiting and ready.
pub fn parked_pulls(conn: &Connection, sync_type: SyncType) -> Result<ParkedPulls, StoreError> {
    let mut statement = conn.prepare_cached(
        "SELECT client_id, parked_reason FROM synced_rows
         WHERE type = ?1 AND parked_reason IS NOT NULL ORDER BY client_id",
    )?;
    let rows = statement.query_map([sync_type.as_str()], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut parked = ParkedPulls::default();
    for row in rows {
        let (client_id, reason) = row?;
        let ready = reason == ParkedReason::NoArenaAccount.as_str()
            && crate::arena_run_sync::account_for_pulled_run(conn, &client_id)?.is_some();
        if ready {
            parked.ready.push(client_id);
        } else {
            parked.waiting.push(client_id);
        }
    }
    Ok(parked)
}

/// A pulled match (07 spec section 10, review fix of Plan Q task 11). A
/// manual match this device holds as the server sent it (its touch key
/// still equals its pulled key) is replaced wholesale by the server's copy,
/// so another device's edit lands; one edited here keeps this device's copy
/// (the 0.x import path, as decks). A manual match that arrives (or is
/// replaced) records its touch key as its pulled key, so it is pushed only
/// once edited here. Pulled manual leagues it joins or leaves stay unedited.
/// Then the bundle's player fields, by client id, for every source
/// (`sync_player`, league self-management spec 6.3).
fn pulled_match(
    tx: &Transaction,
    client_id: &str,
    pulled: &PulledMatch,
    hash: &str,
    carry: &str,
    timezone: Tz,
    now_ms: i64,
) -> Result<u32, StoreError> {
    let token = pulled.imported.token.as_str();
    let mut league_ids: Vec<i64> = league_of_match(tx, token)?.into_iter().collect();
    league_ids.extend(crate::import::pulled_league_id(
        tx,
        pulled.league_client_id.as_deref(),
    )?);
    let unedited = unedited_leagues(tx, &league_ids)?;
    let before = manual_match_state(tx, token)?;
    let replaced = match before {
        Some((match_id, true)) => {
            // Steps aside under a spare token until the server's copy is in.
            tx.execute(
                "UPDATE matches SET token = token || ':replaced' WHERE id = ?1",
                [match_id],
            )?;
            Some(match_id)
        }
        _ => None,
    };
    let imported = import_pulled_match(tx, pulled, now_ms)?;
    if let Some(old_id) = replaced {
        match match_row_id(tx, token)? {
            Some(new_id) => {
                // Its registered decks belong to the run, not the copy.
                tx.execute(
                    "UPDATE OR IGNORE limited_deck_snapshots SET match_id = ?2
                     WHERE match_id = ?1",
                    params![old_id, new_id],
                )?;
                tx.execute("DELETE FROM matches WHERE id = ?1", [old_id])?;
            }
            None => {
                // The server's copy was not stored (no player to file it
                // under): this device keeps its own.
                tx.execute(
                    "UPDATE matches SET token = ?2 WHERE id = ?1",
                    params![old_id, token],
                )?;
            }
        }
    }
    let Some(stored_token) = &imported.token else {
        // Nothing was stored. A tracked match without its source is parked
        // until the server's copy changes (#830); any other is not recorded,
        // so the next manifest offers it again.
        if crate::import::lacks_source(pulled) {
            park_pulled(
                tx,
                SyncType::Match,
                client_id,
                hash,
                ParkedReason::NoSources,
            )?;
        }
        return Ok(0);
    };
    note_pulled_run(tx, token, pulled, now_ms)?;
    // A Brawl copy's commander archetypes first, so its seats find them.
    if let Some(match_id) = match_row_id(tx, stored_token)? {
        crate::archetypes::ensure_pulled_commanders(tx, match_id, now_ms)?;
    }
    let unplaced = apply_pulled_match_archetypes(tx, stored_token, &pulled.archetypes, now_ms)?;
    let carry = match_carry_with_unplaced(carry, &unplaced);
    if stored_token != client_id {
        carry_unplaced_on_live_match(tx, stored_token, &unplaced)?;
    }
    let changed = record_pulled(tx, SyncType::Match, client_id, hash, &carry, now_ms)?;
    if before.is_none_or(|(_, unedited)| unedited) {
        mark_server_copy(tx, SyncType::Match, client_id)?;
    }
    // After the import: a first pull's row exists only now (spec 6.3).
    crate::sync_player::apply_match_fields(tx, pulled, timezone, now_ms)?;
    refresh_pulled_keys(tx, &unedited)?;
    Ok(changed)
}

/// A pulled league: a manual league this device holds as the server sent it
/// is replaced by the server's copy first; then the 0.x league and limited
/// import paths. Like `pulled_match`, a manual league that arrives (or is
/// replaced) records its pulled key. Then the bundle's player fields, by
/// client id, for every source (`sync_player`, league self-management spec
/// 6.3).
fn pulled_league(
    tx: &Transaction,
    client_id: &str,
    pulled: &PulledLeague,
    hash: &str,
    carry: &str,
    timezone: Tz,
    now_ms: i64,
) -> Result<u32, StoreError> {
    let before = match manual_league_id(tx, client_id)? {
        Some(league_id) => Some((league_id, !unedited_leagues(tx, &[league_id])?.is_empty())),
        None => None,
    };
    if let Some((league_id, true)) = before {
        crate::import_leagues::replace_manual_league(tx, league_id, &pulled.league)?;
    }
    if import_pulled_league(tx, &pulled.league, client_id, now_ms)? {
        let limited = ImportedLimited {
            drafts: pulled.drafts.clone(),
            decks: pulled.decks.clone(),
            league_keys: None,
        };
        import_limited_rows(tx, &limited, now_ms)?;
    }
    let changed = record_pulled(tx, SyncType::League, client_id, hash, carry, now_ms)?;
    if before.is_none_or(|(_, unedited)| unedited) {
        mark_server_copy(tx, SyncType::League, client_id)?;
    }
    crate::sync_player::apply_league_fields(tx, client_id, &pulled.player, timezone, now_ms)?;
    Ok(changed)
}

/// A pulled Arena match's run (cross-device sync spec section 8): the run
/// client id its bundle named is kept beside its source, so the run fold
/// joins it to its course's run whenever that run is known here. The fold
/// itself runs once after the pull's matches (`ArenaRunsAfterPull`). A
/// pulled record of its course held under another Arena account moves to the
/// account that played it.
fn note_pulled_run(
    tx: &Transaction,
    token: &str,
    pulled: &PulledMatch,
    now_ms: i64,
) -> Result<(), StoreError> {
    if pulled.platform != BundlePlatform::Arena {
        return Ok(());
    }
    tx.prepare_cached(
        "UPDATE synced_sources SET league_client_id = ?2
         WHERE match_id = (SELECT id FROM matches WHERE token = ?1)
           AND league_client_id IS NOT ?2",
    )?
    .execute(params![token, pulled.league_client_id])?;
    let Some(course_id) = pulled
        .league_client_id
        .as_deref()
        .and_then(|client_id| client_id.strip_prefix("arena_"))
        .filter(|course_id| !course_id.is_empty())
    else {
        return Ok(());
    };
    let account: Option<i64> = tx
        .prepare_cached(
            "SELECT m.account_id FROM matches m
             JOIN accounts a ON a.id = m.account_id AND a.platform = 'arena'
             WHERE m.token = ?1 AND m.match_type = 'limited'",
        )?
        .query_row([token], |row| row.get(0))
        .optional()?;
    if let Some(account_id) = account {
        crate::arena_run_sync::move_pulled_run_to(tx, account_id, course_id, now_ms)?;
    }
    Ok(())
}

/// A pulled Arena course run (cross-device sync spec section 8): its
/// snapshot kept in `synced_arena_runs` (only when newer than the one held),
/// the account's runs folded again, the newer notes and the player's fields
/// A pulled row's hash, carry and the server's `updated_at` of it.
struct PulledRow<'a> {
    hash: &'a str,
    carry: &'a str,
    server_updated_at_ms: Option<i64>,
}

/// A pulled Arena course run (cross-device sync spec section 8): its
/// snapshot kept in `synced_arena_runs` (only when newer than the one held),
/// the account's runs folded again, the newer notes and the player's fields
/// applied to the run, and the server's hash recorded. Never the league
/// import. A run whose client id is not its course's is not recorded, so the
/// next manifest offers it again. One with no Arena account to file it under
/// is parked (#830) until an Arena account exists or its copy changes.
fn pulled_arena_run(
    tx: &Transaction,
    client_id: &str,
    run: &ArenaRunBundle,
    pulled: &PulledRow,
    timezone: Tz,
    now_ms: i64,
) -> Result<u32, StoreError> {
    if client_id != format!("arena_{}", run.course_id) {
        tracing::warn!(
            client_id,
            "a pulled Arena run's client id is not its course's"
        );
        return Ok(0);
    }
    let Some(account_id) = crate::arena_run_sync::account_for_pulled_run(tx, client_id)? else {
        tracing::warn!(
            client_id,
            "a pulled Arena run has no Arena account here yet"
        );
        park_pulled(
            tx,
            SyncType::League,
            client_id,
            pulled.hash,
            ParkedReason::NoArenaAccount,
        )?;
        return Ok(0);
    };
    crate::arena_run_sync::store_pulled_run(tx, account_id, run)?;
    crate::arena_run_sync::sync_arena_runs_guarded(tx, account_id, now_ms)?;
    apply_pulled_run_notes(tx, client_id, run)?;
    let changed = record_pulled_run(tx, client_id, pulled, now_ms)?;
    apply_pulled_run_fields(tx, client_id, &run.player, timezone, now_ms)?;
    Ok(changed)
}

/// Record a pulled Arena run as the server's copy. Bytes this device
/// already builds keep its build (they read as synced, not waiting, when
/// the server offers them again); any other bytes are recorded as every
/// pull is. The server's `updated_at`, when sent, becomes the row's
/// `changed_at`: a merge of this copy is sent a second past it.
fn record_pulled_run(
    tx: &Transaction,
    client_id: &str,
    pulled: &PulledRow,
    now_ms: i64,
) -> Result<u32, StoreError> {
    let built: Option<Option<String>> = tx
        .prepare_cached(
            "SELECT built_hash FROM synced_rows WHERE type = 'league' AND client_id = ?1",
        )?
        .query_row([client_id], |row| row.get(0))
        .optional()?;
    if built.flatten().as_deref() == Some(pulled.hash) {
        tx.prepare_cached(
            "UPDATE synced_rows SET hash = ?2, synced_at = ?3, carry = ?4,
                 parked_hash = NULL, parked_reason = NULL
             WHERE type = 'league' AND client_id = ?1",
        )?
        .execute(params![client_id, pulled.hash, now_ms, pulled.carry])?;
    } else {
        record_pulled(
            tx,
            SyncType::League,
            client_id,
            pulled.hash,
            pulled.carry,
            now_ms,
        )?;
    }
    if let Some(at) = pulled.server_updated_at_ms {
        tx.prepare_cached(
            "UPDATE synced_rows SET changed_at = ?2 WHERE type = 'league' AND client_id = ?1",
        )?
        .execute(params![client_id, at])?;
    }
    Ok(1)
}

/// A pulled run's notes, when they were edited after this device's; on a
/// tie of edit time, the greater text, so two devices that edited at the same
/// moment settle on one value (an Arena run is pushed by every device).
fn apply_pulled_run_notes(
    tx: &Transaction,
    client_id: &str,
    run: &ArenaRunBundle,
) -> Result<(), StoreError> {
    let Some(pulled_at) = run.notes_edited_at_ms else {
        return Ok(());
    };
    let notes = run.notes.as_deref().filter(|notes| !notes.is_empty());
    tx.prepare_cached(
        "UPDATE leagues SET notes = ?2, notes_edited_at = ?3
         WHERE sync_client_id = ?1 AND run_key LIKE 'arena:%'
           AND (notes_edited_at IS NULL OR notes_edited_at < ?3
                OR (notes_edited_at = ?3 AND COALESCE(?2, '') > COALESCE(notes, '')))",
    )?
    .execute(params![client_id, notes, pulled_at])?;
    Ok(())
}

/// A pulled run's player fields under the league rule (`sync_player`), and
/// on a tie of edit time the greater of the two values (removal, status,
/// drop time, in that order), so two devices settle on one.
fn apply_pulled_run_fields(
    tx: &Transaction,
    client_id: &str,
    fields: &PulledLeagueFields,
    timezone: Tz,
    now_ms: i64,
) -> Result<(), StoreError> {
    type Fields = (Option<i64>, Option<String>, Option<i64>);
    let local: Option<(i64, Option<i64>, Fields)> = tx
        .prepare_cached(
            "SELECT id, edited_at, removed_at, user_state, user_dropped_at FROM leagues
             WHERE sync_client_id = ?1 AND run_key LIKE 'arena:%'",
        )?
        .query_row([client_id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                (row.get(2)?, row.get(3)?, row.get(4)?),
            ))
        })
        .optional()?;
    if let Some((league_id, edited_at, held)) = local
        && edited_at.is_some()
        && edited_at == fields.edited_at_ms
    {
        let pulled: Fields = (
            fields.removed_at_ms,
            fields.user_state.clone(),
            fields.user_dropped_at_ms,
        );
        if pulled > held {
            tx.prepare_cached(
                "UPDATE leagues SET removed_at = ?2, user_state = ?3, user_dropped_at = ?4
                 WHERE id = ?1",
            )?
            .execute(params![league_id, pulled.0, pulled.1, pulled.2])?;
        }
        return Ok(());
    }
    crate::sync_player::apply_league_fields(tx, client_id, fields, timezone, now_ms)
}

/// A pulled deck bundle's archetype, sideboard guides and matchup notes
/// (archetypes spec sections 14 and 21.5), and which of them it speaks for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PulledDeckParts {
    pub keys: DeckParts,
    /// The game the deck is from (the bundle's `platform`).
    pub platform: BundlePlatform,
    /// Endstep: the cover card by ref (`DeckBundleInput::endstep_cover`).
    pub endstep_cover: Option<EndstepCardRef>,
    pub archetype_uuid: Option<String>,
    pub guides: Vec<GuideInput>,
    pub notes: Vec<NoteInput>,
}

/// The local id of an archetype uuid a bundle names (0.x
/// `LocalArchetypeId::for`): the held row, else a `downloaded` stub named by
/// its uuid in `format` (a format key) with no definition, which the next
/// check fills. `None` when it is unknown and there is no format to stub it
/// in (a downloaded row always has one).
fn archetype_for_uuid(
    tx: &Transaction,
    uuid: &str,
    format: Option<&str>,
    now_ms: i64,
) -> Result<Option<i64>, StoreError> {
    let held: Option<i64> = tx
        .prepare_cached("SELECT id FROM archetypes WHERE uuid = ?1")?
        .query_row([uuid], |row| row.get(0))
        .optional()?;
    if held.is_some() {
        return Ok(held);
    }
    let Some(format) = format else {
        tracing::info!(
            uuid,
            "Sync pull: an unknown archetype without a format was left out."
        );
        return Ok(None);
    };
    Ok(Some(
        tx.prepare_cached(
            "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
             VALUES (?1, ?1, ?2, 'downloaded', ?3, ?3)
             RETURNING id",
        )?
        .query_row(params![uuid, format, now_ms], |row| row.get(0))?,
    ))
}

/// A hand-set commander side's archetype this device has not made (#827):
/// a `commander` row named by its uuid in the match's format, `None`
/// without one. Its name and colours come with `ensure_commander`.
fn commander_placeholder(
    tx: &Transaction,
    uuid: &str,
    format: Option<&str>,
    now_ms: i64,
) -> Result<Option<i64>, StoreError> {
    let Some(format) = format else {
        return Ok(None);
    };
    Ok(Some(
        tx.prepare_cached(
            "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
             VALUES (?1, ?1, ?2, 'commander', ?3, ?3)
             RETURNING id",
        )?
        .query_row(params![uuid, format, now_ms], |row| row.get(0))?,
    ))
}

/// A pulled deck's archetype, guides and notes. The archetype fills only a
/// deck without one (a set `decks.archetype_id` is input). Guides and
/// notes, only when the bundle carries both keys, replace the deck's in one
/// go (0.x `DeckBundleImporter::replaceGuidesAndNotes`): guides stamped with
/// the pull time, a note keeps its time (none is the pull time), unknown
/// uuids stubbed in the deck's format, and a row that breaks a CHECK (an
/// empty body, a quantity outside 1 to 99, an unknown direction) skipped
/// with a log line.
fn apply_pulled_deck_parts(
    tx: &Transaction,
    mtgo_id: &str,
    parts: &PulledDeckParts,
    now_ms: i64,
) -> Result<Unplaced, StoreError> {
    let mut unplaced = Unplaced::default();
    let found: Option<(i64, String, Option<i64>)> = tx
        .prepare_cached("SELECT id, format, archetype_id FROM decks WHERE mtgo_id = ?1")?
        .query_row([mtgo_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .optional()?;
    let Some((deck_id, deck_format, deck_archetype)) = found else {
        return Ok(unplaced);
    };
    let format = archetypes::format_key(&deck_format);
    if parts.keys.has_archetype
        && deck_archetype.is_none()
        && let Some(uuid) = &parts.archetype_uuid
    {
        if format.is_none() && !archetype_held(tx, uuid)? {
            // Kept in the carry and pushed back (final review F17).
            unplaced.archetype_uuid = Some(uuid.clone());
        } else if let Some(archetype_id) = archetype_for_uuid(tx, uuid, format.as_deref(), now_ms)?
        {
            tx.prepare_cached("UPDATE decks SET archetype_id = ?2 WHERE id = ?1")?
                .execute(params![deck_id, archetype_id])?;
        }
    }
    if !parts.keys.has_guides_and_notes {
        return Ok(unplaced);
    }
    tx.prepare_cached("DELETE FROM sideboard_guides WHERE deck_id = ?1")?
        .execute([deck_id])?;
    tx.prepare_cached("DELETE FROM deck_archetype_notes WHERE deck_id = ?1")?
        .execute([deck_id])?;
    for guide in &parts.guides {
        if format.is_none() && !archetype_held(tx, &guide.archetype_uuid)? {
            unplaced.guides.push(guide.clone());
            continue;
        }
        let Some(archetype_id) =
            archetype_for_uuid(tx, &guide.archetype_uuid, format.as_deref(), now_ms)?
        else {
            continue;
        };
        let guide_id: Option<i64> = tx
            .prepare_cached(
                "INSERT INTO sideboard_guides (deck_id, archetype_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?3)
                 ON CONFLICT (deck_id, archetype_id) DO NOTHING
                 RETURNING id",
            )?
            .query_row(params![deck_id, archetype_id, now_ms], |row| row.get(0))
            .optional()?;
        let Some(guide_id) = guide_id else {
            tracing::info!("Sync pull: a second guide for one archetype was left out.");
            continue;
        };
        for card in &guide.cards {
            if !guide_card_fits(card) {
                tracing::info!(
                    direction = %card.direction,
                    quantity = card.quantity,
                    "Sync pull: a sideboard guide card that breaks a check was left out."
                );
                continue;
            }
            tx.prepare_cached(
                "INSERT OR IGNORE INTO sideboard_guide_cards (guide_id, oracle_id, direction,
                     quantity)
                 VALUES (?1, ?2, ?3, ?4)",
            )?
            .execute(params![
                guide_id,
                card.oracle_id,
                card.direction,
                card.quantity
            ])?;
        }
    }
    for note in &parts.notes {
        if format.is_none() && !archetype_held(tx, &note.archetype_uuid)? {
            unplaced.notes.push(note.clone());
            continue;
        }
        let chars = note.body.chars().count();
        if note.body.trim().is_empty() || chars > archetypes::guides::NOTE_MAX_CHARS {
            tracing::info!(
                chars,
                "Sync pull: a matchup note that breaks a check was left out."
            );
            continue;
        }
        let Some(archetype_id) =
            archetype_for_uuid(tx, &note.archetype_uuid, format.as_deref(), now_ms)?
        else {
            continue;
        };
        tx.prepare_cached(
            "INSERT INTO deck_archetype_notes (deck_id, archetype_id, body, created_at)
             VALUES (?1, ?2, ?3, ?4)",
        )?
        .execute(params![
            deck_id,
            archetype_id,
            note.body,
            note.created_at_ms.unwrap_or(now_ms)
        ])?;
    }
    if !unplaced.guides.is_empty() || !unplaced.notes.is_empty() {
        tracing::info!(
            guides = unplaced.guides.len(),
            notes = unplaced.notes.len(),
            "Sync pull: guides and notes of an unknown archetype on a deck without a format \
             were kept in the carry."
        );
    }
    Ok(unplaced)
}

/// Whether this device holds an archetype with this uuid.
fn archetype_held(tx: &Transaction, uuid: &str) -> Result<bool, StoreError> {
    Ok(tx
        .prepare_cached("SELECT EXISTS (SELECT 1 FROM archetypes WHERE uuid = ?1)")?
        .query_row([uuid], |row| row.get(0))?)
}

/// A guide card the `sideboard_guide_cards` CHECKs accept.
fn guide_card_fits(card: &GuideCardInput) -> bool {
    let oracle_chars = card.oracle_id.chars().count();
    (1..=archetypes::guides::MAX_ORACLE_CHARS).contains(&oracle_chars)
        && (card.direction == "in" || card.direction == "out")
        && (1..=archetypes::guides::MAX_QUANTITY).contains(&card.quantity)
}

/// A pulled match's archetype seats on the match stored under `token` (its
/// own copy or the live row that won). Hand-set rows are input: one is
/// written unless this device's seat is hand-set too; a detected one only
/// where this device has no row for the seat. An empty opponent seat this
/// device decided stays empty (`import_archetypes::opponent_decided`), and
/// an opponent the player set to "none" always does. A uuid this device lacks is
/// stubbed in the match's format. A match that ends with an opponent row is
/// marked detected (as the 0.x import does), so the queue leaves it.
fn apply_pulled_match_archetypes(
    tx: &Transaction,
    token: &str,
    sides: &[ArchetypeSide],
    now_ms: i64,
) -> Result<Vec<ArchetypeSide>, StoreError> {
    let mut unplaced = Vec::new();
    if sides.is_empty() {
        return Ok(unplaced);
    }
    let found: Option<(i64, Option<String>)> = tx
        .prepare_cached("SELECT id, format FROM matches WHERE token = ?1")?
        .query_row([token], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let Some((match_id, match_format)) = found else {
        return Ok(unplaced);
    };
    let format = match_format.as_deref().and_then(archetypes::format_key);
    let mut wrote = false;
    // A seat left to detection keeps the match due.
    let mut skipped_commander = false;
    for side in sides {
        let held_manual: Option<bool> = tx
            .prepare_cached(
                "SELECT manual FROM match_archetypes WHERE match_id = ?1 AND is_opponent = ?2",
            )?
            .query_row(params![match_id, side.opponent], |row| row.get(0))
            .optional()?;
        let write = match held_manual {
            // The player's "none" for the opponent is input (final review I2).
            _ if side.opponent && crate::import_archetypes::opponent_none(tx, match_id)? => false,
            Some(true) => false,
            Some(false) => side.manual,
            // An empty opponent seat this device decided (the player's
            // "none", or detection found nothing) stays empty, as in the
            // 0.x import.
            None => {
                !(side.opponent
                    && crate::import_archetypes::opponent_decided(tx, match_id, side.manual)?)
            }
        };
        if !write {
            continue;
        }
        // A commander archetype this device has not made yet is no server
        // archetype to stub (cross-device sync spec section 7): a detected
        // seat is left to detection, which makes it once the commanders'
        // cards are known. A hand-set one is still the player's choice
        // (#827): it gets a commander placeholder named by its uuid (never a
        // `downloaded` stub, which the definitions check would retire),
        // which `ensure_commander` fills once this device knows the
        // commanders.
        let unknown_commander =
            crate::match_commanders::is_commander_format(match_format.as_deref())
                && !archetype_held(tx, &side.uuid)?;
        if unknown_commander && !side.manual {
            skipped_commander = true;
            continue;
        }
        let archetype_id = if unknown_commander {
            commander_placeholder(tx, &side.uuid, format.as_deref(), now_ms)?
        } else {
            archetype_for_uuid(tx, &side.uuid, format.as_deref(), now_ms)?
        };
        let Some(archetype_id) = archetype_id else {
            // No format to stub in: kept in the carry and pushed back
            // (final review F17).
            unplaced.push(side.clone());
            continue;
        };
        let confidence = if side.confidence.is_finite() {
            side.confidence.clamp(0.0, 1.0)
        } else {
            0.0
        };
        tx.prepare_cached(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (match_id, is_opponent) DO UPDATE SET
                 archetype_id = excluded.archetype_id, confidence = excluded.confidence,
                 manual = excluded.manual",
        )?
        .execute(params![
            match_id,
            side.opponent,
            archetype_id,
            confidence,
            side.manual
        ])?;
        wrote = true;
    }
    if wrote && !skipped_commander {
        tx.prepare_cached(
            "UPDATE matches SET archetypes_detected_at = ?2
             WHERE id = ?1 AND archetypes_detected_at IS NULL
               AND EXISTS (SELECT 1 FROM match_archetypes x
                           WHERE x.match_id = ?1 AND x.is_opponent = 1)",
        )?
        .execute(params![match_id, now_ms])?;
    }
    Ok(unplaced)
}

/// The manual match `token`, and whether it is still as the server sent it.
fn manual_match_state(tx: &Transaction, token: &str) -> Result<Option<(i64, bool)>, StoreError> {
    if !crate::write::is_manual_token(tx, token)? {
        return Ok(None);
    }
    let Some(match_id) = match_row_id(tx, token)? else {
        return Ok(None);
    };
    let key = match_touch_key(tx, match_id)?;
    Ok(Some((
        match_id,
        pulled_key(tx, SyncType::Match, token)? == Some(key),
    )))
}

/// The row id of the match `token`.
fn match_row_id(tx: &Transaction, token: &str) -> Result<Option<i64>, StoreError> {
    Ok(tx
        .prepare_cached("SELECT id FROM matches WHERE token = ?1")?
        .query_row([token], |row| row.get(0))
        .optional()?)
}

/// The league the match `token` is in, if any.
fn league_of_match(tx: &Transaction, token: &str) -> Result<Option<i64>, StoreError> {
    Ok(tx
        .prepare_cached("SELECT league_id FROM matches WHERE token = ?1")?
        .query_row([token], |row| row.get::<_, Option<i64>>(0))
        .optional()?
        .flatten())
}

/// The manual league a league client id names: the one that stores it
/// (`sync_client_id`), else (an id from a device that has not stored its
/// ids yet) its token and its start to the second, as the client id has it.
fn manual_league_id(tx: &Transaction, client_id: &str) -> Result<Option<i64>, StoreError> {
    let stored: Option<i64> = tx
        .prepare_cached(
            "SELECT id FROM leagues WHERE sync_client_id = ?1 AND source = 'manual'
             ORDER BY id LIMIT 1",
        )?
        .query_row([client_id], |row| row.get(0))
        .optional()?;
    if stored.is_some() {
        return Ok(stored);
    }
    manual_league_id_by_start(tx, client_id)
}

/// `manual_league_id` by the client id's token and start.
fn manual_league_id_by_start(tx: &Transaction, client_id: &str) -> Result<Option<i64>, StoreError> {
    let Some((token, started_at)) = crate::import::split_league_client_id(client_id) else {
        return Ok(None);
    };
    Ok(tx
        .prepare_cached(
            "SELECT id FROM leagues WHERE token = ?1 AND started_at / 1000 = ?2 / 1000
                 AND source = 'manual'
             ORDER BY id LIMIT 1",
        )?
        .query_row(params![token, started_at], |row| row.get(0))
        .optional()?)
}

fn pulled_key(
    tx: &Transaction,
    sync_type: SyncType,
    client_id: &str,
) -> Result<Option<String>, StoreError> {
    Ok(tx
        .prepare_cached("SELECT pulled_key FROM synced_rows WHERE type = ?1 AND client_id = ?2")?
        .query_row(params![sync_type.as_str(), client_id], |row| {
            row.get::<_, Option<String>>(0)
        })
        .optional()?
        .flatten())
}

/// The manual leagues among `league_ids` still as the server sent them,
/// with their client ids.
fn unedited_leagues(
    tx: &Transaction,
    league_ids: &[i64],
) -> Result<Vec<(i64, String)>, StoreError> {
    let mut found: Vec<(i64, String)> = Vec::new();
    for &league_id in league_ids {
        if found.iter().any(|(id, _)| *id == league_id) {
            continue;
        }
        let client_id: Option<String> = tx
            .prepare_cached(
                "SELECT sync_client_id FROM leagues
                 WHERE id = ?1 AND source = 'manual' AND token IS NOT NULL
                   AND sync_client_id IS NOT NULL",
            )?
            .query_row([league_id], |row| row.get(0))
            .optional()?;
        let Some(client_id) = client_id else {
            continue;
        };
        let key = league_own_key(tx, league_id)?;
        if pulled_key(tx, SyncType::League, &client_id)? == Some(key) {
            found.push((league_id, client_id));
        }
    }
    Ok(found)
}

/// Unedited pulled leagues a pull changed stay the server's copy.
fn refresh_pulled_keys(tx: &Transaction, leagues: &[(i64, String)]) -> Result<(), StoreError> {
    for (league_id, client_id) in leagues {
        let key = league_own_key(tx, *league_id)?;
        tx.prepare_cached(
            "UPDATE synced_rows SET pulled_key = ?2 WHERE type = 'league' AND client_id = ?1",
        )?
        .execute(params![client_id, key])?;
    }
    Ok(())
}

/// Record a manual match's or manual league's touch key as its pulled key:
/// the row is the server's copy until it is edited here. Rows of any other
/// source are left alone (a live row always wins and is pushed).
pub(crate) fn mark_server_copy(
    tx: &Transaction,
    sync_type: SyncType,
    client_id: &str,
) -> Result<(), StoreError> {
    let key = match sync_type {
        SyncType::Deck => None,
        SyncType::Match => match match_row_id(tx, client_id)? {
            Some(match_id) if crate::write::is_manual_token(tx, client_id)? => {
                Some(match_touch_key(tx, match_id)?)
            }
            _ => None,
        },
        SyncType::League => match manual_league_id(tx, client_id)? {
            Some(league_id) => Some(league_own_key(tx, league_id)?),
            None => None,
        },
    };
    if let Some(key) = key {
        tx.prepare_cached(
            "UPDATE synced_rows SET pulled_key = ?3 WHERE type = ?1 AND client_id = ?2",
        )?
        .execute(params![sync_type.as_str(), client_id, key])?;
    }
    Ok(())
}

/// A league pulled from the cloud, read from its bundle: the league as the
/// 0.x import takes one (under a synthetic 0.x id, `pulled_legacy_id`), its
/// draft and its registered decks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulledLeague {
    pub league: ImportedLeague,
    pub drafts: Vec<LegacyDraft>,
    pub decks: Vec<LegacyLimitedDeck>,
    /// The bundle's player fields (spec 6.3).
    pub player: sync_bundles::PulledLeagueFields,
}

/// The 0.x id a pulled league or draft is imported under: negative (0.x ids
/// are positive) and stable for its key (its client id, its draft token),
/// so pulling it again finds the same row.
pub fn pulled_legacy_id(key: &str) -> i64 {
    let hash = bundle_hash(key);
    let value = i64::from_str_radix(&hash[..15], 16).unwrap_or(0);
    -value - 1
}

// ---- Reads ------------------------------------------------------------------

fn setting(conn: &Connection, key: &str) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()?)
}

/// The stored slot ledger, if any.
pub fn stored_slots(conn: &Connection) -> Result<Option<Slots>, StoreError> {
    let Some(value) = setting(conn, SYNC_SLOTS)? else {
        return Ok(None);
    };
    let json: serde_json::Value = serde_json::from_str(&value).unwrap_or_default();
    Ok(slots_from_json(&json))
}

/// The account's plan: free unless the stored ledger says `supporter`
/// (fails closed).
pub fn tier(conn: &Connection) -> Result<Tier, StoreError> {
    Ok(stored_slots(conn)?.map_or(Tier::Free, |slots| slots.tier))
}

/// The last sync run's error.
pub fn last_error(conn: &Connection) -> Result<Option<String>, StoreError> {
    Ok(setting(conn, SYNC_LAST_ERROR)?
        .and_then(|value| serde_json::from_str::<Option<String>>(&value).ok())
        .flatten())
}

/// A deck's sideboard guides and matchup notes as its touch key reads them
/// (archetypes spec section 21.3, 0.x `$touches`): any guide or note
/// change moves one of them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GuideTouch {
    pub guides: i64,
    /// The newest guide `updated_at` (a plan save, a move).
    pub newest_guide_at: Option<i64>,
    pub notes: i64,
    pub highest_note_id: Option<i64>,
}

/// The cheap key a deck rebuild is made at (07 spec section 10), with the
/// deck archetype's uuid (archetypes spec section 14: a set, cleared or
/// server-moved archetype rebuilds the bundle).
pub fn deck_touch_key(
    updated_at: i64,
    versions: i64,
    newest_modified_at: Option<i64>,
    enabled: bool,
    guides: &GuideTouch,
    archetype_uuid: Option<&str>,
) -> String {
    let text = |value: Option<i64>| value.map_or(String::new(), |at| at.to_string());
    format!(
        "{updated_at}:{versions}:{}:{}:{}:{}:{}:{}:{}",
        text(newest_modified_at),
        u8::from(enabled),
        guides.guides,
        text(guides.newest_guide_at),
        guides.notes,
        text(guides.highest_note_id),
        text_key(archetype_uuid)
    )
}

/// The decks the sync can send (0.x `DirtyRows::base('deck')`), of every
/// platform (cross-device sync spec section 6): not archived, never a
/// local-only `limited:league-` deck, and no limited deck (MTGO or Arena,
/// `limited:` either way) on the free tier. In client id order.
pub fn deck_candidates(conn: &Connection, tier: Tier) -> Result<Vec<DeckCandidate>, StoreError> {
    let supporter = tier == Tier::Supporter;
    let mut statement = conn.prepare_cached(
        "SELECT d.id, d.mtgo_id, d.name, d.original_name, d.format, d.color_identity,
             d.cover_catalog_id, d.source = 'file', d.updated_at, d.cloud_sync_enabled,
             (SELECT COUNT(*) FROM deck_versions v WHERE v.deck_id = d.id),
             (SELECT MAX(v.modified_at) FROM deck_versions v WHERE v.deck_id = d.id),
             (SELECT COUNT(*) FROM sideboard_guides g WHERE g.deck_id = d.id),
             (SELECT MAX(g.updated_at) FROM sideboard_guides g WHERE g.deck_id = d.id),
             (SELECT COUNT(*) FROM deck_archetype_notes n WHERE n.deck_id = d.id),
             (SELECT MAX(n.id) FROM deck_archetype_notes n WHERE n.deck_id = d.id),
             (SELECT a.uuid FROM archetypes a WHERE a.id = d.archetype_id),
             COALESCE((SELECT a.platform FROM accounts a WHERE a.id = d.account_id),
                      d.pulled_platform)
         FROM decks d
         WHERE d.archived_at IS NULL AND d.mtgo_id NOT LIKE 'limited:league-%'
           AND (?1 OR d.mtgo_id NOT LIKE 'limited:%')",
    )?;
    let rows = statement.query_map([supporter], |row| {
        let mtgo_id: String = row.get(1)?;
        let updated_at: i64 = row.get(8)?;
        let enabled: bool = row.get(9)?;
        let versions: i64 = row.get(10)?;
        let newest: Option<i64> = row.get(11)?;
        let guides = GuideTouch {
            guides: row.get(12)?,
            newest_guide_at: row.get(13)?,
            notes: row.get(14)?,
            highest_note_id: row.get(15)?,
        };
        let archetype_uuid: Option<String> = row.get(16)?;
        let platform: Option<String> = row.get(17)?;
        Ok(DeckCandidate {
            deck_id: row.get(0)?,
            client_id: deck_client_id(&mtgo_id),
            mtgo_id,
            name: row.get(2)?,
            original_name: row.get(3)?,
            format: row.get(4)?,
            color_identity: row.get(5)?,
            cover_catalog_id: row.get(6)?,
            platform: bundle_platform(platform.as_deref().unwrap_or("mtgo")),
            owned: row.get(7)?,
            updated_at,
            touch_key: deck_touch_key(
                updated_at,
                versions,
                newest,
                enabled,
                &guides,
                archetype_uuid.as_deref(),
            ),
        })
    })?;
    let mut candidates = Vec::new();
    for row in rows {
        candidates.push(row?);
    }
    candidates.sort_by(|a, b| a.client_id.cmp(&b.client_id));
    Ok(candidates)
}

/// A deck's versions as `(signature, modified_at)`.
pub fn deck_versions(conn: &Connection, deck_id: i64) -> Result<Vec<(String, i64)>, StoreError> {
    let mut statement = conn.prepare_cached(
        "SELECT signature, modified_at FROM deck_versions WHERE deck_id = ?1 ORDER BY id",
    )?;
    let rows = statement.query_map([deck_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let mut versions = Vec::new();
    for row in rows {
        versions.push(row?);
    }
    Ok(versions)
}

/// A deck's archetype, sideboard guides and matchup notes as its bundle
/// takes them (archetypes spec section 21.5; the bundle sorts them).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeckRowParts {
    pub archetype_uuid: Option<String>,
    pub guides: Vec<GuideInput>,
    pub notes: Vec<NoteInput>,
}

/// The archetype, guides (with their cards) and notes of a deck, with the
/// pulled ones it could not place kept in its carry (`Unplaced`).
pub fn deck_bundle_parts(conn: &Connection, deck_id: i64) -> Result<DeckRowParts, StoreError> {
    let archetype_uuid: Option<String> = conn
        .prepare_cached(
            "SELECT a.uuid FROM decks d JOIN archetypes a ON a.id = d.archetype_id
             WHERE d.id = ?1",
        )?
        .query_row([deck_id], |row| row.get(0))
        .optional()?;
    let mut guides: Vec<GuideInput> = Vec::new();
    let mut guide_ids: Vec<i64> = Vec::new();
    {
        let mut statement = conn.prepare_cached(
            "SELECT g.id, a.uuid FROM sideboard_guides g
             JOIN archetypes a ON a.id = g.archetype_id
             WHERE g.deck_id = ?1 ORDER BY g.id",
        )?;
        let rows = statement.query_map([deck_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (guide_id, uuid) = row?;
            guide_ids.push(guide_id);
            guides.push(GuideInput {
                archetype_uuid: uuid,
                cards: Vec::new(),
            });
        }
    }
    let mut cards_statement = conn.prepare_cached(
        "SELECT oracle_id, direction, quantity FROM sideboard_guide_cards WHERE guide_id = ?1",
    )?;
    for (guide, guide_id) in guides.iter_mut().zip(guide_ids) {
        let rows = cards_statement.query_map([guide_id], |row| {
            Ok(GuideCardInput {
                oracle_id: row.get(0)?,
                direction: row.get(1)?,
                quantity: row.get(2)?,
            })
        })?;
        for row in rows {
            guide.cards.push(row?);
        }
    }
    let mut notes = Vec::new();
    let mut statement = conn.prepare_cached(
        "SELECT a.uuid, n.body, n.created_at FROM deck_archetype_notes n
         JOIN archetypes a ON a.id = n.archetype_id
         WHERE n.deck_id = ?1 ORDER BY n.id",
    )?;
    let rows = statement.query_map([deck_id], |row| {
        Ok(NoteInput {
            archetype_uuid: row.get(0)?,
            body: row.get(1)?,
            created_at_ms: Some(row.get(2)?),
        })
    })?;
    for row in rows {
        notes.push(row?);
    }
    // The pulled guides and notes this device could not place (final
    // review F17) go back as they came, never twice.
    let carried = carried_unplaced(conn, deck_id)?;
    // A set `decks.archetype_id` wins; else the pulled one it could not place.
    let archetype_uuid = archetype_uuid.or(carried.archetype_uuid);
    for guide in carried.guides {
        if !guides
            .iter()
            .any(|held| held.archetype_uuid == guide.archetype_uuid)
        {
            guides.push(guide);
        }
    }
    for note in carried.notes {
        if !notes.contains(&note) {
            notes.push(note);
        }
    }
    Ok(DeckRowParts {
        archetype_uuid,
        guides,
        notes,
    })
}

/// A deck candidate's bundle input: its row, versions, archetype, guides
/// and notes. An Endstep deck names its cards by ref (spec section 5.3):
/// each version's signature in the portable form and the cover as a ref,
/// as the match bundle writes its signature (`portable_signature`).
pub fn deck_bundle_input(
    conn: &Connection,
    candidate: &DeckCandidate,
) -> Result<sync_bundles::DeckBundleInput, StoreError> {
    let mut versions = deck_versions(conn, candidate.deck_id)?;
    let parts = deck_bundle_parts(conn, candidate.deck_id)?;
    let mut cover_catalog_id = candidate
        .cover_catalog_id
        .and_then(|id| u32::try_from(id).ok());
    let mut endstep_cover = None;
    // Endstep catalog ids are this device's own: every card goes by ref.
    if candidate.platform == BundlePlatform::Endstep {
        let mut named: BTreeSet<u32> = BTreeSet::new();
        for (signature, _) in &versions {
            named.extend(
                decks::decode_signature(signature)
                    .iter()
                    .map(|row| row.catalog_id),
            );
        }
        named.extend(cover_catalog_id);
        let named: Vec<u32> = named.into_iter().collect();
        let refs = crate::endstep_cards::refs_of(conn, &named)?;
        versions = portable_versions(&versions, &refs);
        endstep_cover = cover_catalog_id.and_then(|id| refs.get(&id).cloned());
        cover_catalog_id = None;
    }
    Ok(sync_bundles::DeckBundleInput {
        platform: candidate.platform,
        mtgo_id: candidate.mtgo_id.clone(),
        name: candidate.name.clone(),
        original_name: candidate.original_name.clone(),
        format: Some(candidate.format.clone()),
        color_identity: candidate.color_identity.clone(),
        cover_catalog_id,
        endstep_cover,
        versions,
        archetype_uuid: parts.archetype_uuid,
        guides: parts.guides,
        notes: parts.notes,
    })
}

/// An Endstep deck's versions with portable signatures, one per signature:
/// two local versions can read the same once a card without a ref is left
/// out, and a bundle lists each signature once, at its newest time.
fn portable_versions(
    versions: &[(String, i64)],
    refs: &BTreeMap<u32, EndstepCardRef>,
) -> Vec<(String, i64)> {
    let mut newest: BTreeMap<String, i64> = BTreeMap::new();
    for (local, modified_at) in versions {
        let portable = portable_signature(local, refs);
        let at = newest.entry(portable).or_insert(*modified_at);
        *at = (*at).max(*modified_at);
    }
    newest.into_iter().collect()
}

/// Every client id this device holds of a type (sent whole with the last
/// manifest call): every candidate, owned or not (a match still in its
/// `SEND_HOLD_MS` at `now_ms` is not one yet).
pub fn known_ids(
    conn: &Connection,
    sync_type: SyncType,
    tier: Tier,
    now_ms: i64,
) -> Result<Vec<String>, StoreError> {
    match sync_type {
        SyncType::Deck => Ok(deck_candidates(conn, tier)?
            .into_iter()
            .map(|candidate| candidate.client_id)
            .collect()),
        SyncType::League => Ok(league_candidates(conn, tier)?
            .into_iter()
            .map(|candidate| candidate.client_id)
            .collect()),
        SyncType::Match => Ok(match_candidates(conn, tier, now_ms)?
            .into_iter()
            .map(|candidate| candidate.client_id)
            .collect()),
    }
}

/// This device's rows of a type, by client id.
pub fn synced_rows(
    conn: &Connection,
    sync_type: SyncType,
) -> Result<HashMap<String, SyncedRow>, StoreError> {
    let mut statement = conn.prepare_cached(
        "SELECT client_id, hash, synced_at, built_hash, touch_key, changed_at, carry, pulled_key
         FROM synced_rows WHERE type = ?1",
    )?;
    let rows = statement.query_map([sync_type.as_str()], |row| {
        Ok(SyncedRow {
            client_id: row.get(0)?,
            hash: row.get(1)?,
            synced_at: row.get(2)?,
            built_hash: row.get(3)?,
            touch_key: row.get(4)?,
            changed_at: row.get(5)?,
            carry: row.get(6)?,
            pulled_key: row.get(7)?,
        })
    })?;
    let mut found = HashMap::new();
    for row in rows {
        let row = row?;
        found.insert(row.client_id.clone(), row);
    }
    Ok(found)
}

/// A type's run state, if it ever ran.
pub fn sync_state(
    conn: &Connection,
    sync_type: SyncType,
) -> Result<Option<SyncStateRow>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT last_synced_at, canonical_version, full_cursor, download_cursor
             FROM sync_state WHERE type = ?1",
            [sync_type.as_str()],
            |row| {
                Ok(SyncStateRow {
                    last_synced_at: row.get(0)?,
                    canonical_version: row.get(1)?,
                    full_cursor: row.get(2)?,
                    download_cursor: row.get(3)?,
                })
            },
        )
        .optional()?)
}

/// A type's rejections: client id to `(reason, hash)`.
pub fn rejections(
    conn: &Connection,
    sync_type: SyncType,
) -> Result<HashMap<String, (String, String)>, StoreError> {
    let mut statement =
        conn.prepare_cached("SELECT client_id, reason, hash FROM sync_rejections WHERE type = ?1")?;
    let rows = statement.query_map([sync_type.as_str()], |row| {
        Ok((row.get::<_, String>(0)?, (row.get(1)?, row.get(2)?)))
    })?;
    let mut found = HashMap::new();
    for row in rows {
        let (client_id, rejection) = row?;
        found.insert(client_id, rejection);
    }
    Ok(found)
}

/// Whether a candidate waits to be sent, without a rebuild (0.x
/// `DirtyRows::query`): never synced, never built, touched since the last
/// build, or built differently from what the server holds.
pub fn is_pending(candidate: &DeckCandidate, row: Option<&SyncedRow>) -> bool {
    is_pending_key(&candidate.touch_key, row)
}

/// How many rows of a type wait to be sent (the Account page's figure).
/// Only rows v1 owns are counted (ruling 5). A match still in its
/// `SEND_HOLD_MS` at `now_ms` is not counted yet (the same rule as
/// `match_candidates`, so the figure is what the next run sends).
pub fn pending_count(
    conn: &Connection,
    sync_type: SyncType,
    now_ms: i64,
) -> Result<u32, StoreError> {
    match sync_type {
        SyncType::Deck => {
            let rows = synced_rows(conn, SyncType::Deck)?;
            let pending = deck_candidates(conn, tier(conn)?)?
                .iter()
                .filter(|candidate| candidate.owned)
                .filter(|candidate| is_pending(candidate, rows.get(&candidate.client_id)))
                .count();
            Ok(pending as u32)
        }
        SyncType::League => {
            let rows = synced_rows(conn, sync_type)?;
            let mut pending = 0;
            for candidate in league_candidates(conn, tier(conn)?)? {
                if candidate.owned
                    && is_league_pending(conn, &candidate, rows.get(&candidate.client_id))?
                {
                    pending += 1;
                }
            }
            Ok(pending)
        }
        SyncType::Match => {
            let rows = synced_rows(conn, sync_type)?;
            let pending = match_candidates(conn, tier(conn)?, now_ms)?
                .iter()
                .filter(|candidate| candidate.owned)
                .filter(|candidate| {
                    is_pending_key(&candidate.touch_key, rows.get(&candidate.client_id))
                })
                .count();
            Ok(pending as u32)
        }
    }
}

/// Whether a league waits to be sent (`is_pending_key`), except an Arena
/// course run whose build here would be the bytes the server holds (#833):
/// a run pulled on a device that has not built it yet (a pull leaves no
/// build) reads as synced until something here changes it. The next run
/// builds it and, its hash being the server's, sends nothing.
pub fn is_league_pending(
    conn: &Connection,
    candidate: &RowCandidate,
    row: Option<&SyncedRow>,
) -> Result<bool, StoreError> {
    if !is_pending_key(&candidate.touch_key, row) {
        return Ok(false);
    }
    let Some(row) = row else {
        return Ok(true);
    };
    if candidate.platform != BundlePlatform::Arena {
        return Ok(true);
    }
    // The build waits for its carry, as the runner's does.
    let (Some(hash), Some(carry)) = (row.hash.as_deref(), row.carry.as_deref()) else {
        return Ok(true);
    };
    let Some(input) = arena_run_bundle_input(conn, candidate.row_id)? else {
        return Ok(true);
    };
    let carry: Value = serde_json::from_str(carry).unwrap_or_default();
    let carry = Carry::from_json(&carry, LEAGUE_CARRY);
    let bundle = arena_run_bundle(&input, carry.as_ref());
    Ok(bundle_hash(&canonical_json(&bundle)) != hash)
}

/// The newest `last_synced_at` of any type.
pub fn last_synced_at(conn: &Connection) -> Result<Option<i64>, StoreError> {
    Ok(
        conn.query_row("SELECT MAX(last_synced_at) FROM sync_state", [], |row| {
            row.get(0)
        })?,
    )
}

pub fn rejection_count(conn: &Connection) -> Result<u32, StoreError> {
    Ok(conn.query_row("SELECT COUNT(*) FROM sync_rejections", [], |row| row.get(0))?)
}

/// Archived decks: never offered again, so counted as not synced (0.x's
/// trashed decks and leagues; v1 leagues are never trashed).
pub fn not_synced_count(conn: &Connection) -> Result<u32, StoreError> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM decks WHERE archived_at IS NOT NULL",
        [],
        |row| row.get(0),
    )?)
}

/// The client ids this device holds of a type, for the cloud count: every
/// `known_ids` id, and for decks also the archived (deleted) decks the
/// server has a copy of (a `synced_rows` row with a hash, sent or pulled),
/// which the server keeps but this device never offers again.
pub fn held_ids(
    conn: &Connection,
    sync_type: SyncType,
    tier: Tier,
    now_ms: i64,
) -> Result<BTreeSet<String>, StoreError> {
    let mut held: BTreeSet<String> = known_ids(conn, sync_type, tier, now_ms)?
        .into_iter()
        .collect();
    if sync_type != SyncType::Deck {
        return Ok(held);
    }
    let mut statement = conn.prepare_cached(
        "SELECT client_id FROM synced_rows WHERE type = 'deck' AND hash IS NOT NULL",
    )?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut uploaded = BTreeSet::new();
    for row in rows {
        uploaded.insert(row?);
    }
    for client_id in archived_deck_client_ids(conn)? {
        if uploaded.contains(&client_id) {
            held.insert(client_id);
        }
    }
    Ok(held)
}

/// The client ids of archived decks (their slots are released by the run).
pub fn archived_deck_client_ids(conn: &Connection) -> Result<Vec<String>, StoreError> {
    let mut statement =
        conn.prepare_cached("SELECT mtgo_id FROM decks WHERE archived_at IS NOT NULL ORDER BY id")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(deck_client_id(&row?));
    }
    Ok(ids)
}

/// The limited decks whose slot is off, for a Supporter's run to turn on
/// (#825: limited decks have no switch): not archived, never a local-only
/// `limited:league-` deck. `(client id, platform)` in deck id order.
pub fn limited_decks_without_slot(
    conn: &Connection,
) -> Result<Vec<(String, BundlePlatform)>, StoreError> {
    let mut statement = conn.prepare_cached(
        "SELECT d.mtgo_id, COALESCE(a.platform, d.pulled_platform)
         FROM decks d LEFT JOIN accounts a ON a.id = d.account_id
         WHERE d.archived_at IS NULL AND d.cloud_sync_enabled = 0
           AND d.mtgo_id LIKE 'limited:%' AND d.mtgo_id NOT LIKE 'limited:league-%'
         ORDER BY d.id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
    })?;
    let mut decks = Vec::new();
    for row in rows {
        let (mtgo_id, platform) = row?;
        decks.push((
            deck_client_id(&mtgo_id),
            bundle_platform(platform.as_deref().unwrap_or("mtgo")),
        ));
    }
    Ok(decks)
}

/// A deck's MTGO id and whether it is archived.
pub fn deck_identity(
    conn: &Connection,
    deck_id: i64,
) -> Result<Option<(String, bool)>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT mtgo_id, archived_at IS NOT NULL FROM decks WHERE id = ?1",
            [deck_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}

/// The name of a deck holding a slot (0.x's slot limit message).
pub fn held_deck_name(conn: &Connection) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT name FROM decks WHERE cloud_sync_enabled = 1 ORDER BY id LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?)
}

/// The Account page's deck list (0.x `BuildDeckSyncList`): the constructed
/// decks of the accounts `account_ids`, plus pulled Arena or Endstep decks
/// still waiting for their account (`pulled_platform`); every deck of every
/// platform when `None`; never limited or archived. Enabled first, then the most
/// recently played, then by name.
pub fn deck_sync_list(
    conn: &Connection,
    account_ids: Option<&[i64]>,
) -> Result<Vec<DeckSyncRow>, StoreError> {
    let account_ids: Option<String> =
        account_ids.map(|ids| serde_json::Value::from(ids.to_vec()).to_string());
    let mut statement = conn.prepare_cached(
        "SELECT d.id, d.name, d.format, d.mtgo_id, d.cloud_sync_enabled,
             (SELECT MAX(m.started_at) FROM matches m
              JOIN deck_versions v ON v.id = m.deck_version_id WHERE v.deck_id = d.id) AS played,
             COALESCE(a.platform, d.pulled_platform)
         FROM decks d
         LEFT JOIN accounts a ON a.id = d.account_id
         WHERE d.archived_at IS NULL AND d.mtgo_id NOT LIKE 'limited:%' AND d.format <> 'Limited'
           AND (?1 IS NULL OR d.account_id IN (SELECT value FROM json_each(?1))
                OR (d.account_id IS NULL AND d.pulled_platform IS NOT NULL))
         ORDER BY d.cloud_sync_enabled DESC, played DESC, d.name, d.id",
    )?;
    let rows = statement.query_map([account_ids], |row| {
        let mtgo_id: String = row.get(3)?;
        Ok(DeckSyncRow {
            id: row.get(0)?,
            name: row.get(1)?,
            format: row.get(2)?,
            client_id: deck_client_id(&mtgo_id),
            cloud_sync_enabled: row.get(4)?,
            last_played_at: row.get(5)?,
            platform: row
                .get::<_, Option<crate::Platform>>(6)?
                .unwrap_or(crate::Platform::Mtgo),
        })
    })?;
    let mut list = Vec::new();
    for row in rows {
        list.push(row?);
    }
    Ok(list)
}

// ---- Matches and leagues (Plan Q task 11) ------------------------------------

/// A match or league the sync can send: its row, client id, whether v1 may
/// push it (its source is v1's or the player edited it, ruling 5 and spec
/// 6.2), its start (the push order), its own time (the first build's
/// `changed_at`) and its touch key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowCandidate {
    pub row_id: i64,
    pub client_id: String,
    pub owned: bool,
    pub started_at: i64,
    pub changed_at: i64,
    pub touch_key: String,
    /// The player edited the row (`edited_at IS NOT NULL`): it is pushed
    /// whatever its source (league self-management spec 6.2).
    pub edited: bool,
    /// The game of its bundle: a match's account's; an Arena course run's
    /// Arena; any other league MTGO.
    pub platform: BundlePlatform,
}

/// A short hash of free text for a touch key.
fn text_key(text: Option<&str>) -> String {
    text.map_or(String::new(), |text| bundle_hash(text)[..12].to_string())
}

/// The game sync's write stamp of a match: every column of its games and
/// their rows (ids aside, so a rewrite of the same rows keeps it) as JSON,
/// in a fixed order, hashed in `match_key_at` like the league key (#264).
/// A timeline counts by its frame count and blob sizes, not its bytes.
const GAME_STAMP_SQL: &str = "
    json_array(
     (SELECT json_group_array(json_array(g.game_number, g.mtgo_game_id, g.result,
          g.end_reason, g.on_play, g.local_mulligans, g.opp_mulligans,
          g.local_opening_hand_size, g.local_starting_hand_size, g.opp_opening_hand_size,
          g.opp_starting_hand_size, g.local_die_roll, g.opp_die_roll, g.turn_count,
          g.started_at, g.ended_at) ORDER BY g.game_number)
      FROM games g WHERE g.match_id = m.id),
     (SELECT json_group_array(json_array(g.game_number, c.side, c.catalog_id, c.main_qty,
          c.side_qty, c.sided_in, c.sided_out, c.opening_hand, c.bottomed, c.starting_hand,
          c.first_draw, c.seen, c.pregame_revealed, c.pregame_played, c.`cast`, c.played,
          c.activated, c.discarded, c.cast_turn, c.hand_seen, c.graveyard_seen,
          c.exile_seen, c.battlefield_seen, c.has_zone_data, c.inferred)
          ORDER BY g.game_number, c.side, c.catalog_id)
      FROM game_cards c JOIN games g ON g.id = c.game_id WHERE g.match_id = m.id),
     (SELECT json_group_array(json_array(g.game_number, k.side, k.catalog_id, k.method,
          k.count) ORDER BY g.game_number, k.side, k.catalog_id, k.method)
      FROM game_card_casts k JOIN games g ON g.id = k.game_id WHERE g.match_id = m.id),
     (SELECT json_group_array(json_array(g.game_number, u.hand, u.catalog_id, u.quantity)
          ORDER BY g.game_number, u.hand, u.catalog_id)
      FROM game_mulligan_cards u JOIN games g ON g.id = u.game_id WHERE g.match_id = m.id),
     (SELECT json_group_array(json_array(g.game_number, t.frame_count, length(t.raw),
          length(t.times)) ORDER BY g.game_number)
      FROM game_timelines t JOIN games g ON g.id = t.game_id WHERE g.match_id = m.id))";

/// The columns a match's touch key is made of (`m` the match, `l` its
/// league), read by `match_key_at`.
fn match_key_columns() -> String {
    format!(
        "m.revision, m.notes, m.hidden_at, m.deck_version_id, m.league_id, l.token,
         l.started_at, {GAME_STAMP_SQL}, {SEATS_STAMP_SQL}, m.no_league, m.league_linked,
         m.edited_at, l.sync_client_id, {ENDSTEP_RANKED_STAMP_SQL}"
    )
}

/// An Endstep match's ranked result for its touch key (the bundle carries
/// it as `endstep_ranked`), null when it has none.
const ENDSTEP_RANKED_STAMP_SQL: &str = "
    (SELECT json_array(r.status, r.format_id, r.rating_before, r.rating_after,
         r.score_before, r.score_after, r.delta, r.rd_before, r.rd_after, r.rank)
     FROM endstep_ranked_results r WHERE r.match_id = m.id)";

/// The match's archetype seats for its touch key (archetypes spec section
/// 14): each seat's uuid, confidence and hand-set flag, so a detection, a
/// hand assignment, a "none" or a server move rebuilds the bundle.
const SEATS_STAMP_SQL: &str = "
    COALESCE((SELECT a.uuid || '/' || x.confidence || '/' || x.manual
              FROM match_archetypes x JOIN archetypes a ON a.id = x.archetype_id
              WHERE x.match_id = m.id AND x.is_opponent = 0), '')
    || '|' ||
    COALESCE((SELECT a.uuid || '/' || x.confidence || '/' || x.manual
              FROM match_archetypes x JOIN archetypes a ON a.id = x.archetype_id
              WHERE x.match_id = m.id AND x.is_opponent = 1), '')";

/// A match's touch key from `match_key_columns`, read from column `first` on.
fn match_key_at(row: &rusqlite::Row, first: usize) -> Result<String, rusqlite::Error> {
    let revision: i64 = row.get(first)?;
    let notes: Option<String> = row.get(first + 1)?;
    let hidden_at: Option<i64> = row.get(first + 2)?;
    let deck_version_id: Option<i64> = row.get(first + 3)?;
    let league_id: Option<i64> = row.get(first + 4)?;
    let league_token: Option<String> = row.get(first + 5)?;
    let league_started: Option<i64> = row.get(first + 6)?;
    let games: String = row.get(first + 7)?;
    let seats: String = row.get(first + 8)?;
    let no_league: bool = row.get(first + 9)?;
    let league_linked: bool = row.get(first + 10)?;
    let edited_at: Option<i64> = row.get(first + 11)?;
    let league_client_id: Option<String> = row.get(first + 12)?;
    let endstep_ranked: Option<String> = row.get(first + 13)?;
    let mut key = format!(
        "{revision}:{}:{}:{}:{}:{}:{}:{}:{}",
        text_key(notes.as_deref()),
        hidden_at.map_or(String::new(), |at| at.to_string()),
        deck_version_id.map_or(String::new(), |id| id.to_string()),
        league_id.map_or(String::new(), |id| id.to_string()),
        text_key(league_token.as_deref()),
        league_started.map_or(String::new(), |at| at.to_string()),
        text_key(Some(&games)),
        text_key(Some(&seats)),
    );
    // The player's columns join the key once the row has an edit (league
    // self-management spec 6.2), so a row never edited keeps the key it
    // had and a pulled key stored before them still matches.
    if let Some(edited_at) = edited_at {
        key.push_str(&format!(
            ":{}:{}:{edited_at}",
            u8::from(no_league),
            u8::from(league_linked)
        ));
    }
    // The league's sync client id, which the bundle carries, joins the key
    // only when it is not the formula of the token and start already in it
    // (a swap by `take_synced_id`, a rekey, a pulled or imported id). A
    // league that kept its formula id keeps the key it had, so no bundle
    // rebuilds on upgrade and a pulled key stored before still matches.
    if let Some(client_id) = league_client_id {
        let formula = match (&league_token, league_started) {
            (Some(token), Some(started)) => Some(sync_bundles::league_client_id(token, started)),
            _ => None,
        };
        if formula.as_deref() != Some(client_id.as_str()) {
            key.push(':');
            key.push_str(&text_key(Some(&client_id)));
        }
    }
    // An Endstep ranked result joins the key only when there is one, so
    // every other match keeps the key it had.
    if let Some(ranked) = endstep_ranked {
        key.push_str(":r");
        key.push_str(&text_key(Some(&ranked)));
    }
    Ok(key)
}

/// One match's touch key, whether or not it is a candidate.
pub(crate) fn match_touch_key(conn: &Connection, match_id: i64) -> Result<String, StoreError> {
    let sql = format!(
        "SELECT {} FROM matches m LEFT JOIN leagues l ON l.id = m.league_id WHERE m.id = ?1",
        match_key_columns()
    );
    Ok(conn
        .prepare_cached(&sql)?
        .query_row([match_id], |row| match_key_at(row, 0))?)
}

/// The pulled keys of a type's rows (`synced_rows.pulled_key`), by client id.
fn pulled_keys(
    conn: &Connection,
    sync_type: SyncType,
) -> Result<HashMap<String, String>, StoreError> {
    let mut statement = conn.prepare_cached(
        "SELECT client_id, pulled_key FROM synced_rows
         WHERE type = ?1 AND pulled_key IS NOT NULL",
    )?;
    let rows = statement.query_map([sync_type.as_str()], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut found = HashMap::new();
    for row in rows {
        let (client_id, key) = row?;
        found.insert(client_id, key);
    }
    Ok(found)
}

/// A row v1 may push: its source is v1's (ruling 5) or the player edited it
/// (spec 6.2), and it is not a pulled row still as the server sent it (its
/// touch key equals its pulled key).
fn may_push(
    own_source: bool,
    touch_key: &str,
    pulled: &HashMap<String, String>,
    client_id: &str,
) -> bool {
    own_source && pulled.get(client_id).is_none_or(|key| key != touch_key)
}

/// The matches the sync can send (0.x `DirtyRows::base('match')`): complete
/// or abandoned (never detached), played with a deck version whose deck is
/// enabled for cloud sync; on the free tier, never a limited deck's; ended
/// (`ended_at`, else `last_activity_at`) at least `SEND_HOLD_MS` before
/// `now_ms`. In client id order. A pulled manual match is owned only once
/// edited here.
pub fn match_candidates(
    conn: &Connection,
    tier: Tier,
    now_ms: i64,
) -> Result<Vec<RowCandidate>, StoreError> {
    let supporter = tier == Tier::Supporter;
    let pulled = pulled_keys(conn, SyncType::Match)?;
    let sql = format!(
        "SELECT m.id, m.token, m.source IN ('log', 'manual'), m.started_at, m.last_activity_at,
             m.edited_at IS NOT NULL, a.platform, {}
         FROM matches m
         JOIN deck_versions v ON v.id = m.deck_version_id
         JOIN decks d ON d.id = v.deck_id
         JOIN accounts a ON a.id = m.account_id
         LEFT JOIN leagues l ON l.id = m.league_id
         WHERE m.state IN ('complete', 'abandoned') AND m.detached_at IS NULL
           AND d.cloud_sync_enabled = 1 AND (?1 OR d.mtgo_id NOT LIKE 'limited:%')
           AND COALESCE(m.ended_at, m.last_activity_at) <= ?2
         ORDER BY m.token",
        match_key_columns()
    );
    let mut statement = conn.prepare_cached(&sql)?;
    let rows = statement.query_map(params![supporter, held_until(now_ms)], |row| {
        let client_id: String = row.get(1)?;
        let own_source: bool = row.get(2)?;
        let edited: bool = row.get(5)?;
        let platform = bundle_platform(&row.get::<_, String>(6)?);
        let touch_key = match_key_at(row, 7)?;
        Ok(RowCandidate {
            row_id: row.get(0)?,
            owned: may_push(own_source || edited, &touch_key, &pulled, &client_id),
            client_id,
            started_at: row.get(3)?,
            changed_at: row.get(4)?,
            touch_key,
            edited,
            platform,
        })
    })?;
    let mut candidates = Vec::new();
    for row in rows {
        candidates.push(row?);
    }
    Ok(candidates)
}

/// The leagues the sync can send (0.x `DirtyRows::base('league')`): runs
/// with a token whose deck (`leagues.deck_id`) is enabled for cloud sync;
/// on the free tier, never a limited deck's. In client id order. A pulled
/// manual league is owned only once edited here. A removed league is sent
/// even once its row is detached (every match deleted with it), so its
/// removal reaches the other devices. Arena: only a course run with a
/// record (`arena:<course id>`, its deck the course deck), always owned;
/// never an orphan run (`arena:<event>:<token>`) or another league of an
/// Arena account. Endstep has no leagues.
pub fn league_candidates(conn: &Connection, tier: Tier) -> Result<Vec<RowCandidate>, StoreError> {
    let supporter = tier == Tier::Supporter;
    let pulled = pulled_keys(conn, SyncType::League)?;
    let mut statement = conn.prepare_cached(&format!(
        "SELECT l.id, l.sync_client_id, l.source IN ('log', 'manual'), l.started_at,
             MAX(l.started_at, COALESCE(l.joined_at, 0), COALESCE(l.completed_at, 0),
                 COALESCE(l.dropped_at, 0)),
             l.edited_at IS NOT NULL, l.run_key LIKE 'arena:%'
         FROM leagues l JOIN decks d ON d.id = l.deck_id
         WHERE l.token IS NOT NULL AND l.sync_client_id IS NOT NULL
           AND (l.detached_at IS NULL OR l.removed_at IS NOT NULL)
           AND d.cloud_sync_enabled = 1 AND (?1 OR d.mtgo_id NOT LIKE 'limited:%')
           AND ((l.run_key NOT LIKE 'arena:%'
                 AND (l.account_id IS NULL OR {league_mtgo})
                 AND (d.account_id IS NULL OR {deck_mtgo}))
                OR (l.run_key LIKE 'arena:%' AND instr(substr(l.run_key, 7), ':') = 0
                    AND l.course_observed_at IS NOT NULL
                    AND {league_arena}))",
        league_mtgo = account_of_mtgo("l.account_id"),
        deck_mtgo = account_of_mtgo("d.account_id"),
        league_arena = account_of_platform("l.account_id", "'arena'"),
    ))?;
    let rows = statement.query_map([supporter], |row| {
        let own_source: bool = row.get(2)?;
        let edited: bool = row.get(5)?;
        let arena_run: bool = row.get(6)?;
        Ok((
            RowCandidate {
                row_id: row.get(0)?,
                client_id: row.get(1)?,
                owned: own_source || edited,
                started_at: row.get(3)?,
                changed_at: row.get(4)?,
                touch_key: String::new(),
                edited,
                platform: if arena_run {
                    BundlePlatform::Arena
                } else {
                    BundlePlatform::Mtgo
                },
            },
            arena_run,
        ))
    })?;
    let mut candidates = Vec::new();
    let mut arena_runs = HashSet::new();
    for row in rows {
        let (candidate, arena_run) = row?;
        if arena_run {
            arena_runs.insert(candidate.row_id);
        }
        candidates.push(candidate);
    }
    drop(statement);
    for candidate in &mut candidates {
        candidate.touch_key = league_touch_key(conn, candidate.row_id)?;
        if arena_runs.contains(&candidate.row_id) {
            // An Arena course run is every device's fold output: each device
            // holding the course pushes it whenever its bundle is not the
            // server's (cross-device sync spec section 8).
            candidate.owned = true;
            continue;
        }
        // A pulled league is weighed by what its bundle holds.
        let own_key = league_own_key(conn, candidate.row_id)?;
        candidate.owned = may_push(candidate.owned, &own_key, &pulled, &candidate.client_id);
    }
    candidates.sort_by(|a, b| a.client_id.cmp(&b.client_id));
    Ok(candidates)
}

/// A league's touch key (07 spec section 10): its own columns, its matches'
/// revisions, its draft, its picks and their notes, and its matches'
/// registered decks, hashed. It drives the rebuild of a league v1 owns.
fn league_touch_key(conn: &Connection, league_id: i64) -> Result<String, StoreError> {
    league_key(conn, league_id, true)
}

/// A league's own key: what its bundle holds (its columns, draft, picks and
/// registered decks), without its matches, which travel in their own
/// bundles. A pulled league's `pulled_key` is this key, so editing a member
/// match never makes a pulled league this device's.
pub(crate) fn league_own_key(conn: &Connection, league_id: i64) -> Result<String, StoreError> {
    league_key(conn, league_id, false)
}

fn league_key(conn: &Connection, league_id: i64, with_matches: bool) -> Result<String, StoreError> {
    let matches = if with_matches {
        "(SELECT json_group_array(json_array(m.token, m.revision))
          FROM matches m WHERE m.league_id = l.id),"
    } else {
        ""
    };
    let sql = format!(
        "SELECT json_array(l.source, l.kind, l.state, l.token, l.mtgo_course_id, l.name,
             l.format, l.set_code, l.deck_version_id, l.deck_id, l.started_at, l.joined_at,
             l.completed_at, l.dropped_at, l.notes,
             {matches}
             (SELECT json_group_array(json_array(d.draft_token, d.mtgo_draft_id, d.pod_token,
                  d.seat_count, d.seat_index, d.booster_catalog_id, d.state, d.pack_size,
                  d.picks_expected, d.started_at, d.ended_at))
              FROM drafts d WHERE d.league_id = l.id),
             (SELECT json_group_array(json_array(p.ordinal, p.pack_number, p.pick_number,
                  p.pack_id, p.direction, p.cards_available, p.picked_catalog_id,
                  p.picked_card_id, p.reservations, p.shown_at, p.deadline_at, p.picked_at,
                  p.note, p.detached_at))
              FROM draft_picks p JOIN drafts d ON d.id = p.draft_id WHERE d.league_id = l.id),
             (SELECT json_group_array(json_array(m.token, s.source, s.signature,
                  s.captured_at))
              FROM limited_deck_snapshots s JOIN matches m ON m.id = s.match_id
              WHERE m.league_id = l.id))
             -- An Arena course run's record and notes time, which its bundle
             -- carries; only on such a run, so every other key is unchanged.
             || CASE WHEN l.course_observed_at IS NULL THEN ''
                     ELSE '|course' || json_array(l.course_wins, l.course_losses,
                                                  l.course_observed_at, l.notes_edited_at)
                END,
             l.edited_at, l.removed_at, l.user_state, l.user_dropped_at
         FROM leagues l WHERE l.id = ?1"
    );
    let (mut parts, edited_at, removed_at, user_state, user_dropped_at): (
        String,
        Option<i64>,
        Option<i64>,
        Option<String>,
        Option<i64>,
    ) = conn.prepare_cached(&sql)?.query_row([league_id], |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        ))
    })?;
    // The player's columns join the key once the league has an edit (spec
    // 6.2), as the match key does.
    if let Some(edited_at) = edited_at {
        parts.push_str(&format!(
            "|{edited_at}|{}|{}|{}",
            removed_at.map_or(String::new(), |at| at.to_string()),
            user_state.unwrap_or_default(),
            user_dropped_at.map_or(String::new(), |at| at.to_string()),
        ));
    }
    Ok(bundle_hash(&parts))
}

/// Whether a row waits to be sent, without a rebuild: never synced, never
/// built, touched since the last build, or built differently from what the
/// server holds.
pub fn is_pending_key(touch_key: &str, row: Option<&SyncedRow>) -> bool {
    let Some(row) = row else {
        return true;
    };
    row.hash.is_none()
        || row.built_hash.is_none()
        || row.touch_key.as_deref() != Some(touch_key)
        || row.built_hash != row.hash
}

/// A deck version's list, for a manual game's postboard sideboard.
fn registered_rows(signature: Option<&str>) -> Vec<DeckRow> {
    decks::decode_signature(signature.unwrap_or(""))
        .into_iter()
        .map(|card| DeckRow {
            catalog_id: card.catalog_id,
            quantity: card.quantity,
            sideboard: card.sideboard,
        })
        .collect()
}

/// A live match's frames by Game ID, each with its event's UTC time.
fn live_frames(
    conn: &Connection,
    token: &str,
) -> Result<HashMap<u64, Vec<FrameInput>>, StoreError> {
    let mut frames: HashMap<u64, Vec<FrameInput>> = HashMap::new();
    for stored in crate::read::events_for_match(conn, token)? {
        let Some(event) = mtgo_protocol::prepare_at(
            stored.log_file_id as u64,
            stored.byte_offset,
            stored.logged_at_ms,
            &stored.raw,
            None,
        ) else {
            continue;
        };
        if event.kind != mtgo_protocol::EventKind::GameStateUpdate {
            continue;
        }
        let (Some(game_id), Some(content)) = (
            event.subjects.game_id,
            game_detail::frame_value(&event.text),
        ) else {
            continue;
        };
        frames.entry(game_id).or_default().push(FrameInput {
            at_ms: event.at_ms,
            content,
        });
    }
    Ok(frames)
}

/// 0.x `MatchBundleBuilder::frameInstant` (replays spec section 10, ruling
/// 12): the UTC instant of an imported frame. A time of day (`HH:MM:SS`,
/// `HH:MM:SS.mmm`) is dated on the local day of the game's end
/// (`ended_ms`) in `timezone`, a day earlier when that puts it more than 12
/// hours after the end (0.x `ConvertMtgoTimestamp`), so a game that crossed
/// midnight keeps its order. Any other text is read as an instant (ISO 8601
/// with its zone, or `Y-m-d H:i:s` in UTC, as 0.x's `Carbon::parse`); `None`
/// when it does not read.
fn frame_instant(time: &str, ended_ms: i64, timezone: Tz) -> Option<i64> {
    if !replay::frames::is_time_of_day(time) {
        if let Ok(instant) = chrono::DateTime::parse_from_rfc3339(time) {
            return Some(instant.timestamp_millis());
        }
        let utc = chrono::NaiveDateTime::parse_from_str(time, "%Y-%m-%d %H:%M:%S%.f").ok()?;
        return Some(utc.and_utc().timestamp_millis());
    }
    let clock = chrono::NaiveTime::parse_from_str(time, "%H:%M:%S%.f").ok()?;
    let ended = chrono::DateTime::from_timestamp_millis(ended_ms)?.with_timezone(&timezone);
    let day = ended.date_naive();
    let at = local_to_utc_ms(day.and_time(clock), timezone)?;
    if at > ended_ms + 12 * 60 * 60 * 1000 {
        return local_to_utc_ms(day.pred_opt()?.and_time(clock), timezone);
    }
    Some(at)
}

/// The oracle id of every catalog id a match's rows name.
fn oracle_ids(conn: &Connection, match_id: i64) -> Result<HashMap<u32, String>, StoreError> {
    let mut statement = conn.prepare_cached(
        "SELECT DISTINCT c.catalog_id, k.oracle_id FROM game_cards c
         JOIN games g ON g.id = c.game_id
         JOIN cards k ON k.catalog_id = c.catalog_id
         WHERE g.match_id = ?1 AND k.oracle_id IS NOT NULL",
    )?;
    let rows = statement.query_map([match_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let mut found = HashMap::new();
    for row in rows {
        let (catalog_id, oracle_id) = row?;
        found.insert(catalog_id, oracle_id);
    }
    Ok(found)
}

/// One stored game's bundle columns (its rows and frames are read after).
fn game_input(row: &rusqlite::Row) -> Result<(i64, MatchGameInput), rusqlite::Error> {
    let game_id: i64 = row.get(0)?;
    Ok((
        game_id,
        MatchGameInput {
            game_number: row.get(1)?,
            mtgo_game_id: row.get(2)?,
            result: row.get(3)?,
            on_play: row.get(4)?,
            local_mulligans: row.get(5)?,
            opp_mulligans: row.get(6)?,
            local_die_roll: row.get(7)?,
            opp_die_roll: row.get(8)?,
            turn_count: row.get(9)?,
            local_starting_hand_size: row.get(10)?,
            opp_starting_hand_size: row.get(11)?,
            started_at_ms: row.get(12)?,
            ended_at_ms: row.get(13)?,
            cards: Vec::new(),
            casts: Vec::new(),
            mulligans: Vec::new(),
            frames: Vec::new(),
        },
    ))
}

/// A match's bundle columns from its stored rows (07 spec section 10):
/// `None` when the match is gone. Frames: a log match's from its events
/// (live data), any other's from `game_timelines`, each dated from its own
/// time text (`frame_instant`, in `timezone`, the OS one), else at the
/// game's end.
pub fn match_bundle_input(
    conn: &Connection,
    match_id: i64,
    timezone: Tz,
) -> Result<Option<MatchBundleInput>, StoreError> {
    let head = conn
        .prepare_cached(
            "SELECT m.token, m.mtgo_match_id, m.source, m.format, m.match_type, m.state,
                 m.outcome, m.started_at, m.ended_at, m.notes, m.games_won_imported,
                 m.games_lost_imported, d.mtgo_id, v.signature, l.sync_client_id, l.token,
                 m.tournament_event_id, m.tournament_round, a.username, o.username,
                 m.hidden_at, m.no_league, m.league_linked, m.edited_at, a.platform,
                 a.display_name, a.arena_user_id, o.display_name, m.game_structure
             FROM matches m
             JOIN accounts a ON a.id = m.account_id
             LEFT JOIN opponents o ON o.id = m.opponent_id
             LEFT JOIN deck_versions v ON v.id = m.deck_version_id
             LEFT JOIN decks d ON d.id = v.deck_id
             LEFT JOIN leagues l ON l.id = m.league_id
             WHERE m.id = ?1",
        )?
        .query_row([match_id], |row| {
            // The league's stored id, for a league with a token (spec 2.3).
            let league_id: Option<String> = row.get(14)?;
            let league_token: Option<String> = row.get(15)?;
            let signature: Option<String> = row.get(13)?;
            let linked: bool = row.get(22)?;
            let edited_at_ms: Option<i64> = row.get(23)?;
            let platform = bundle_platform(&row.get::<_, String>(24)?);
            let opponent_username: Option<String> = row.get(19)?;
            let (local_player_fields, opponent_player_fields) = if platform == BundlePlatform::Arena
            {
                arena_player_fields(
                    row.get(25)?,
                    row.get(26)?,
                    row.get(27)?,
                    opponent_username.as_deref(),
                )
            } else {
                Default::default()
            };
            Ok(MatchBundleInput {
                platform,
                sources: None,
                endstep_cards: BTreeMap::new(),
                endstep_ranked: None,
                local_player_fields,
                opponent_player_fields,
                token: row.get(0)?,
                mtgo_match_id: row.get(1)?,
                source: row.get(2)?,
                format: row.get(3)?,
                match_type: row.get(4)?,
                game_structure: row.get(28)?,
                state: row.get(5)?,
                outcome: row.get(6)?,
                started_at_ms: row.get(7)?,
                ended_at_ms: row.get(8)?,
                notes: row.get(9)?,
                games_won_imported: row.get(10)?,
                games_lost_imported: row.get(11)?,
                deck_mtgo_id: row.get(12)?,
                registered: registered_rows(signature.as_deref()),
                deck_version_signature: signature,
                league_client_id: match league_token {
                    Some(_) => league_id,
                    None => None,
                },
                tournament_event_id: row.get(16)?,
                tournament_round: row.get(17)?,
                local_username: row.get(18)?,
                opponent_username,
                games: Vec::new(),
                archetypes: Vec::new(),
                hidden_at_ms: row.get(20)?,
                no_league: row.get(21)?,
                // A link the player made (spec 4.1): imports link every 0.x
                // and pulled member, so the bare flag would change those
                // bundles.
                league_linked: linked && edited_at_ms.is_some(),
                edited_at_ms,
            })
        })
        .optional()?;
    let Some(mut input) = head else {
        return Ok(None);
    };
    let oracles = oracle_ids(conn, match_id)?;
    let mut games: Vec<(i64, MatchGameInput)> = Vec::new();
    {
        let mut statement = conn.prepare_cached(
            "SELECT id, game_number, mtgo_game_id, result, on_play, local_mulligans,
                 opp_mulligans, local_die_roll, opp_die_roll, turn_count,
                 local_starting_hand_size, opp_starting_hand_size, started_at, ended_at
             FROM games WHERE match_id = ?1 ORDER BY game_number, id",
        )?;
        let mut rows = statement.query([match_id])?;
        while let Some(row) = rows.next()? {
            games.push(game_input(row)?);
        }
    }
    let mut live = if input.source == "log" {
        live_frames(conn, &input.token)?
    } else {
        HashMap::new()
    };
    for (game_id, game) in &mut games {
        let cards = crate::game_sync::read_game_cards(conn, *game_id)?;
        for row in &cards.cards {
            game.cards.push(CardRowInput {
                opponent: row.side == Side::Opponent,
                catalog_id: row.catalog_id,
                oracle_id: oracles.get(&row.catalog_id).cloned(),
                main_qty: row.main_qty,
                side_qty: row.side_qty,
                sided_in: row.sided_in,
                sided_out: row.sided_out,
                opening_hand: row.opening_hand,
                bottomed: row.bottomed,
                starting_hand: row.starting_hand,
                seen: row.seen,
                pregame_revealed: row.pregame_revealed,
                pregame_played: row.pregame_played,
                cast: row.cast,
                played: row.played,
                activated: row.activated,
            });
        }
        for cast in &cards.casts {
            game.casts.push(CastInput {
                opponent: cast.side == Side::Opponent,
                catalog_id: cast.catalog_id,
                method: cast.method.clone(),
                count: cast.count,
            });
        }
        game.mulligans = cards.mulligans.clone();
        if input.source == "log" {
            let game_id = game.mtgo_game_id.and_then(|id| u64::try_from(id).ok());
            game.frames = game_id.and_then(|id| live.remove(&id)).unwrap_or_default();
        } else if let Some(frames) = crate::game_sync::timeline(conn, *game_id)? {
            let ended_ms = game
                .ended_at_ms
                .or(game.started_at_ms)
                .unwrap_or(input.started_at_ms);
            for frame in frames {
                // Strict: the whole text is one JSON value. The viewer's
                // tolerant read would change the bundle's hash.
                if let Ok(content) = serde_json::from_str::<Value>(&frame.text) {
                    let at_ms = frame_instant(&frame.time, ended_ms, timezone).unwrap_or(ended_ms);
                    game.frames.push(FrameInput { at_ms, content });
                }
            }
        }
    }
    input.games = games.into_iter().map(|(_, game)| game).collect();
    input.sources =
        crate::sync_sources::match_sources(conn, match_id, &input.token, input.platform)?;
    if input.platform == BundlePlatform::Endstep {
        input.endstep_cards = crate::endstep_cards::refs_of(conn, &named_cards(&input))?;
        input.deck_version_signature = input
            .deck_version_signature
            .as_deref()
            .map(|local| portable_signature(local, &input.endstep_cards));
        input.endstep_ranked = crate::endstep_sync::endstep_rating(conn, match_id)?.map(|ranked| {
            EndstepRankedFields {
                status: ranked.status,
                format_id: ranked.format_id,
                rating_before: ranked.rating_before,
                rating_after: ranked.rating_after,
                score_before: ranked.score_before,
                score_after: ranked.score_after,
                delta: ranked.delta,
                rd_before: ranked.rd_before,
                rd_after: ranked.rd_after,
                rank: ranked.rank,
            }
        });
    }
    input.archetypes = match_seats(conn, match_id)?;
    // The pulled rows it could not place (final review F17) go back for
    // every seat without a row of its own, but not an opponent the player
    // has set to "none" since (#300 R2).
    let opponent_none: bool = conn
        .prepare_cached("SELECT opponent_archetype_none = 1 FROM matches WHERE id = ?1")?
        .query_row([match_id], |row| row.get(0))?;
    for side in carried_match_sides(conn, &input.token)? {
        if side.opponent && opponent_none {
            continue;
        }
        if !input
            .archetypes
            .iter()
            .any(|held| held.opponent == side.opponent)
        {
            input.archetypes.push(side);
        }
    }
    Ok(Some(input))
}

/// The platform of an `accounts.platform` value (MTGO for any other).
fn bundle_platform(text: &str) -> BundlePlatform {
    match text {
        "arena" => BundlePlatform::Arena,
        "endstep" => BundlePlatform::Endstep,
        _ => BundlePlatform::Mtgo,
    }
}

/// An Arena match's seat fields (spec section 5.2), each when known: the
/// account's screen name and Arena user id, the opponent's screen name and
/// the user id its `arena:<user id>` username holds.
fn arena_player_fields(
    account_display_name: Option<String>,
    account_user_id: Option<String>,
    opponent_display_name: Option<String>,
    opponent_username: Option<&str>,
) -> (ArenaPlayerFields, ArenaPlayerFields) {
    let local = ArenaPlayerFields {
        display_name: account_display_name,
        arena_user_id: account_user_id,
    };
    let opponent = ArenaPlayerFields {
        display_name: opponent_display_name,
        arena_user_id: opponent_username
            .and_then(|name| name.strip_prefix("arena:"))
            .filter(|id| !id.is_empty())
            .map(str::to_string),
    };
    (local, opponent)
}

/// Every catalog id a match's rows name (cards, casts, mulligan hands and
/// the registered list), sorted.
fn named_cards(input: &MatchBundleInput) -> Vec<u32> {
    let mut ids = BTreeSet::new();
    for game in &input.games {
        ids.extend(game.cards.iter().map(|row| row.catalog_id));
        ids.extend(game.casts.iter().map(|cast| cast.catalog_id));
        ids.extend(game.mulligans.iter().map(|(_, catalog_id, _)| *catalog_id));
    }
    ids.extend(input.registered.iter().map(|row| row.catalog_id));
    ids.into_iter().collect()
}

/// An Endstep deck version's local signature as its portable form
/// (`sync_bundles::endstep_cards::endstep_signature`): each row's card by
/// ref. A row whose card has no ref (no `endstep_cards` row) is left out:
/// its local id means nothing on another device.
fn portable_signature(local: &str, refs: &BTreeMap<u32, EndstepCardRef>) -> String {
    let mut cards = Vec::new();
    for row in decks::decode_signature(local) {
        if let Some(card) = refs.get(&row.catalog_id) {
            cards.push(EndstepDeckCard {
                card: card.clone(),
                quantity: row.quantity,
                sideboard: row.sideboard,
            });
        }
    }
    endstep_signature(&cards)
}

/// A match's `match_archetypes` seats, player first.
fn match_seats(conn: &Connection, match_id: i64) -> Result<Vec<ArchetypeSide>, StoreError> {
    let mut statement = conn.prepare_cached(
        "SELECT a.uuid, x.confidence, x.manual, x.is_opponent FROM match_archetypes x
         JOIN archetypes a ON a.id = x.archetype_id
         WHERE x.match_id = ?1 ORDER BY x.is_opponent",
    )?;
    let rows = statement.query_map([match_id], |row| {
        Ok(ArchetypeSide {
            uuid: row.get(0)?,
            confidence: row.get(1)?,
            manual: row.get(2)?,
            opponent: row.get(3)?,
        })
    })?;
    let mut seats = Vec::new();
    for row in rows {
        seats.push(row?);
    }
    Ok(seats)
}

/// A league's bundle columns from its stored rows: the run, its draft with
/// its attached picks, and its matches' registered decks. `None` when the
/// league is gone or has no token.
pub fn league_bundle_input(
    conn: &Connection,
    league_id: i64,
) -> Result<Option<LeagueBundleInput>, StoreError> {
    let head = conn
        .prepare_cached(
            "SELECT l.token, l.name, l.format, l.started_at, l.joined_at, l.dropped_at,
                 l.completed_at, l.state, l.notes, l.source, l.kind, l.set_code,
                 l.mtgo_course_id, d.mtgo_id, v.signature, l.removed_at, l.user_state,
                 l.user_dropped_at, l.edited_at
             FROM leagues l
             LEFT JOIN deck_versions v ON v.id = l.deck_version_id
             LEFT JOIN decks d ON d.id = v.deck_id
             WHERE l.id = ?1 AND l.token IS NOT NULL",
        )?
        .query_row([league_id], |row| {
            let source: String = row.get(9)?;
            Ok(LeagueBundleInput {
                platform: sync_bundles::BundlePlatform::Mtgo,
                token: row.get(0)?,
                name: row.get(1)?,
                format: row.get(2)?,
                started_at_ms: row.get(3)?,
                joined_at_ms: row.get(4)?,
                dropped_at_ms: row.get(5)?,
                completed_at_ms: row.get(6)?,
                state: row.get(7)?,
                notes: row.get(8)?,
                manual: source == "manual",
                kind: row.get(10)?,
                set_code: row.get(11)?,
                mtgo_course_id: row.get(12)?,
                deck_mtgo_id: row.get(13)?,
                deck_version_signature: row.get(14)?,
                drafts: Vec::new(),
                snapshots: Vec::new(),
                removed_at_ms: row.get(15)?,
                user_state: row.get(16)?,
                user_dropped_at_ms: row.get(17)?,
                edited_at_ms: row.get(18)?,
            })
        })
        .optional()?;
    let Some(mut input) = head else {
        return Ok(None);
    };
    let mut drafts: Vec<(i64, DraftInput)> = Vec::new();
    {
        let mut statement = conn.prepare_cached(
            "SELECT id, draft_token, mtgo_draft_id, pod_token, seat_count, seat_index,
                 booster_catalog_id, state, pack_size, picks_expected, started_at, ended_at
             FROM drafts WHERE league_id = ?1 ORDER BY id",
        )?;
        let rows = statement.query_map([league_id], |row| {
            Ok((
                row.get(0)?,
                DraftInput {
                    draft_token: row.get(1)?,
                    mtgo_draft_id: row.get(2)?,
                    pod_token: row.get(3)?,
                    seat_count: row.get(4)?,
                    seat_index: row.get(5)?,
                    booster_catalog_id: row.get(6)?,
                    state: row.get(7)?,
                    pack_size: row.get(8)?,
                    picks_expected: row.get(9)?,
                    started_at_ms: row.get(10)?,
                    ended_at_ms: row.get(11)?,
                    picks: Vec::new(),
                },
            ))
        })?;
        for row in rows {
            drafts.push(row?);
        }
    }
    for (draft_id, draft) in &mut drafts {
        let mut statement = conn.prepare_cached(
            "SELECT ordinal, pack_number, pick_number, pack_id, direction, cards_available,
                 picked_catalog_id, picked_card_id, reservations, shown_at, deadline_at,
                 picked_at, note
             FROM draft_picks WHERE draft_id = ?1 AND detached_at IS NULL ORDER BY ordinal",
        )?;
        let rows = statement.query_map([*draft_id], |row| {
            let cards: String = row.get(5)?;
            let reservations: String = row.get(8)?;
            Ok((
                PickInput {
                    ordinal: row.get(0)?,
                    pack_number: row.get(1)?,
                    pick_number: row.get(2)?,
                    pack_id: row.get(3)?,
                    direction: row.get(4)?,
                    cards_available: serde_json::from_str(&cards)
                        .unwrap_or(Value::Array(Vec::new())),
                    picked_catalog_id: row.get(6)?,
                    picked_card_id: row.get(7)?,
                    reservations: Vec::new(),
                    shown_at_ms: row.get(9)?,
                    deadline_at_ms: row.get(10)?,
                    picked_at_ms: row.get(11)?,
                    note: row.get(12)?,
                },
                reservations,
            ))
        })?;
        for row in rows {
            let (mut pick, reservations) = row?;
            let listed: Value = serde_json::from_str(&reservations).unwrap_or_default();
            for entry in listed.as_array().map(Vec::as_slice).unwrap_or(&[]) {
                let catalog_id = entry["catalog_id"]
                    .as_u64()
                    .and_then(|id| u32::try_from(id).ok());
                if let (Some(catalog_id), Some(at_ms)) = (catalog_id, entry["at_ms"].as_i64()) {
                    pick.reservations.push((catalog_id, at_ms));
                }
            }
            draft.picks.push(pick);
        }
    }
    input.drafts = drafts.into_iter().map(|(_, draft)| draft).collect();
    let mut statement = conn.prepare_cached(
        "SELECT s.cards, s.signature, s.captured_at, m.token
         FROM limited_deck_snapshots s JOIN matches m ON m.id = s.match_id
         WHERE m.league_id = ?1 ORDER BY s.captured_at, s.signature, m.token, s.source",
    )?;
    let rows = statement.query_map([league_id], |row| {
        let cards: String = row.get(0)?;
        Ok(SnapshotInput {
            cards: serde_json::from_str(&cards).unwrap_or(Value::Array(Vec::new())),
            signature: row.get(1)?,
            captured_at_ms: row.get(2)?,
            match_token: row.get(3)?,
        })
    })?;
    for row in rows {
        input.snapshots.push(row?);
    }
    Ok(Some(input))
}

/// Whether a league is an Arena run (`arena:`): its bundle is an Arena run
/// bundle or none, never the 0.x one.
pub fn is_arena_league(conn: &Connection, league_id: i64) -> Result<bool, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM leagues WHERE id = ?1 AND run_key LIKE 'arena:%')",
        )?
        .query_row([league_id], |row| row.get(0))?)
}

/// An Arena course run's bundle input (cross-device sync spec section 8):
/// the course, its event (from a local observation, else the pulled run),
/// its record and state, its notes and the player's fields. `None` for a
/// league that is not an Arena course run, or one with no record yet.
pub fn arena_run_bundle_input(
    conn: &Connection,
    league_id: i64,
) -> Result<Option<ArenaRunBundle>, StoreError> {
    let row = conn
        .prepare_cached(
            "SELECT run_key, account_id, name, COALESCE(format, ''), kind, state, course_wins,
                 course_losses, course_observed_at, notes, notes_edited_at, removed_at,
                 user_state, user_dropped_at, edited_at
             FROM leagues WHERE id = ?1",
        )?
        .query_row([league_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                (
                    row.get::<_, Option<u32>>(6)?,
                    row.get::<_, Option<u32>>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                ),
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<i64>>(10)?,
                PulledLeagueFields {
                    removed_at_ms: row.get(11)?,
                    user_state: row.get(12)?,
                    user_dropped_at_ms: row.get(13)?,
                    edited_at_ms: row.get(14)?,
                },
            ))
        })
        .optional()?;
    let Some((run_key, account_id, name, format, kind, state, record, notes, notes_at, player)) =
        row
    else {
        return Ok(None);
    };
    let Some(course_id) = leagues::arena_course_id(&run_key) else {
        return Ok(None);
    };
    let (Some(wins), Some(losses), Some(observed_at_ms)) = record else {
        return Ok(None);
    };
    let event_name: Option<String> = conn
        .prepare_cached(
            "SELECT MIN(event_name) FROM (
                 SELECT event_name FROM arena_course_observations
                 WHERE account_id IS ?1 AND course_id = ?2
                 UNION ALL
                 SELECT event_name FROM synced_arena_runs
                 WHERE account_id IS ?1 AND course_id = ?2
             )",
        )?
        .query_row(params![account_id, course_id], |row| {
            row.get::<_, Option<String>>(0)
        })
        .optional()?
        .flatten();
    let Some(event_name) = event_name else {
        return Err(StoreError::Corrupt(format!(
            "the Arena run {run_key} has no event name"
        )));
    };
    Ok(Some(ArenaRunBundle {
        course_id: course_id.to_string(),
        event_name,
        name,
        format,
        kind,
        snapshot: RunSnapshot {
            wins,
            losses,
            state,
            observed_at_ms,
        },
        notes,
        notes_edited_at_ms: notes_at,
        player,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_reads::SEND_HOLD_MS;
    use crate::import_decks::{ImportedDeck, ImportedVersion};
    use crate::test_support::{account, db, insert_manual};
    use mymtgo_api::{SlotDeck, Slots, SyncType, Tier};
    use rusqlite::params;
    use sync_bundles::league_client_id;

    const T: i64 = 1_800_000_000_000;
    /// A now long after every fixture match's `SEND_HOLD_MS`.
    const AFTER_THE_HOLD: i64 = i64::MAX;

    fn deck(conn: &Connection, mtgo_id: &str, enabled: bool) -> i64 {
        conn.query_row(
            "INSERT INTO decks (mtgo_id, name, format, created_at, updated_at, source,
                 cloud_sync_enabled)
             VALUES (?1, 'Qdeck ' || ?1, 'CMODERN', ?2, ?2, 'file', ?3) RETURNING id",
            params![mtgo_id, T, enabled],
            |row| row.get(0),
        )
        .expect("deck")
    }

    fn version(conn: &Connection, deck_id: i64, signature: &str, at: i64) -> i64 {
        conn.query_row(
            "INSERT INTO deck_versions (deck_id, signature, modified_at) VALUES (?1, ?2, ?3)
             RETURNING id",
            params![deck_id, signature, at],
            |row| row.get(0),
        )
        .expect("version")
    }

    fn played_with(conn: &Connection, token: &str, version_id: i64) {
        insert_manual(conn, token);
        conn.execute(
            "UPDATE matches SET deck_version_id = ?2 WHERE token = ?1",
            params![token, version_id],
        )
        .expect("link");
    }

    fn league(conn: &Connection, token: &str, deck_id: i64) -> String {
        conn.execute(
            "INSERT INTO leagues (run_key, source, kind, state, token, name, deck_id, started_at,
                 created_at)
             VALUES ('log:' || ?1, 'log', 'constructed', 'complete', ?1, 'Qleague', ?2, ?3, ?3)",
            params![token, deck_id, T],
        )
        .expect("league");
        sync_bundles::league_client_id(token, T)
    }

    fn ledger(enabled: &[&str], cooling: &[&str]) -> Slots {
        let mut decks: Vec<SlotDeck> = enabled
            .iter()
            .map(|id| SlotDeck {
                client_id: (*id).to_string(),
                enabled_at: Some("x".to_string()),
                disabled_at: None,
                frees_at: None,
            })
            .collect();
        decks.extend(cooling.iter().map(|id| SlotDeck {
            client_id: (*id).to_string(),
            enabled_at: Some("x".to_string()),
            disabled_at: Some("y".to_string()),
            frees_at: Some("z".to_string()),
        }));
        Slots {
            limit: Some(1),
            used: enabled.len() as u32,
            tier: Tier::Free,
            decks: Some(decks),
        }
    }

    fn apply(conn: &mut Connection, slots: Slots) {
        edit_sync(conn, &SyncEdit::ApplySlots { slots }).expect("apply");
    }

    fn enabled(conn: &Connection, deck_id: i64) -> bool {
        conn.query_row(
            "SELECT cloud_sync_enabled FROM decks WHERE id = ?1",
            [deck_id],
            |row| row.get(0),
        )
        .expect("flag")
    }

    fn updated_at(conn: &Connection, deck_id: i64) -> i64 {
        conn.query_row(
            "SELECT updated_at FROM decks WHERE id = ?1",
            [deck_id],
            |row| row.get(0),
        )
        .expect("updated_at")
    }

    fn reject(conn: &mut Connection, sync_type: SyncType, client_id: &str, reason: &str) {
        let rows = vec![Rejection {
            client_id: client_id.to_string(),
            reason: reason.to_string(),
            hash: "q".repeat(64),
        }];
        edit_sync(conn, &SyncEdit::Rejected { sync_type, rows }).expect("reject");
    }

    fn rejected(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT client_id FROM sync_rejections ORDER BY client_id")
            .expect("prepare");
        stmt.query_map([], |row| row.get(0))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect()
    }

    fn store_hash(conn: &mut Connection, sync_type: SyncType, client_id: &str, hash: &str) {
        let edit = SyncEdit::Stored {
            sync_type,
            rows: vec![(client_id.to_string(), hash.to_string())],
            now_ms: T,
        };
        edit_sync(conn, &edit).expect("stored");
    }

    fn built(conn: &mut Connection, candidate: &DeckCandidate, hash: &str) {
        let edit = SyncEdit::Built {
            sync_type: SyncType::Deck,
            rows: vec![BuiltRow {
                client_id: candidate.client_id.clone(),
                built_hash: hash.to_string(),
                touch_key: candidate.touch_key.clone(),
                changed_at: candidate.updated_at,
            }],
        };
        edit_sync(conn, &edit).expect("built");
    }

    fn pending_decks(conn: &Connection) -> u32 {
        pending_count(conn, SyncType::Deck, AFTER_THE_HOLD).expect("pending")
    }

    fn client_ids(conn: &Connection) -> Vec<String> {
        deck_candidates(conn, tier(conn).expect("tier"))
            .expect("candidates")
            .into_iter()
            .map(|candidate| candidate.client_id)
            .collect()
    }

    fn set_tier(conn: &Connection, value: &str) {
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('sync_slots', ?1)",
            [value],
        )
        .expect("slots row");
    }

    // ---- DirtinessTest (deck cases) ---------------------------------------

    #[test]
    fn treats_a_never_synced_row_as_dirty() {
        let (_dir, conn) = db("sync-never-synced");
        deck(&conn, "111", false);
        assert_eq!(pending_decks(&conn), 1);
    }

    #[test]
    fn marks_a_deck_dirty_when_a_version_is_added_and_excludes_archived_decks() {
        let (_dir, mut conn) = db("sync-version-added");
        let id = deck(&conn, "111", false);
        version(&conn, id, "QsigA", T);
        let candidate = deck_candidates(&conn, Tier::Free)
            .expect("candidates")
            .remove(0);
        built(&mut conn, &candidate, "h1");
        store_hash(&mut conn, SyncType::Deck, "111", "h1");
        assert_eq!(pending_decks(&conn), 0, "clean once stored");

        version(&conn, id, "QsigB", T + 1_000);
        assert_eq!(pending_decks(&conn), 1, "a new version touches the deck");

        conn.execute("UPDATE decks SET archived_at = 1 WHERE id = ?1", [id])
            .expect("archive");
        assert_eq!(pending_decks(&conn), 0, "an archived deck never syncs");
        assert!(client_ids(&conn).is_empty());
    }

    #[test]
    fn lists_known_ids_per_type() {
        let (_dir, conn) = db("sync-known-ids");
        deck(&conn, "111", false);
        deck(&conn, "limited:abc", false);
        deck(&conn, "Q deck/1", false);
        let tier = tier(&conn).expect("tier");
        assert_eq!(
            known_ids(&conn, SyncType::Deck, tier, AFTER_THE_HOLD).expect("known"),
            vec!["111".to_string(), "Q_deck_1".to_string()]
        );
        assert!(
            known_ids(&conn, SyncType::League, tier, AFTER_THE_HOLD)
                .expect("known")
                .is_empty()
        );
        assert!(
            known_ids(&conn, SyncType::Match, tier, AFTER_THE_HOLD)
                .expect("known")
                .is_empty()
        );
    }

    #[test]
    fn an_imported_deck_is_known_but_never_pending() {
        let (_dir, conn) = db("sync-imported-deck");
        let id = deck(&conn, "111", false);
        conn.execute("UPDATE decks SET source = 'imported' WHERE id = ?1", [id])
            .expect("imported");
        assert_eq!(client_ids(&conn), vec!["111".to_string()]);
        assert_eq!(pending_decks(&conn), 0, "v1 pushes only the decks it owns");
    }

    #[test]
    fn the_touch_key_follows_updated_at_versions_and_the_flag() {
        let (_dir, mut conn) = db("sync-touch-key");
        let id = deck(&conn, "111", false);
        let before = deck_candidates(&conn, Tier::Free)
            .expect("candidates")
            .remove(0);
        conn.execute(
            "UPDATE decks SET cloud_sync_enabled = 1 WHERE id = ?1",
            [id],
        )
        .expect("flag");
        let after = deck_candidates(&conn, Tier::Free)
            .expect("candidates")
            .remove(0);
        assert_ne!(before.touch_key, after.touch_key);
        built(&mut conn, &after, "h1");
        store_hash(&mut conn, SyncType::Deck, "111", "h1");
        assert_eq!(pending_decks(&conn), 0);
        conn.execute(
            "UPDATE decks SET updated_at = updated_at + 1 WHERE id = ?1",
            [id],
        )
        .expect("touch");
        assert_eq!(pending_decks(&conn), 1);
    }

    fn mark_synced(conn: &mut Connection) {
        let candidate = deck_candidates(conn, Tier::Free)
            .expect("candidates")
            .remove(0);
        built(conn, &candidate, "h1");
        store_hash(conn, SyncType::Deck, &candidate.client_id, "h1");
        assert_eq!(pending_decks(conn), 0);
    }

    /// 0.x `DirtinessTest` "marks a deck dirty when a sideboard plan is
    /// saved or a matchup note lands" (spec 21.3): the touch key holds the
    /// deck's guide count, newest guide time, note count and highest note
    /// id.
    #[test]
    fn a_saved_plan_or_a_new_note_makes_the_deck_pending() {
        let (_dir, mut conn) = db("sync-touch-key-guides");
        let id = deck(&conn, "111", false);
        let archetype: i64 = conn
            .query_row(
                "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
                 VALUES ('q-arch', 'Qarch', 'modern', 'downloaded', 1, 1) RETURNING id",
                [],
                |row| row.get(0),
            )
            .expect("archetype");
        mark_synced(&mut conn);

        let created = crate::archetype_edit::ArchetypeEdit::CreateGuide {
            deck_id: id,
            archetype_id: archetype,
            now_ms: T + 1_000,
        };
        let guide_id =
            match crate::archetype_edit::edit_archetypes(&mut conn, &created).expect("guide") {
                crate::archetype_edit::ArchetypeEditOutcome::Guide { guide_id } => guide_id,
                other => panic!("no guide: {other:?}"),
            };
        assert_eq!(pending_decks(&conn), 1, "a new guide");
        mark_synced(&mut conn);

        let saved = crate::archetype_edit::ArchetypeEdit::SaveGuideCards {
            deck_id: id,
            guide_id,
            cards: Vec::new(),
            now_ms: T + 2_000,
        };
        crate::archetype_edit::edit_archetypes(&mut conn, &saved).expect("save");
        assert_eq!(pending_decks(&conn), 1, "a saved plan");
        mark_synced(&mut conn);

        let noted = crate::archetype_edit::ArchetypeEdit::AddNote {
            deck_id: id,
            archetype_id: archetype,
            body: "Qnote".to_string(),
            now_ms: T + 2_000,
        };
        crate::archetype_edit::edit_archetypes(&mut conn, &noted).expect("note");
        assert_eq!(pending_decks(&conn), 1, "a new note");
        mark_synced(&mut conn);

        let note_id: i64 = conn
            .query_row("SELECT id FROM deck_archetype_notes", [], |row| row.get(0))
            .expect("note id");
        let deleted = crate::archetype_edit::ArchetypeEdit::DeleteNote { note_id };
        crate::archetype_edit::edit_archetypes(&mut conn, &deleted).expect("delete");
        assert_eq!(pending_decks(&conn), 1, "a deleted note");
    }

    /// T11 review I1: deleting the newest note and adding another between
    /// two sync checks must change the touch key. Note ids are never
    /// reused (`AUTOINCREMENT`), so the highest id moves.
    #[test]
    fn a_note_added_after_deleting_the_newest_one_makes_the_deck_pending() {
        let (_dir, mut conn) = db("sync-touch-key-note-reuse");
        let id = deck(&conn, "111", false);
        let archetype: i64 = conn
            .query_row(
                "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
                 VALUES ('q-arch', 'Qarch', 'modern', 'downloaded', 1, 1) RETURNING id",
                [],
                |row| row.get(0),
            )
            .expect("archetype");
        let add = |conn: &mut Connection, body: &str| {
            let edit = crate::archetype_edit::ArchetypeEdit::AddNote {
                deck_id: id,
                archetype_id: archetype,
                body: body.to_string(),
                now_ms: T + 1_000,
            };
            crate::archetype_edit::edit_archetypes(conn, &edit).expect("note");
        };
        add(&mut conn, "Qolder");
        add(&mut conn, "Qnewest");
        mark_synced(&mut conn);

        let newest: i64 = conn
            .query_row("SELECT MAX(id) FROM deck_archetype_notes", [], |row| {
                row.get(0)
            })
            .expect("newest");
        let deleted = crate::archetype_edit::ArchetypeEdit::DeleteNote { note_id: newest };
        crate::archetype_edit::edit_archetypes(&mut conn, &deleted).expect("delete");
        add(&mut conn, "Qreplacement");
        assert_eq!(pending_decks(&conn), 1, "the replacement note is sent");
    }

    #[test]
    fn the_touch_key_holds_the_guide_parts() {
        let none = GuideTouch::default();
        assert_eq!(
            deck_touch_key(5, 2, Some(9), true, &none, None),
            "5:2:9:1:0::0::"
        );
        let some = GuideTouch {
            guides: 2,
            newest_guide_at: Some(40),
            notes: 3,
            highest_note_id: Some(17),
        };
        assert_eq!(
            deck_touch_key(5, 2, None, false, &some, Some("q-arch")),
            format!("5:2::0:2:40:3:17:{}", &bundle_hash("q-arch")[..12])
        );
    }

    // ---- LimitedTierGateTest (deck cases) ---------------------------------

    #[test]
    fn treats_a_missing_tier_as_free() {
        let (_dir, conn) = db("sync-tier-missing");
        assert_eq!(tier(&conn).expect("tier"), Tier::Free);
        set_tier(
            &conn,
            r#"{"tier":"supporter","limit":null,"used":0,"decks":[]}"#,
        );
        assert_eq!(tier(&conn).expect("tier"), Tier::Supporter);
    }

    #[test]
    fn keeps_a_free_account_limited_deck_out_of_the_dirty_set() {
        let (_dir, conn) = db("sync-tier-free");
        set_tier(&conn, r#"{"tier":"free","limit":1,"used":0,"decks":[]}"#);
        deck(&conn, "limited:abc", false);
        deck(&conn, "110186502", false);
        assert_eq!(client_ids(&conn), vec!["110186502".to_string()]);
        assert_eq!(pending_decks(&conn), 1);
    }

    #[test]
    fn includes_a_supporter_limited_deck_in_the_dirty_set() {
        let (_dir, conn) = db("sync-tier-supporter");
        set_tier(
            &conn,
            r#"{"tier":"supporter","limit":null,"used":0,"decks":[]}"#,
        );
        deck(&conn, "limited:abc", false);
        assert_eq!(client_ids(&conn), vec!["limited_abc".to_string()]);
    }

    #[test]
    fn excludes_a_local_only_limited_deck_from_every_tier() {
        let (_dir, conn) = db("sync-tier-local");
        set_tier(
            &conn,
            r#"{"tier":"supporter","limit":null,"used":0,"decks":[]}"#,
        );
        deck(&conn, "limited:league-7", false);
        assert!(client_ids(&conn).is_empty());
    }

    #[test]
    fn treats_a_non_string_or_unrecognized_tier_as_free() {
        for value in [
            r#"{"tier":123,"limit":1,"used":0,"decks":[]}"#,
            r#"{"tier":{"nested":"array"},"limit":1,"used":0,"decks":[]}"#,
            r#"{"tier":"pro","limit":1,"used":0,"decks":[]}"#,
        ] {
            let (_dir, conn) = db("sync-tier-unknown");
            set_tier(&conn, value);
            deck(&conn, "limited:abc", false);
            assert_eq!(tier(&conn).expect("tier"), Tier::Free, "{value}");
            assert!(client_ids(&conn).is_empty(), "{value}");
        }
    }

    // ---- ApplyDeckSyncSlotsTest -------------------------------------------

    #[test]
    fn sets_deck_flags_from_the_server_slot_list_and_stores_the_summary() {
        let (_dir, mut conn) = db("sync-slots-flags");
        let on = deck(&conn, "111", false);
        let cooling = deck(&conn, "222", true);
        let limited = deck(&conn, "limited:abc", false);
        let other = deck(&conn, "333", true);
        let slots = ledger(&["111", "limited_abc"], &["222"]);
        let changed = edit_sync(
            &mut conn,
            &SyncEdit::ApplySlots {
                slots: slots.clone(),
            },
        )
        .expect("apply");
        assert_eq!(changed, 4, "the deck flags it changed");
        assert_eq!(
            edit_sync(
                &mut conn,
                &SyncEdit::ApplySlots {
                    slots: slots.clone()
                }
            )
            .expect("again"),
            0,
            "the same ledger again changes no flag"
        );
        assert!(enabled(&conn, on));
        assert!(!enabled(&conn, cooling));
        assert!(enabled(&conn, limited));
        assert!(!enabled(&conn, other));
        assert_eq!(stored_slots(&conn).expect("slots"), Some(slots));
    }

    #[test]
    fn clears_deck_not_synced_rejections_for_a_deck_that_just_became_enabled() {
        let (_dir, mut conn) = db("sync-slots-clear");
        let id = deck(&conn, "111", false);
        let version_id = version(&conn, id, "QsigA", T);
        played_with(&conn, "qmatch", version_id);
        reject(&mut conn, SyncType::Match, "qmatch", "deck_not_synced");
        reject(&mut conn, SyncType::Match, "unrelated", "hash_mismatch");
        apply(&mut conn, ledger(&["111"], &[]));
        assert_eq!(rejected(&conn), vec!["unrelated".to_string()]);
    }

    #[test]
    fn leaves_updated_at_alone_so_a_second_apply_of_the_same_ledger_changes_nothing() {
        let (_dir, mut conn) = db("sync-slots-updated-at");
        let id = deck(&conn, "111", false);
        let before = updated_at(&conn, id);
        apply(&mut conn, ledger(&["111"], &[]));
        let candidate = deck_candidates(&conn, Tier::Free)
            .expect("candidates")
            .remove(0);
        built(&mut conn, &candidate, "h1");
        store_hash(&mut conn, SyncType::Deck, "111", "h1");
        apply(&mut conn, ledger(&["111"], &[]));
        assert!(enabled(&conn, id));
        assert_eq!(updated_at(&conn, id), before);
        assert_eq!(pending_decks(&conn), 0);
    }

    #[test]
    fn lowers_the_flag_on_an_archived_deck_and_never_raises_it() {
        let (_dir, mut conn) = db("sync-slots-archived");
        let absent = deck(&conn, "444", true);
        let listed = deck(&conn, "555", true);
        conn.execute("UPDATE decks SET archived_at = 1", [])
            .expect("archive");
        apply(&mut conn, ledger(&["555"], &[]));
        apply(&mut conn, ledger(&["555"], &[]));
        assert!(!enabled(&conn, absent));
        assert!(!enabled(&conn, listed));
    }

    #[test]
    fn clears_rejections_parked_against_an_already_enabled_deck_and_keeps_a_disabled_ones() {
        let (_dir, mut conn) = db("sync-slots-parked");
        let on = deck(&conn, "111", true);
        let on_version = version(&conn, on, "QsigA", T);
        played_with(&conn, "qon", on_version);
        let on_league = league(&conn, "qleague", on);
        let off = deck(&conn, "222", false);
        let off_version = version(&conn, off, "QsigB", T);
        played_with(&conn, "qoff", off_version);
        reject(&mut conn, SyncType::Match, "qon", "deck_not_synced");
        reject(&mut conn, SyncType::League, &on_league, "deck_not_synced");
        reject(&mut conn, SyncType::Match, "qoff", "deck_not_synced");
        apply(&mut conn, ledger(&["111"], &[]));
        apply(&mut conn, ledger(&["111"], &[]));
        assert_eq!(rejected(&conn), vec!["qoff".to_string()]);
    }

    #[test]
    fn forgets_the_synced_state_of_a_decks_matches_and_leagues_when_it_becomes_enabled() {
        let (_dir, mut conn) = db("sync-slots-forget");
        let id = deck(&conn, "111", false);
        let version_id = version(&conn, id, "QsigA", T);
        played_with(&conn, "qmatch", version_id);
        let league_id = league(&conn, "qleague", id);
        let other = deck(&conn, "222", true);
        let other_version = version(&conn, other, "QsigB", T);
        played_with(&conn, "qother", other_version);
        store_hash(&mut conn, SyncType::Match, "qmatch", "ha");
        store_hash(&mut conn, SyncType::League, &league_id, "hb");
        store_hash(&mut conn, SyncType::Match, "qother", "hc");
        apply(&mut conn, ledger(&["111", "222"], &[]));
        let rows = synced_rows(&conn, SyncType::Match).expect("rows");
        assert_eq!(rows["qmatch"].hash, None);
        assert_eq!(rows["qmatch"].synced_at, None);
        assert_eq!(rows["qother"].hash.as_deref(), Some("hc"));
        let leagues = synced_rows(&conn, SyncType::League).expect("rows");
        assert_eq!(leagues[&league_id].hash, None);
    }

    // ---- Rows, rejections, state ------------------------------------------

    #[test]
    fn a_stored_row_gets_its_hash_an_empty_carry_and_loses_its_rejection() {
        let (_dir, mut conn) = db("sync-stored");
        reject(&mut conn, SyncType::Deck, "111", "hash_mismatch");
        store_hash(&mut conn, SyncType::Deck, "111", "h1");
        let row = synced_rows(&conn, SyncType::Deck).expect("rows")["111"].clone();
        assert_eq!(row.hash.as_deref(), Some("h1"));
        assert_eq!(row.synced_at, Some(T));
        assert_eq!(row.carry.as_deref(), Some("{}"), "never fetched again");
        assert!(rejected(&conn).is_empty());
        let carried = SyncEdit::Carried {
            sync_type: SyncType::Deck,
            rows: vec![("111".to_string(), r#"{"/notes":[]}"#.to_string())],
        };
        edit_sync(&mut conn, &carried).expect("carry");
        store_hash(&mut conn, SyncType::Deck, "111", "h2");
        let row = synced_rows(&conn, SyncType::Deck).expect("rows")["111"].clone();
        assert_eq!(
            row.carry.as_deref(),
            Some(r#"{"/notes":[]}"#),
            "a carry is kept"
        );
    }

    // A carry the server withholds (a deck switched off server-side, a blob
    // missing on disk) is asked for again until it has been asked enough
    // times AND enough time has passed; only then is it settled as empty.
    #[test]
    fn a_withheld_carry_stays_unset_for_a_bounded_number_of_runs_and_hours() {
        let (_dir, mut conn) = db("sync-carry-withheld");
        conn.execute(
            "INSERT INTO synced_rows (type, client_id, hash) VALUES ('league', 'ql', 'h0')",
            [],
        )
        .expect("a 0.x synced row");
        let day = 24 * 3_600_000;
        let withheld = |now_ms: i64| SyncEdit::CarryWithheld {
            sync_type: SyncType::League,
            client_ids: vec!["ql".to_string()],
            max_asks: 3,
            min_age_ms: day,
            now_ms,
        };
        let carry = |conn: &Connection| {
            synced_rows(conn, SyncType::League).expect("rows")["ql"]
                .carry
                .clone()
        };
        for minute in 0..5 {
            edit_sync(&mut conn, &withheld(T + minute * 60_000)).expect("withheld");
            assert_eq!(carry(&conn), None, "quick runs do not settle it");
        }
        edit_sync(&mut conn, &withheld(T + day - 1)).expect("withheld");
        assert_eq!(carry(&conn), None, "just under a day");
        edit_sync(&mut conn, &withheld(T + day)).expect("withheld");
        assert_eq!(carry(&conn).as_deref(), Some("{}"), "enough asks and a day");
    }

    #[test]
    fn a_rejection_is_replaced_with_its_new_reason_and_hash() {
        let (_dir, mut conn) = db("sync-rejected");
        reject(&mut conn, SyncType::Match, "qm", "hash_mismatch");
        let rows = vec![Rejection {
            client_id: "qm".to_string(),
            reason: "too_large_local".to_string(),
            hash: "h2".to_string(),
        }];
        edit_sync(
            &mut conn,
            &SyncEdit::Rejected {
                sync_type: SyncType::Match,
                rows,
            },
        )
        .expect("reject");
        let found = rejections(&conn, SyncType::Match).expect("rejections");
        assert_eq!(
            found["qm"],
            ("too_large_local".to_string(), "h2".to_string())
        );
        assert_eq!(rejection_count(&conn).expect("count"), 1);
    }

    #[test]
    fn the_cursor_and_the_finish_are_kept_per_type() {
        let (_dir, mut conn) = db("sync-state");
        assert_eq!(sync_state(&conn, SyncType::Deck).expect("state"), None);
        let cursor = SyncEdit::Cursor {
            sync_type: SyncType::Deck,
            full_cursor: "222".to_string(),
            canonical_version: 4,
        };
        edit_sync(&mut conn, &cursor).expect("cursor");
        assert_eq!(
            sync_state(&conn, SyncType::Deck).expect("state"),
            Some(SyncStateRow {
                last_synced_at: None,
                canonical_version: 4,
                full_cursor: Some("222".to_string()),
                download_cursor: None,
            })
        );
        let finished = SyncEdit::Finished {
            sync_type: SyncType::Deck,
            last_synced_at: T,
            canonical_version: 4,
            download_cursor: None,
        };
        edit_sync(&mut conn, &finished).expect("finish");
        assert_eq!(
            sync_state(&conn, SyncType::Deck).expect("state"),
            Some(SyncStateRow {
                last_synced_at: Some(T),
                canonical_version: 4,
                full_cursor: None,
                download_cursor: None,
            })
        );
        assert_eq!(last_synced_at(&conn).expect("last"), Some(T));
    }

    // Sync identity spec 3.1: a finish stores the server's cursor exactly;
    // a finish without one (an older server) keeps the stored cursor.
    #[test]
    fn a_finish_stores_the_server_cursor_and_one_without_keeps_it() {
        let (_dir, mut conn) = db("sync-state-download-cursor");
        let finish = |cursor: Option<&str>, at: i64| SyncEdit::Finished {
            sync_type: SyncType::Match,
            last_synced_at: at,
            canonical_version: 4,
            download_cursor: cursor.map(str::to_string),
        };
        edit_sync(&mut conn, &finish(Some("2026-10-16T09:58:00.123456Z"), T)).expect("first");
        let stored = |conn: &Connection| {
            sync_state(conn, SyncType::Match)
                .expect("state")
                .and_then(|state| state.download_cursor)
        };
        assert_eq!(
            stored(&conn).as_deref(),
            Some("2026-10-16T09:58:00.123456Z")
        );
        edit_sync(&mut conn, &finish(None, T + 1)).expect("older server");
        assert_eq!(
            stored(&conn).as_deref(),
            Some("2026-10-16T09:58:00.123456Z")
        );
        edit_sync(
            &mut conn,
            &finish(Some("2026-10-16T10:30:00.000001Z"), T + 2),
        )
        .expect("newer");
        assert_eq!(
            stored(&conn).as_deref(),
            Some("2026-10-16T10:30:00.000001Z")
        );
        assert_eq!(sync_state(&conn, SyncType::Deck).expect("deck"), None);
    }

    #[test]
    fn the_last_error_is_set_and_cleared() {
        let (_dir, mut conn) = db("sync-last-error");
        assert_eq!(last_error(&conn).expect("error"), None);
        let set = SyncEdit::LastError {
            error: Some("Sync API request failed with status 422: Q".to_string()),
        };
        edit_sync(&mut conn, &set).expect("set");
        assert_eq!(
            last_error(&conn).expect("error").as_deref(),
            Some("Sync API request failed with status 422: Q")
        );
        edit_sync(&mut conn, &SyncEdit::LastError { error: None }).expect("clear");
        assert_eq!(last_error(&conn).expect("error"), None);
    }

    // ---- SyncSettingsTest: unlink -----------------------------------------

    #[test]
    fn clears_deck_flags_and_stored_slots_on_sign_out() {
        let (_dir, mut conn) = db("sync-signed-out");
        let id = deck(&conn, "111", true);
        let before = updated_at(&conn, id);
        apply(&mut conn, ledger(&["111"], &[]));
        edit_sync(&mut conn, &SyncEdit::SignedOut).expect("signed out");
        assert!(!enabled(&conn, id));
        assert_eq!(stored_slots(&conn).expect("slots"), None);
        assert_eq!(updated_at(&conn, id), before);
    }

    // An archived deck the server holds is this device's, not one waiting
    // in the cloud; one never sent is not on the server at all.
    #[test]
    fn held_ids_count_archived_decks_the_server_has_once() {
        let (_dir, mut conn) = db("sync-held-ids");
        deck(&conn, "111", false);
        store_hash(&mut conn, SyncType::Deck, "111", "h1");
        let archived = deck(&conn, "Q deck/2", false);
        store_hash(&mut conn, SyncType::Deck, "Q_deck_2", "h2");
        let never_sent = deck(&conn, "333", false);
        conn.execute(
            "UPDATE decks SET archived_at = 1 WHERE id IN (?1, ?2)",
            [archived, never_sent],
        )
        .expect("archive");
        let tier = tier(&conn).expect("tier");
        let held = held_ids(&conn, SyncType::Deck, tier, AFTER_THE_HOLD).expect("held");
        assert_eq!(
            held.into_iter().collect::<Vec<_>>(),
            vec!["111".to_string(), "Q_deck_2".to_string()]
        );
        assert!(
            held_ids(&conn, SyncType::League, tier, AFTER_THE_HOLD)
                .expect("held")
                .is_empty()
        );
    }

    #[test]
    fn counts_archived_decks_as_not_synced() {
        let (_dir, conn) = db("sync-not-synced");
        deck(&conn, "111", false);
        let archived = deck(&conn, "222", false);
        conn.execute("UPDATE decks SET archived_at = 1 WHERE id = ?1", [archived])
            .expect("archive");
        assert_eq!(not_synced_count(&conn).expect("count"), 1);
        assert_eq!(
            archived_deck_client_ids(&conn).expect("ids"),
            vec!["222".to_string()]
        );
    }

    // ---- Pulled decks (DeckImportAccountTest, BundleRoundTripTest) ------

    fn pulled(mtgo_id: &str) -> ImportedDeck {
        ImportedDeck {
            mtgo_id: mtgo_id.to_string(),
            account: None,
            name: "Qpulled".to_string(),
            original_name: None,
            format: "CMODERN".to_string(),
            color_identity: Some("UR".to_string()),
            cover_catalog_id: Some(424_242),
            archived_at_ms: None,
            created_at_ms: None,
            updated_at_ms: None,
            versions: vec![ImportedVersion {
                signature: "QsigA".to_string(),
                modified_at_ms: T,
            }],
        }
    }

    fn pull(conn: &mut Connection, deck: ImportedDeck) {
        pull_with(conn, deck, PulledDeckParts::default(), T);
    }

    fn pull_with(conn: &mut Connection, deck: ImportedDeck, parts: PulledDeckParts, now_ms: i64) {
        let edit = SyncEdit::PulledDeck {
            client_id: sync_bundles::deck_client_id(&deck.mtgo_id),
            deck: Box::new(deck),
            parts: Box::new(parts),
            hash: "hpulled".to_string(),
            carry: "{}".to_string(),
            now_ms,
        };
        edit_sync(conn, &edit).expect("pull");
    }

    fn deck_account(conn: &Connection, mtgo_id: &str) -> Option<i64> {
        conn.query_row(
            "SELECT account_id FROM decks WHERE mtgo_id = ?1",
            [mtgo_id],
            |row| row.get(0),
        )
        .expect("deck")
    }

    #[test]
    fn attaches_a_pulled_deck_to_the_current_account() {
        let (_dir, mut conn) = db("sync-pull-account");
        let current = account(&conn, "Qplayer");
        pull(&mut conn, pulled("111"));
        assert_eq!(deck_account(&conn, "111"), Some(current));
    }

    #[test]
    fn leaves_a_deck_that_already_belongs_to_another_account_alone() {
        let (_dir, mut conn) = db("sync-pull-other");
        let owner = account(&conn, "Qowner");
        let id = deck(&conn, "111", false);
        conn.execute(
            "UPDATE decks SET account_id = ?2, source = 'imported' WHERE id = ?1",
            params![id, owner],
        )
        .expect("owner");
        account(&conn, "Qsecond");
        pull(&mut conn, pulled("111"));
        assert_eq!(deck_account(&conn, "111"), Some(owner));
    }

    #[test]
    fn imports_a_deck_unattached_when_no_account_is_known_yet() {
        let (_dir, mut conn) = db("sync-pull-none");
        pull(&mut conn, pulled("111"));
        assert_eq!(deck_account(&conn, "111"), None);
    }

    #[test]
    fn a_pulled_deck_keeps_its_versions_cover_hash_and_carry() {
        let (_dir, mut conn) = db("sync-pull-row");
        pull(&mut conn, pulled("limited:abc"));
        let (cover, format, source): (Option<i64>, String, String) = conn
            .query_row(
                "SELECT cover_catalog_id, format, source FROM decks WHERE mtgo_id = 'limited:abc'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("deck");
        assert_eq!(cover, Some(424_242), "kept with no cards row yet");
        assert_eq!(format, "CMODERN");
        assert_eq!(source, "imported");
        let row = synced_rows(&conn, SyncType::Deck).expect("rows")["limited_abc"].clone();
        assert_eq!(row.hash.as_deref(), Some("hpulled"));
        assert_eq!(row.synced_at, Some(T));
        assert_eq!(row.built_hash, None, "rebuilt at the next run");
        assert_eq!(row.carry.as_deref(), Some("{}"));
    }

    #[test]
    fn a_pulled_deck_leaves_the_local_cover_file_alone() {
        let (_dir, mut conn) = db("sync-pull-cover-file");
        pull(&mut conn, pulled("limited:abc"));
        conn.execute(
            "UPDATE decks SET cover_file = '1-5.png' WHERE mtgo_id = 'limited:abc'",
            [],
        )
        .expect("custom cover");
        pull(&mut conn, pulled("limited:abc"));
        let file: Option<String> = conn
            .query_row(
                "SELECT cover_file FROM decks WHERE mtgo_id = 'limited:abc'",
                [],
                |row| row.get(0),
            )
            .expect("deck");
        assert_eq!(file.as_deref(), Some("1-5.png"));
    }

    // ---- The Account page's deck list -------------------------------------

    #[test]
    fn the_deck_sync_list_shows_constructed_decks_enabled_first() {
        let (_dir, conn) = db("sync-deck-list");
        let quiet = deck(&conn, "111", false);
        let synced = deck(&conn, "222", true);
        let played = deck(&conn, "333", false);
        deck(&conn, "limited:abc", false);
        let archived = deck(&conn, "444", false);
        conn.execute("UPDATE decks SET archived_at = 1 WHERE id = ?1", [archived])
            .expect("archive");
        let version_id = version(&conn, played, "QsigA", T);
        played_with(&conn, "qplayed", version_id);
        let list = deck_sync_list(&conn, None).expect("list");
        let ids: Vec<i64> = list.iter().map(|row| row.id).collect();
        assert_eq!(ids, vec![synced, played, quiet]);
        assert_eq!(list[1].last_played_at, Some(1000));
        assert_eq!(list[0].client_id, "222");
        assert!(list[0].cloud_sync_enabled);
    }

    #[test]
    fn the_deck_sync_list_lists_arena_decks() {
        let (_dir, conn) = db("sync-deck-list-arena");
        let mtgo = deck(&conn, "111", false);
        let arena = deck(&conn, "222", false);
        crate::test_support::move_deck_to_arena(&conn, arena);
        let waiting = deck(&conn, "333", false);
        conn.execute(
            "UPDATE decks SET pulled_platform = 'endstep' WHERE id = ?1",
            [waiting],
        )
        .expect("a pulled Endstep deck waiting for its account");
        let list = deck_sync_list(&conn, None).expect("list");
        let ids: Vec<i64> = list.iter().map(|row| row.id).collect();
        assert_eq!(ids, vec![mtgo, arena, waiting]);
        let platforms: Vec<crate::Platform> = list.iter().map(|row| row.platform).collect();
        assert_eq!(
            platforms,
            vec![
                crate::Platform::Mtgo,
                crate::Platform::Arena,
                crate::Platform::Endstep
            ],
            "each deck's platform: its account's, else the pulled one, else MTGO"
        );
        let arena_account = crate::test_support::arena_account(&conn);
        let list = deck_sync_list(&conn, Some(&[arena_account])).expect("list");
        let ids: Vec<i64> = list.iter().map(|row| row.id).collect();
        assert_eq!(
            ids,
            vec![arena, waiting],
            "the accounts asked for, and a pulled deck waiting for its account"
        );
    }

    // ---- Matches and leagues (Plan Q task 11) -----------------------------

    use crate::import::{ImportedMatch, PulledMatch};
    use crate::import_leagues::{ImportedLeague, LeagueDeck};
    use crate::import_limited::{LegacyDraft, LegacyLimitedDeck};
    use crate::imported_detail::{DetailCard, ImportedDetail, ImportedGameDetail};
    use crate::test_support::{arena_account, imported, insert_manual_league};
    use sync_bundles::match_bundle::match_bundle;
    use sync_bundles::{PulledLeagueFields, PulledPlayerFields};
    use sync_bundles::{bundle_hash, canonical_json};

    /// An enabled deck "111" with a version; a manual match `token` played
    /// with it. Returns `(deck id, version id, match id)`.
    fn enabled_match(conn: &Connection, token: &str) -> (i64, i64, i64) {
        let deck_id = match conn
            .query_row("SELECT id FROM decks WHERE mtgo_id = '111'", [], |row| {
                row.get(0)
            })
            .optional()
            .expect("deck")
        {
            Some(id) => id,
            None => deck(conn, "111", true),
        };
        let version_id = match conn
            .query_row(
                "SELECT id FROM deck_versions WHERE deck_id = ?1",
                [deck_id],
                |row| row.get(0),
            )
            .optional()
            .expect("version")
        {
            Some(id) => id,
            None => version(conn, deck_id, "MTAwMTo0OmZhbHNl", T),
        };
        played_with(conn, token, version_id);
        let match_id = conn
            .query_row("SELECT id FROM matches WHERE token = ?1", [token], |row| {
                row.get(0)
            })
            .expect("match id");
        (deck_id, version_id, match_id)
    }

    fn match_ids(conn: &Connection, tier: Tier) -> Vec<String> {
        match_candidates(conn, tier, AFTER_THE_HOLD)
            .expect("candidates")
            .into_iter()
            .map(|candidate| candidate.client_id)
            .collect()
    }

    fn match_key(conn: &Connection, token: &str) -> String {
        match_candidates(conn, Tier::Supporter, AFTER_THE_HOLD)
            .expect("candidates")
            .into_iter()
            .find(|candidate| candidate.client_id == token)
            .expect("a candidate")
            .touch_key
    }

    fn league_key(conn: &Connection, client_id: &str) -> String {
        league_candidates(conn, Tier::Supporter)
            .expect("candidates")
            .into_iter()
            .find(|candidate| candidate.client_id == client_id)
            .expect("a candidate")
            .touch_key
    }

    fn hash_of(conn: &Connection, match_id: i64) -> String {
        let input = match_bundle_input(conn, match_id, chrono_tz::UTC)
            .expect("input")
            .expect("a match");
        bundle_hash(&canonical_json(&match_bundle(&input, None)))
    }

    // 0.x DirtinessTest "leaves unfinished matches out of sync until they end".
    #[test]
    fn leaves_unfinished_matches_out_of_sync_until_they_end() {
        let (_dir, conn) = db("sync-unfinished-matches");
        enabled_match(&conn, "q-m");
        for (state, offered) in [
            ("pending", false),
            ("in_progress", false),
            ("ended", false),
            ("abandoned", true),
            ("complete", true),
        ] {
            conn.execute("UPDATE matches SET state = ?1", [state])
                .expect("state");
            assert_eq!(!match_ids(&conn, Tier::Free).is_empty(), offered, "{state}");
        }
        conn.execute("UPDATE matches SET detached_at = 1", [])
            .expect("detach");
        assert!(match_ids(&conn, Tier::Free).is_empty(), "a detached match");
    }

    // Edit a match spec section 5: a match waits 5 minutes after it ends,
    // and the pending count agrees with the candidates.
    #[test]
    fn a_match_is_synced_only_5_minutes_after_it_ended() {
        let (_dir, conn) = db("sync-match-hold");
        enabled_match(&conn, "q-m");
        conn.execute("UPDATE matches SET ended_at = ?1", [T])
            .expect("ended");
        let four_minutes = T + 4 * 60_000;
        let five_minutes = T + SEND_HOLD_MS;
        let ids = |now_ms: i64| -> Vec<String> {
            match_candidates(&conn, Tier::Supporter, now_ms)
                .expect("candidates")
                .into_iter()
                .map(|candidate| candidate.client_id)
                .collect()
        };
        assert!(ids(four_minutes).is_empty());
        assert_eq!(
            pending_count(&conn, SyncType::Match, four_minutes).expect("pending"),
            0
        );
        assert_eq!(ids(five_minutes), vec!["q-m".to_string()]);
        assert_eq!(
            pending_count(&conn, SyncType::Match, five_minutes).expect("pending"),
            1
        );
        // A match with no end waits on its last activity.
        conn.execute(
            "UPDATE matches SET ended_at = NULL, last_activity_at = ?1",
            [T + 60_000],
        )
        .expect("no end");
        assert!(ids(five_minutes).is_empty());
        assert_eq!(ids(five_minutes + 60_000), vec!["q-m".to_string()]);
    }

    // 0.x SyncRunnerTest "only offers matches and leagues of enabled decks as
    // dirty or known, decks regardless".
    #[test]
    fn only_offers_matches_and_leagues_of_enabled_decks() {
        let (_dir, conn) = db("sync-enabled-decks");
        let (deck_id, _, _) = enabled_match(&conn, "q-on");
        let off = deck(&conn, "222", false);
        let off_version = version(&conn, off, "QsigOff", T);
        played_with(&conn, "q-off", off_version);
        let on_league = league(&conn, "q-league-on", deck_id);
        league(&conn, "q-league-off", off);
        assert_eq!(
            known_ids(&conn, SyncType::Match, Tier::Free, AFTER_THE_HOLD).expect("known"),
            vec!["q-on".to_string()]
        );
        assert_eq!(
            known_ids(&conn, SyncType::League, Tier::Free, AFTER_THE_HOLD).expect("known"),
            vec![on_league]
        );
        assert_eq!(
            known_ids(&conn, SyncType::Deck, Tier::Free, AFTER_THE_HOLD)
                .expect("known")
                .len(),
            2,
            "decks regardless"
        );
        assert_eq!(
            pending_count(&conn, SyncType::Match, AFTER_THE_HOLD).expect("pending"),
            1
        );
        assert_eq!(
            pending_count(&conn, SyncType::League, AFTER_THE_HOLD).expect("pending"),
            1
        );
    }

    #[test]
    fn deck_sync_sends_arena_decks() {
        let (_dir, conn) = db("sync-arena-decks");
        let mtgo_account = account(&conn, "Qmtgo");
        let arena_account = arena_account(&conn);
        let mine = deck(&conn, "111", true);
        let theirs = deck(&conn, "222", true);
        let unattached = deck(&conn, "333", true);
        conn.execute(
            "UPDATE decks SET account_id = ?2 WHERE id = ?1",
            params![mine, mtgo_account],
        )
        .expect("mtgo deck");
        conn.execute(
            "UPDATE decks SET account_id = ?2 WHERE id = ?1",
            params![theirs, arena_account],
        )
        .expect("arena deck");
        let mut ids: Vec<i64> = deck_candidates(&conn, Tier::Supporter)
            .expect("decks")
            .into_iter()
            .map(|candidate| candidate.deck_id)
            .collect();
        ids.sort();
        assert_eq!(ids, vec![mine, theirs, unattached]);
        let (bundle, sidecar) = deck_bundle_of(&conn, "222");
        assert_eq!(bundle["platform"], Value::from("arena"));
        assert_eq!(sidecar["platform"], Value::from("arena"));
        for mtgo in ["111", "333"] {
            let (bundle, sidecar) = deck_bundle_of(&conn, mtgo);
            assert!(bundle.get("platform").is_none(), "{mtgo}");
            assert!(sidecar.get("platform").is_none(), "{mtgo}");
        }
    }

    #[test]
    fn arena_limited_deck_is_supporter_only() {
        let (_dir, conn) = db("sync-arena-limited-deck");
        let arena = deck(&conn, "limited:arena_q-course", true);
        crate::test_support::move_deck_to_arena(&conn, arena);
        let constructed = deck(&conn, "222", true);
        crate::test_support::move_deck_to_arena(&conn, constructed);
        let ids = |tier: Tier| -> Vec<i64> {
            let mut ids: Vec<i64> = deck_candidates(&conn, tier)
                .expect("decks")
                .into_iter()
                .map(|candidate| candidate.deck_id)
                .collect();
            ids.sort();
            ids
        };
        assert_eq!(ids(Tier::Free), vec![constructed], "like MTGO limited");
        assert_eq!(ids(Tier::Supporter), vec![arena, constructed]);
    }

    /// The deck bundle and sidecar of the deck `mtgo_id`, built as the sync
    /// runner builds it.
    fn deck_bundle_of(conn: &Connection, mtgo_id: &str) -> (Value, Value) {
        let candidate = deck_candidates(conn, Tier::Supporter)
            .expect("decks")
            .into_iter()
            .find(|candidate| candidate.mtgo_id == mtgo_id)
            .expect("a candidate");
        let input = deck_bundle_input(conn, &candidate).expect("input");
        let bundle = sync_bundles::deck_bundle(&input, None);
        let sidecar = sync_bundles::deck_sidecar(&bundle);
        (bundle, sidecar)
    }

    /// The bundle and sidecar of the match `token`.
    fn bundle_of(conn: &Connection, token: &str) -> (Value, Value) {
        let match_id: i64 = conn
            .query_row("SELECT id FROM matches WHERE token = ?1", [token], |row| {
                row.get(0)
            })
            .expect("match id");
        let input = match_bundle_input(conn, match_id, chrono_tz::UTC)
            .expect("input")
            .expect("a match");
        let bundle = sync_bundles::match_bundle::match_bundle(&input, None);
        let sidecar = sync_bundles::match_bundle::match_sidecar(&bundle);
        (bundle, sidecar)
    }

    #[test]
    fn match_sync_sends_arena_with_platform() {
        let (_dir, conn) = db("sync-arena-matches");
        enabled_match(&conn, "q-mtgo");
        enabled_match(&conn, "q-arena");
        let arena = arena_account(&conn);
        conn.execute(
            "UPDATE matches SET account_id = ?1 WHERE token = 'q-arena'",
            [arena],
        )
        .expect("move");
        assert_eq!(
            match_ids(&conn, Tier::Supporter),
            vec!["q-arena".to_string(), "q-mtgo".to_string()]
        );
        let (bundle, sidecar) = bundle_of(&conn, "q-arena");
        assert_eq!(bundle["platform"], Value::from("arena"));
        assert_eq!(sidecar["platform"], Value::from("arena"));
        // An MTGO match still writes no platform.
        let (bundle, sidecar) = bundle_of(&conn, "q-mtgo");
        assert!(bundle.get("platform").is_none());
        assert!(sidecar.get("platform").is_none());
        assert!(bundle.get("timelines").is_some());
    }

    /// An Arena course run `arena:<course>` of `account_id`, as the run
    /// sync stores it, on the deck `deck_id`, observed at `T`.
    fn arena_run(conn: &Connection, account_id: i64, run_key: &str, deck_id: i64) {
        conn.execute(
            "INSERT INTO leagues (run_key, account_id, source, kind, state, token, name, format,
                 deck_id, started_at, created_at, sync_client_id, course_wins, course_losses,
                 course_observed_at)
             VALUES (?1, ?2, 'log', 'draft', 'active', ?1, 'Premier Draft \u{b7} FRA',
                 'DFRAFRAFRA', ?3, ?4, ?4, ?5, 2, 1, ?4)",
            params![
                run_key,
                account_id,
                deck_id,
                T,
                leagues::arena_course_id(run_key).map(|course| format!("arena_{course}"))
            ],
        )
        .expect("arena run");
    }

    fn league_ids(conn: &Connection, tier: Tier) -> Vec<String> {
        league_candidates(conn, tier)
            .expect("leagues")
            .into_iter()
            .map(|candidate| candidate.client_id)
            .collect()
    }

    #[test]
    fn league_sync_sends_arena_course_runs() {
        let (_dir, conn) = db("sync-arena-leagues");
        let mtgo_account = account(&conn, "Qmtgo");
        let arena = arena_account(&conn);
        let deck_id = deck(&conn, "111", true);
        let mine = league(&conn, "q-league-mtgo", deck_id);
        conn.execute(
            "UPDATE leagues SET account_id = ?1 WHERE token = 'q-league-mtgo'",
            [mtgo_account],
        )
        .expect("mtgo league");
        // A league of the Arena account that is no course run is never sent.
        league(&conn, "q-league-arena", deck_id);
        conn.execute(
            "UPDATE leagues SET account_id = ?1 WHERE token = 'q-league-arena'",
            [arena],
        )
        .expect("arena league");
        let course_deck = deck(&conn, "limited:arena_q-course", true);
        crate::test_support::move_deck_to_arena(&conn, course_deck);
        // The event name of a course known only from another device.
        conn.execute(
            "INSERT INTO synced_arena_runs (account_id, course_id, event_name, kind, wins,
                 losses, state, observed_at)
             VALUES (?1, 'q-course', 'PremierDraft_FRA_20260929', 'draft', 1, 1, 'active', 5)",
            [arena],
        )
        .expect("synced run");
        arena_run(&conn, arena, "arena:q-course", course_deck);

        assert_eq!(
            league_ids(&conn, Tier::Supporter),
            vec!["arena_q-course".to_string(), mine.clone()]
        );
        assert_eq!(league_ids(&conn, Tier::Free), vec![mine], "supporter only");

        let candidate = league_candidates(&conn, Tier::Supporter)
            .expect("leagues")
            .into_iter()
            .find(|candidate| candidate.client_id == "arena_q-course")
            .expect("the run");
        assert!(candidate.owned);
        let input = arena_run_bundle_input(&conn, candidate.row_id)
            .expect("input")
            .expect("an Arena run");
        assert_eq!(input.course_id, "q-course");
        assert_eq!(
            input.snapshot,
            sync_bundles::league::RunSnapshot {
                wins: 2,
                losses: 1,
                state: "active".to_string(),
                observed_at_ms: T,
            }
        );
        let bundle = sync_bundles::league::arena_run_bundle(&input, None);
        let sidecar = sync_bundles::league::league_sidecar(&bundle);
        assert_eq!(bundle["platform"], Value::from("arena"));
        assert_eq!(sidecar["platform"], Value::from("arena"));
        assert_eq!(
            sidecar["deck_client_id"],
            Value::from("limited_arena_q-course")
        );
        // An MTGO league is no Arena run, and its bundle has no platform.
        let mtgo_id: i64 = conn
            .query_row(
                "SELECT id FROM leagues WHERE token = 'q-league-mtgo'",
                [],
                |row| row.get(0),
            )
            .expect("id");
        assert!(
            arena_run_bundle_input(&conn, mtgo_id)
                .expect("input")
                .is_none()
        );
        let mtgo = league_bundle_input(&conn, mtgo_id)
            .expect("input")
            .expect("league");
        assert!(
            sync_bundles::league::league_bundle(&mtgo, None)
                .get("platform")
                .is_none()
        );
    }

    #[test]
    fn orphan_runs_are_never_candidates() {
        let (_dir, conn) = db("sync-arena-orphans");
        let arena = arena_account(&conn);
        let course_deck = deck(&conn, "limited:arena_q-course", true);
        crate::test_support::move_deck_to_arena(&conn, course_deck);
        // An orphan on an enabled deck, with the id an old trigger gave it,
        // and one with none.
        arena_run(
            &conn,
            arena,
            "arena:PremierDraft_FRA_20260929:q-m1",
            course_deck,
        );
        conn.execute(
            "UPDATE leagues SET sync_client_id = 'arena:PremierDraft_FRA_20260929:q-m1_1'
             WHERE run_key = 'arena:PremierDraft_FRA_20260929:q-m1'",
            [],
        )
        .expect("old id");
        arena_run(
            &conn,
            arena,
            "arena:QuickDraft_FDN_20261101:q-m2",
            course_deck,
        );
        assert!(league_ids(&conn, Tier::Supporter).is_empty());
        arena_run(&conn, arena, "arena:q-course", course_deck);
        assert_eq!(
            league_ids(&conn, Tier::Supporter),
            vec!["arena_q-course".to_string()]
        );
    }

    #[test]
    fn deck_sync_sends_endstep() {
        let (_dir, mut conn) = db("sync-endstep-decks");
        let mtgo_account = account(&conn, "Qmtgo");
        let mine = deck(&conn, "111", true);
        conn.execute(
            "UPDATE decks SET account_id = ?2 WHERE id = ?1",
            params![mine, mtgo_account],
        )
        .expect("mtgo deck");
        let seed = crate::test_support::seed_endstep(&mut conn, "q-endstep");
        let mut ids: Vec<i64> = deck_candidates(&conn, Tier::Supporter)
            .expect("decks")
            .into_iter()
            .map(|candidate| candidate.deck_id)
            .collect();
        ids.sort();
        assert_eq!(ids, vec![mine, seed.deck_id]);
        let listed: Vec<i64> = deck_sync_list(&conn, None)
            .expect("list")
            .into_iter()
            .map(|row| row.id)
            .collect();
        assert!(listed.contains(&seed.deck_id));
        let listed: Vec<i64> = deck_sync_list(&conn, Some(&[seed.account_id]))
            .expect("list")
            .into_iter()
            .map(|row| row.id)
            .collect();
        assert_eq!(listed, vec![seed.deck_id]);
        let (bundle, sidecar) = deck_bundle_of(&conn, "endstep:d1");
        assert_eq!(bundle["platform"], Value::from("endstep"));
        assert_eq!(sidecar["platform"], Value::from("endstep"));
        let (bundle, _) = deck_bundle_of(&conn, "111");
        assert!(bundle.get("platform").is_none());
    }

    // ---- Endstep decks across devices (cross-device sync spec 5.3) -------

    fn endstep_ref(name: &str) -> EndstepCardRef {
        EndstepCardRef {
            name: name.to_string(),
            set: "qqq".to_string(),
            number: "7".to_string(),
        }
    }

    /// This device's catalog id of an Endstep card, made when new.
    fn endstep_id(conn: &mut Connection, name: &str) -> u32 {
        let tx = conn.transaction().expect("tx");
        let id = crate::endstep_cards::catalog_for_ref(&tx, &endstep_ref(name), 1).expect("card");
        tx.commit().expect("commit");
        id
    }

    /// An Endstep account (the current one) and its enabled deck
    /// `endstep:qd` with two versions of Endstep cards and a cover.
    fn endstep_deck(conn: &mut Connection) {
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at, platform)
             VALUES ('endstep:qu1', 0, 'endstep')",
            [],
        )
        .expect("account");
        let alpha = endstep_id(conn, "Qalpha");
        let beta = endstep_id(conn, "Qbeta");
        let side = endstep_id(conn, "Qside");
        let id = deck(conn, "endstep:qd", true);
        conn.execute(
            "UPDATE decks SET account_id = (SELECT id FROM accounts WHERE platform = 'endstep'),
                 cover_catalog_id = ?2 WHERE id = ?1",
            params![id, beta],
        )
        .expect("endstep deck");
        let card = |catalog_id: u32, quantity: u32, sideboard: bool| decks::DeckCard {
            catalog_id,
            quantity,
            sideboard,
        };
        version(
            conn,
            id,
            &decks::signature(&[card(alpha, 4, false), card(side, 2, true)]),
            T,
        );
        version(
            conn,
            id,
            &decks::signature(&[
                card(alpha, 3, false),
                card(beta, 1, false),
                card(side, 2, true),
            ]),
            T + 1_000,
        );
    }

    /// Every version of the deck `mtgo_id` with its cards named by ref, and
    /// its cover's ref.
    fn endstep_deck_cards(conn: &Connection, mtgo_id: &str) -> (Vec<Vec<String>>, Option<String>) {
        let named = |catalog_id: u32| -> String {
            let refs = crate::endstep_cards::refs_of(conn, &[catalog_id]).expect("refs");
            let card = refs.get(&catalog_id).expect("a ref");
            format!("{}|{}|{}", card.name, card.set, card.number)
        };
        let deck_id: i64 = conn
            .query_row(
                "SELECT id FROM decks WHERE mtgo_id = ?1",
                [mtgo_id],
                |row| row.get(0),
            )
            .expect("deck");
        let mut versions = Vec::new();
        for (signature, _) in deck_versions(conn, deck_id).expect("versions") {
            let mut rows: Vec<String> = decks::decode_signature(&signature)
                .into_iter()
                .map(|row| {
                    format!(
                        "{}x{}:{}",
                        named(row.catalog_id),
                        row.quantity,
                        row.sideboard
                    )
                })
                .collect();
            rows.sort();
            versions.push(rows);
        }
        versions.sort();
        let cover: Option<i64> = conn
            .query_row(
                "SELECT cover_catalog_id FROM decks WHERE id = ?1",
                [deck_id],
                |row| row.get(0),
            )
            .expect("cover");
        let cover = cover.map(|id| named(u32::try_from(id).expect("an id")));
        (versions, cover)
    }

    /// A deck bundle read back and pulled as the sync runner does.
    fn pull_deck_bundle(conn: &mut Connection, bundle: &Value) {
        let (input, keys, _) = sync_bundles::read_deck_bundle(bundle).expect("reads");
        let parts = PulledDeckParts {
            keys,
            platform: input.platform,
            endstep_cover: input.endstep_cover.clone(),
            archetype_uuid: input.archetype_uuid.clone(),
            guides: input.guides.clone(),
            notes: input.notes.clone(),
        };
        let deck = ImportedDeck {
            mtgo_id: input.mtgo_id.clone(),
            account: None,
            name: input.name.clone(),
            original_name: input.original_name.clone(),
            format: input.format.clone().unwrap_or_default(),
            color_identity: input.color_identity.clone(),
            cover_catalog_id: input.cover_catalog_id.map(i64::from),
            archived_at_ms: None,
            created_at_ms: None,
            updated_at_ms: None,
            versions: input
                .versions
                .iter()
                .map(|(signature, modified_at_ms)| ImportedVersion {
                    signature: signature.clone(),
                    modified_at_ms: *modified_at_ms,
                })
                .collect(),
        };
        pull_with(conn, deck, parts, T);
    }

    /// No number at or above `ENDSTEP_ID_OFFSET` anywhere in the bundle.
    fn assert_no_local_endstep_id(bundle: &Value) {
        let text = canonical_json(bundle);
        let mut digits = String::new();
        for ch in text.chars().chain(std::iter::once(' ')) {
            if ch.is_ascii_digit() {
                digits.push(ch);
                continue;
            }
            if !digits.is_empty() {
                let number: u64 = digits.parse().unwrap_or(u64::MAX);
                assert!(
                    number < u64::from(cards::ENDSTEP_ID_OFFSET),
                    "a local Endstep id in {text}"
                );
                digits.clear();
            }
        }
    }

    #[test]
    fn endstep_deck_bundle_names_cards_portably() {
        let (_dir, mut conn) = db("sync-endstep-deck-portable");
        endstep_deck(&mut conn);
        let (bundle, _) = deck_bundle_of(&conn, "endstep:qd");
        assert_no_local_endstep_id(&bundle);
        assert_eq!(
            bundle["deck"]["cover_mtgo_id"],
            endstep_ref("Qbeta").to_value()
        );
        for version in bundle["versions"].as_array().expect("versions") {
            let signature = version["signature"].as_str().expect("signature");
            assert!(
                sync_bundles::endstep_cards::read_endstep_signature(signature).is_some(),
                "{signature}"
            );
        }
    }

    #[test]
    fn pulled_endstep_deck_maps_cards_locally() {
        let (_a, mut here) = db("sync-endstep-deck-here");
        endstep_deck(&mut here);
        let (bundle, _) = deck_bundle_of(&here, "endstep:qd");
        let (_b, mut there) = db("sync-endstep-deck-there");
        there
            .execute(
                "INSERT INTO accounts (username, first_seen_at, platform)
                 VALUES ('endstep:qu1', 0, 'endstep')",
                [],
            )
            .expect("account");
        // Unrelated cards first, so this device's ids differ from A's.
        endstep_id(&mut there, "Qother");
        endstep_id(&mut there, "Qside");
        pull_deck_bundle(&mut there, &bundle);
        let (versions, cover) = endstep_deck_cards(&there, "endstep:qd");
        assert_eq!(versions.len(), 2);
        assert_eq!((versions, cover), endstep_deck_cards(&here, "endstep:qd"));
        let local: Vec<u32> = conn_ids(&there, "endstep:qd");
        assert!(local.iter().all(|id| *id >= cards::ENDSTEP_ID_OFFSET));
        assert_ne!(local, conn_ids(&here, "endstep:qd"), "ids differ by device");
        // The pulled deck builds the same bytes again.
        assert_eq!(
            canonical_json(&deck_bundle_of(&there, "endstep:qd").0),
            canonical_json(&bundle)
        );
    }

    /// Every catalog id the deck `mtgo_id`'s versions name, sorted.
    fn conn_ids(conn: &Connection, mtgo_id: &str) -> Vec<u32> {
        let deck_id: i64 = conn
            .query_row(
                "SELECT id FROM decks WHERE mtgo_id = ?1",
                [mtgo_id],
                |row| row.get(0),
            )
            .expect("deck");
        let mut ids: Vec<u32> = deck_versions(conn, deck_id)
            .expect("versions")
            .iter()
            .flat_map(|(signature, _)| decks::decode_signature(signature))
            .map(|row| row.catalog_id)
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    #[test]
    fn pulled_arena_deck_lands_under_arena_account() {
        let (_dir, mut conn) = db("sync-pull-arena-deck");
        let mtgo = account(&conn, "Qmtgo");
        let arena = arena_account(&conn);
        let mut deck = pulled("q-arena-deck");
        deck.cover_catalog_id = Some(100_000_042);
        let parts = PulledDeckParts {
            platform: sync_bundles::BundlePlatform::Arena,
            ..PulledDeckParts::default()
        };
        pull_with(&mut conn, deck, parts, T);
        assert_eq!(deck_account(&conn, "q-arena-deck"), Some(arena));
        // An MTGO deck still takes the MTGO account.
        pull(&mut conn, pulled("111"));
        assert_eq!(deck_account(&conn, "111"), Some(mtgo));
    }

    /// Pull the Arena deck `q-arena-deck` onto a device with no Arena
    /// account.
    fn pull_unfiled_arena_deck(conn: &mut Connection) {
        let parts = PulledDeckParts {
            platform: sync_bundles::BundlePlatform::Arena,
            ..PulledDeckParts::default()
        };
        pull_with(conn, pulled("q-arena-deck"), parts, T);
    }

    /// Pull a manual Arena match played with `q-arena-deck`; it makes the
    /// Arena account. Returns that account's id.
    fn pull_arena_match_with_the_deck(conn: &mut Connection) -> i64 {
        let mut played = pulled_match("q-arena-match", "complete");
        played.platform = sync_bundles::BundlePlatform::Arena;
        played.imported.mtgo_match_id = None;
        played.imported.local_username = "Qarena#12345".to_string();
        played.imported.manual = Some(Vec::new());
        played.local_player_fields.arena_user_id = Some("Qarena-id".to_string());
        played.deck = Some(("q-arena-deck".to_string(), "QsigA".to_string()));
        pull_match(conn, played);
        conn.query_row(
            "SELECT id FROM accounts WHERE platform = 'arena'",
            [],
            |row| row.get(0),
        )
        .expect("the Arena account")
    }

    #[test]
    fn a_pulled_arena_deck_with_no_arena_account_is_filed_by_its_match() {
        let (_dir, mut conn) = db("sync-pull-arena-deck-later");
        account(&conn, "Qmtgo");
        pull_unfiled_arena_deck(&mut conn);
        assert_eq!(
            deck_account(&conn, "q-arena-deck"),
            None,
            "no Arena account here yet, and never the MTGO one"
        );
        // Unfiled, it still syncs as an Arena deck.
        let (bundle, _) = deck_bundle_of(&conn, "q-arena-deck");
        assert_eq!(bundle["platform"], Value::from("arena"));
        let arena = pull_arena_match_with_the_deck(&mut conn);
        assert_eq!(deck_account(&conn, "q-arena-deck"), Some(arena));
    }

    #[test]
    fn an_mtgo_login_never_adopts_a_pulled_arena_deck() {
        let (_dir, mut conn) = db("sync-pull-arena-deck-orphans");
        pull_unfiled_arena_deck(&mut conn);
        // An MTGO deck file with no account yet, as before a first login.
        let mtgo = deck(&conn, "111", false);
        let first_login = |conn: &mut Connection, writes: Vec<crate::decks::DeckWrite>| {
            crate::write::commit(
                conn,
                &crate::write::WriteBatch {
                    decks: writes,
                    now_ms: T,
                    ..crate::write::WriteBatch::default()
                },
            )
            .expect("first login");
        };
        first_login(
            &mut conn,
            vec![
                crate::decks::DeckWrite::AdoptOrphans {
                    account: "Qmtgo".to_string(),
                    at_ms: T,
                },
                crate::decks::DeckWrite::Adopt {
                    mtgo_id: "q-arena-deck".to_string(),
                    account: "Qmtgo".to_string(),
                    at_ms: T,
                },
            ],
        );
        let qmtgo: i64 = conn
            .query_row(
                "SELECT id FROM accounts WHERE username = 'Qmtgo'",
                [],
                |row| row.get(0),
            )
            .expect("the MTGO account");
        assert_eq!(
            conn.query_row(
                "SELECT account_id FROM decks WHERE id = ?1",
                [mtgo],
                |row| row.get::<_, Option<i64>>(0)
            )
            .expect("deck"),
            Some(qmtgo),
            "an MTGO orphan is still adopted"
        );
        assert_eq!(deck_account(&conn, "q-arena-deck"), None);
        // An MTGO pull of the same key does not file it under MTGO either.
        pull(&mut conn, pulled("q-arena-deck"));
        assert_eq!(deck_account(&conn, "q-arena-deck"), None);
        let arena = pull_arena_match_with_the_deck(&mut conn);
        assert_eq!(deck_account(&conn, "q-arena-deck"), Some(arena));
    }

    #[test]
    fn an_endstep_deck_sends_one_version_per_portable_signature() {
        let (_dir, mut conn) = db("sync-endstep-deck-duplicate-versions");
        endstep_deck(&mut conn);
        let deck_id: i64 = conn
            .query_row(
                "SELECT id FROM decks WHERE mtgo_id = 'endstep:qd'",
                [],
                |row| row.get(0),
            )
            .expect("deck");
        let alpha = endstep_id(&mut conn, "Qalpha");
        // A card with no `endstep_cards` row is left out of the portable
        // signature, so these two versions read the same on every device.
        let card = |catalog_id: u32| decks::DeckCard {
            catalog_id,
            quantity: 4,
            sideboard: false,
        };
        version(&conn, deck_id, &decks::signature(&[card(alpha)]), T + 5_000);
        let unknown = cards::ENDSTEP_ID_OFFSET + 999_999;
        version(
            &conn,
            deck_id,
            &decks::signature(&[card(alpha), card(unknown)]),
            T + 9_000,
        );
        let (bundle, _) = deck_bundle_of(&conn, "endstep:qd");
        let versions = bundle["versions"].as_array().expect("versions");
        let signatures: Vec<&str> = versions
            .iter()
            .map(|version| version["signature"].as_str().expect("signature"))
            .collect();
        let mut unique = signatures.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(signatures.len(), unique.len(), "{signatures:?}");
        assert_eq!(versions.len(), 3);
        let only_alpha = sync_bundles::endstep_cards::endstep_signature(&[
            sync_bundles::endstep_cards::EndstepDeckCard {
                card: endstep_ref("Qalpha"),
                quantity: 4,
                sideboard: false,
            },
        ]);
        let kept = versions
            .iter()
            .find(|version| version["signature"].as_str() == Some(only_alpha.as_str()))
            .expect("the shared signature");
        assert_eq!(
            kept["modified_at"],
            Value::from(sync_bundles::bundle_datetime(T + 9_000)),
            "the newest time is kept"
        );
    }

    #[test]
    fn an_endstep_deck_guide_travels_without_local_ids() {
        let (_a, mut here) = db("sync-endstep-deck-guide-here");
        endstep_deck(&mut here);
        let foe = held_archetype(&here, "q-foe", "downloaded");
        here.execute_batch(&format!(
            "INSERT INTO sideboard_guides (id, deck_id, archetype_id, created_at, updated_at)
             VALUES (90, (SELECT id FROM decks WHERE mtgo_id = 'endstep:qd'), {foe}, 1, 1);
             INSERT INTO sideboard_guide_cards (guide_id, oracle_id, direction, quantity)
             VALUES (90, 'q-oracle-in', 'in', 2), (90, 'q-oracle-out', 'out', 1);"
        ))
        .expect("guide");
        let (bundle, _) = deck_bundle_of(&here, "endstep:qd");
        assert_no_local_endstep_id(&bundle);
        assert_eq!(bundle["guides"].as_array().map(Vec::len), Some(1));
        let (_b, mut there) = db("sync-endstep-deck-guide-there");
        there
            .execute(
                "INSERT INTO accounts (username, first_seen_at, platform)
                 VALUES ('endstep:qu1', 0, 'endstep')",
                [],
            )
            .expect("account");
        endstep_id(&mut there, "Qother");
        pull_deck_bundle(&mut there, &bundle);
        assert_eq!(
            guide_rows(&there, "endstep:qd").0,
            guide_rows(&here, "endstep:qd").0
        );
        assert_eq!(
            canonical_json(&deck_bundle_of(&there, "endstep:qd").0),
            canonical_json(&bundle)
        );
    }

    #[test]
    fn match_sync_sends_endstep_with_platform() {
        let (_dir, mut conn) = db("sync-endstep-matches");
        enabled_match(&conn, "q-mtgo");
        crate::test_support::seed_endstep(&mut conn, "q-endstep");
        assert_eq!(
            match_ids(&conn, Tier::Supporter),
            vec!["q-endstep".to_string(), "q-mtgo".to_string()]
        );
        let (bundle, sidecar) = bundle_of(&conn, "q-endstep");
        assert_eq!(bundle["platform"], Value::from("endstep"));
        assert_eq!(sidecar["platform"], Value::from("endstep"));
        assert!(bundle.get("timelines").is_none());
        let (bundle, _) = bundle_of(&conn, "q-mtgo");
        assert!(bundle.get("platform").is_none());
    }

    #[test]
    fn an_endstep_ranked_result_changes_the_match_touch_key() {
        let (_dir, mut conn) = db("sync-endstep-ranked-key");
        crate::test_support::seed_endstep(&mut conn, "q-endstep");
        let before = match_key(&conn, "q-endstep");
        let mut ranked = crate::endstep_sync::EndstepRanked {
            match_token: "q-endstep".to_string(),
            status: Some("done".to_string()),
            format_id: None,
            rating_before: Some(1500),
            rating_after: Some(1488),
            score_before: None,
            score_after: None,
            delta: Some(-12),
            rd_before: None,
            rd_after: None,
            rank: None,
        };
        let write = |conn: &mut Connection, ranked: &crate::endstep_sync::EndstepRanked| {
            crate::write::commit(
                conn,
                &crate::write::WriteBatch {
                    endstep_ranked: vec![ranked.clone()],
                    ..crate::write::WriteBatch::default()
                },
            )
            .expect("ranked");
        };
        write(&mut conn, &ranked);
        let with_ranked = match_key(&conn, "q-endstep");
        assert_ne!(with_ranked, before, "the bundle carries the ranked result");
        assert!(
            with_ranked.starts_with(&before),
            "a key without one is unchanged"
        );
        ranked.rating_after = Some(1490);
        write(&mut conn, &ranked);
        assert_ne!(match_key(&conn, "q-endstep"), with_ranked);
    }

    #[test]
    fn league_sync_never_sends_endstep() {
        let (_dir, mut conn) = db("sync-endstep-leagues");
        let mtgo_account = account(&conn, "Qmtgo");
        let mtgo_deck = deck(&conn, "111", true);
        let mine = league(&conn, "q-league-mtgo", mtgo_deck);
        conn.execute(
            "UPDATE leagues SET account_id = ?1 WHERE token = 'q-league-mtgo'",
            [mtgo_account],
        )
        .expect("mtgo league");
        let seed = crate::test_support::seed_endstep(&mut conn, "q-endstep");
        // A league of the Endstep account on the Endstep deck, and one of
        // the Endstep account on an MTGO deck, and one of an MTGO account on
        // the Endstep deck.
        league(&conn, "q-league-endstep", seed.deck_id);
        conn.execute(
            "UPDATE leagues SET account_id = ?1 WHERE token = 'q-league-endstep'",
            [seed.account_id],
        )
        .expect("endstep league");
        league(&conn, "q-league-account", mtgo_deck);
        conn.execute(
            "UPDATE leagues SET account_id = ?1 WHERE token = 'q-league-account'",
            [seed.account_id],
        )
        .expect("endstep account league");
        league(&conn, "q-league-deck", seed.deck_id);
        conn.execute(
            "UPDATE leagues SET account_id = ?1 WHERE token = 'q-league-deck'",
            [mtgo_account],
        )
        .expect("endstep deck league");
        let ids: Vec<String> = league_candidates(&conn, Tier::Supporter)
            .expect("leagues")
            .into_iter()
            .map(|candidate| candidate.client_id)
            .collect();
        assert_eq!(ids, vec![mine]);
    }

    // 0.x LimitedTierGateTest "offers no limited matches or leagues to a free
    // account whose limited deck slot survived a tier lapse".
    #[test]
    fn offers_no_limited_matches_or_leagues_to_a_free_account() {
        let (_dir, conn) = db("sync-limited-tier");
        let limited = deck(&conn, "limited:abc", true);
        let limited_version = version(&conn, limited, "QsigL", T);
        played_with(&conn, "q-limited", limited_version);
        let client_id = league(&conn, "q-limited-league", limited);
        assert!(match_ids(&conn, Tier::Free).is_empty());
        assert!(
            league_candidates(&conn, Tier::Free)
                .expect("leagues")
                .is_empty()
        );
        assert_eq!(
            match_ids(&conn, Tier::Supporter),
            vec!["q-limited".to_string()]
        );
        let supporter: Vec<String> = league_candidates(&conn, Tier::Supporter)
            .expect("leagues")
            .into_iter()
            .map(|candidate| candidate.client_id)
            .collect();
        assert_eq!(supporter, vec![client_id]);
    }

    // 0.x DirtinessTest "marks a match dirty when a hand-entered opening hand
    // lands on a game player pivot".
    #[test]
    fn marks_a_match_dirty_when_a_hand_entered_opening_hand_lands() {
        let (_dir, conn) = db("sync-match-hand");
        enabled_match(&conn, "q-m");
        let before = match_key(&conn, "q-m");
        conn.execute_batch(
            "INSERT INTO game_cards (game_id, side, catalog_id, opening_hand, starting_hand)
             SELECT id, 'local', 1001, 1, 1 FROM games WHERE game_number = 1;
             UPDATE games SET local_starting_hand_size = 1 WHERE game_number = 1;",
        )
        .expect("a hand");
        let with_hand = match_key(&conn, "q-m");
        assert_ne!(before, with_hand);
        conn.execute(
            "INSERT INTO game_mulligan_cards (game_id, hand, catalog_id, quantity)
             SELECT id, 1, 1001, 7 FROM games WHERE game_number = 1",
            [],
        )
        .expect("a mulligan");
        assert_ne!(with_hand, match_key(&conn, "q-m"));
    }

    // #264 (Q11 M1): the game part of the key is a hash of the rows, not a
    // weighted sum, so edits that keep the sum still move it.
    #[test]
    fn the_match_touch_key_moves_on_game_row_edits_that_keep_a_weighted_sum() {
        let (_dir, conn) = db("sync-match-touch-hash");
        enabled_match(&conn, "q-m");
        conn.execute_batch(
            "INSERT INTO game_cards (game_id, side, catalog_id, main_qty)
             SELECT id, 'local', 1000, 1 FROM games WHERE game_number = 1;
             INSERT INTO game_cards (game_id, side, catalog_id, main_qty)
             SELECT id, 'local', 1001, 2 FROM games WHERE game_number = 1;",
        )
        .expect("cards");
        let mut seen = vec![match_key(&conn, "q-m")];
        let mut moved = |sql: &str| {
            conn.execute_batch(sql).expect(sql);
            let key = match_key(&conn, "q-m");
            assert!(!seen.contains(&key), "{sql}");
            seen.push(key);
        };
        // catalog_id + main_qty * 3 stays 1003 + 1007.
        moved("UPDATE game_cards SET catalog_id = 1003, main_qty = 0 WHERE catalog_id = 1000");
        // Two cards trade their counts: the sum over the rows stays.
        moved(
            "UPDATE game_cards SET main_qty = CASE catalog_id WHEN 1003 THEN 2 ELSE 0 END,
                 catalog_id = CASE catalog_id WHEN 1003 THEN 1004 ELSE 1000 END",
        );
        // A column the sum never weighed.
        moved("UPDATE game_cards SET discarded = 1 WHERE catalog_id = 1000");
        moved("UPDATE games SET end_reason = 'concede' WHERE game_number = 1");
        // The games trade their turn counts.
        moved("UPDATE games SET turn_count = game_number");
        moved("UPDATE games SET turn_count = 3 - game_number");
    }

    #[test]
    fn the_match_touch_key_follows_notes_hidden_deck_and_league() {
        let (_dir, conn) = db("sync-match-touch");
        let (deck_id, _, _) = enabled_match(&conn, "q-m");
        let mut seen = vec![match_key(&conn, "q-m")];
        let mut moved = |sql: &str| {
            conn.execute_batch(sql).expect(sql);
            let key = match_key(&conn, "q-m");
            assert!(!seen.contains(&key), "{sql}");
            seen.push(key);
        };
        moved("UPDATE matches SET notes = 'Qnote'");
        moved("UPDATE matches SET hidden_at = 5");
        moved("UPDATE matches SET revision = revision + 1");
        let other = version(&conn, deck_id, "QsigB", T + 1);
        moved(&format!("UPDATE matches SET deck_version_id = {other}"));
        league(&conn, "q-l", deck_id);
        moved("UPDATE matches SET league_id = (SELECT id FROM leagues), league_linked = 1");
        // A merge that swaps the league's sync client id (`take_synced_id`)
        // changes what the bundle says, so the key moves too.
        moved("UPDATE leagues SET sync_client_id = 'q-l_20270102T000000Z'");
    }

    // The sync client id joins the key only when it is not the 0.x formula
    // of the token and start already in it, so a league that never swapped
    // or rekeyed keeps the key it had (no rebuild on upgrade, and a pulled
    // key stored before still matches).
    #[test]
    fn a_formula_sync_client_id_leaves_the_match_touch_key_as_it_was() {
        let (_dir, conn) = db("sync-match-touch-client-id");
        let (deck_id, _, _) = enabled_match(&conn, "q-m");
        let client_id = league(&conn, "q-l", deck_id);
        conn.execute_batch(
            "UPDATE matches SET league_id = (SELECT id FROM leagues), league_linked = 1",
        )
        .expect("member");
        let stored: String = conn
            .query_row("SELECT sync_client_id FROM leagues", [], |row| row.get(0))
            .expect("stored id");
        assert_eq!(stored, client_id, "the formula");
        let key = match_key(&conn, "q-m");
        assert_eq!(
            key.matches(':').count(),
            8,
            "the key a never edited match had before the id: {key}"
        );
        conn.execute_batch("UPDATE leagues SET sync_client_id = 'q-l_20270102T000000Z'")
            .expect("swapped");
        let swapped = match_key(&conn, "q-m");
        assert_ne!(swapped, key);
        assert!(swapped.starts_with(&format!("{key}:")), "{swapped}");
    }

    // 0.x BundleBuilderTest "does not churn the hash when excluded pipeline
    // columns move".
    #[test]
    fn does_not_churn_the_hash_when_excluded_pipeline_columns_move() {
        let (_dir, conn) = db("sync-match-churn");
        let (_, _, match_id) = enabled_match(&conn, "q-m");
        let before = hash_of(&conn, match_id);
        conn.execute_batch(
            "UPDATE matches SET needs_attention = 1, attention_reason = 'Q', last_activity_at = 9,
                 registered_deck_checked_at = 9, tournament_token = 'q-t', derived_version = 7",
        )
        .expect("pipeline columns");
        assert_eq!(hash_of(&conn, match_id), before);
        // A deleted match travels (league self-management spec 6.1).
        conn.execute_batch("UPDATE matches SET hidden_at = 3")
            .expect("deleted");
        assert_ne!(hash_of(&conn, match_id), before);
    }

    #[test]
    fn a_match_bundle_input_reads_the_stored_rows() {
        let (_dir, conn) = db("sync-match-input");
        let (deck_id, _, match_id) = enabled_match(&conn, "q-m");
        let client_id = league(&conn, "q-l", deck_id);
        conn.execute_batch(
            "UPDATE matches SET league_id = (SELECT id FROM leagues), league_linked = 1,
                 notes = 'Qnote', tournament_event_id = 77, tournament_round = 2;
             INSERT INTO cards (catalog_id, oracle_id) VALUES (1001, 'q-oracle');
             INSERT INTO game_cards (game_id, side, catalog_id, main_qty, seen, `cast`)
             SELECT id, 'local', 1001, 4, 2, 1 FROM games WHERE game_number = 1;
             INSERT INTO game_cards (game_id, side, catalog_id, seen)
             SELECT id, 'opponent', 2001, 1 FROM games WHERE game_number = 1;
             INSERT INTO game_card_casts (game_id, side, catalog_id, method, count)
             SELECT id, 'local', 1001, 'kicked', 1 FROM games WHERE game_number = 1;",
        )
        .expect("rows");
        let input = match_bundle_input(&conn, match_id, chrono_tz::UTC)
            .expect("input")
            .expect("a match");
        assert_eq!(input.token, "q-m");
        assert_eq!(input.source, "manual");
        assert_eq!(input.deck_mtgo_id.as_deref(), Some("111"));
        assert_eq!(
            input.deck_version_signature.as_deref(),
            Some("MTAwMTo0OmZhbHNl")
        );
        assert_eq!(input.registered.len(), 1, "the version's list");
        assert_eq!(input.league_client_id, Some(client_id));
        assert_eq!(input.local_username, "Qlocal");
        assert_eq!(input.opponent_username.as_deref(), Some("Qopponent"));
        assert_eq!(input.tournament_event_id, Some(77));
        assert_eq!(input.games.len(), 2);
        let first = &input.games[0];
        assert_eq!(first.cards.len(), 2);
        assert_eq!(first.cards[0].oracle_id.as_deref(), Some("q-oracle"));
        assert_eq!(first.cards[0].main_qty, 4);
        assert!(first.cards[1].opponent);
        assert_eq!(first.cards[1].oracle_id, None);
        assert_eq!(first.casts.len(), 1);
        assert!(
            match_bundle_input(&conn, 999, chrono_tz::UTC)
                .expect("input")
                .is_none()
        );
    }

    #[test]
    fn a_log_match_takes_its_frames_from_its_events() {
        let (_dir, conn) = db("sync-match-frames");
        let header = "12:26:09 [INF] (Twitch Info|Game Play Status Update for Game ID: 945500844, Match ID: 285746550, Event ID: 285746550) ";
        assert_eq!(
            game_detail::frame_value(&format!(
                "{header}{{\"Players\":[],\"Cards\":[]}}\n  trailing {{"
            )),
            Some(serde_json::json!({"Players": [], "Cards": []}))
        );
        assert_eq!(game_detail::frame_value("no frame here"), None);
        // An imported game's frames come from game_timelines, each dated
        // from its own time on the game's end day (replays spec section 10,
        // 0.x `frameInstant`); a time that does not read takes the game's end.
        let (_, _, match_id) = enabled_match(&conn, "q-m");
        conn.execute("UPDATE matches SET source = 'imported'", [])
            .expect("imported");
        let game_id: i64 = conn
            .query_row("SELECT id FROM games WHERE game_number = 1", [], |row| {
                row.get(0)
            })
            .expect("game");
        let frame = |time: &str, text: &str| crate::game_sync::FrameText {
            time: time.to_string(),
            text: text.to_string(),
        };
        let tx = conn.unchecked_transaction().expect("tx");
        crate::game_sync::store_timeline(
            &tx,
            game_id,
            &[
                frame("00:00:01.500", "{\"turn\":1}"),
                frame("1970-01-01T00:00:02Z", "{\"turn\":2}"),
                frame("later", "{\"turn\":3}"),
                // Not one whole JSON value: left out, as before the audit,
                // so the bundle's hash does not change.
                frame("00:00:04", "{\"turn\":4} trailing"),
            ],
        )
        .expect("frames");
        tx.commit().expect("commit");
        let input = match_bundle_input(&conn, match_id, chrono_tz::UTC)
            .expect("input")
            .expect("a match");
        let dated: Vec<i64> = input.games[0]
            .frames
            .iter()
            .map(|frame| frame.at_ms)
            .collect();
        assert_eq!(dated, vec![1_500, 2_000, 3_000]);
    }

    /// The golden's 0.x match as v1 imports it (`imported`).
    fn imported_golden() -> Value {
        let golden: Value = serde_json::from_str(include_str!(
            "../../../fixtures/parity/sync_match_bundles.golden.json"
        ))
        .expect("the match golden is JSON");
        golden["bundles"]
            .as_array()
            .expect("bundles")
            .iter()
            .find(|case| case["token"] == "q-golden-imported-match")
            .expect("the imported case")["input"]
            .clone()
    }

    // Replays spec section 10 (ruling 12): an imported game's frames are
    // dated as 0.x's `MatchBundleBuilder::frameInstant` dates them, from
    // their time of day on the game's end day in the system timezone, across
    // local midnight. The golden's `at_ms` are 0.x's own instants; its
    // bundle proves the rest (`every_golden_match_bundle_matches_0x_byte_for_byte`).
    #[test]
    fn an_imported_game_is_dated_from_its_frame_times_as_0x_dates_it() {
        let (_dir, conn) = db("sync-imported-frame-times");
        let case = imported_golden();
        let game = &case["games"][0];
        conn.execute_batch(
            "INSERT INTO accounts (username, first_seen_at) VALUES ('Qplayer', 0);
             INSERT INTO opponents (username, first_seen_at) VALUES ('Qfoe', 0);",
        )
        .expect("registries");
        conn.execute(
            "INSERT INTO matches (token, mtgo_match_id, account_id, opponent_id, source, format,
                 match_type, state, outcome, started_at, ended_at, last_activity_at, revision,
                 derived_version)
             VALUES (?1, ?2, 1, 1, 'imported', 'CMODERN', 'Constructed', 'complete', 'win', ?3,
                 ?4, ?4, 1, 1)",
            params![
                case["token"].as_str(),
                case["mtgo_match_id"].as_i64(),
                case["started_at_ms"].as_i64(),
                case["ended_at_ms"].as_i64()
            ],
        )
        .expect("match");
        let match_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO games (match_id, mtgo_game_id, game_number, result, end_reason,
                 started_at, ended_at)
             VALUES (?1, ?2, 1, 'win', 'unknown', ?3, ?4)",
            params![
                match_id,
                game["mtgo_game_id"].as_i64(),
                game["started_at_ms"].as_i64(),
                game["ended_at_ms"].as_i64()
            ],
        )
        .expect("game");
        let game_id = conn.last_insert_rowid();
        let golden_frames = game["frames"].as_array().expect("frames");
        let stored: Vec<crate::game_sync::FrameText> = golden_frames
            .iter()
            .map(|frame| crate::game_sync::FrameText {
                time: frame["time"].as_str().expect("time").to_string(),
                text: frame["content"].to_string(),
            })
            .collect();
        let tx = conn.unchecked_transaction().expect("tx");
        crate::game_sync::store_timeline(&tx, game_id, &stored).expect("frames");
        tx.commit().expect("commit");

        let zone: Tz = case["timezone"]
            .as_str()
            .expect("zone")
            .parse()
            .expect("tz");
        let input = match_bundle_input(&conn, match_id, zone)
            .expect("input")
            .expect("a match");
        let dated: Vec<(i64, Value)> = input.games[0]
            .frames
            .iter()
            .map(|frame| (frame.at_ms, frame.content.clone()))
            .collect();
        let expected: Vec<(i64, Value)> = golden_frames
            .iter()
            .map(|frame| {
                (
                    frame["at_ms"].as_i64().expect("ms"),
                    frame["content"].clone(),
                )
            })
            .collect();
        assert_eq!(dated, expected);
    }

    // A frame time the clocks skip on the game's end day is moved on by the
    // real length of the gap, as `Carbon::parse` reads it.
    #[test]
    fn a_frame_time_in_a_dst_gap_moves_on_by_the_gap() {
        let london: Tz = "Europe/London".parse().expect("zone");
        // 01:30 on 2026-03-29 is 02:30 BST, 01:30 UTC.
        let ended = chrono::DateTime::parse_from_rfc3339("2026-03-29T02:00:00Z")
            .expect("end")
            .timestamp_millis();
        assert_eq!(
            frame_instant("01:30:00", ended, london),
            Some(1_774_747_800_000)
        );
        // Lord Howe's gap is half an hour: 02:15 (+10:30) is 02:45 (+11:00).
        let lord_howe: Tz = "Australia/Lord_Howe".parse().expect("zone");
        let ended = chrono::DateTime::parse_from_rfc3339("2026-10-03T16:00:00Z")
            .expect("end")
            .timestamp_millis();
        assert_eq!(
            frame_instant("02:15:00", ended, lord_howe),
            Some(1_791_042_300_000)
        );
    }

    // Replays spec section 8: the share columns are input outside every
    // match touch key, so sharing (or switching a link off) never makes a
    // match dirty for sync (0.x `withoutTimestamps`).
    #[test]
    fn a_share_never_moves_the_match_touch_key() {
        let (_dir, mut conn) = db("sync-share-touch-key");
        enabled_match(&conn, "q-m");
        let before = match_key(&conn, "q-m");
        let share = crate::match_edits::MatchEdit::SetReplayShare {
            token: "q-m".to_string(),
            uuid: "9b2f6f4e-0000-4000-8000-000000000001".to_string(),
            url: "https://mymtgo.test/players/Qlocal/replays/9b2f6f4e".to_string(),
            shared_at_ms: T,
        };
        assert_eq!(
            crate::match_edits::edit_match(&mut conn, &share).expect("share"),
            crate::match_edits::MatchEditOutcome::Done
        );
        assert_eq!(match_key(&conn, "q-m"), before);
        let clear = crate::match_edits::MatchEdit::ClearReplayShare {
            token: "q-m".to_string(),
        };
        crate::match_edits::edit_match(&mut conn, &clear).expect("clear");
        assert_eq!(match_key(&conn, "q-m"), before);
    }

    // Replays spec section 8: 0.x's `MatchBundleBuilder` never sent the
    // share columns, and neither does v1.
    #[test]
    fn the_share_columns_are_never_in_the_bundle() {
        let (_dir, mut conn) = db("sync-share-bundle");
        let (_, _, match_id) = enabled_match(&conn, "q-m");
        let before = hash_of(&conn, match_id);
        let share = crate::match_edits::MatchEdit::SetReplayShare {
            token: "q-m".to_string(),
            uuid: "9b2f6f4e-0000-4000-8000-000000000001".to_string(),
            url: "https://mymtgo.test/players/Qlocal/replays/9b2f6f4e".to_string(),
            shared_at_ms: T,
        };
        crate::match_edits::edit_match(&mut conn, &share).expect("share");
        assert_eq!(hash_of(&conn, match_id), before);
        let input = match_bundle_input(&conn, match_id, chrono_tz::UTC)
            .expect("input")
            .expect("a match");
        let canonical = canonical_json(&match_bundle(&input, None));
        assert!(!canonical.contains("9b2f6f4e"), "{canonical}");
        assert!(!canonical.contains("replay"), "{canonical}");
    }

    // 0.x DirtinessTest "marks the league dirty when a draft pick lands or a
    // snapshot is recorded".
    #[test]
    fn marks_the_league_dirty_when_a_draft_pick_lands_or_a_snapshot_is_recorded() {
        let (_dir, conn) = db("sync-league-dirty");
        let (deck_id, _, _) = enabled_match(&conn, "q-m");
        let client_id = league(&conn, "q-l", deck_id);
        let mut seen = vec![league_key(&conn, &client_id)];
        let mut moved = |sql: &str| {
            conn.execute_batch(sql).expect(sql);
            let key = league_key(&conn, &client_id);
            assert!(!seen.contains(&key), "{sql}");
            seen.push(key);
        };
        moved(
            "INSERT INTO drafts (draft_token, source, league_id, state)
             VALUES ('q-draft', 'log', (SELECT id FROM leagues), 'picking')",
        );
        moved(
            "INSERT INTO draft_picks (draft_id, ordinal, pack_number, pick_number,
                 cards_available, reservations)
             VALUES ((SELECT id FROM drafts), 1, 1, 1, '[5,6]', '[]')",
        );
        moved("UPDATE draft_picks SET note = 'Qnote', note_updated_at = 1");
        moved("UPDATE matches SET league_id = (SELECT id FROM leagues), league_linked = 1");
        moved(
            "INSERT INTO limited_deck_snapshots (match_id, source, cards, signature, captured_at)
             SELECT id, 'imported', '[]', 'q-sig', 1 FROM matches",
        );
        moved("UPDATE leagues SET notes = 'Qnote'");
    }

    #[test]
    fn a_league_bundle_input_reads_the_run_its_draft_and_snapshots() {
        let (_dir, conn) = db("sync-league-input");
        let (deck_id, _, _) = enabled_match(&conn, "q-m");
        league(&conn, "q-l", deck_id);
        conn.execute_batch(
            "UPDATE leagues SET kind = 'draft', set_code = 'DSK', deck_version_id =
                 (SELECT id FROM deck_versions);
             UPDATE matches SET league_id = (SELECT id FROM leagues), league_linked = 1;
             INSERT INTO drafts (draft_token, source, league_id, state)
             VALUES ('q-draft', 'log', (SELECT id FROM leagues), 'finished');
             INSERT INTO draft_picks (draft_id, ordinal, pack_number, pick_number,
                 cards_available, reservations, note, note_updated_at)
             VALUES ((SELECT id FROM drafts), 2, 1, 2, '[7]', '[]', NULL, NULL),
                 ((SELECT id FROM drafts), 1, 1, 1, '[5,6]',
                  '[{\"catalog_id\":6,\"at_ms\":1787133605000}]', 'Qnote', 1);
             INSERT INTO draft_picks (draft_id, ordinal, pack_number, pick_number,
                 cards_available, reservations, detached_at, note, note_updated_at)
             VALUES ((SELECT id FROM drafts), 3, 1, 3, '[]', '[]', 1, 'Qkept', 1);
             INSERT INTO limited_deck_snapshots (match_id, source, cards, signature, captured_at)
             SELECT id, 'imported', '[{\"catalog_id\":5,\"quantity\":1,\"sideboard\":false}]',
                 'q-sig', 9 FROM matches;",
        )
        .expect("rows");
        let league_id: i64 = conn
            .query_row("SELECT id FROM leagues", [], |row| row.get(0))
            .expect("league");
        let input = league_bundle_input(&conn, league_id)
            .expect("input")
            .expect("a league");
        assert_eq!(input.kind, "draft");
        assert_eq!(input.deck_mtgo_id.as_deref(), Some("111"));
        assert!(!input.manual);
        let picks: Vec<i64> = input.drafts[0].picks.iter().map(|p| p.ordinal).collect();
        assert_eq!(picks, vec![1, 2], "attached picks in ordinal order");
        assert_eq!(
            input.drafts[0].picks[0].reservations,
            vec![(6, 1_787_133_605_000)]
        );
        assert_eq!(input.drafts[0].picks[0].note.as_deref(), Some("Qnote"));
        assert_eq!(input.snapshots.len(), 1);
        assert_eq!(input.snapshots[0].match_token.as_deref(), Some("q-m"));
    }

    // ---- Pulled rows ----------------------------------------------------------

    fn pulled_match(token: &str, state: &str) -> PulledMatch {
        let mut match_row: ImportedMatch = imported(token, Some(285_746_550));
        match_row.state = state.to_string();
        PulledMatch {
            platform: sync_bundles::BundlePlatform::Mtgo,
            sources: None,
            local_player_fields: Default::default(),
            opponent_player_fields: Default::default(),
            endstep_ranked: None,
            endstep_manual_cards: Vec::new(),
            game_structure: None,
            imported: match_row,
            detail: ImportedDetail {
                games: vec![ImportedGameDetail {
                    mtgo_game_id: Some(7),
                    game_number: 1,
                    local_deck: vec![DetailCard {
                        mtgo_id: 1001,
                        quantity: 4,
                        sideboard: false,
                    }],
                    ..ImportedGameDetail::default()
                }],
                log: Vec::new(),
            },
            deck: Some(("111".to_string(), "QsigV2".to_string())),
            league_client_id: None,
            history: false,
            archetypes: Vec::new(),
            player: PulledPlayerFields::default(),
        }
    }

    fn pull_match(conn: &mut Connection, pulled: PulledMatch) {
        let edit = SyncEdit::PulledMatch {
            client_id: pulled.imported.token.clone(),
            pulled: Box::new(pulled),
            hash: "h-pulled".to_string(),
            carry: "{}".to_string(),
            timezone: chrono_tz::UTC,
            now_ms: T,
        };
        edit_sync(conn, &edit).expect("pulled match");
    }

    fn report_status(conn: &Connection, token: &str) -> Option<String> {
        conn.query_row(
            "SELECT status FROM outbox WHERE kind = 'match_report' AND subject = ?1",
            [token],
            |row| row.get(0),
        )
        .optional()
        .expect("outbox")
    }

    #[test]
    fn a_pulled_match_imports_like_a_0x_match_onto_its_deck_version() {
        let (_dir, mut conn) = db("sync-pulled-match");
        let deck_id = deck(&conn, "111", true);
        version(&conn, deck_id, "QsigV1", T);
        pull_match(&mut conn, pulled_match("q-pulled", "complete"));
        let (source, signature): (String, String) = conn
            .query_row(
                "SELECT m.source, v.signature FROM matches m
                 JOIN deck_versions v ON v.id = m.deck_version_id WHERE m.token = 'q-pulled'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("pulled row");
        assert_eq!(source, "imported");
        // 0.x "restores a match onto version two of a deck, not version one".
        assert_eq!(signature, "QsigV2");
        let local_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM game_cards WHERE side = 'local' AND main_qty = 4",
                [],
                |row| row.get(0),
            )
            .expect("detail");
        assert_eq!(local_rows, 1, "its detail through the 0.x detail path");
        let row = synced_rows(&conn, SyncType::Match).expect("rows")["q-pulled"].clone();
        assert_eq!(row.hash.as_deref(), Some("h-pulled"));
        assert_eq!(row.carry.as_deref(), Some("{}"));
        assert_eq!(report_status(&conn, "q-pulled").as_deref(), Some("sent"));
    }

    fn set_deck(conn: &mut Connection, token: &str, version_id: i64) {
        let edit = crate::match_edits::MatchEdit::SetDeckVersion {
            tokens: vec![token.to_string()],
            deck_version_id: version_id,
        };
        assert_eq!(
            crate::match_edits::edit_match(conn, &edit).expect("edit"),
            crate::match_edits::MatchEditOutcome::Done
        );
    }

    fn deck_signature(conn: &Connection, token: &str) -> Option<String> {
        conn.query_row(
            "SELECT v.signature FROM matches m LEFT JOIN deck_versions v ON v.id = m.deck_version_id
             WHERE m.token = ?1",
            [token],
            |row| row.get(0),
        )
        .expect("row")
    }

    // Issue #332 (pinned, follow-up #323): the server's copy wins. A deck set
    // by hand on a pulled match is replaced by the next pull that updates it,
    // and is never pushed (a pulled match is not this device's to send).
    #[test]
    fn a_pull_that_updates_a_pulled_match_replaces_a_deck_set_by_hand() {
        let (_dir, mut conn) = db("sync-pulled-set-deck");
        let deck_id = deck(&conn, "111", true);
        version(&conn, deck_id, "QsigV1", T);
        let other_deck = deck(&conn, "222", true);
        let other = version(&conn, other_deck, "QsigOther", T);
        pull_match(&mut conn, pulled_match("q-pulled", "complete"));
        assert_eq!(deck_signature(&conn, "q-pulled").as_deref(), Some("QsigV2"));

        set_deck(&mut conn, "q-pulled", other);
        assert_eq!(
            deck_signature(&conn, "q-pulled").as_deref(),
            Some("QsigOther")
        );
        let pushed = match_candidates(&conn, Tier::Supporter, AFTER_THE_HOLD).expect("candidates");
        assert!(
            pushed
                .iter()
                .all(|candidate| candidate.client_id != "q-pulled" || !candidate.owned),
            "not pushed"
        );

        let mut changed = pulled_match("q-pulled", "complete");
        changed.imported.outcome = "loss".to_string();
        pull_match(&mut conn, changed);
        assert_eq!(
            deck_signature(&conn, "q-pulled").as_deref(),
            Some("QsigV2"),
            "the pull's deck replaces the hand-set one"
        );
    }

    // Issue #332: a log match's new deck changes its touch key, so the match
    // waits to be pushed again.
    #[test]
    fn a_log_matchs_new_deck_changes_its_touch_key() {
        let (_dir, mut conn) = db("sync-log-set-deck");
        crate::write::commit(
            &mut conn,
            &crate::write::WriteBatch {
                matches: vec![crate::write::MatchWrite::Upsert {
                    view: Box::new(crate::test_support::view(
                        crate::test_support::TOKEN,
                        "Qopponent",
                        T,
                    )),
                    revision: 1,
                }],
                ..crate::write::WriteBatch::default()
            },
        )
        .expect("a log match");
        let deck_id = deck(&conn, "111", true);
        let first = version(&conn, deck_id, "QsigV1", T);
        let second = version(&conn, deck_id, "QsigV2", T + 1);
        conn.execute(
            "UPDATE matches SET deck_version_id = ?1 WHERE token = ?2",
            params![first, crate::test_support::TOKEN],
        )
        .expect("linked");
        let key = |conn: &Connection| {
            match_candidates(conn, Tier::Supporter, AFTER_THE_HOLD)
                .expect("candidates")
                .into_iter()
                .find(|candidate| candidate.client_id == crate::test_support::TOKEN)
                .expect("a candidate")
        };
        let before = key(&conn);
        assert!(before.owned);
        let synced = SyncedRow {
            client_id: crate::test_support::TOKEN.to_string(),
            hash: Some("h".to_string()),
            synced_at: Some(T),
            built_hash: Some("h".to_string()),
            touch_key: Some(before.touch_key.clone()),
            changed_at: Some(T),
            carry: None,
            pulled_key: None,
        };
        assert!(!is_pending_key(&before.touch_key, Some(&synced)));

        set_deck(&mut conn, crate::test_support::TOKEN, second);
        let after = key(&conn);
        assert_ne!(after.touch_key, before.touch_key);
        assert!(after.owned);
        assert!(
            is_pending_key(&after.touch_key, Some(&synced)),
            "pending a push"
        );
    }

    #[test]
    fn a_pulled_match_without_a_known_deck_has_no_deck_and_an_unfinished_one_no_report() {
        let (_dir, mut conn) = db("sync-pulled-no-deck");
        let mut pulled = pulled_match("q-limited", "abandoned");
        pulled.deck = None;
        pull_match(&mut conn, pulled);
        let version: Option<i64> = conn
            .query_row(
                "SELECT deck_version_id FROM matches WHERE token = 'q-limited'",
                [],
                |row| row.get(0),
            )
            .expect("row");
        assert_eq!(version, None);
        assert_eq!(report_status(&conn, "q-limited"), None);
    }

    #[test]
    fn a_live_match_wins_over_a_pulled_copy() {
        let (_dir, mut conn) = db("sync-pulled-live-wins");
        insert_manual(&conn, "q-live");
        conn.execute(
            "UPDATE matches SET source = 'log', mtgo_match_id = 285746550, derived_version = 1
             WHERE token = 'q-live'",
            [],
        )
        .expect("live");
        pull_match(&mut conn, pulled_match("q-other-token", "complete"));
        let tokens: Vec<String> = conn
            .prepare("SELECT token FROM matches ORDER BY id")
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect();
        assert_eq!(tokens, vec!["q-live".to_string()]);
        // The live row is the reported one.
        assert_eq!(report_status(&conn, "q-live").as_deref(), Some("sent"));
    }

    #[test]
    fn a_pulled_report_never_goes_back_and_a_pending_one_becomes_sent() {
        let (_dir, mut conn) = db("sync-pulled-report");
        crate::outbox::edit_outbox(
            &mut conn,
            &crate::outbox::OutboxEdit::Enqueue {
                rows: vec![crate::outbox::NewOutboxRow {
                    kind: crate::outbox::OutboxKind::MatchReport,
                    subject: "q-pulled".to_string(),
                    payload: Some(b"q".to_vec()),
                    now_ms: 1,
                }],
            },
        )
        .expect("queued");
        pull_match(&mut conn, pulled_match("q-pulled", "complete"));
        assert_eq!(report_status(&conn, "q-pulled").as_deref(), Some("sent"));
        let sent_at: i64 = conn
            .query_row("SELECT sent_at FROM outbox", [], |row| row.get(0))
            .expect("sent_at");
        pull_match(&mut conn, pulled_match("q-pulled", "complete"));
        let again: i64 = conn
            .query_row("SELECT sent_at FROM outbox", [], |row| row.get(0))
            .expect("sent_at");
        assert_eq!(sent_at, again);
    }

    // A pulled copy that could not be stored (no player, no account) must
    // not record the server's hash: the next manifest offers it again.
    #[test]
    fn a_pulled_match_that_was_not_stored_records_no_hash() {
        let (_dir, mut conn) = db("sync-pulled-unstored-hash");
        let mut nameless = pulled_match("q-nameless", "complete");
        nameless.imported.local_username = String::new();
        pull_match(&mut conn, nameless);
        let rows = synced_rows(&conn, SyncType::Match).expect("rows");
        assert!(!rows.contains_key("q-nameless"), "no hash was recorded");
        pull_match(&mut conn, pulled_match("q-nameless", "complete"));
        let rows = synced_rows(&conn, SyncType::Match).expect("rows");
        assert_eq!(rows["q-nameless"].hash.as_deref(), Some("h-pulled"));
    }

    // The client id keeps the start to the second; the stored run may hold
    // milliseconds (`manual_league_id` compares on the second too).
    #[test]
    fn a_pulled_match_joins_a_run_whose_start_has_milliseconds() {
        let (_dir, mut conn) = db("sync-pulled-league-second");
        pull_league(&mut conn, pulled_league("q-run", "draft", true));
        conn.execute("UPDATE leagues SET started_at = started_at + 500", [])
            .expect("milliseconds");
        let mut played = pulled_match("q-in-run", "complete");
        played.league_client_id = Some(league_client_id("q-run", T));
        pull_match(&mut conn, played);
        let linked: Option<i64> = conn
            .query_row(
                "SELECT league_id FROM matches WHERE token = 'q-in-run'",
                [],
                |row| row.get(0),
            )
            .expect("match");
        assert!(linked.is_some(), "the run is found on the second");
    }

    fn pulled_league(token: &str, kind: &str, manual: bool) -> PulledLeague {
        let client_id = league_client_id(token, T);
        let legacy_id = pulled_legacy_id(&client_id);
        PulledLeague {
            league: ImportedLeague {
                legacy_id,
                manual,
                kind: kind.to_string(),
                set_code: None,
                course_id: None,
                token: token.to_string(),
                event_id: None,
                name: "Qpulled league".to_string(),
                format: "CMODERN".to_string(),
                state: "complete".to_string(),
                started_at_ms: T,
                joined_at_ms: None,
                completed_at_ms: None,
                dropped_at_ms: None,
                created_at_ms: None,
                deck: Some(LeagueDeck {
                    deck_mtgo_id: "111".to_string(),
                    signature: "MTAwMTo0OmZhbHNl".to_string(),
                }),
                notes: Some("Qnote".to_string()),
                matches: Vec::new(),
            },
            drafts: Vec::new(),
            decks: Vec::new(),
            player: PulledLeagueFields::default(),
        }
    }

    fn pull_league(conn: &mut Connection, pulled: PulledLeague) {
        let client_id = league_client_id(&pulled.league.token, pulled.league.started_at_ms);
        pull_league_as(conn, &client_id, pulled);
    }

    /// Pull a league bundle under `client_id`, which can disagree with its
    /// start (the device that sent it rekeyed its run).
    fn pull_league_as(conn: &mut Connection, client_id: &str, pulled: PulledLeague) {
        let edit = SyncEdit::PulledLeague {
            client_id: client_id.to_string(),
            pulled: Box::new(pulled),
            hash: "h-league".to_string(),
            carry: "{}".to_string(),
            timezone: chrono_tz::UTC,
            now_ms: T,
        };
        edit_sync(conn, &edit).expect("pulled league");
    }

    #[test]
    fn a_pulled_league_imports_as_a_run_with_its_draft_picks_and_snapshots() {
        let (_dir, mut conn) = db("sync-pulled-league");
        let mut pulled = pulled_league("q-run", "draft", false);
        let legacy_id = pulled.league.legacy_id;
        let mut draft = limited::Draft {
            token: "q-draft".to_string(),
            league_event_id: None,
            course_id: None,
            draft_id: Some(9),
            pod_token: None,
            seat_index: Some(1),
            seat_count: 8,
            booster_catalog_id: None,
            state: limited::DraftState::Finished,
            pack_size: 14,
            picks_expected: 42,
            started_at_ms: Some(T),
            ended_at_ms: Some(T + 1),
            picks: Vec::new(),
        };
        for ordinal in [1, 2] {
            draft.picks.push(limited::Pick {
                ordinal,
                pack_number: 1,
                pick_number: ordinal,
                pack_id: None,
                direction: None,
                cards_available: vec![5, 6],
                picked_catalog_id: Some(5),
                picked_card_id: None,
                reservations: Vec::new(),
                shown_at_ms: None,
                deadline_at_ms: None,
                picked_at_ms: None,
            });
        }
        pulled.drafts.push(LegacyDraft {
            legacy_id: pulled_legacy_id("q-draft"),
            league_id: Some(legacy_id),
            draft,
            notes: vec![(1, "Qfirst".to_string())],
        });
        let snapshot = LegacyLimitedDeck {
            league_id: legacy_id,
            match_token: "q-later-match".to_string(),
            mtgo_match_id: None,
            cards: vec![limited::RegisteredCard {
                catalog_id: 5,
                quantity: 1,
                sideboard: false,
            }],
            captured_at_ms: T,
        };
        pulled.decks.push(snapshot.clone());
        pull_league(&mut conn, pulled.clone());
        let (source, notes): (String, String) = conn
            .query_row("SELECT source, notes FROM leagues", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .expect("run");
        assert_eq!((source.as_str(), notes.as_str()), ("imported", "Qnote"));
        let picks: i64 = conn
            .query_row("SELECT COUNT(*) FROM draft_picks", [], |row| row.get(0))
            .expect("picks");
        assert_eq!(picks, 2);
        let note: String = conn
            .query_row(
                "SELECT note FROM draft_picks WHERE ordinal = 1",
                [],
                |row| row.get(0),
            )
            .expect("note");
        assert_eq!(note, "Qfirst");
        // The snapshot waits for its match; once it is here it is placed.
        let snapshots = |conn: &Connection| -> i64 {
            conn.query_row("SELECT COUNT(*) FROM limited_deck_snapshots", [], |row| {
                row.get(0)
            })
            .expect("snapshots")
        };
        assert_eq!(snapshots(&conn), 0);
        pull_match(&mut conn, {
            let mut later = pulled_match("q-later-match", "complete");
            later.imported.mtgo_match_id = Some(1);
            later.league_client_id = Some(league_client_id("q-run", T));
            later
        });
        let linked: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM matches WHERE token = 'q-later-match' AND league_linked = 1
                 AND league_id = (SELECT id FROM leagues)",
                [],
                |row| row.get(0),
            )
            .expect("linked");
        assert_eq!(linked, 1, "a pulled match joins its pulled run");
        edit_sync(
            &mut conn,
            &SyncEdit::PulledSnapshots {
                decks: vec![snapshot],
                now_ms: T,
            },
        )
        .expect("snapshots");
        assert_eq!(snapshots(&conn), 1);
        let draft_ids = |conn: &Connection| -> (Option<i64>, Option<i64>) {
            conn.query_row(
                "SELECT imported_from, legacy_draft_id FROM drafts",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("draft")
        };
        let before = draft_ids(&conn);
        assert_eq!(before.1, Some(pulled_legacy_id("q-draft")));
        // Pulled again: the same rows, the link kept, and the draft keeps its
        // ids (final review F2).
        pull_league(&mut conn, pulled);
        assert_eq!(draft_ids(&conn), before, "a re-pull keeps the draft's ids");
        let leagues: i64 = conn
            .query_row("SELECT COUNT(*) FROM leagues", [], |row| row.get(0))
            .expect("leagues");
        assert_eq!(leagues, 1);
        let still: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM matches WHERE league_linked = 1",
                [],
                |row| row.get(0),
            )
            .expect("linked");
        assert_eq!(still, 1);
    }

    #[test]
    fn a_pulled_manual_league_keeps_its_token_and_a_local_run_wins() {
        let (_dir, mut conn) = db("sync-pulled-manual-league");
        pull_league(
            &mut conn,
            pulled_league("manual_QqQqQqQqQqQqQqQqQqQqQqQq", "constructed", true),
        );
        let (source, token): (String, String) = conn
            .query_row("SELECT source, token FROM leagues", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .expect("manual");
        assert_eq!(source, "manual");
        assert_eq!(token, "manual_QqQqQqQqQqQqQqQqQqQqQqQq");
        // This device's own manual league with the same token and start wins.
        let (_dir2, mut other) = db("sync-pulled-manual-league-local");
        insert_manual_league(&other, 3, None, None, "Qmine", T, T);
        other
            .execute("UPDATE leagues SET token = 'manual_Qmine'", [])
            .expect("token");
        pull_league(
            &mut other,
            pulled_league("manual_Qmine", "constructed", true),
        );
        let names: Vec<String> = other
            .prepare("SELECT name FROM leagues")
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect();
        assert_eq!(names, vec!["Qmine".to_string()]);
        let row = synced_rows(&other, SyncType::League).expect("rows");
        assert_eq!(
            row[&league_client_id("manual_Qmine", T)].hash.as_deref(),
            Some("h-league")
        );
    }

    fn owned_league(conn: &Connection, client_id: &str) -> bool {
        league_candidates(conn, Tier::Supporter)
            .expect("candidates")
            .into_iter()
            .find(|candidate| candidate.client_id == client_id)
            .expect("a candidate")
            .owned
    }

    // Review fix of Plan Q task 11: a pulled manual league stays the
    // server's copy when its matches and registered decks arrive (they move
    // its touch key, but they are the server's too); an edit here makes it
    // this device's.
    #[test]
    fn a_pulled_manual_league_stays_the_servers_copy_as_its_matches_arrive() {
        let (_dir, mut conn) = db("sync-pulled-manual-league-members");
        let deck_id = deck(&conn, "111", true);
        version(&conn, deck_id, "MTAwMTo0OmZhbHNl", T);
        let token = "manual_QqQqQqQqQqQqQqQqQqQqQqQq";
        let client_id = league_client_id(token, T);
        pull_league(&mut conn, pulled_league(token, "constructed", true));
        assert!(!owned_league(&conn, &client_id), "the server's copy");
        let key = league_key(&conn, &client_id);
        pull_match(&mut conn, {
            let mut member = pulled_match("q-member", "complete");
            member.league_client_id = Some(client_id.clone());
            member
        });
        let linked: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM matches WHERE token = 'q-member' AND league_id IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .expect("linked");
        assert_eq!(linked, 1);
        assert_ne!(league_key(&conn, &client_id), key, "its key moved");
        assert!(!owned_league(&conn, &client_id), "still the server's copy");
        conn.execute("UPDATE leagues SET notes = 'Qedited here'", [])
            .expect("edit");
        assert!(owned_league(&conn, &client_id), "edited here: pushed");
    }

    #[test]
    fn a_pulled_manual_match_is_the_servers_copy_until_edited_and_a_live_row_is_never() {
        let (_dir, mut conn) = db("sync-pulled-manual-match-owned");
        let deck_id = deck(&conn, "111", true);
        version(&conn, deck_id, "QsigV2", T);
        let mut manual = pulled_match("q-hand", "complete");
        manual.imported.manual = Some(Vec::new());
        manual.imported.mtgo_match_id = None;
        pull_match(&mut conn, manual);
        let owned = |conn: &Connection, token: &str| {
            match_candidates(conn, Tier::Supporter, AFTER_THE_HOLD)
                .expect("candidates")
                .into_iter()
                .find(|candidate| candidate.client_id == token)
                .expect("a candidate")
                .owned
        };
        assert!(!owned(&conn, "q-hand"));
        assert_eq!(
            pending_count(&conn, SyncType::Match, AFTER_THE_HOLD).expect("pending"),
            0
        );
        conn.execute(
            "UPDATE matches SET hidden_at = 5, revision = revision + 1 WHERE token = 'q-hand'",
            [],
        )
        .expect("edit");
        assert!(owned(&conn, "q-hand"));

        // A pulled copy of a match this device tracked live: the live row
        // wins and stays this device's to push.
        insert_manual(&conn, "q-live");
        conn.execute(
            "UPDATE matches SET source = 'log', mtgo_match_id = 42, deck_version_id =
                 (SELECT id FROM deck_versions LIMIT 1) WHERE token = 'q-live'",
            [],
        )
        .expect("live");
        pull_match(&mut conn, pulled_match("q-live", "complete"));
        assert!(owned(&conn, "q-live"));
    }

    // Fix round 2: the league bundle holds no match data, so editing a
    // member match never makes a pulled league this device's; editing the
    // league does.
    #[test]
    fn editing_a_member_match_leaves_a_pulled_manual_league_the_servers_copy() {
        let (_dir, mut conn) = db("sync-pulled-manual-league-member-edit");
        let deck_id = deck(&conn, "111", true);
        version(&conn, deck_id, "MTAwMTo0OmZhbHNl", T);
        let token = "manual_QqQqQqQqQqQqQqQqQqQqQqQq";
        let client_id = league_client_id(token, T);
        pull_league(&mut conn, pulled_league(token, "constructed", true));
        pull_match(&mut conn, {
            let mut member = pulled_match("q-member", "complete");
            member.imported.manual = Some(Vec::new());
            member.imported.mtgo_match_id = None;
            member.league_client_id = Some(client_id.clone());
            member
        });
        conn.execute(
            "UPDATE matches SET notes = 'Qedited here', revision = revision + 1
             WHERE token = 'q-member'",
            [],
        )
        .expect("member edit");
        assert!(
            !owned_league(&conn, &client_id),
            "a member's edit is not the league's"
        );
        conn.execute("UPDATE leagues SET name = 'Qrenamed here'", [])
            .expect("league edit");
        assert!(owned_league(&conn, &client_id), "the league's own edit is");
    }

    // Fix round 2: a re-pull whose copy cannot be stored (no player in the
    // bundle, no current account) leaves the replaced row as it was.
    #[test]
    fn a_re_pull_that_cannot_be_stored_keeps_the_old_manual_match_intact() {
        let (_dir, mut conn) = db("sync-pulled-manual-restore");
        let deck_id = deck(&conn, "111", true);
        version(&conn, deck_id, "QsigV2", T);
        let mut manual = pulled_match("q-hand", "complete");
        manual.imported.manual = Some(Vec::new());
        manual.imported.mtgo_match_id = None;
        manual.imported.note = Some("Qcloud".to_string());
        pull_match(&mut conn, manual.clone());
        let snapshot = |conn: &Connection| -> (i64, String, Option<String>, i64, Option<i64>) {
            conn.query_row(
                "SELECT m.id, m.source, m.notes,
                     (SELECT COUNT(*) FROM games g WHERE g.match_id = m.id), m.deck_version_id
                 FROM matches m WHERE m.token = 'q-hand'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("the manual match")
        };
        let before = snapshot(&conn);
        // Two accounts and no log: no current account to file it under.
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at) VALUES ('Qsecond', 0)",
            [],
        )
        .expect("account");
        let mut nameless = manual;
        nameless.imported.local_username = String::new();
        nameless.imported.note = Some("Qother device".to_string());
        pull_match(&mut conn, nameless);
        assert_eq!(snapshot(&conn), before, "kept whole");
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM matches", [], |row| row.get(0))
            .expect("count");
        assert_eq!(rows, 1, "no spare-token row left behind");
        let owned = match_candidates(&conn, Tier::Supporter, AFTER_THE_HOLD)
            .expect("candidates")
            .into_iter()
            .find(|candidate| candidate.client_id == "q-hand")
            .expect("a candidate")
            .owned;
        assert!(!owned, "still the server's copy, never pushed");
    }

    #[test]
    fn pulled_legacy_ids_are_negative_and_stable() {
        let id = pulled_legacy_id("q-run_20260819T100005Z");
        assert!(id < 0);
        assert_eq!(id, pulled_legacy_id("q-run_20260819T100005Z"));
        assert_ne!(id, pulled_legacy_id("q-run_20260819T100006Z"));
    }

    /// One synthetic `GameStateUpdate` line of a game: two players and
    /// `cards` cards in play (a frame grows from a hand of seven to a board).
    fn measured_frame(match_id: i64, game_id: i64, cards: i64) -> String {
        let cards: Vec<String> = (0..cards)
            .map(|id| {
                format!(
                    "{{\"Id\":{},\"CatalogID\":{},\"Zone\":\"Battlefield\",\"ActualZone\":\"Battlefield\",\"Owner\":{},\"Controller\":{}}}",
                    100 + id,
                    1000 + id % 40,
                    1 + id % 2,
                    1 + id % 2
                )
            })
            .collect();
        format!(
            "12:00:00 [INF] (Twitch Info|Game Play Status Update for Game ID: {game_id}, Match ID: {match_id}, Event ID: {match_id}) {{\"Players\":[{{\"Id\":1,\"Name\":\"Qlocal\",\"LibraryCount\":53,\"HandCount\":7,\"Life\":20}},{{\"Id\":2,\"Name\":\"Qopponent\",\"LibraryCount\":53,\"HandCount\":7,\"Life\":20}}],\"Cards\":[{}]}}",
            cards.join(",")
        )
    }

    /// Measured, not gated (07 spec section 15): a full rebuild of 1,000
    /// synthetic log matches (three games, 40 card rows each, and 100 stored
    /// `GameStateUpdate` events per game, read back as the bundle's frames) against the
    /// background rebuild budget (foundation section 8, under 10 s). Run with
    /// `cargo test -p store --release -- --ignored a_full_rebuild --nocapture`.
    #[test]
    #[ignore = "a measurement, not a check"]
    fn a_full_rebuild_of_1000_matches_is_measured() {
        let (_dir, conn) = db("sync-rebuild-measure");
        let deck_id = deck(&conn, "111", true);
        let version_id = version(&conn, deck_id, "MTAwMTo0OmZhbHNl", T);
        conn.execute_batch(
            "INSERT INTO accounts (username, first_seen_at) VALUES ('Qlocal', 0);
             INSERT INTO opponents (username, first_seen_at) VALUES ('Qopponent', 0);",
        )
        .expect("registries");
        for catalog_id in 1000..1040 {
            conn.execute(
                "INSERT INTO cards (catalog_id, oracle_id) VALUES (?1, ?2)",
                params![catalog_id, format!("q-oracle-{catalog_id}")],
            )
            .expect("card");
        }
        conn.execute_batch("BEGIN").expect("begin");
        for index in 0..1_000i64 {
            conn.execute(
                "INSERT INTO matches (token, mtgo_match_id, account_id, opponent_id, source,
                     format, match_type, state, outcome, started_at, ended_at,
                     last_activity_at, revision, derived_version, deck_version_id)
                 VALUES (?1, ?2, 1, 1, 'log', 'CMODERN', 'Constructed', 'complete', 'win',
                     ?3, ?3, ?3, 1, 1, ?4)",
                params![format!("q-{index:04}"), index, T + index, version_id],
            )
            .expect("match");
            let match_id = conn.last_insert_rowid();
            for number in 1..=3i64 {
                conn.execute(
                    "INSERT INTO games (match_id, mtgo_game_id, game_number, result, end_reason,
                         started_at, ended_at)
                     VALUES (?1, ?2, ?3, 'win', 'win', ?4, ?4)",
                    params![match_id, index * 10 + number, number, T + index],
                )
                .expect("game");
                let game_id = conn.last_insert_rowid();
                for catalog_id in 1000..1040i64 {
                    let side = if catalog_id < 1030 {
                        "local"
                    } else {
                        "opponent"
                    };
                    conn.execute(
                        "INSERT INTO game_cards (game_id, side, catalog_id, main_qty, seen, `cast`)
                         VALUES (?1, ?2, ?3, 2, 1, 1)",
                        params![game_id, side, catalog_id],
                    )
                    .expect("card row");
                }
            }
        }
        conn.execute_batch("COMMIT").expect("commit");
        // The log events a live match's frames are read from: 100 per game,
        // from a hand of seven to a full board, ten matches per write batch.
        let mut conn = conn;
        let file_id = crate::write::insert_log_file(
            &conn,
            &crate::test_support::new_log_file("q-measure/mtgo.log"),
        )
        .expect("log file");
        let mut offset = 0u64;
        for chunk in 0..100i64 {
            let mut batch = crate::write::WriteBatch::default();
            for index in chunk * 10..chunk * 10 + 10 {
                for number in 1..=3i64 {
                    let game_id = index * 10 + number;
                    for frame in 0..100i64 {
                        offset += 1;
                        batch.events.push(crate::write::NewEvent {
                            log_file_id: file_id,
                            byte_offset: offset,
                            kind: Some(mtgo_protocol::EventKind::GameStateUpdate),
                            subjects: mtgo_protocol::Subjects {
                                match_id: Some(index as u64),
                                game_id: Some(game_id as u64),
                                ..Default::default()
                            },
                            logged_at_ms: T + index + frame,
                            raw: measured_frame(index, game_id, 7 + frame * 33 / 99),
                            quarantined: false,
                        });
                    }
                }
            }
            crate::write::commit(&mut conn, &batch).expect("events");
        }
        let started = std::time::Instant::now();
        let candidates = match_candidates(&conn, Tier::Free, AFTER_THE_HOLD).expect("candidates");
        println!(
            "touch keys of {} matches: {:?}",
            candidates.len(),
            started.elapsed()
        );
        let mut hashes = 0;
        let mut frames = 0usize;
        for candidate in &candidates {
            let input = match_bundle_input(&conn, candidate.row_id, chrono_tz::UTC)
                .expect("input")
                .expect("a match");
            frames += input
                .games
                .iter()
                .map(|game| game.frames.len())
                .sum::<usize>();
            // As the sync runner builds it: the frames move into the bundle.
            let bundle = sync_bundles::match_bundle::match_bundle_owned(input, None);
            if !bundle_hash(&canonical_json(&bundle)).is_empty() {
                hashes += 1;
            }
        }
        let elapsed = started.elapsed();
        println!("full rebuild of {hashes} matches with log events: {elapsed:?}");
        assert_eq!(hashes, 1_000);
        assert_eq!(
            frames, 300_000,
            "every game's 100 stored frames reach its bundle"
        );
    }

    // ---- Archetypes, guides and notes in the bundles (Plan T task 15) -----

    fn held_archetype(conn: &Connection, uuid: &str, source: &str) -> i64 {
        let format = if source == "fallback" {
            None
        } else {
            Some("modern")
        };
        conn.query_row(
            "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
             VALUES (?1, 'Q ' || ?1, ?2, ?3, 1, 1) RETURNING id",
            params![uuid, format, source],
            |row| row.get(0),
        )
        .expect("archetype")
    }

    fn seat(conn: &Connection, match_id: i64, opponent: bool, archetype: i64, manual: bool) {
        conn.execute(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
             VALUES (?1, ?2, ?3, 0.75, ?4)
             ON CONFLICT (match_id, is_opponent) DO UPDATE SET archetype_id = excluded.archetype_id,
                 confidence = excluded.confidence, manual = excluded.manual",
            params![match_id, opponent, archetype, manual],
        )
        .expect("seat");
    }

    /// `(seat, uuid, manual)` of a match's rows, player first.
    fn seats(conn: &Connection, token: &str) -> Vec<(bool, String, bool)> {
        let mut stmt = conn
            .prepare(
                "SELECT x.is_opponent, a.uuid, x.manual FROM match_archetypes x
                 JOIN archetypes a ON a.id = x.archetype_id
                 JOIN matches m ON m.id = x.match_id
                 WHERE m.token = ?1 ORDER BY x.is_opponent",
            )
            .expect("prepare");
        stmt.query_map([token], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect()
    }

    fn side(uuid: &str, confidence: f64, manual: bool, opponent: bool) -> ArchetypeSide {
        ArchetypeSide {
            uuid: uuid.to_string(),
            confidence,
            manual,
            opponent,
        }
    }

    // 0.x BundleBuilderTest "builds a match bundle carrying ... archetype
    // sides", from the stored seats (spec section 14).
    #[test]
    fn a_match_bundle_input_reads_its_archetype_seats() {
        let (_dir, conn) = db("sync-match-archetypes");
        let (_, _, match_id) = enabled_match(&conn, "q-m");
        let mine = held_archetype(&conn, "q-mine", "downloaded");
        let theirs = held_archetype(&conn, "00000000-q-homebrew", "fallback");
        seat(&conn, match_id, false, mine, false);
        seat(&conn, match_id, true, theirs, true);
        let input = match_bundle_input(&conn, match_id, chrono_tz::UTC)
            .expect("input")
            .expect("a match");
        assert_eq!(
            input.archetypes,
            vec![
                side("q-mine", 0.75, false, false),
                side("00000000-q-homebrew", 0.75, true, true),
            ]
        );
        let bundle = match_bundle(&input, None);
        assert_eq!(bundle["archetypes"][0]["uuid"], "00000000-q-homebrew");
        assert_eq!(bundle["archetypes"][0]["player_username"], "Qopponent");
        assert_eq!(bundle["archetypes"][1]["player_username"], "Qlocal");
        assert_eq!(bundle["archetypes"][1]["player_is_player"], true);
        assert_eq!(bundle["archetypes"][1]["confidence"], "0.75");
    }

    // Spec section 14: the touch key follows the match's archetype rows.
    #[test]
    fn a_match_archetype_change_rebuilds_the_match_bundle() {
        let (_dir, conn) = db("sync-match-archetype-touch");
        let (_, _, match_id) = enabled_match(&conn, "q-m");
        let first = held_archetype(&conn, "q-first", "downloaded");
        let second = held_archetype(&conn, "q-second", "downloaded");
        let mut seen = vec![match_key(&conn, "q-m")];
        let mut moved = |change: &dyn Fn()| {
            change();
            let key = match_key(&conn, "q-m");
            assert!(!seen.contains(&key), "{key}");
            seen.push(key);
        };
        moved(&|| seat(&conn, match_id, true, first, false));
        moved(&|| seat(&conn, match_id, false, second, false));
        moved(&|| seat(&conn, match_id, true, second, false));
        moved(&|| seat(&conn, match_id, true, second, true));
        moved(&|| {
            conn.execute("UPDATE match_archetypes SET confidence = 0.5", [])
                .expect("confidence");
        });
        moved(&|| {
            conn.execute(
                "UPDATE archetypes SET uuid = 'q-moved' WHERE id = ?1",
                [second],
            )
            .expect("uuid");
        });
        moved(&|| {
            conn.execute("DELETE FROM match_archetypes WHERE is_opponent = 1", [])
                .expect("none");
        });
    }

    // Spec section 14: the deck's archetype is in its touch key and its
    // bundle.
    #[test]
    fn a_deck_archetype_change_rebuilds_the_deck_bundle() {
        let (_dir, mut conn) = db("sync-deck-archetype-touch");
        let id = deck(&conn, "111", false);
        let archetype = held_archetype(&conn, "q-deck-arch", "downloaded");
        mark_synced(&mut conn);
        conn.execute(
            "UPDATE decks SET archetype_id = ?2 WHERE id = ?1",
            params![id, archetype],
        )
        .expect("archetype");
        assert_eq!(pending_decks(&conn), 1);
        let parts = deck_bundle_parts(&conn, id).expect("parts");
        assert_eq!(parts.archetype_uuid.as_deref(), Some("q-deck-arch"));
        mark_synced(&mut conn);
        conn.execute(
            "UPDATE archetypes SET uuid = 'q-moved' WHERE id = ?1",
            [archetype],
        )
        .expect("moved uuid");
        assert_eq!(pending_decks(&conn), 1, "a server move changes the uuid");
    }

    // 0.x DirtinessTest guide and note case, the bundle side: a note lands,
    // the deck is pending and its rebuilt bundle holds the note.
    #[test]
    fn a_note_edit_rebuilds_the_deck_bundle() {
        let (_dir, mut conn) = db("sync-deck-note-rebuild");
        let id = deck(&conn, "111", false);
        let archetype = held_archetype(&conn, "q-foe", "downloaded");
        mark_synced(&mut conn);
        let noted = crate::archetype_edit::ArchetypeEdit::AddNote {
            deck_id: id,
            archetype_id: archetype,
            body: "Qnote: keep the counters in".to_string(),
            now_ms: T + 2_500,
        };
        crate::archetype_edit::edit_archetypes(&mut conn, &noted).expect("note");
        assert_eq!(pending_decks(&conn), 1);
        let parts = deck_bundle_parts(&conn, id).expect("parts");
        assert_eq!(
            parts.notes,
            vec![NoteInput {
                archetype_uuid: "q-foe".to_string(),
                body: "Qnote: keep the counters in".to_string(),
                created_at_ms: Some(T + 2_500),
            }]
        );
        assert_eq!(
            parts.guides,
            vec![GuideInput {
                archetype_uuid: "q-foe".to_string(),
                cards: Vec::new(),
            }],
            "the note's pair has its empty guide"
        );
    }

    fn guide_parts(guides: Vec<GuideInput>, notes: Vec<NoteInput>) -> PulledDeckParts {
        PulledDeckParts {
            keys: DeckParts {
                has_archetype: true,
                has_guides_and_notes: true,
            },
            platform: BundlePlatform::Mtgo,
            endstep_cover: None,
            archetype_uuid: None,
            guides,
            notes,
        }
    }

    fn guide(uuid: &str, cards: &[(&str, &str, i64)]) -> GuideInput {
        GuideInput {
            archetype_uuid: uuid.to_string(),
            cards: cards
                .iter()
                .map(|(oracle_id, direction, quantity)| GuideCardInput {
                    oracle_id: oracle_id.to_string(),
                    direction: direction.to_string(),
                    quantity: *quantity,
                })
                .collect(),
        }
    }

    fn note(uuid: &str, body: &str, at: Option<i64>) -> NoteInput {
        NoteInput {
            archetype_uuid: uuid.to_string(),
            body: body.to_string(),
            created_at_ms: at,
        }
    }

    fn deck_id_of(conn: &Connection, mtgo_id: &str) -> i64 {
        conn.query_row(
            "SELECT id FROM decks WHERE mtgo_id = ?1",
            [mtgo_id],
            |row| row.get(0),
        )
        .expect("deck")
    }

    /// `(uuid, cards as direction:oracle:qty)` per guide, and `(uuid, body,
    /// created_at)` per note, of a deck.
    fn guide_rows(conn: &Connection, mtgo_id: &str) -> (Vec<String>, Vec<(String, String, i64)>) {
        let deck_id = deck_id_of(conn, mtgo_id);
        let mut stmt = conn
            .prepare(
                "SELECT a.uuid || '|' || COALESCE((SELECT group_concat(c.direction || ':'
                     || c.oracle_id || ':' || c.quantity, ',') FROM (SELECT * FROM
                     sideboard_guide_cards WHERE guide_id = g.id ORDER BY direction, oracle_id) c),
                     '')
                 FROM sideboard_guides g JOIN archetypes a ON a.id = g.archetype_id
                 WHERE g.deck_id = ?1 ORDER BY a.uuid",
            )
            .expect("prepare");
        let guides = stmt
            .query_map([deck_id], |row| row.get(0))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect();
        let mut stmt = conn
            .prepare(
                "SELECT a.uuid, n.body, n.created_at FROM deck_archetype_notes n
                 JOIN archetypes a ON a.id = n.archetype_id
                 WHERE n.deck_id = ?1 ORDER BY a.uuid, n.body",
            )
            .expect("prepare");
        let notes = stmt
            .query_map([deck_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect();
        (guides, notes)
    }

    // 0.x BundleRoundTripTest "restores cover and archetype from a deck
    // bundle": a deck this device has never seen takes the bundle's
    // archetype (a known uuid resolves to its row).
    #[test]
    fn restores_cover_and_archetype_from_a_deck_bundle() {
        let (_dir, mut conn) = db("sync-pull-deck-archetype");
        let known = held_archetype(&conn, "q-archetype", "downloaded");
        let mut parts = guide_parts(Vec::new(), Vec::new());
        parts.archetype_uuid = Some("q-archetype".to_string());
        pull_with(&mut conn, pulled("777"), parts, T);
        let (cover, archetype): (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT cover_catalog_id, archetype_id FROM decks WHERE mtgo_id = '777'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("deck");
        assert_eq!(cover, Some(424_242));
        assert_eq!(archetype, Some(known));
        let rebuilt = deck_bundle_parts(&conn, deck_id_of(&conn, "777")).expect("parts");
        assert_eq!(rebuilt.archetype_uuid.as_deref(), Some("q-archetype"));
    }

    // Input is never overwritten (spec section 14, global constraints): a
    // deck whose archetype is set here keeps it; one without takes the
    // pulled one; a bundle without the key (before canonical version 4)
    // changes nothing.
    #[test]
    fn a_pulled_deck_archetype_fills_only_an_empty_one() {
        let (_dir, mut conn) = db("sync-pull-deck-archetype-kept");
        let local = held_archetype(&conn, "q-local", "manual");
        let id = deck(&conn, "777", false);
        conn.execute(
            "UPDATE decks SET archetype_id = ?2, source = 'imported' WHERE id = ?1",
            params![id, local],
        )
        .expect("set");
        let mut parts = guide_parts(Vec::new(), Vec::new());
        parts.archetype_uuid = Some("q-remote".to_string());
        pull_with(&mut conn, pulled("777"), parts.clone(), T);
        let archetype = |conn: &Connection| -> Option<i64> {
            conn.query_row(
                "SELECT archetype_id FROM decks WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .expect("deck")
        };
        assert_eq!(archetype(&conn), Some(local));
        conn.execute("UPDATE decks SET archetype_id = NULL", [])
            .expect("clear");
        parts.keys.has_archetype = false;
        pull_with(&mut conn, pulled("777"), parts.clone(), T);
        assert_eq!(archetype(&conn), None, "no key: unknown, not none");
        parts.keys.has_archetype = true;
        pull_with(&mut conn, pulled("777"), parts, T);
        let remote: i64 = conn
            .query_row(
                "SELECT id FROM archetypes WHERE uuid = 'q-remote'",
                [],
                |row| row.get(0),
            )
            .expect("stub");
        assert_eq!(archetype(&conn), Some(remote));
    }

    // 0.x BundleRoundTripTest "round-trips a deck with its sideboard guide
    // and matchup note, replacing rather than duplicating them".
    #[test]
    fn round_trips_a_deck_with_its_sideboard_guide_and_matchup_note_replacing_rather_than_duplicating_them()
     {
        let (_dir, mut conn) = db("sync-pull-guides-round-trip");
        held_archetype(&conn, "q-foe", "downloaded");
        let local_only = held_archetype(&conn, "q-local-only", "downloaded");
        let id = deck(&conn, "777", false);
        conn.execute(
            "INSERT INTO sideboard_guides (deck_id, archetype_id, created_at, updated_at)
             VALUES (?1, ?2, 1, 1)",
            params![id, local_only],
        )
        .expect("local guide");
        let parts = guide_parts(
            vec![guide(
                "q-foe",
                &[("q-oracle", "in", 2), ("q-out", "out", 1)],
            )],
            vec![note("q-foe", "Qnote", Some(T - 5_000))],
        );
        pull_with(&mut conn, pulled("777"), parts.clone(), T);
        pull_with(&mut conn, pulled("777"), parts, T + 1);
        let (guides, notes) = guide_rows(&conn, "777");
        assert_eq!(guides, vec!["q-foe|in:q-oracle:2,out:q-out:1".to_string()]);
        assert_eq!(
            notes,
            vec![("q-foe".to_string(), "Qnote".to_string(), T - 5_000)]
        );
        let stamped: i64 = conn
            .query_row("SELECT updated_at FROM sideboard_guides", [], |row| {
                row.get(0)
            })
            .expect("guide");
        assert_eq!(stamped, T + 1, "guides stamped with the pull time");
        // Built again from the rows, the parts are the pulled ones.
        let rebuilt = deck_bundle_parts(&conn, id).expect("parts");
        assert_eq!(rebuilt.guides.len(), 1);
        assert_eq!(rebuilt.notes[0].created_at_ms, Some(T - 5_000));
    }

    // 0.x BundleRoundTripTest "stubs an archetype the local cache lacks for
    // a synced guide, as a match import does": a `downloaded` row named by
    // its uuid, in the deck's format, with no definition (the next check
    // fills it). A note without a time gets the pull's.
    #[test]
    fn stubs_an_archetype_the_local_cache_lacks_for_a_synced_guide() {
        let (_dir, mut conn) = db("sync-pull-guide-stub");
        let parts = guide_parts(
            vec![guide("q-unknown-uuid", &[("q-oracle", "in", 1)])],
            vec![note("q-unknown-uuid", "Qnote", None)],
        );
        pull_with(&mut conn, pulled("777"), parts, T + 9);
        let (name, format, source, definition): (String, Option<String>, String, Option<String>) =
            conn.query_row(
                "SELECT name, format, source, definition FROM archetypes
                 WHERE uuid = 'q-unknown-uuid'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("a stub");
        assert_eq!(name, "q-unknown-uuid");
        assert_eq!(format.as_deref(), Some("modern"));
        assert_eq!(source, "downloaded");
        assert_eq!(definition, None);
        let (guides, notes) = guide_rows(&conn, "777");
        assert_eq!(guides, vec!["q-unknown-uuid|in:q-oracle:1".to_string()]);
        assert_eq!(notes[0].2, T + 9);
    }

    // Final review F17 (controller ruling, data loss): a deck without a
    // constructed format (a limited or blank-format deck) cannot stub an
    // unknown uuid. Its guides and notes stay in the deck's carry and the
    // next push sends them back byte-identical, so the server never loses
    // them. A known uuid still lands as rows.
    #[test]
    fn an_unplaceable_guide_is_carried_and_pushed_back_untouched() {
        use sync_bundles::{DeckBundleInput, canonical_json, deck_bundle, read_deck_bundle};
        let (_dir, mut conn) = db("sync-pull-guide-carried");
        let homebrew = "00000000-0000-0000-0000-000000000001";
        let server = deck_bundle(
            &DeckBundleInput {
                platform: sync_bundles::BundlePlatform::Mtgo,
                mtgo_id: "777".to_string(),
                name: "Qpulled".to_string(),
                original_name: None,
                format: Some("DZNR".to_string()),
                color_identity: None,
                cover_catalog_id: None,
                endstep_cover: None,
                versions: vec![("QsigA".to_string(), T)],
                archetype_uuid: None,
                guides: vec![
                    guide(
                        "q-unknown-z",
                        &[("q-oracle-2", "out", 1), ("q-oracle-1", "in", 2)],
                    ),
                    guide(homebrew, &[("q-oracle-3", "in", 1)]),
                    guide("q-unknown-a", &[]),
                ],
                notes: vec![
                    note("q-unknown-z", "Qnote: bring the counters", Some(T - 5_000)),
                    note("q-unknown-a", "Qnote: no time", None),
                    note(homebrew, "Qnote: homebrew", Some(T - 9_000)),
                ],
            },
            None,
        );
        let (input, keys, carry) = read_deck_bundle(&server).expect("readable");
        let parts = PulledDeckParts {
            keys,
            platform: input.platform,
            endstep_cover: input.endstep_cover,
            archetype_uuid: input.archetype_uuid,
            guides: input.guides,
            notes: input.notes,
        };
        let mut deck = pulled("777");
        deck.format = "DZNR".to_string();
        let edit = SyncEdit::PulledDeck {
            client_id: sync_bundles::deck_client_id("777"),
            deck: Box::new(deck),
            parts: Box::new(parts),
            hash: "hpulled".to_string(),
            carry: carry.to_json().to_string(),
            now_ms: T + 1,
        };
        edit_sync(&mut conn, &edit).expect("pull");
        let (guides, _) = guide_rows(&conn, "777");
        assert_eq!(
            guides,
            vec![format!("{homebrew}|in:q-oracle-3:1")],
            "the known uuid lands as rows, no stub without a format"
        );

        // The push: the deck's bundle built from its rows and carry.
        let rebuilt = deck_bundle_parts(&conn, deck_id_of(&conn, "777")).expect("parts");
        let pushed = deck_bundle(
            &DeckBundleInput {
                platform: sync_bundles::BundlePlatform::Mtgo,
                mtgo_id: "777".to_string(),
                name: "Qpulled".to_string(),
                original_name: None,
                format: Some("DZNR".to_string()),
                color_identity: None,
                cover_catalog_id: None,
                endstep_cover: None,
                versions: vec![("QsigA".to_string(), T)],
                archetype_uuid: rebuilt.archetype_uuid,
                guides: rebuilt.guides,
                notes: rebuilt.notes,
            },
            None,
        );
        assert_eq!(
            canonical_json(&pushed["guides"]),
            canonical_json(&server["guides"]),
            "every guide goes back byte-identical"
        );
        assert_eq!(
            canonical_json(&pushed["notes"]),
            canonical_json(&server["notes"]),
            "every note goes back byte-identical"
        );
    }

    // Final review F17 sibling (controller ruling, data loss): a pulled
    // deck archetype uuid this device cannot place (unknown, and the deck
    // has no constructed format) stays in the deck's carry and the next
    // push sends it back byte-identical.
    #[test]
    fn an_unplaceable_deck_archetype_is_carried_and_pushed_back_untouched() {
        use sync_bundles::{DeckBundleInput, canonical_json, deck_bundle};
        let (_dir, mut conn) = db("sync-pull-deck-archetype-carried");
        let mut parts = guide_parts(Vec::new(), Vec::new());
        parts.archetype_uuid = Some("q-unknown-deck-arch".to_string());
        let mut deck = pulled("777");
        deck.format = "DZNR".to_string();
        pull_with(&mut conn, deck, parts, T);
        let archetype: Option<i64> = conn
            .query_row(
                "SELECT archetype_id FROM decks WHERE mtgo_id = '777'",
                [],
                |row| row.get(0),
            )
            .expect("deck");
        assert_eq!(archetype, None, "no stub without a format");
        let rebuilt = deck_bundle_parts(&conn, deck_id_of(&conn, "777")).expect("parts");
        let input = |archetype_uuid: Option<String>| DeckBundleInput {
            platform: sync_bundles::BundlePlatform::Mtgo,
            mtgo_id: "777".to_string(),
            name: "Qpulled".to_string(),
            original_name: None,
            format: Some("DZNR".to_string()),
            color_identity: None,
            cover_catalog_id: None,
            endstep_cover: None,
            versions: Vec::new(),
            archetype_uuid,
            guides: Vec::new(),
            notes: Vec::new(),
        };
        let server = deck_bundle(&input(Some("q-unknown-deck-arch".to_string())), None);
        let pushed = deck_bundle(&input(rebuilt.archetype_uuid), None);
        assert_eq!(
            canonical_json(&pushed["deck"]),
            canonical_json(&server["deck"])
        );
    }

    // Review Focus 6: 0.x "leaves local guides and notes alone when a
    // pre-version-3 deck bundle carries no guide keys".
    #[test]
    fn a_bundle_without_guide_keys_leaves_local_guides_alone() {
        let (_dir, mut conn) = db("sync-pull-no-guide-keys");
        let foe = held_archetype(&conn, "q-foe", "downloaded");
        let id = deck(&conn, "777", false);
        let edit = crate::archetype_edit::ArchetypeEdit::AddNote {
            deck_id: id,
            archetype_id: foe,
            body: "Qlocal note".to_string(),
            now_ms: T,
        };
        crate::archetype_edit::edit_archetypes(&mut conn, &edit).expect("note");
        let before = guide_rows(&conn, "777");
        assert_eq!(before.1.len(), 1);
        let mut parts = guide_parts(Vec::new(), Vec::new());
        parts.keys.has_guides_and_notes = false;
        pull_with(&mut conn, pulled("777"), parts, T + 1);
        assert_eq!(guide_rows(&conn, "777"), before);
    }

    // Spec section 21.5: a pulled row that breaks a CHECK is skipped with a
    // log line, the rest of the pull lands.
    #[test]
    fn a_pulled_bundle_skips_a_guide_card_that_breaks_a_check() {
        let (_dir, mut conn) = db("sync-pull-guide-checks");
        let parts = guide_parts(
            vec![guide(
                "q-foe",
                &[
                    ("q-good", "in", 2),
                    ("q-zero", "in", 0),
                    ("q-many", "out", 100),
                    ("q-side", "sideways", 1),
                    ("", "in", 1),
                    (&"q".repeat(65), "in", 1),
                    ("q-good", "in", 3),
                ],
            )],
            vec![
                note("q-foe", "Qkept", Some(T)),
                note("q-foe", "   ", Some(T)),
                note("q-foe", &"q".repeat(2_001), Some(T)),
            ],
        );
        pull_with(&mut conn, pulled("777"), parts, T);
        let (guides, notes) = guide_rows(&conn, "777");
        assert_eq!(guides, vec!["q-foe|in:q-good:2".to_string()]);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].1, "Qkept");
    }

    fn pulled_with_archetypes(token: &str, archetypes: Vec<ArchetypeSide>) -> PulledMatch {
        let mut pulled = pulled_match(token, "complete");
        pulled.archetypes = archetypes;
        pulled
    }

    // Review Focus 1 (spec section 14): a pulled match's manual rows are
    // input and replace nothing hand-set locally; its detected rows fill
    // only seats this device has no row for.
    #[test]
    fn a_pulled_bundle_never_replaces_a_local_manual_row() {
        let (_dir, mut conn) = db("sync-pull-manual-row");
        insert_manual(&conn, "q-live");
        conn.execute(
            "UPDATE matches SET source = 'log', mtgo_match_id = 285746550, derived_version = 1
             WHERE token = 'q-live'",
            [],
        )
        .expect("live");
        let match_id: i64 = conn
            .query_row("SELECT id FROM matches WHERE token = 'q-live'", [], |row| {
                row.get(0)
            })
            .expect("match");
        let hand_set = held_archetype(&conn, "q-hand-set", "manual");
        let detected = held_archetype(&conn, "q-detected", "downloaded");
        seat(&conn, match_id, true, hand_set, true);
        seat(&conn, match_id, false, detected, false);
        held_archetype(&conn, "q-remote-a", "downloaded");
        held_archetype(&conn, "q-remote-b", "downloaded");
        pull_match(
            &mut conn,
            pulled_with_archetypes(
                "q-other-token",
                vec![
                    side("q-remote-a", 1.0, true, true),
                    side("q-remote-b", 0.9, false, false),
                ],
            ),
        );
        assert_eq!(
            seats(&conn, "q-live"),
            vec![
                (false, "q-detected".to_string(), false),
                (true, "q-hand-set".to_string(), true),
            ],
            "neither the hand-set row nor the detected one is replaced by detected rows"
        );
        // A pulled hand-set row replaces a detected local row.
        pull_match(
            &mut conn,
            pulled_with_archetypes(
                "q-other-token",
                vec![
                    side("q-remote-b", 1.0, true, false),
                    side("q-remote-a", 1.0, true, true),
                ],
            ),
        );
        assert_eq!(
            seats(&conn, "q-live"),
            vec![
                (false, "q-remote-b".to_string(), true),
                (true, "q-hand-set".to_string(), true),
            ]
        );
    }

    // Final review I2 (Review Focus 1): the player's "none" for the
    // opponent is input. A pulled opponent row, hand-set or detected,
    // never lands on it, even after a re-detection cleared the marker.
    #[test]
    fn a_pulled_bundle_never_fills_an_opponent_the_player_set_to_none() {
        let (_dir, mut conn) = db("sync-pull-none-flag");
        insert_manual(&conn, "q-live");
        conn.execute(
            "UPDATE matches SET source = 'log', mtgo_match_id = 285746550, derived_version = 1,
                 opponent_archetype_none = 1, archetypes_detected_at = NULL
             WHERE token = 'q-live'",
            [],
        )
        .expect("live, none chosen, due again");
        held_archetype(&conn, "q-remote-a", "downloaded");
        held_archetype(&conn, "q-remote-b", "downloaded");
        for manual in [false, true] {
            pull_match(
                &mut conn,
                pulled_with_archetypes(
                    "q-other-token",
                    vec![
                        side("q-remote-a", 1.0, manual, true),
                        side("q-remote-b", 1.0, manual, false),
                    ],
                ),
            );
            assert_eq!(
                seats(&conn, "q-live"),
                vec![(false, "q-remote-b".to_string(), manual)],
                "the player seat lands, the opponent stays none (manual {manual})"
            );
        }
    }

    // #827: a pulled Brawl match's hand-set commander side this device has
    // not made is still the player's choice: it becomes a manual seat row
    // here (the stub every unknown uuid gets), not a carry for detection to
    // overwrite. A detected side is still left to detection. Once this
    // device learns the commanders, the stub becomes the commander row.
    #[test]
    fn a_pulled_hand_set_commander_side_becomes_a_seat() {
        let (_dir, mut conn) = db("sync-pull-commander-manual");
        let mut brawl = pulled_with_archetypes(
            "q-brawl",
            vec![
                side("q-commander-foe", 1.0, true, true),
                side("q-commander-mine", 0.8, false, false),
            ],
        );
        brawl.imported.format = Some("ARENA_BRAWL".to_string());
        pull_match(&mut conn, brawl);
        assert_eq!(
            seats(&conn, "q-brawl"),
            vec![(true, "q-commander-foe".to_string(), true)]
        );
        assert!(
            carried_match_sides(&conn, "q-brawl")
                .expect("carry")
                .is_empty(),
            "nothing left in the carry"
        );
        let detected_at: Option<i64> = conn
            .query_row(
                "SELECT archetypes_detected_at FROM matches WHERE token = 'q-brawl'",
                [],
                |row| row.get(0),
            )
            .expect("match");
        assert_eq!(detected_at, None, "the player seat is left to detection");
        // A commander placeholder, never a `downloaded` stub: the
        // definitions check (which retires uuids the server does not know)
        // never sees it.
        let source: String = conn
            .query_row(
                "SELECT source FROM archetypes WHERE uuid = 'q-commander-foe'",
                [],
                |row| row.get(0),
            )
            .expect("placeholder");
        assert_eq!(source, "commander");
        assert!(
            !crate::archetypes::undefined_uuids(&conn)
                .expect("undefined")
                .contains(&"q-commander-foe".to_string())
        );
        // Retired anyway (an older build's stub): the heal brings it back.
        conn.execute(
            "UPDATE archetypes SET retired_at = 5 WHERE uuid = 'q-commander-foe'",
            [],
        )
        .expect("retired");

        let tx = conn.transaction().expect("tx");
        crate::archetype_edit::ensure_commander(
            &tx,
            "q-commander-foe",
            "Qcommander",
            "brawl",
            Some("WU"),
            T + 1,
        )
        .expect("ensure");
        tx.commit().expect("commit");
        let row: (String, String, Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT name, source, color_identity, retired_at FROM archetypes
                 WHERE uuid = 'q-commander-foe'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("archetype");
        assert_eq!(
            row,
            (
                "Qcommander".to_string(),
                "commander".to_string(),
                Some("WU".to_string()),
                None
            )
        );
    }

    // Final review F17 sibling (controller ruling, data loss): a pulled
    // limited match's archetype rows naming an unknown uuid have no format
    // to stub in. They stay in the match's carry and the next push sends
    // them back byte-identical; a seat with a local row keeps its row.
    #[test]
    fn an_unplaceable_match_archetype_is_carried_and_pushed_back_untouched() {
        use sync_bundles::canonical_json;
        use sync_bundles::match_bundle::match_bundle;
        let (_dir, mut conn) = db("sync-pull-match-archetype-carried");
        let sides = vec![
            side("q-draft-foe", 0.42, false, true),
            side("q-draft-mine", 1.0, true, false),
        ];
        let mut limited = pulled_with_archetypes("q-limited", sides.clone());
        limited.imported.format = Some("DZNR".to_string());
        limited.imported.mtgo_match_id = Some(285_746_999);
        limited.deck = None;
        pull_match(&mut conn, limited);
        assert!(
            seats(&conn, "q-limited").is_empty(),
            "no rows without a format"
        );
        let match_id: i64 = conn
            .query_row(
                "SELECT id FROM matches WHERE token = 'q-limited'",
                [],
                |row| row.get(0),
            )
            .expect("match");
        let rebuilt = match_bundle_input(&conn, match_id, chrono_tz::UTC)
            .expect("input")
            .expect("a match");
        let mut server = rebuilt.clone();
        server.archetypes = sides;
        assert_eq!(
            canonical_json(&match_bundle(&rebuilt, None)["archetypes"]),
            canonical_json(&match_bundle(&server, None)["archetypes"]),
            "the pulled rows go back as they came"
        );
    }

    /// The archetype sides a match's next push sends, `(opponent, uuid)`.
    fn pushed_sides(conn: &Connection, token: &str) -> Vec<(bool, String)> {
        let match_id: i64 = conn
            .query_row("SELECT id FROM matches WHERE token = ?1", [token], |row| {
                row.get(0)
            })
            .expect("match");
        let input = match_bundle_input(conn, match_id, chrono_tz::UTC)
            .expect("input")
            .expect("a match");
        let mut sides: Vec<(bool, String)> = input
            .archetypes
            .into_iter()
            .map(|side| (side.opponent, side.uuid))
            .collect();
        sides.sort();
        sides
    }

    fn limited_pull(token: &str, sides: Vec<ArchetypeSide>) -> PulledMatch {
        let mut limited = pulled_with_archetypes(token, sides);
        limited.imported.format = Some("DZNR".to_string());
        limited.imported.mtgo_match_id = Some(285_746_999);
        limited.deck = None;
        limited
    }

    // #300 R2 (Review Focus 1): after the player set the match's opponent
    // to "none", a carried opponent side is no longer pushed; the carried
    // player side still is.
    #[test]
    fn a_carried_opponent_side_is_not_pushed_after_none() {
        let (_dir, mut conn) = db("sync-carried-none");
        pull_match(
            &mut conn,
            limited_pull(
                "q-limited",
                vec![
                    side("q-draft-foe", 0.42, false, true),
                    side("q-draft-mine", 1.0, true, false),
                ],
            ),
        );
        assert_eq!(
            pushed_sides(&conn, "q-limited"),
            vec![
                (false, "q-draft-mine".to_string()),
                (true, "q-draft-foe".to_string())
            ]
        );
        conn.execute(
            "UPDATE matches SET opponent_archetype_none = 1 WHERE token = 'q-limited'",
            [],
        )
        .expect("none chosen");
        assert_eq!(
            pushed_sides(&conn, "q-limited"),
            vec![(false, "q-draft-mine".to_string())]
        );
    }

    // #300 R4: a pulled match that lands on a live match with another
    // token leaves its unplaced sides with the live match too, so its push
    // sends them.
    #[test]
    fn unplaced_sides_of_a_pull_on_a_live_match_go_with_the_live_match() {
        let (_dir, mut conn) = db("sync-carried-live");
        insert_manual(&conn, "q-live");
        conn.execute(
            "UPDATE matches SET source = 'log', mtgo_match_id = 285746999, derived_version = 1,
                 format = 'DZNR'
             WHERE token = 'q-live'",
            [],
        )
        .expect("live");
        pull_match(
            &mut conn,
            limited_pull(
                "q-other-token",
                vec![
                    side("q-draft-foe", 0.42, false, true),
                    side("q-draft-mine", 1.0, true, false),
                ],
            ),
        );
        assert_eq!(
            pushed_sides(&conn, "q-live"),
            vec![
                (false, "q-draft-mine".to_string()),
                (true, "q-draft-foe".to_string())
            ]
        );
    }

    // Spec section 14 (0.x `LocalArchetypeId`): an unknown pulled uuid
    // becomes a `downloaded` stub named by its uuid, in the match's format,
    // which the next check fills; a match whose opponent seat lands is
    // marked detected.
    #[test]
    fn an_unknown_pulled_uuid_becomes_a_stub_the_check_fills() {
        let (_dir, mut conn) = db("sync-pull-match-stub");
        let deck_id = deck(&conn, "111", true);
        version(&conn, deck_id, "QsigV1", T);
        pull_match(
            &mut conn,
            pulled_with_archetypes(
                "q-pulled",
                vec![
                    side("q-never-seen", 0.42, false, true),
                    side("q-never-seen-either", 1.0, true, false),
                ],
            ),
        );
        assert_eq!(
            seats(&conn, "q-pulled"),
            vec![
                (false, "q-never-seen-either".to_string(), true),
                (true, "q-never-seen".to_string(), false),
            ]
        );
        let (name, format): (String, Option<String>) = conn
            .query_row(
                "SELECT name, format FROM archetypes WHERE uuid = 'q-never-seen'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("stub");
        assert_eq!(
            (name.as_str(), format.as_deref()),
            ("q-never-seen", Some("modern"))
        );
        assert_eq!(
            crate::archetypes::undefined_uuids(&conn).expect("undefined"),
            vec![
                "q-never-seen".to_string(),
                "q-never-seen-either".to_string()
            ]
        );
        let confidence: f64 = conn
            .query_row(
                "SELECT confidence FROM match_archetypes WHERE is_opponent = 1",
                [],
                |row| row.get(0),
            )
            .expect("confidence");
        assert!((confidence - 0.42).abs() < 1e-9);
        let marker: Option<i64> = conn
            .query_row(
                "SELECT archetypes_detected_at FROM matches WHERE token = 'q-pulled'",
                [],
                |row| row.get(0),
            )
            .expect("marker");
        assert_eq!(marker, Some(T));
        // A limited match has no format key to stub in: its rows are left out.
        let mut limited =
            pulled_with_archetypes("q-limited", vec![side("q-draft", 1.0, false, true)]);
        limited.imported.format = Some("DZNR".to_string());
        limited.imported.mtgo_match_id = Some(285_746_999);
        limited.deck = None;
        pull_match(&mut conn, limited);
        assert!(seats(&conn, "q-limited").is_empty());
    }

    // ---- League self-management (Task 9) ---------------------------------

    fn match_id_of(conn: &Connection, token: &str) -> i64 {
        conn.query_row("SELECT id FROM matches WHERE token = ?1", [token], |row| {
            row.get(0)
        })
        .expect("match id")
    }

    fn league_id_of(conn: &Connection, token: &str) -> i64 {
        conn.query_row("SELECT id FROM leagues WHERE token = ?1", [token], |row| {
            row.get(0)
        })
        .expect("league id")
    }

    fn match_candidate(conn: &Connection, token: &str) -> RowCandidate {
        match_candidates(conn, Tier::Supporter, AFTER_THE_HOLD)
            .expect("candidates")
            .into_iter()
            .find(|candidate| candidate.client_id == token)
            .expect("a candidate")
    }

    // League self-management spec 6.2: a player edit moves the match's key
    // and makes even a pulled copy this device's to push.
    #[test]
    fn a_player_edit_moves_the_key_and_makes_an_imported_match_pushable() {
        let (_dir, mut conn) = db("sync-edited-imported-match");
        deck(&conn, "111", true);
        pull_match(&mut conn, pulled_match("q-pulled", "complete"));
        let before = match_candidate(&conn, "q-pulled");
        assert!(
            !before.edited && !before.owned,
            "an unedited copy is not sent"
        );

        conn.execute(
            "UPDATE matches SET no_league = 1, edited_at = ?1 WHERE token = 'q-pulled'",
            [T + 1],
        )
        .expect("remove from league");
        let after = match_candidate(&conn, "q-pulled");
        assert_ne!(after.touch_key, before.touch_key);
        assert!(after.edited && after.owned);

        conn.execute(
            "UPDATE matches SET edited_at = ?1 WHERE token = 'q-pulled'",
            [T + 2],
        )
        .expect("edited again");
        assert_ne!(
            match_candidate(&conn, "q-pulled").touch_key,
            after.touch_key,
            "every edit moves the key"
        );
    }

    // Deviation 1: a row never edited keeps the key it had, so pulled keys
    // stored before the player columns still match.
    #[test]
    fn an_unedited_rows_key_leaves_out_the_player_columns() {
        let (_dir, mut conn) = db("sync-unedited-key");
        deck(&conn, "111", true);
        pull_match(&mut conn, pulled_match("q-pulled", "complete"));
        let before = match_candidate(&conn, "q-pulled");
        conn.execute(
            "UPDATE matches SET no_league = 1 WHERE token = 'q-pulled'",
            [],
        )
        .expect("flag without an edit");
        assert_eq!(
            match_candidate(&conn, "q-pulled").touch_key,
            before.touch_key
        );
    }

    // Spec 6.1: the bundle input carries the player columns; a league link
    // only when the player made it (Deviation 2).
    #[test]
    fn the_match_bundle_input_carries_the_player_columns() {
        let (_dir, conn) = db("sync-match-player-input");
        let deck_id = deck(&conn, "111", true);
        league(&conn, "q-run", deck_id);
        insert_manual(&conn, "q-m");
        crate::test_support::link_league_match(&conn, league_id_of(&conn, "q-run"), "q-m");
        let match_id = match_id_of(&conn, "q-m");
        let input = match_bundle_input(&conn, match_id, chrono_tz::UTC)
            .expect("input")
            .expect("a match");
        assert_eq!(
            (
                input.hidden_at_ms,
                input.no_league,
                input.league_linked,
                input.edited_at_ms
            ),
            (None, false, false, None),
            "a link no player made is not written"
        );
        conn.execute(
            "UPDATE matches SET hidden_at = ?1, edited_at = ?1 WHERE id = ?2",
            params![T + 5, match_id],
        )
        .expect("deleted");
        let input = match_bundle_input(&conn, match_id, chrono_tz::UTC)
            .expect("input")
            .expect("a match");
        assert_eq!(
            (
                input.hidden_at_ms,
                input.no_league,
                input.league_linked,
                input.edited_at_ms
            ),
            (Some(T + 5), false, true, Some(T + 5))
        );
    }

    #[test]
    fn the_league_bundle_input_carries_the_player_columns() {
        let (_dir, conn) = db("sync-league-player-input");
        let deck_id = deck(&conn, "111", true);
        league(&conn, "q-run", deck_id);
        let league_id = league_id_of(&conn, "q-run");
        conn.execute(
            "UPDATE leagues SET removed_at = ?1, user_state = 'dropped', user_dropped_at = ?2,
                 edited_at = ?1
             WHERE id = ?3",
            params![T + 5, T + 4, league_id],
        )
        .expect("deleted");
        let input = league_bundle_input(&conn, league_id)
            .expect("input")
            .expect("a league");
        assert_eq!(input.removed_at_ms, Some(T + 5));
        assert_eq!(input.user_state.as_deref(), Some("dropped"));
        assert_eq!(input.user_dropped_at_ms, Some(T + 4));
        assert_eq!(input.edited_at_ms, Some(T + 5));
    }

    // Spec 6.2 and Deviation 6: a league edit moves its keys; a removed
    // league is still sent once its row is detached (all its matches
    // deleted), so the other devices remove it too.
    #[test]
    fn a_league_edit_moves_its_key_and_a_removed_league_stays_a_candidate() {
        let (_dir, conn) = db("sync-league-edit-key");
        let deck_id = deck(&conn, "111", true);
        let client_id = league(&conn, "q-run", deck_id);
        let find = |conn: &Connection| {
            league_candidates(conn, Tier::Supporter)
                .expect("candidates")
                .into_iter()
                .find(|candidate| candidate.client_id == client_id)
        };
        let before = find(&conn).expect("a candidate");
        assert!(!before.edited);
        conn.execute(
            "UPDATE leagues SET removed_at = ?1, edited_at = ?1, detached_at = ?1
             WHERE token = 'q-run'",
            [T + 1],
        )
        .expect("deleted, its matches too");
        let after = find(&conn).expect("a removed league is still sent");
        assert_ne!(after.touch_key, before.touch_key);
        assert!(after.edited && after.owned);
    }

    // ---- League self-management (Task 10) --------------------------------

    use crate::test_support::{TOKEN, deleted_from};

    /// The log match `TOKEN` as the tracker writes it, played with deck
    /// "111" (a sync candidate); its row id.
    fn log_match_with_deck(conn: &mut Connection) -> i64 {
        crate::write::commit(
            conn,
            &crate::write::WriteBatch {
                matches: vec![crate::write::MatchWrite::Upsert {
                    view: Box::new(crate::test_support::view(TOKEN, "Qopponent", T)),
                    revision: 1,
                }],
                ..crate::write::WriteBatch::default()
            },
        )
        .expect("a log match");
        let deck_id = deck(conn, "111", true);
        let version_id = version(conn, deck_id, "QsigV1", T);
        conn.execute(
            "UPDATE matches SET deck_version_id = ?1 WHERE token = ?2",
            params![version_id, TOKEN],
        )
        .expect("its deck");
        match_id_of(conn, TOKEN)
    }

    fn edited(token: &str, player: PulledPlayerFields) -> PulledMatch {
        let mut pulled = pulled_match(token, "complete");
        pulled.player = player;
        pulled
    }

    /// `(hidden_at, no_league, league_linked, edited_at)` of a match.
    fn player_columns(conn: &Connection, token: &str) -> (Option<i64>, bool, bool, Option<i64>) {
        conn.query_row(
            "SELECT hidden_at, no_league, league_linked, edited_at FROM matches WHERE token = ?1",
            [token],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("match")
    }

    fn pulled_key_of(conn: &Connection, sync_type: SyncType, client_id: &str) -> Option<String> {
        conn.query_row(
            "SELECT pulled_key FROM synced_rows WHERE type = ?1 AND client_id = ?2",
            params![sync_type.as_str(), client_id],
            |row| row.get(0),
        )
        .optional()
        .expect("row")
        .flatten()
    }

    fn tombstones(conn: &mut Connection, since_ms: Option<i64>, now_ms: i64) -> u32 {
        let edit = SyncEdit::Tombstones {
            client_ids: vec![TOKEN.to_string(), "q-unknown".to_string()],
            since_ms,
            now_ms,
        };
        edit_sync(conn, &edit).expect("tombstones")
    }

    // League self-management spec 6.4: a match deleted on the website is
    // hidden here without an edit, and is no longer offered to push.
    #[test]
    fn a_tombstone_hides_a_match_and_keeps_it_unsent() {
        let (_dir, mut conn) = db("sync-tombstone");
        let match_id = log_match_with_deck(&mut conn);
        assert_eq!(tombstones(&mut conn, None, T + 5), 1);
        assert_eq!(
            player_columns(&conn, TOKEN),
            (Some(T + 5), false, false, None)
        );
        assert_eq!(
            pulled_key_of(&conn, SyncType::Match, TOKEN),
            Some(match_touch_key(&conn, match_id).expect("key"))
        );
        assert!(!match_candidate(&conn, TOKEN).owned, "left out of dirty");
        assert_eq!(tombstones(&mut conn, None, T + 6), 0, "hidden once");
        assert_eq!(player_columns(&conn, TOKEN).0, Some(T + 5));
    }

    // #830: a parked match deleted on the website is forgotten; another
    // parked match, and a synced row, stay.
    #[test]
    fn a_tombstone_forgets_a_parked_match() {
        let (_dir, mut conn) = db("sync-tombstone-parked");
        conn.execute_batch(
            "INSERT INTO synced_rows (type, client_id, parked_hash, parked_reason)
             VALUES ('match', 'q-unknown', 'h1', 'no_sources'),
                    ('match', 'q-kept', 'h2', 'no_sources');
             INSERT INTO synced_rows (type, client_id, hash) VALUES ('match', 'q-synced', 'h3');",
        )
        .expect("rows");
        let edit = SyncEdit::Tombstones {
            client_ids: vec!["q-unknown".to_string(), "q-synced".to_string()],
            since_ms: None,
            now_ms: T,
        };
        edit_sync(&mut conn, &edit).expect("tombstones");
        let left: Vec<String> = conn
            .prepare("SELECT client_id FROM synced_rows ORDER BY client_id")
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        assert_eq!(left, vec!["q-kept".to_string(), "q-synced".to_string()]);
    }

    // Deviation 7: a match restored here after the last run keeps its
    // restore; one edited before it is hidden.
    #[test]
    fn a_tombstone_leaves_a_match_restored_since_the_last_run() {
        let (_dir, mut conn) = db("sync-tombstone-restored");
        log_match_with_deck(&mut conn);
        conn.execute(
            "UPDATE matches SET edited_at = ?1 WHERE token = ?2",
            params![T + 10, TOKEN],
        )
        .expect("restored here");
        assert_eq!(tombstones(&mut conn, Some(T), T + 20), 0);
        assert_eq!(tombstones(&mut conn, None, T + 20), 0, "a full run too");
        assert_eq!(player_columns(&conn, TOKEN).0, None);
        assert_eq!(
            tombstones(&mut conn, Some(T + 15), T + 20),
            1,
            "deleted after the edit"
        );
        assert_eq!(player_columns(&conn, TOKEN).0, Some(T + 20));
    }

    fn league_token_of(conn: &Connection, token: &str) -> Option<String> {
        conn.query_row(
            "SELECT l.token FROM matches m LEFT JOIN leagues l ON l.id = m.league_id
             WHERE m.token = ?1",
            [token],
            |row| row.get(0),
        )
        .expect("match")
    }

    // League self-management spec 6.3: another device's newer edit reaches
    // the log match this device played, and is not sent back.
    #[test]
    fn a_newer_pulled_edit_lands_on_a_log_match_and_is_not_echoed() {
        let (_dir, mut conn) = db("sync-player-log-match");
        let match_id = log_match_with_deck(&mut conn);
        pull_match(
            &mut conn,
            edited(
                TOKEN,
                PulledPlayerFields {
                    hidden_at_ms: Some(T + 5),
                    no_league: true,
                    league_linked: false,
                    edited_at_ms: Some(T + 5),
                },
            ),
        );
        assert_eq!(
            player_columns(&conn, TOKEN),
            (Some(T + 5), true, false, Some(T + 5))
        );
        assert_eq!(
            pulled_key_of(&conn, SyncType::Match, TOKEN),
            Some(match_touch_key(&conn, match_id).expect("key"))
        );
        assert!(!match_candidate(&conn, TOKEN).owned, "not sent back");
    }

    /// Log matches "q-first" and `TOKEN` (not the run's first) of MTGO
    /// league `q-league-416`, folded into one log league; its row id.
    fn in_a_log_league(conn: &mut Connection) -> i64 {
        let mut first = crate::test_support::view("q-first", "Qopponent", T);
        first.mtgo_match_id = Some(98);
        first.league_token = Some("q-league-416".to_string());
        let mut second = crate::test_support::view(TOKEN, "Qopponent", T + 60_000);
        second.league_token = Some("q-league-416".to_string());
        crate::write::commit(
            conn,
            &crate::write::WriteBatch {
                matches: vec![
                    crate::write::MatchWrite::Upsert {
                        view: Box::new(first),
                        revision: 1,
                    },
                    crate::write::MatchWrite::Upsert {
                        view: Box::new(second),
                        revision: 1,
                    },
                ],
                ..crate::write::WriteBatch::default()
            },
        )
        .expect("log matches");
        crate::test_support::sync_leagues(conn);
        conn.query_row("SELECT id FROM leagues", [], |row| row.get(0))
            .expect("one log league")
    }

    /// The one log league marked Complete and deleted with "Delete them too"
    /// at `deleted_at`.
    fn delete_the_league_and_its_matches(conn: &mut Connection, deleted_at: i64) {
        use crate::leagues::{DeleteMatches, LeagueEdit, LeagueEditOutcome, UserState};
        let run_key: String = conn
            .query_row("SELECT run_key FROM leagues", [], |row| row.get(0))
            .expect("one log league");
        let edits = [
            LeagueEdit::SetStatus {
                run_key: run_key.clone(),
                status: Some(UserState::Complete),
                now_ms: deleted_at - 1,
            },
            LeagueEdit::Delete {
                run_key,
                matches: DeleteMatches::DeleteToo,
                now_ms: deleted_at,
            },
        ];
        for edit in edits {
            assert_eq!(
                crate::leagues::edit_league(conn, &edit, chrono_tz::UTC).expect("edit"),
                LeagueEditOutcome::Done
            );
        }
    }

    // Spec section 13: a pulled hide and a website tombstone record the
    // league the match was in here, before the league sync clears it.
    #[test]
    fn a_pulled_hide_and_a_tombstone_record_the_league_the_match_was_in() {
        let (_dir, mut conn) = db("sync-pulled-hide-records-league");
        let league_id = in_a_log_league(&mut conn);
        pull_match(
            &mut conn,
            edited(
                TOKEN,
                PulledPlayerFields {
                    hidden_at_ms: Some(T + 5),
                    no_league: false,
                    league_linked: false,
                    edited_at_ms: Some(T + 5),
                },
            ),
        );
        assert_eq!(player_columns(&conn, TOKEN).0, Some(T + 5));
        assert_eq!(deleted_from(&conn, TOKEN), Some(league_id));

        let (_dir_b, mut tombstoned) = db("sync-tombstone-records-league");
        let league_id = in_a_log_league(&mut tombstoned);
        assert_eq!(tombstones(&mut tombstoned, None, T + 5), 1);
        assert_eq!(player_columns(&tombstoned, TOKEN).0, Some(T + 5));
        assert_eq!(deleted_from(&tombstoned, TOKEN), Some(league_id));
    }

    /// A league's bundle as another device pulls it (`read_league_bundle`
    /// and the sync runner's `pulled_league`, without a deck or drafts):
    /// the league under its synthetic 0.x id, with its player fields.
    fn pulled_copy_of_league(conn: &Connection, league_id: i64) -> PulledLeague {
        let input = league_bundle_input(conn, league_id)
            .expect("input")
            .expect("a league");
        let client_id = league_client_id(&input.token, input.started_at_ms);
        PulledLeague {
            league: ImportedLeague {
                legacy_id: pulled_legacy_id(&client_id),
                manual: input.manual,
                kind: input.kind.clone(),
                set_code: input.set_code.clone(),
                course_id: input.mtgo_course_id,
                token: input.token.clone(),
                event_id: None,
                name: input.name.clone(),
                format: input.format.clone().unwrap_or_default(),
                state: input.state.clone(),
                started_at_ms: input.started_at_ms,
                joined_at_ms: input.joined_at_ms,
                completed_at_ms: input.completed_at_ms,
                dropped_at_ms: input.dropped_at_ms,
                created_at_ms: None,
                deck: None,
                notes: input.notes.clone(),
                matches: Vec::new(),
            },
            drafts: Vec::new(),
            decks: Vec::new(),
            player: PulledLeagueFields {
                removed_at_ms: input.removed_at_ms,
                user_state: input.user_state.clone(),
                user_dropped_at_ms: input.user_dropped_at_ms,
                edited_at_ms: input.edited_at_ms,
            },
        }
    }

    /// A match's bundle fields as another device pulls them: its player
    /// fields and league link.
    fn pulled_copy_of_match(conn: &Connection, token: &str) -> PulledMatch {
        let input = match_bundle_input(conn, match_id_of(conn, token), chrono_tz::UTC)
            .expect("input")
            .expect("a match");
        let mut pulled = edited(
            token,
            PulledPlayerFields {
                hidden_at_ms: input.hidden_at_ms,
                no_league: input.no_league,
                league_linked: input.league_linked,
                edited_at_ms: input.edited_at_ms,
            },
        );
        pulled.league_client_id = input.league_client_id;
        pulled
    }

    // Issue #418: a match deleted with its league cannot be restored on its
    // own. Device B, which never played the league, holds the league and
    // the match from an earlier pull; it then pulls the league's delete and
    // then the match (the sync runner's `SYNC_TYPES` order). Applying the
    // hide records B's own row of the league (spec section 13), so the
    // match is deleted with its league there too, and its Restore is
    // refused there as well.
    #[test]
    fn a_match_deleted_with_its_league_stays_deleted_with_it_on_both_devices() {
        let deleted_at = T + 10 * 60_000;
        let restored_at = deleted_at + 60_000;
        let (_dir_a, mut here) = db("sync-restore-removed-league-here");
        let (_dir_b, mut there) = db("sync-restore-removed-league-there");
        let league_id = in_a_log_league(&mut here);
        deck(&there, "111", true);
        pull_league(&mut there, pulled_copy_of_league(&here, league_id));
        pull_match(&mut there, pulled_copy_of_match(&here, TOKEN));
        let there_league: i64 = there
            .query_row("SELECT id FROM leagues", [], |row| row.get(0))
            .expect("the pulled league");
        assert_eq!(
            crate::test_support::number(
                &there,
                &format!("SELECT league_id FROM matches WHERE token = '{TOKEN}'")
            ),
            there_league,
            "in the pulled league"
        );

        delete_the_league_and_its_matches(&mut here, deleted_at);
        let restore = crate::match_edits::MatchEdit::Restore {
            token: TOKEN.to_string(),
            now_ms: restored_at,
        };
        assert_eq!(
            crate::match_edits::edit_match(&mut here, &restore).expect("restore"),
            crate::match_edits::MatchEditOutcome::DeletedWithLeague
        );

        pull_league(&mut there, pulled_copy_of_league(&here, league_id));
        let pulled = pulled_copy_of_match(&here, TOKEN);
        assert_eq!(
            (pulled.player.hidden_at_ms, pulled.player.edited_at_ms),
            (Some(deleted_at), Some(deleted_at)),
            "the refusal wrote nothing"
        );
        pull_match(&mut there, pulled);

        assert_eq!(player_columns(&there, TOKEN).0, Some(deleted_at));
        assert_eq!(player_columns(&there, TOKEN).3, Some(deleted_at));
        assert_eq!(
            deleted_from(&there, TOKEN),
            Some(there_league),
            "B records its own row"
        );
        let removed: Option<i64> = there
            .query_row("SELECT removed_at FROM leagues", [], |row| row.get(0))
            .expect("the pulled league");
        assert_eq!(removed, Some(deleted_at));
        let listed =
            crate::deleted::deleted_items(&there, crate::Platform::Mtgo).expect("deleted items");
        let row = listed
            .matches
            .iter()
            .find(|row| row.token == TOKEN)
            .expect("listed as deleted");
        assert!(row.deleted_with_league.is_some(), "deleted with its league");
        assert_eq!(
            crate::match_edits::edit_match(&mut there, &restore).expect("restore"),
            crate::match_edits::MatchEditOutcome::DeletedWithLeague
        );
        for conn in [&here, &there] {
            assert_eq!(
                crate::test_support::number(conn, "SELECT COUNT(*) FROM listed_leagues"),
                0,
                "no new league"
            );
            assert_eq!(
                crate::test_support::number(conn, "SELECT COUNT(*) FROM leagues"),
                1,
                "only the deleted one"
            );
        }
    }

    // Spec 4.5 and section 13 (pulled Restore): device B first sees the
    // league after A deleted it with "Delete them too", so B has no record
    // of it and restores one match. A pulls that restore (it cannot be
    // refused): the match follows A's restore rule and comes back as it was
    // just before the delete, a match of the deleted league (read as
    // non-league), with no new league row. The league's Restore on A then
    // brings it back whole.
    #[test]
    fn a_pulled_restore_of_a_match_deleted_with_its_league_stays_in_the_league() {
        let deleted_at = T + 10 * 60_000;
        let restored_at = deleted_at + 60_000;
        let (_dir_a, mut here) = db("sync-pulled-restore-deleted-league-here");
        let (_dir_b, mut there) = db("sync-pulled-restore-deleted-league-there");
        let league_id = in_a_log_league(&mut here);
        delete_the_league_and_its_matches(&mut here, deleted_at);

        deck(&there, "111", true);
        pull_league(&mut there, pulled_copy_of_league(&here, league_id));
        pull_match(&mut there, pulled_copy_of_match(&here, TOKEN));
        assert_eq!(deleted_from(&there, TOKEN), None, "first sight: no record");
        let restore = crate::match_edits::MatchEdit::Restore {
            token: TOKEN.to_string(),
            now_ms: restored_at,
        };
        assert_eq!(
            crate::match_edits::edit_match(&mut there, &restore).expect("restore"),
            crate::match_edits::MatchEditOutcome::Done
        );

        pull_match(&mut here, pulled_copy_of_match(&there, TOKEN));
        assert_eq!(
            player_columns(&here, TOKEN),
            (None, false, false, Some(restored_at)),
            "restored, in no other league"
        );
        assert_eq!(deleted_from(&here, TOKEN), None);
        assert_eq!(
            crate::test_support::number(
                &here,
                &format!("SELECT league_id FROM matches WHERE token = '{TOKEN}'")
            ),
            league_id,
            "a match of the deleted league again"
        );
        assert_eq!(
            crate::test_support::number(&here, "SELECT COUNT(*) FROM listed_leagues"),
            0,
            "no new league"
        );
        assert_eq!(
            crate::test_support::number(&here, "SELECT COUNT(*) FROM leagues"),
            1,
            "only the deleted one"
        );

        let run_key: String = here
            .query_row("SELECT run_key FROM leagues", [], |row| row.get(0))
            .expect("the league");
        let restore_league = crate::leagues::LeagueEdit::Restore {
            run_key,
            now_ms: restored_at + 60_000,
        };
        assert_eq!(
            crate::leagues::edit_league(&mut here, &restore_league, chrono_tz::UTC)
                .expect("restore"),
            crate::leagues::LeagueEditOutcome::Done
        );
        assert_eq!(
            crate::test_support::number(
                &here,
                &format!("SELECT COUNT(*) FROM listed_matches WHERE league_id = {league_id}")
            ),
            2,
            "both matches in the league, the same row"
        );
    }

    // Issue #416: a match linked by hand to a manual league, deleted on its
    // own, then restored after the league is deleted, comes back non-league
    // (`no_league = 1`). Device B, which pulled the link earlier, pulls the
    // league's delete and then the restored match's fields: the match is
    // non-league there too, and no league appears.
    #[test]
    fn a_match_restored_while_its_league_is_deleted_is_non_league_on_both_devices() {
        use crate::leagues::{DeleteMatches, LeagueEdit, LeagueEditOutcome};
        let linked_at = T + 60_000;
        let hidden_at = T + 2 * 60_000;
        let deleted_at = T + 3 * 60_000;
        let restored_at = T + 4 * 60_000;
        let manual_token = "manual_QqQqQqQqQqQqQqQqQqQqQqQq";
        let (_dir_a, mut here) = db("sync-restore-no-league-here");
        let (_dir_b, mut there) = db("sync-restore-no-league-there");
        for conn in [&mut here, &mut there] {
            deck(conn, "111", true);
            let mut league = pulled_league(manual_token, "constructed", true);
            league.league.deck = None;
            pull_league(conn, league);
            let mut linked = edited(
                TOKEN,
                PulledPlayerFields {
                    league_linked: true,
                    edited_at_ms: Some(linked_at),
                    ..PulledPlayerFields::default()
                },
            );
            linked.league_client_id = Some(league_client_id(manual_token, T));
            pull_match(conn, linked);
            assert_eq!(league_token_of(conn, TOKEN).as_deref(), Some(manual_token));
        }
        let league_id = league_id_of(&here, manual_token);
        let run_key: String = here
            .query_row(
                "SELECT run_key FROM leagues WHERE id = ?1",
                [league_id],
                |row| row.get(0),
            )
            .expect("run key");

        let hide = crate::match_edits::MatchEdit::Hide {
            token: TOKEN.to_string(),
            now_ms: hidden_at,
        };
        crate::match_edits::edit_match(&mut here, &hide).expect("hide");
        let delete = LeagueEdit::Delete {
            run_key,
            matches: DeleteMatches::KeepAsNonLeague,
            now_ms: deleted_at,
        };
        assert_eq!(
            crate::leagues::edit_league(&mut here, &delete, chrono_tz::UTC).expect("delete"),
            LeagueEditOutcome::Done
        );
        let restore = crate::match_edits::MatchEdit::Restore {
            token: TOKEN.to_string(),
            now_ms: restored_at,
        };
        assert_eq!(
            crate::match_edits::edit_match(&mut here, &restore).expect("restore"),
            crate::match_edits::MatchEditOutcome::Done
        );

        pull_league(&mut there, pulled_copy_of_league(&here, league_id));
        let pulled = pulled_copy_of_match(&here, TOKEN);
        assert_eq!(
            (
                pulled.player.hidden_at_ms,
                pulled.player.no_league,
                pulled.player.league_linked,
                pulled.player.edited_at_ms
            ),
            (None, true, false, Some(restored_at)),
            "the restore's bundle fields"
        );
        assert_eq!(pulled.league_client_id, None);
        pull_match(&mut there, pulled);

        for conn in [&here, &there] {
            assert_eq!(
                player_columns(conn, TOKEN),
                (None, true, false, Some(restored_at))
            );
            assert_eq!(league_token_of(conn, TOKEN), None);
            assert_eq!(
                crate::test_support::number(conn, "SELECT COUNT(*) FROM listed_leagues"),
                0,
                "no new league"
            );
            assert_eq!(
                crate::test_support::number(conn, "SELECT COUNT(*) FROM leagues"),
                1,
                "only the deleted one"
            );
        }
    }

    // Task 9 review: an edited row whose touch key is its pulled key is the
    // server's copy (not owned, not pushed); a local edit makes it owned.
    #[test]
    fn a_pulled_edit_is_not_owned_until_it_is_edited_here() {
        let (_dir, mut conn) = db("sync-player-owned-after-edit");
        deck(&conn, "111", true);
        pull_match(
            &mut conn,
            edited(
                "q-pulled",
                PulledPlayerFields {
                    hidden_at_ms: Some(T + 5),
                    edited_at_ms: Some(T + 5),
                    ..PulledPlayerFields::default()
                },
            ),
        );
        let match_id = match_id_of(&conn, "q-pulled");
        assert_eq!(
            pulled_key_of(&conn, SyncType::Match, "q-pulled"),
            Some(match_touch_key(&conn, match_id).expect("key"))
        );
        let pulled = match_candidate(&conn, "q-pulled");
        assert!(pulled.edited && !pulled.owned, "the server's copy");

        conn.execute(
            "UPDATE matches SET hidden_at = NULL, edited_at = ?1 WHERE token = 'q-pulled'",
            [T + 9],
        )
        .expect("restored here");
        assert!(
            match_candidate(&conn, "q-pulled").owned,
            "pushed once edited"
        );
    }

    // Spec 6.3: a local edit newer than the pulled copy stays, and the row
    // is marked changed so the next run uploads it; a tie changes nothing.
    #[test]
    fn a_newer_local_edit_stays_and_is_marked_for_upload() {
        let (_dir, mut conn) = db("sync-player-local-wins");
        log_match_with_deck(&mut conn);
        conn.execute(
            "UPDATE matches SET hidden_at = ?1, edited_at = ?1 WHERE token = ?2",
            params![T + 10, TOKEN],
        )
        .expect("deleted here");
        let older = PulledPlayerFields {
            edited_at_ms: Some(T + 5),
            ..PulledPlayerFields::default()
        };
        pull_match(&mut conn, edited(TOKEN, older));
        assert_eq!(
            player_columns(&conn, TOKEN),
            (Some(T + 10), false, false, Some(T + 10))
        );
        let row = synced_rows(&conn, SyncType::Match)
            .expect("rows")
            .remove(TOKEN)
            .expect("row");
        assert_eq!(row.changed_at, Some(T), "changed at the pull");
        assert_eq!((row.built_hash, row.pulled_key), (None, None));

        let tie = PulledPlayerFields {
            edited_at_ms: Some(T + 10),
            ..PulledPlayerFields::default()
        };
        pull_match(&mut conn, edited(TOKEN, tie));
        let row = synced_rows(&conn, SyncType::Match)
            .expect("rows")
            .remove(TOKEN)
            .expect("row");
        assert_eq!(row.changed_at, None, "a tie keeps the local row, unmarked");
        assert_eq!(player_columns(&conn, TOKEN).0, Some(T + 10));
    }

    /// `(removed_at, user_state, user_dropped_at, edited_at)` of a league.
    type LeaguePlayerColumns = (Option<i64>, Option<String>, Option<i64>, Option<i64>);

    // Spec 6.3: a league the player deleted on another device is removed
    // on the device that played it (a log row the import skips).
    #[test]
    fn a_newer_pulled_league_edit_removes_a_log_league() {
        let (_dir, mut conn) = db("sync-player-log-league");
        let deck_id = deck(&conn, "111", true);
        let client_id = league(&conn, "q-run", deck_id);
        let mut pulled = pulled_league("q-run", "constructed", false);
        pulled.player = PulledLeagueFields {
            removed_at_ms: Some(T + 5),
            user_state: Some("dropped".to_string()),
            user_dropped_at_ms: Some(T + 4),
            edited_at_ms: Some(T + 5),
        };
        pull_league(&mut conn, pulled);
        let (league_id, columns): (i64, LeaguePlayerColumns) = conn
            .query_row(
                "SELECT id, removed_at, user_state, user_dropped_at, edited_at FROM leagues
                 WHERE token = 'q-run' AND source = 'log'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        (row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?),
                    ))
                },
            )
            .expect("the log row");
        assert_eq!(
            columns,
            (
                Some(T + 5),
                Some("dropped".to_string()),
                Some(T + 4),
                Some(T + 5)
            )
        );
        assert_eq!(
            pulled_key_of(&conn, SyncType::League, &client_id),
            Some(league_own_key(&conn, league_id).expect("key"))
        );
    }

    // Spec 6.3: a pull that changed only the league link is applied (the
    // match itself is Unchanged).
    #[test]
    fn a_pull_that_changes_only_the_league_link_links_the_copy() {
        let (_dir, mut conn) = db("sync-link-only-pull");
        deck(&conn, "111", true);
        pull_league(&mut conn, pulled_league("q-run", "constructed", false));
        pull_match(&mut conn, pulled_match("q-pulled", "complete"));
        assert_eq!(league_token_of(&conn, "q-pulled"), None);
        let mut linked = pulled_match("q-pulled", "complete");
        linked.league_client_id = Some(league_client_id("q-run", T));
        pull_match(&mut conn, linked);
        assert_eq!(league_token_of(&conn, "q-pulled").as_deref(), Some("q-run"));
    }

    // Spec 4.6 and 6.3: a pulled link never overrides the player's choice;
    // only a newer edit does (through the player-field step).
    #[test]
    fn a_pulled_link_never_overrides_the_players_choice() {
        let (_dir, mut conn) = db("sync-link-player-choice");
        deck(&conn, "111", true);
        pull_league(&mut conn, pulled_league("q-run", "constructed", false));
        pull_match(&mut conn, pulled_match("q-pulled", "complete"));
        conn.execute(
            "UPDATE matches SET no_league = 1, edited_at = ?1 WHERE token = 'q-pulled'",
            [T + 1],
        )
        .expect("removed from leagues");
        let mut linked = pulled_match("q-pulled", "complete");
        linked.league_client_id = Some(league_client_id("q-run", T));
        pull_match(&mut conn, linked);
        assert_eq!(league_token_of(&conn, "q-pulled"), None);
        assert!(player_columns(&conn, "q-pulled").1, "still in no league");
    }

    // Spec 6.3: a pulled link reaches a log run of this device when the
    // move rules (4.1) allow it, and is dropped when they do not.
    #[test]
    fn a_pulled_link_reaches_a_log_run_only_within_the_move_rules() {
        let (_dir, mut conn) = db("sync-link-log-run");
        let deck_id = deck(&conn, "111", true);
        league(&conn, "q-run", deck_id);
        pull_match(&mut conn, pulled_match("q-pulled", "complete"));
        conn.execute(
            "UPDATE leagues SET account_id =
                 (SELECT account_id FROM matches WHERE token = 'q-pulled')",
            [],
        )
        .expect("same account");
        let mut linked = pulled_match("q-pulled", "complete");
        linked.league_client_id = Some(league_client_id("q-run", T));
        pull_match(&mut conn, linked);
        assert_eq!(league_token_of(&conn, "q-pulled").as_deref(), Some("q-run"));
    }

    // A hidden copy is judged as if listed, as `sync_player`'s links are
    // (`move_refusal_judged_listed`): the pulled link to a log run of this
    // device is accepted, the copy stays hidden, and the league it was
    // deleted from becomes that run (a hidden match given a league, spec
    // section 13).
    #[test]
    fn a_pulled_link_reaches_a_log_run_for_a_hidden_copy_too() {
        let (_dir, mut conn) = db("sync-link-log-run-hidden");
        let deck_id = deck(&conn, "111", true);
        league(&conn, "q-run", deck_id);
        pull_match(&mut conn, pulled_match("q-pulled", "complete"));
        conn.execute(
            "UPDATE leagues SET account_id =
                 (SELECT account_id FROM matches WHERE token = 'q-pulled')",
            [],
        )
        .expect("same account");
        conn.execute(
            "UPDATE matches SET hidden_at = ?1 WHERE token = 'q-pulled'",
            [T + 1],
        )
        .expect("deleted here");
        assert_eq!(deleted_from(&conn, "q-pulled"), None, "in no league");
        let mut linked = pulled_match("q-pulled", "complete");
        linked.league_client_id = Some(league_client_id("q-run", T));
        pull_match(&mut conn, linked);
        assert_eq!(league_token_of(&conn, "q-pulled").as_deref(), Some("q-run"));
        assert_eq!(
            player_columns(&conn, "q-pulled").0,
            Some(T + 1),
            "still hidden"
        );
        let run_id: i64 = conn
            .query_row("SELECT id FROM leagues WHERE token = 'q-run'", [], |row| {
                row.get(0)
            })
            .expect("the run");
        assert_eq!(deleted_from(&conn, "q-pulled"), Some(run_id));
    }

    #[test]
    fn a_pulled_link_to_another_decks_log_run_is_dropped() {
        let (_dir, mut conn) = db("sync-link-log-run-refused");
        deck(&conn, "111", true);
        let other = deck(&conn, "222", true);
        league(&conn, "q-run", other);
        pull_match(&mut conn, pulled_match("q-pulled", "complete"));
        conn.execute(
            "UPDATE leagues SET account_id =
                 (SELECT account_id FROM matches WHERE token = 'q-pulled')",
            [],
        )
        .expect("same account");
        let mut linked = pulled_match("q-pulled", "complete");
        linked.league_client_id = Some(league_client_id("q-run", T));
        pull_match(&mut conn, linked);
        assert_eq!(league_token_of(&conn, "q-pulled"), None);
    }

    /// Log runs "q-a" and "q-b" of deck "111" and run "q-other" of deck
    /// "222", all of the log match `TOKEN`'s account, which is linked into
    /// "q-a" (a fold would not put this bare match in a run).
    fn log_match_in_a_run(conn: &mut Connection) {
        log_match_with_deck(conn);
        let deck_id: i64 = conn
            .query_row("SELECT id FROM decks WHERE mtgo_id = '111'", [], |row| {
                row.get(0)
            })
            .expect("deck");
        let other = deck(conn, "222", true);
        league(conn, "q-a", deck_id);
        league(conn, "q-b", deck_id);
        league(conn, "q-other", other);
        conn.execute(
            "UPDATE leagues SET account_id = (SELECT account_id FROM matches WHERE token = ?1)",
            [TOKEN],
        )
        .expect("same account");
        conn.execute(
            "UPDATE matches SET league_linked = 1,
                 league_id = (SELECT id FROM leagues WHERE token = 'q-a')
             WHERE token = ?1",
            [TOKEN],
        )
        .expect("linked into q-a");
    }

    /// `TOKEN` pulled with a newer edit that links it to run `run` (and
    /// hides it when `hidden`).
    fn pull_link_edit(conn: &mut Connection, run: &str, hidden: bool) {
        let mut pulled = edited(
            TOKEN,
            PulledPlayerFields {
                hidden_at_ms: hidden.then_some(T + 5),
                league_linked: true,
                edited_at_ms: Some(T + 5),
                ..PulledPlayerFields::default()
            },
        );
        pulled.league_client_id = Some(league_client_id(run, T));
        pull_match(conn, pulled);
    }

    // Spec 6.3: a newer pulled move of a log match into another run of its
    // deck lands here.
    #[test]
    fn a_pulled_move_takes_a_log_match_into_another_run_of_its_deck() {
        let (_dir, mut conn) = db("sync-player-move");
        log_match_in_a_run(&mut conn);
        pull_link_edit(&mut conn, "q-b", false);
        assert_eq!(league_token_of(&conn, TOKEN).as_deref(), Some("q-b"));
        assert_eq!(
            player_columns(&conn, TOKEN),
            (None, false, true, Some(T + 5))
        );
    }

    // Spec 4.1 and 6.3: a pulled move the move rules refuse (another deck's
    // run) is dropped; the match keeps its league.
    #[test]
    fn a_pulled_move_into_another_decks_run_is_refused() {
        let (_dir, mut conn) = db("sync-player-move-refused");
        log_match_in_a_run(&mut conn);
        pull_link_edit(&mut conn, "q-other", false);
        assert_eq!(league_token_of(&conn, TOKEN).as_deref(), Some("q-a"));
        assert_eq!(
            player_columns(&conn, TOKEN).3,
            Some(T + 5),
            "the edit lands"
        );
    }

    // Task 10 review: a match moved and then deleted on another device
    // arrives in one bundle; the move is judged before the hide, so the
    // match lands hidden in the new run. The league is written before the
    // hide, so the match records the new run as the league it was deleted
    // from (spec section 13, write order).
    #[test]
    fn a_pulled_move_then_delete_lands_hidden_in_the_new_run() {
        let (_dir, mut conn) = db("sync-player-move-hidden");
        log_match_in_a_run(&mut conn);
        pull_link_edit(&mut conn, "q-b", true);
        assert_eq!(league_token_of(&conn, TOKEN).as_deref(), Some("q-b"));
        assert_eq!(
            player_columns(&conn, TOKEN),
            (Some(T + 5), false, true, Some(T + 5))
        );
        let run_b: i64 = conn
            .query_row("SELECT id FROM leagues WHERE token = 'q-b'", [], |row| {
                row.get(0)
            })
            .expect("run q-b");
        assert_eq!(deleted_from(&conn, TOKEN), Some(run_b));
    }

    // The same for a match hidden here that another device restored and
    // moved: the move is judged with the match listed.
    #[test]
    fn a_pulled_restore_and_move_lands_in_the_new_run() {
        let (_dir, mut conn) = db("sync-player-restore-move");
        log_match_in_a_run(&mut conn);
        conn.execute(
            "UPDATE matches SET hidden_at = ?1, edited_at = ?1 WHERE token = ?2",
            params![T + 1, TOKEN],
        )
        .expect("deleted here");
        pull_link_edit(&mut conn, "q-b", false);
        assert_eq!(league_token_of(&conn, TOKEN).as_deref(), Some("q-b"));
        assert_eq!(player_columns(&conn, TOKEN).0, None, "restored");
    }

    // Final review finding 3: a match hidden before this release has a
    // `hidden_at` but no edit time. Its bundle still reaches a device where
    // the match is listed: the hide lands once, without an edit time, and
    // is not sent back. A listed copy never unhides a local hide.
    #[test]
    fn a_hide_without_an_edit_time_lands_once_and_is_not_echoed() {
        let (_dir, mut conn) = db("sync-player-old-hide");
        let match_id = log_match_with_deck(&mut conn);
        let old_hide = PulledPlayerFields {
            hidden_at_ms: Some(T + 5),
            ..PulledPlayerFields::default()
        };
        pull_match(&mut conn, edited(TOKEN, old_hide));
        assert_eq!(
            player_columns(&conn, TOKEN),
            (Some(T + 5), false, false, None)
        );
        assert_eq!(
            pulled_key_of(&conn, SyncType::Match, TOKEN),
            Some(match_touch_key(&conn, match_id).expect("key"))
        );
        assert!(!match_candidate(&conn, TOKEN).owned, "not sent back");

        pull_match(&mut conn, edited(TOKEN, PulledPlayerFields::default()));
        assert_eq!(
            player_columns(&conn, TOKEN).0,
            Some(T + 5),
            "a listed copy without an edit time leaves the hide"
        );
    }

    // Final review finding 5: a newer pulled edit with no link and no
    // `no_league` (the other device let the fold place the match) clears a
    // player link here; the fold places the match again.
    #[test]
    fn a_newer_pulled_edit_without_a_link_clears_a_player_link() {
        let (_dir, mut conn) = db("sync-player-unlink");
        log_match_in_a_run(&mut conn);
        conn.execute(
            "UPDATE matches SET edited_at = ?1 WHERE token = ?2",
            params![T + 1, TOKEN],
        )
        .expect("moved here");
        pull_match(
            &mut conn,
            edited(
                TOKEN,
                PulledPlayerFields {
                    edited_at_ms: Some(T + 5),
                    ..PulledPlayerFields::default()
                },
            ),
        );
        assert_eq!(
            player_columns(&conn, TOKEN),
            (None, false, false, Some(T + 5))
        );
        assert_eq!(league_token_of(&conn, TOKEN), None, "placed by the fold");
    }

    // A link without an edit time (the 0.x import's) is not the player's:
    // a newer pulled edit without a link leaves it.
    #[test]
    fn a_newer_pulled_edit_without_a_link_keeps_an_imported_link() {
        let (_dir, mut conn) = db("sync-player-keep-import-link");
        log_match_in_a_run(&mut conn);
        pull_match(
            &mut conn,
            edited(
                TOKEN,
                PulledPlayerFields {
                    edited_at_ms: Some(T + 5),
                    ..PulledPlayerFields::default()
                },
            ),
        );
        assert_eq!(
            player_columns(&conn, TOKEN),
            (None, false, true, Some(T + 5))
        );
        assert_eq!(league_token_of(&conn, TOKEN).as_deref(), Some("q-a"));
    }

    // Review focus 5: a delete and its Undo in the same second reach the
    // other device (this one, which played the league: a log row) in order,
    // and the league ends live here too. Player times are integer ms (spec
    // 6.1), so the two edits never tie. A late copy of the delete, and the
    // Undo pulled again (the device that made it pulls its own copy back:
    // a tie), change nothing.
    #[test]
    fn an_undo_in_the_same_second_wins_on_the_other_device() {
        let (_dir, mut conn) = db("sync-player-undo-same-second");
        let deck_id = deck(&conn, "111", true);
        league(&conn, "q-run", deck_id);
        let deleted_at = T + 1_000;
        let undone_at = deleted_at + 400;
        assert_eq!(deleted_at / 1_000, undone_at / 1_000, "the same second");
        let pulled = |player: PulledLeagueFields| {
            let mut copy = pulled_league("q-run", "constructed", false);
            copy.player = player;
            copy
        };
        let delete = || PulledLeagueFields {
            removed_at_ms: Some(deleted_at),
            edited_at_ms: Some(deleted_at),
            ..PulledLeagueFields::default()
        };
        let undo = || PulledLeagueFields {
            edited_at_ms: Some(undone_at),
            ..PulledLeagueFields::default()
        };
        let removed = |conn: &Connection| -> (Option<i64>, Option<i64>) {
            conn.query_row(
                "SELECT removed_at, edited_at FROM leagues
                 WHERE token = 'q-run' AND source = 'log'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("the log row")
        };

        pull_league(&mut conn, pulled(delete()));
        assert_eq!(removed(&conn), (Some(deleted_at), Some(deleted_at)));
        pull_league(&mut conn, pulled(undo()));
        assert_eq!(removed(&conn), (None, Some(undone_at)), "the Undo wins");
        let listed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM listed_leagues WHERE token = 'q-run' AND source = 'log'",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(listed, 1, "listed again");

        pull_league(&mut conn, pulled(delete()));
        assert_eq!(
            removed(&conn),
            (None, Some(undone_at)),
            "a late delete loses"
        );
        pull_league(&mut conn, pulled(undo()));
        assert_eq!(
            removed(&conn),
            (None, Some(undone_at)),
            "a tie changes nothing"
        );
    }

    // ---- Sync identity (sync identity and cursor spec, section 2) --------

    const HOUR: i64 = 3_600_000;

    /// The stored sync client id of each league row, in row order.
    fn sync_ids(conn: &Connection) -> Vec<Option<String>> {
        conn.prepare("SELECT sync_client_id FROM leagues ORDER BY id")
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect()
    }

    /// The league row `token` is linked to.
    fn league_of(conn: &Connection, token: &str) -> Option<i64> {
        conn.query_row(
            "SELECT league_id FROM matches WHERE token = ?1",
            [token],
            |row| row.get(0),
        )
        .expect("match")
    }

    // Spec 2.4: a first-match rekey moves `started_at` but keeps the
    // league's sync client id, so neither the league nor its matches change
    // identity on the server.
    #[test]
    fn a_rekeyed_league_keeps_its_sync_client_id() {
        let (_dir, mut conn) = db("sync-rekeyed-league-id");
        let mut matches = Vec::new();
        for (index, token) in ["q-m1", "q-m2", "q-m3"].into_iter().enumerate() {
            let mut played = crate::test_support::view(token, "Qopponent", T + index as i64 * HOUR);
            played.league_token = Some("q-league-431".to_string());
            played.mtgo_match_id = None;
            for game in &mut played.games {
                game.mtgo_game_id = None;
            }
            matches.push(crate::write::MatchWrite::Upsert {
                view: Box::new(played),
                revision: 1,
            });
        }
        crate::write::commit(
            &mut conn,
            &crate::write::WriteBatch {
                matches,
                ..crate::write::WriteBatch::default()
            },
        )
        .expect("log matches");
        let deck_id = deck(&conn, "111", true);
        let version_id = version(&conn, deck_id, "QsigV1", T);
        conn.execute("UPDATE matches SET deck_version_id = ?1", [version_id])
            .expect("their deck");
        crate::test_support::sync_leagues(&conn);
        let before = league_candidates(&conn, Tier::Supporter).expect("candidates");
        assert_eq!(before.len(), 1);
        let client_id = before[0].client_id.clone();
        assert_eq!(client_id, league_client_id("q-league-431", T));
        store_hash(&mut conn, SyncType::League, &client_id, "h-league");
        let hashes = |conn: &Connection| {
            vec![
                hash_of(conn, match_id_of(conn, "q-m2")),
                hash_of(conn, match_id_of(conn, "q-m3")),
            ]
        };
        let hashes_before = hashes(&conn);

        // The first match goes on its own: the run is rekeyed to the second.
        crate::test_support::hide(&conn, "q-m1");
        crate::test_support::sync_leagues(&conn);
        let started_at: i64 = conn
            .query_row("SELECT started_at FROM leagues", [], |row| row.get(0))
            .expect("one league");
        assert_eq!(started_at, T + HOUR, "the run starts at its second match");

        let after: Vec<String> = league_candidates(&conn, Tier::Supporter)
            .expect("candidates")
            .into_iter()
            .map(|candidate| candidate.client_id)
            .collect();
        assert_eq!(after, vec![client_id.clone()], "no new league id to upload");
        assert_eq!(
            hashes(&conn),
            hashes_before,
            "the members' bundles are unchanged"
        );
        let input = match_bundle_input(&conn, match_id_of(&conn, "q-m2"), chrono_tz::UTC)
            .expect("input")
            .expect("a match");
        assert_eq!(input.league_client_id, Some(client_id));
    }

    // Spec 2.2: a pulled league stores the id its bundle came with, even
    // when it disagrees with the start; a newer pull updates that row.
    #[test]
    fn a_pulled_league_keeps_the_client_id_it_came_with() {
        let (_dir, mut conn) = db("sync-pulled-league-given-id");
        deck(&conn, "111", true);
        // The other device rekeyed its run: the id keeps the old start.
        let client_id = league_client_id("q-run", T - HOUR);
        let mut pulled = pulled_league("q-run", "constructed", false);
        pulled.league.legacy_id = pulled_legacy_id(&client_id);
        pull_league_as(&mut conn, &client_id, pulled.clone());
        assert_eq!(sync_ids(&conn), vec![Some(client_id.clone())]);

        let mut newer = pulled;
        newer.player = PulledLeagueFields {
            removed_at_ms: Some(T + 5),
            edited_at_ms: Some(T + 5),
            ..PulledLeagueFields::default()
        };
        pull_league_as(&mut conn, &client_id, newer.clone());
        assert_eq!(sync_ids(&conn), vec![Some(client_id.clone())], "one row");

        // The other device rekeys its run again: same id, a later start.
        newer.league.started_at_ms = T + HOUR;
        pull_league_as(&mut conn, &client_id, newer);
        assert_eq!(
            sync_ids(&conn),
            vec![Some(client_id.clone())],
            "still one row"
        );
        let removed: Option<i64> = conn
            .query_row("SELECT removed_at FROM leagues", [], |row| row.get(0))
            .expect("the row");
        assert_eq!(removed, Some(T + 5), "the player fields found it by its id");
    }

    // Spec 2.3: a pulled match names its league by the stored id first,
    // then (an id from a device that has not updated) by token and start.
    #[test]
    fn a_match_names_its_league_by_the_stored_id() {
        let (_dir, mut conn) = db("sync-match-league-by-stored-id");
        deck(&conn, "111", true);
        let client_id = league_client_id("q-run", T - HOUR);
        let mut pulled = pulled_league("q-run", "constructed", false);
        pulled.league.legacy_id = pulled_legacy_id(&client_id);
        pull_league_as(&mut conn, &client_id, pulled);
        let league_id: i64 = conn
            .query_row("SELECT id FROM leagues", [], |row| row.get(0))
            .expect("the pulled league");

        let mut by_id = pulled_match("q-by-id", "complete");
        by_id.league_client_id = Some(client_id);
        pull_match(&mut conn, by_id);
        assert_eq!(league_of(&conn, "q-by-id"), Some(league_id));

        let mut by_start = pulled_match("q-by-start", "complete");
        by_start.league_client_id = Some(league_client_id("q-run", T));
        pull_match(&mut conn, by_start);
        assert_eq!(
            league_of(&conn, "q-by-start"),
            Some(league_id),
            "the fallback"
        );
    }

    #[test]
    fn manual_league_id_finds_the_stored_id_then_the_token_and_start() {
        let (_dir, mut conn) = db("sync-manual-league-by-stored-id");
        insert_manual_league(&conn, 3, None, None, "Qmine", T, T);
        let token: String = conn
            .query_row("SELECT token FROM leagues", [], |row| row.get(0))
            .expect("token");
        conn.execute(
            "UPDATE leagues SET sync_client_id = 'manual_Qgiven_20270101T000000Z'",
            [],
        )
        .expect("an id that disagrees with the start");
        let tx = conn.transaction().expect("transaction");
        assert_eq!(
            manual_league_id(&tx, "manual_Qgiven_20270101T000000Z").expect("by id"),
            Some(3)
        );
        assert_eq!(
            manual_league_id(&tx, &league_client_id(&token, T)).expect("by start"),
            Some(3),
            "the fallback"
        );
        assert_eq!(
            manual_league_id(&tx, "manual_Qother_20270101T000000Z").expect("unknown"),
            None
        );
    }

    #[test]
    fn pulled_league_id_finds_the_stored_id_then_the_token_and_start() {
        let (_dir, mut conn) = db("sync-pulled-league-id-by-stored-id");
        insert_manual_league(&conn, 3, None, None, "Qmine", T, T);
        let token: String = conn
            .query_row("SELECT token FROM leagues", [], |row| row.get(0))
            .expect("token");
        conn.execute(
            "UPDATE leagues SET sync_client_id = 'manual_Qgiven_20270101T000000Z'",
            [],
        )
        .expect("an id that disagrees with the start");
        let tx = conn.transaction().expect("transaction");
        let find = |client_id: &str| {
            crate::import::pulled_league_id(&tx, Some(client_id)).expect("lookup")
        };
        assert_eq!(find("manual_Qgiven_20270101T000000Z"), Some(3));
        assert_eq!(find(&league_client_id(&token, T)), Some(3), "the fallback");
        assert_eq!(find("manual_Qother_20270101T000000Z"), None);
    }

    #[test]
    fn apply_league_fields_finds_the_stored_id_then_the_token_and_start() {
        let (_dir, mut conn) = db("sync-league-fields-by-stored-id");
        let deck_id = deck(&conn, "111", true);
        league(&conn, "q-run", deck_id);
        conn.execute(
            "UPDATE leagues SET sync_client_id = 'q-run_20270101T000000Z'",
            [],
        )
        .expect("an id that disagrees with the start");
        let removed = |conn: &Connection| -> Option<i64> {
            conn.query_row("SELECT removed_at FROM leagues", [], |row| row.get(0))
                .expect("the row")
        };
        let apply = |conn: &mut Connection, client_id: &str, fields: PulledLeagueFields| {
            let tx = conn.transaction().expect("transaction");
            crate::sync_player::apply_league_fields(&tx, client_id, &fields, chrono_tz::UTC, T)
                .expect("apply");
            tx.commit().expect("commit");
        };
        apply(
            &mut conn,
            "q-run_20270101T000000Z",
            PulledLeagueFields {
                removed_at_ms: Some(T + 5),
                edited_at_ms: Some(T + 5),
                ..PulledLeagueFields::default()
            },
        );
        assert_eq!(removed(&conn), Some(T + 5), "by its stored id");
        apply(
            &mut conn,
            &league_client_id("q-run", T),
            PulledLeagueFields {
                edited_at_ms: Some(T + 6),
                ..PulledLeagueFields::default()
            },
        );
        assert_eq!(removed(&conn), None, "by token and start, the fallback");
    }

    #[test]
    fn pulled_mtgo_bundle_without_platform_imports_as_before() {
        let (_dir, mut conn) = db("sync-pulled-mtgo-platform");
        let deck_id = deck(&conn, "111", true);
        version(&conn, deck_id, "QsigV1", T);
        pull_match(&mut conn, pulled_match("q-pulled", "complete"));
        let (source, platform): (String, String) = conn
            .query_row(
                "SELECT m.source, a.platform FROM matches m JOIN accounts a ON a.id = m.account_id
                 WHERE m.token = 'q-pulled'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("pulled");
        assert_eq!((source.as_str(), platform.as_str()), ("imported", "mtgo"));
        assert!(
            crate::test_support::number(&conn, "SELECT COUNT(*) FROM game_cards") > 0,
            "the MTGO detail is stored as before"
        );
        assert_eq!(
            crate::test_support::number(&conn, "SELECT COUNT(*) FROM synced_sources"),
            0
        );
        let signature: String = conn
            .query_row(
                "SELECT v.signature FROM matches m JOIN deck_versions v ON v.id = m.deck_version_id",
                [],
                |row| row.get(0),
            )
            .expect("linked");
        assert_eq!(signature, "QsigV2", "the signature is taken as it is");
    }
}
