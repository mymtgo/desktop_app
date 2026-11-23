//! 0.x drafts, picks, pick notes and limited registered decks imported into
//! v1 (limited spec section 7, target schema section 9). Runs after the
//! matches and the leagues, so a draft finds its league's imported run and a
//! registered deck its match.
//!
//! - A draft becomes a `drafts` row with `source = 'imported'` and
//!   `imported_from` = the 0.x id (the next free id when another 0.x
//!   database took it), with its picks in `draft_picks`. It is found again
//!   by its token and 0.x id (`legacy_draft_id`). A draft another 0.x
//!   database brought (its token under another 0.x id) is kept as it is:
//!   only notes its picks lack are added (issue #194). Its account
//!   is its run's, else the only account; its league event id and course id
//!   are its 0.x league's (0.x kept them on the league; v1 keeps them on the
//!   draft). A draft v1 has from the log is never written: the log wins.
//! - A pick's note goes into `draft_picks.note`, only where the pick has none
//!   (a note written in v1 is never overwritten), trimmed and cut to 2000
//!   characters as a v1 edit is. The picks of a log draft take 0.x's notes
//!   the same way.
//! - A registered deck (0.x `limited_deck_snapshots`, or the match's limited
//!   deck version when it has none) becomes the `imported` snapshot of its
//!   v1 match: the live row when v1 tracked that match live, else the
//!   imported one; the newest wins. One without an imported run, or whose
//!   match v1 does not have, is left out.
//!
//! A 0.x league id (a draft's or a registered deck's) names the imported
//! run of that league in this 0.x database: `ImportedLimited::league_keys`
//! finds it by its natural key, since a second 0.x database reuses the ids.
//!
//! The league sync writes only `state` (the 30-minute rule), `abandons_at`
//! and `league_id` of an imported draft. A rerun updates what 0.x changed
//! (clearing `abandons_at`, which the next sync sets again) and writes
//! nothing when nothing did.

use std::collections::{BTreeMap, HashMap};

use limited::{Draft, RegisteredCard};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::StoreError;
use crate::cards::{CardRef, ensure_stubs};
use crate::import::live_row_token;
use crate::import_leagues::{LeagueKey, find_imported_run, only_account};
use crate::limited::{
    MAX_NOTE_CHARS, cards_json, clamp_cards, draft_state_str, registered, reservations_json,
};

/// One 0.x draft with its picks and their notes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyDraft {
    /// The 0.x `drafts.id`.
    pub legacy_id: i64,
    /// The 0.x `drafts.league_id`.
    pub league_id: Option<i64>,
    /// The draft as 0.x stored it. Its league event id and course id are
    /// not stored: 0.x kept them on the league, and they are read from the
    /// imported run.
    pub draft: Draft,
    /// `(ordinal, note)` of the picks with a note (0.x `draft_picks.note`).
    pub notes: Vec<(u32, String)>,
}

/// The deck MTGO registered for one 0.x limited match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyLimitedDeck {
    /// The 0.x league (an imported run's id).
    pub league_id: i64,
    pub match_token: String,
    pub mtgo_match_id: Option<i64>,
    pub cards: Vec<RegisteredCard>,
    pub captured_at_ms: i64,
}

/// What `import_limited` stores.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportedLimited {
    pub drafts: Vec<LegacyDraft>,
    pub decks: Vec<LegacyLimitedDeck>,
    /// The 0.x database's leagues, by which a 0.x league id finds its
    /// imported run (a league not here was not imported). `None`: the
    /// league ids are already run `imported_from` ids (a pulled league).
    pub league_keys: Option<Vec<LeagueKey>>,
}

/// What one limited import did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LimitedImportCounts {
    pub drafts_added: u32,
    pub drafts_updated: u32,
    pub drafts_unchanged: u32,
    /// Left as they are: v1 has the draft from the log.
    pub drafts_from_log: u32,
    /// Stored with no account: no run with one, and more than one account.
    pub drafts_without_account: u32,
    /// Notes added; a note v1 already has for that pick is kept.
    pub notes_added: u32,
    /// Left out: v1's draft (from the log) has no such pick yet. A rerun
    /// after the pick is read adds the note.
    pub notes_without_pick: u32,
    pub decks_added: u32,
    pub decks_updated: u32,
    pub decks_unchanged: u32,
    /// Left out: their 0.x league was not imported.
    pub decks_without_run: u32,
    /// Left out: v1 has no row for their match.
    pub decks_without_match: u32,
}

/// Write the drafts, their picks and notes, and the registered decks in one
/// transaction.
pub fn import_limited(
    conn: &mut Connection,
    data: &ImportedLimited,
    now_ms: i64,
) -> Result<LimitedImportCounts, StoreError> {
    let tx = conn.transaction()?;
    let counts = import_limited_rows(&tx, data, now_ms)?;
    tx.commit()?;
    Ok(counts)
}

/// `import_limited` inside a transaction the caller owns (a pulled league,
/// 07 spec section 10).
pub(crate) fn import_limited_rows(
    tx: &Transaction,
    data: &ImportedLimited,
    now_ms: i64,
) -> Result<LimitedImportCounts, StoreError> {
    let only = only_account(tx)?;
    let mut counts = LimitedImportCounts::default();
    for legacy in &data.drafts {
        let league = match legacy.league_id {
            Some(league_id) => run_imported_from(tx, league_id, data.league_keys.as_deref())?,
            None => None,
        };
        import_draft(tx, legacy, league, only, now_ms, &mut counts)?;
    }
    import_decks(tx, &data.decks, data.league_keys.as_deref(), &mut counts)?;
    Ok(counts)
}

/// The `imported_from` of the imported run of 0.x league `league_id`: found
/// by the league's natural key in `keys`, or `league_id` itself when there
/// are no keys (a pulled league). `None` when the league was not imported.
fn run_imported_from(
    tx: &Transaction,
    league_id: i64,
    keys: Option<&[LeagueKey]>,
) -> Result<Option<i64>, StoreError> {
    let Some(keys) = keys else {
        return Ok(Some(league_id));
    };
    let Some(key) = keys.iter().find(|key| key.legacy_id == league_id) else {
        return Ok(None);
    };
    Ok(find_imported_run(tx, key)?.map(|(_, imported_from, _)| imported_from))
}

/// The `imported_from` a new draft row takes: its 0.x id, unless a draft
/// from another 0.x database has it, then the next free id.
fn free_draft_id(tx: &Transaction, legacy_id: i64) -> Result<i64, StoreError> {
    let taken: bool = tx
        .prepare_cached("SELECT EXISTS (SELECT 1 FROM drafts WHERE imported_from = ?1)")?
        .query_row([legacy_id], |row| row.get(0))?;
    if !taken {
        return Ok(legacy_id);
    }
    let highest: i64 = tx
        .prepare_cached("SELECT MAX(imported_from) FROM drafts")?
        .query_row([], |row| row.get(0))?;
    Ok(highest.max(legacy_id) + 1)
}

/// The columns of an imported draft the import compares and writes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DraftFields {
    draft_token: String,
    account_id: Option<i64>,
    league_event_id: Option<i64>,
    mtgo_course_id: Option<i64>,
    mtgo_draft_id: Option<i64>,
    pod_token: Option<String>,
    seat_count: i64,
    seat_index: Option<i64>,
    booster_catalog_id: Option<i64>,
    state: String,
    pack_size: i64,
    picks_expected: i64,
    started_at: Option<i64>,
    ended_at: Option<i64>,
    imported_league_from: Option<i64>,
}

impl DraftFields {
    /// Whether the stored row says what 0.x says. A draft the sync abandoned
    /// (the 30-minute rule) while 0.x left it open is the same draft.
    fn same_as(&self, wanted: &DraftFields) -> bool {
        let open = wanted.state == "connecting" || wanted.state == "picking";
        let state_kept = self.state == wanted.state || (open && self.state == "abandoned");
        let mut stored = self.clone();
        stored.state = wanted.state.clone();
        state_kept && stored == *wanted
    }
}

/// The columns of an imported pick the import compares and writes (never
/// its note).
#[derive(Debug, Clone, PartialEq, Eq)]
struct PickFields {
    pack_number: i64,
    pick_number: i64,
    pack_id: Option<i64>,
    direction: Option<i64>,
    cards_available: String,
    picked_catalog_id: Option<i64>,
    picked_card_id: Option<i64>,
    reservations: String,
    shown_at: Option<i64>,
    deadline_at: Option<i64>,
    picked_at: Option<i64>,
}

/// A fold value as a stored integer; one too large reads as none.
fn signed(value: Option<u64>) -> Option<i64> {
    value.and_then(|value| i64::try_from(value).ok())
}

/// One pick per ordinal (0.x: unique), by ordinal.
fn wanted_picks(draft: &Draft) -> Result<BTreeMap<i64, PickFields>, StoreError> {
    let mut by_ordinal: BTreeMap<i64, PickFields> = BTreeMap::new();
    for pick in &draft.picks {
        if pick.ordinal == 0 {
            continue;
        }
        by_ordinal.insert(
            i64::from(pick.ordinal),
            PickFields {
                pack_number: i64::from(pick.pack_number),
                pick_number: i64::from(pick.pick_number),
                pack_id: signed(pick.pack_id),
                direction: pick.direction.map(i64::from),
                cards_available: serde_json::to_string(&pick.cards_available)?,
                picked_catalog_id: pick.picked_catalog_id.map(i64::from),
                picked_card_id: signed(pick.picked_card_id),
                reservations: reservations_json(&pick.reservations)?,
                shown_at: pick.shown_at_ms,
                deadline_at: pick.deadline_at_ms,
                picked_at: pick.picked_at_ms,
            },
        );
    }
    Ok(by_ordinal)
}

/// A stored draft the import may meet: its id, source, `imported_from`,
/// 0.x draft id and fields.
struct StoredDraftRow {
    id: i64,
    source: String,
    imported_from: Option<i64>,
    legacy_draft_id: Option<i64>,
    fields: DraftFields,
}

impl StoredDraftRow {
    /// Imported from this 0.x draft (its 0.x id; a row stored before the
    /// 0.x id was kept by `imported_from`), or a pulled copy (negative
    /// `imported_from`), which a 0.x draft (positive id) takes over whatever
    /// its stored 0.x id. Any other imported row with the token came from
    /// another 0.x database.
    fn is_own(&self, legacy_id: i64) -> bool {
        let pulled_copy = self.imported_from.is_some_and(|id| id < 0) && legacy_id > 0;
        pulled_copy
            || match (self.legacy_draft_id, self.imported_from) {
                (Some(id), _) => id == legacy_id,
                (None, Some(id)) => id == legacy_id,
                (None, None) => true,
            }
    }
}

/// The stored draft with this 0.x draft's token (from the log, imported
/// from this 0.x draft, or from another 0.x database or a pull).
fn stored_draft(tx: &Transaction, token: &str) -> Result<Option<StoredDraftRow>, StoreError> {
    Ok(tx
        .prepare_cached(
            "SELECT id, source, draft_token, account_id, league_event_id, mtgo_course_id,
                 mtgo_draft_id, pod_token, seat_count, seat_index, booster_catalog_id, state,
                 pack_size, picks_expected, started_at, ended_at, imported_league_from,
                 imported_from, legacy_draft_id
             FROM drafts WHERE draft_token = ?1",
        )?
        .query_row([token], |row| {
            Ok(StoredDraftRow {
                id: row.get(0)?,
                source: row.get(1)?,
                imported_from: row.get(17)?,
                legacy_draft_id: row.get(18)?,
                fields: DraftFields {
                    draft_token: row.get(2)?,
                    account_id: row.get(3)?,
                    league_event_id: row.get(4)?,
                    mtgo_course_id: row.get(5)?,
                    mtgo_draft_id: row.get(6)?,
                    pod_token: row.get(7)?,
                    seat_count: row.get(8)?,
                    seat_index: row.get(9)?,
                    booster_catalog_id: row.get(10)?,
                    state: row.get(11)?,
                    pack_size: row.get(12)?,
                    picks_expected: row.get(13)?,
                    started_at: row.get(14)?,
                    ended_at: row.get(15)?,
                    imported_league_from: row.get(16)?,
                },
            })
        })
        .optional()?)
}

/// A stored pick's fields and whether it is detached and noted.
struct StoredPick {
    fields: PickFields,
    detached: bool,
    noted: bool,
}

fn stored_picks(tx: &Transaction, draft_id: i64) -> Result<BTreeMap<i64, StoredPick>, StoreError> {
    let mut stmt = tx.prepare_cached(
        "SELECT ordinal, pack_number, pick_number, pack_id, direction, cards_available,
             picked_catalog_id, picked_card_id, reservations, shown_at, deadline_at, picked_at,
             detached_at IS NOT NULL, note IS NOT NULL
         FROM draft_picks WHERE draft_id = ?1",
    )?;
    let rows = stmt.query_map([draft_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            StoredPick {
                fields: PickFields {
                    pack_number: row.get(1)?,
                    pick_number: row.get(2)?,
                    pack_id: row.get(3)?,
                    direction: row.get(4)?,
                    cards_available: row.get(5)?,
                    picked_catalog_id: row.get(6)?,
                    picked_card_id: row.get(7)?,
                    reservations: row.get(8)?,
                    shown_at: row.get(9)?,
                    deadline_at: row.get(10)?,
                    picked_at: row.get(11)?,
                },
                detached: row.get(12)?,
                noted: row.get(13)?,
            },
        ))
    })?;
    let mut picks = BTreeMap::new();
    for row in rows {
        let (ordinal, pick) = row?;
        picks.insert(ordinal, pick);
    }
    Ok(picks)
}

/// The imported run (0.x league): `None` when it was not imported, else its
/// account, league event id and course id (from its seed).
type RunFacts = (Option<i64>, Option<i64>, Option<i64>);

fn run_facts(tx: &Transaction, league_id: i64) -> Result<Option<RunFacts>, StoreError> {
    Ok(tx
        .prepare_cached(
            "SELECT account_id, json_extract(imported_seed, '$.event_id'),
                 json_extract(imported_seed, '$.mtgo_course_id')
             FROM leagues WHERE source = 'imported' AND imported_from = ?1",
        )?
        .query_row([league_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()?)
}

/// What the v1 draft row should hold for this 0.x draft: its own facts, with
/// the account, league event id and course id of its imported run.
fn wanted_draft_fields(
    tx: &Transaction,
    legacy: &LegacyDraft,
    league: Option<i64>,
    only: Option<i64>,
) -> Result<DraftFields, StoreError> {
    let draft = &legacy.draft;
    let run = match league {
        Some(league_id) => run_facts(tx, league_id)?,
        None => None,
    };
    Ok(DraftFields {
        draft_token: draft.token.clone(),
        account_id: run.and_then(|(account, _, _)| account).or(only),
        league_event_id: run.and_then(|(_, event_id, _)| event_id),
        mtgo_course_id: run.and_then(|(_, _, course_id)| course_id),
        mtgo_draft_id: signed(draft.draft_id),
        pod_token: draft.pod_token.clone(),
        seat_count: i64::from(draft.seat_count),
        seat_index: draft.seat_index.map(i64::from),
        booster_catalog_id: draft.booster_catalog_id.map(i64::from),
        state: draft_state_str(draft.state).to_string(),
        pack_size: i64::from(draft.pack_size),
        picks_expected: i64::from(draft.picks_expected),
        started_at: draft.started_at_ms,
        ended_at: draft.ended_at_ms,
        imported_league_from: league,
    })
}

/// Bring a draft this 0.x database imported before up to date: rewrite the
/// row and the picks only where they differ, and count it as unchanged or
/// updated. Returns the draft's id.
fn update_own_draft(
    tx: &Transaction,
    found: &StoredDraftRow,
    legacy: &LegacyDraft,
    wanted: &DraftFields,
    picks: &BTreeMap<i64, PickFields>,
    now_ms: i64,
    counts: &mut LimitedImportCounts,
) -> Result<i64, StoreError> {
    let same_picks = picks_unchanged(tx, found.id, picks)?;
    // A pulled copy (negative id) becomes this 0.x draft's; a re-pull keeps
    // the row's own id (final review F2).
    let imported_from = match found.imported_from {
        Some(id) if id > 0 || legacy.legacy_id < 0 => id,
        _ => free_draft_id(tx, legacy.legacy_id)?,
    };
    let same_id = found.imported_from == Some(imported_from)
        && found.legacy_draft_id == Some(legacy.legacy_id);
    let same_fields = found.fields.same_as(wanted);
    if same_fields && same_picks && same_id {
        counts.drafts_unchanged += 1;
    } else {
        counts.drafts_updated += 1;
        if !same_fields || !same_id {
            update_draft(tx, found.id, imported_from, legacy.legacy_id, wanted)?;
        }
        write_picks(tx, found.id, picks, now_ms)?;
    }
    Ok(found.id)
}

/// Import one draft; `league` is the `imported_from` of its 0.x league's
/// imported run (`run_imported_from`).
fn import_draft(
    tx: &Transaction,
    legacy: &LegacyDraft,
    league: Option<i64>,
    only: Option<i64>,
    now_ms: i64,
    counts: &mut LimitedImportCounts,
) -> Result<(), StoreError> {
    let draft = &legacy.draft;
    let wanted = wanted_draft_fields(tx, legacy, league, only)?;
    let picks = wanted_picks(draft)?;
    let draft_id = match stored_draft(tx, &draft.token)? {
        Some(found) if found.source == "log" => {
            // The log wins: only notes the picks have none of.
            counts.drafts_from_log += 1;
            add_notes(tx, found.id, legacy, now_ms, counts)?;
            return Ok(());
        }
        Some(found) if !found.is_own(legacy.legacy_id) => {
            // Another 0.x database's copy of this draft: it stays as it
            // is, and takes only notes its picks lack.
            counts.drafts_unchanged += 1;
            add_notes(tx, found.id, legacy, now_ms, counts)?;
            return Ok(());
        }
        Some(found) => update_own_draft(tx, &found, legacy, &wanted, &picks, now_ms, counts)?,
        None => {
            counts.drafts_added += 1;
            let imported_from = free_draft_id(tx, legacy.legacy_id)?;
            let id = insert_draft(tx, imported_from, legacy.legacy_id, &wanted)?;
            write_picks(tx, id, &picks, now_ms)?;
            id
        }
    };
    if wanted.account_id.is_none() {
        counts.drafts_without_account += 1;
    }
    add_notes(tx, draft_id, legacy, now_ms, counts)
}

/// Insert an imported draft under `imported_from`, keeping its 0.x id.
fn insert_draft(
    tx: &Transaction,
    imported_from: i64,
    legacy_id: i64,
    wanted: &DraftFields,
) -> Result<i64, StoreError> {
    tx.prepare_cached(
        "INSERT INTO drafts (draft_token, account_id, source, league_event_id, mtgo_course_id,
             mtgo_draft_id, pod_token, seat_count, seat_index, booster_catalog_id, state,
             pack_size, picks_expected, started_at, ended_at, imported_from,
             imported_league_from, legacy_draft_id)
         VALUES (?1, ?2, 'imported', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
             ?16, ?17)",
    )?
    .execute(params![
        wanted.draft_token,
        wanted.account_id,
        wanted.league_event_id,
        wanted.mtgo_course_id,
        wanted.mtgo_draft_id,
        wanted.pod_token,
        wanted.seat_count,
        wanted.seat_index,
        wanted.booster_catalog_id,
        wanted.state,
        wanted.pack_size,
        wanted.picks_expected,
        wanted.started_at,
        wanted.ended_at,
        imported_from,
        wanted.imported_league_from,
        legacy_id
    ])?;
    Ok(tx.last_insert_rowid())
}

/// Write 0.x's columns of an imported draft. `abandons_at` is cleared: the
/// next sync sets it again from the new values.
fn update_draft(
    tx: &Transaction,
    id: i64,
    imported_from: i64,
    legacy_id: i64,
    wanted: &DraftFields,
) -> Result<(), StoreError> {
    tx.prepare_cached(
        "UPDATE drafts SET draft_token = ?2, account_id = ?3, league_event_id = ?4,
             mtgo_course_id = ?5, mtgo_draft_id = ?6, pod_token = ?7, seat_count = ?8,
             seat_index = ?9, booster_catalog_id = ?10, state = ?11, pack_size = ?12,
             picks_expected = ?13, started_at = ?14, ended_at = ?15, imported_from = ?16,
             imported_league_from = ?17, legacy_draft_id = ?18, abandons_at = NULL
         WHERE id = ?1",
    )?
    .execute(params![
        id,
        wanted.draft_token,
        wanted.account_id,
        wanted.league_event_id,
        wanted.mtgo_course_id,
        wanted.mtgo_draft_id,
        wanted.pod_token,
        wanted.seat_count,
        wanted.seat_index,
        wanted.booster_catalog_id,
        wanted.state,
        wanted.pack_size,
        wanted.picks_expected,
        wanted.started_at,
        wanted.ended_at,
        imported_from,
        wanted.imported_league_from,
        legacy_id
    ])?;
    Ok(())
}

/// Whether the draft's stored picks are 0.x's: the same ordinals, attached,
/// with the same columns.
fn picks_unchanged(
    tx: &Transaction,
    draft_id: i64,
    wanted: &BTreeMap<i64, PickFields>,
) -> Result<bool, StoreError> {
    let stored = stored_picks(tx, draft_id)?;
    let attached: BTreeMap<&i64, &PickFields> = stored
        .iter()
        .filter(|(_, pick)| !pick.detached)
        .map(|(ordinal, pick)| (ordinal, &pick.fields))
        .collect();
    let wanted: BTreeMap<&i64, &PickFields> = wanted.iter().collect();
    Ok(attached == wanted)
}

/// Store 0.x's picks by ordinal, in place (a note stays). A stored pick 0.x
/// no longer has is deleted, or detached when it carries a note.
fn write_picks(
    tx: &Transaction,
    draft_id: i64,
    wanted: &BTreeMap<i64, PickFields>,
    now_ms: i64,
) -> Result<(), StoreError> {
    let stored = stored_picks(tx, draft_id)?;
    for (ordinal, pick) in wanted {
        let same = stored
            .get(ordinal)
            .is_some_and(|found| !found.detached && found.fields == *pick);
        if !same {
            upsert_pick(tx, draft_id, *ordinal, pick)?;
        }
    }
    for (ordinal, found) in &stored {
        if !wanted.contains_key(ordinal) {
            retire_pick(tx, draft_id, *ordinal, found, now_ms)?;
        }
    }
    Ok(())
}

/// Insert or update one pick, and give its cards `cards` rows.
fn upsert_pick(
    tx: &Transaction,
    draft_id: i64,
    ordinal: i64,
    pick: &PickFields,
) -> Result<(), StoreError> {
    tx.prepare_cached(
        "INSERT INTO draft_picks (draft_id, ordinal, pack_number, pick_number, pack_id,
             direction, cards_available, picked_catalog_id, picked_card_id, reservations,
             shown_at, deadline_at, picked_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT (draft_id, ordinal) DO UPDATE SET
             pack_number = excluded.pack_number, pick_number = excluded.pick_number,
             pack_id = excluded.pack_id, direction = excluded.direction,
             cards_available = excluded.cards_available,
             picked_catalog_id = excluded.picked_catalog_id,
             picked_card_id = excluded.picked_card_id,
             reservations = excluded.reservations, shown_at = excluded.shown_at,
             deadline_at = excluded.deadline_at, picked_at = excluded.picked_at,
             detached_at = NULL",
    )?
    .execute(params![
        draft_id,
        ordinal,
        pick.pack_number,
        pick.pick_number,
        pick.pack_id,
        pick.direction,
        pick.cards_available,
        pick.picked_catalog_id,
        pick.picked_card_id,
        pick.reservations,
        pick.shown_at,
        pick.deadline_at,
        pick.picked_at
    ])?;
    // The pack's cards and the pick get `cards` rows.
    let mut stubs: Vec<CardRef> = Vec::new();
    let available = crate::cards::catalog_ids_in(&pick.cards_available);
    for catalog_id in available.into_iter().chain(pick.picked_catalog_id) {
        stubs.push(CardRef {
            catalog_id,
            name: None,
        });
    }
    ensure_stubs(tx, &stubs)?;
    Ok(())
}

/// A stored pick 0.x no longer has: detached when it carries a note (and is
/// not already), else deleted.
fn retire_pick(
    tx: &Transaction,
    draft_id: i64,
    ordinal: i64,
    found: &StoredPick,
    now_ms: i64,
) -> Result<(), StoreError> {
    if !found.noted {
        tx.prepare_cached("DELETE FROM draft_picks WHERE draft_id = ?1 AND ordinal = ?2")?
            .execute(params![draft_id, ordinal])?;
    } else if !found.detached {
        tx.prepare_cached(
            "UPDATE draft_picks SET detached_at = ?3 WHERE draft_id = ?1 AND ordinal = ?2",
        )?
        .execute(params![draft_id, ordinal, now_ms])?;
    }
    Ok(())
}

/// Store the draft's non-blank 0.x notes, trimmed and cut to 2000
/// characters, on its picks that have no note (a note written in v1 is never
/// overwritten). A note whose pick v1 does not have (a log draft read only
/// so far) is counted in `notes_without_pick`.
fn add_notes(
    tx: &Transaction,
    draft_id: i64,
    legacy: &LegacyDraft,
    now_ms: i64,
    counts: &mut LimitedImportCounts,
) -> Result<(), StoreError> {
    for (ordinal, note) in &legacy.notes {
        let note: String = note.trim().chars().take(MAX_NOTE_CHARS).collect();
        let note = note.trim_end();
        if *ordinal == 0 || note.is_empty() {
            continue;
        }
        let added = tx
            .prepare_cached(
                "UPDATE draft_picks SET note = ?3, note_updated_at = ?4
                 WHERE draft_id = ?1 AND ordinal = ?2 AND note IS NULL",
            )?
            .execute(params![draft_id, ordinal, note, now_ms])?;
        counts.notes_added += u32::try_from(added).unwrap_or(0);
        if added == 0 {
            let has_pick: bool = tx
                .prepare_cached(
                    "SELECT EXISTS (SELECT 1 FROM draft_picks
                     WHERE draft_id = ?1 AND ordinal = ?2 AND detached_at IS NULL)",
                )?
                .query_row(params![draft_id, ordinal], |row| row.get(0))?;
            if !has_pick {
                counts.notes_without_pick += 1;
            }
        }
    }
    Ok(())
}

/// What one v1 match's imported snapshot should hold: when it was captured,
/// the `imported_from` of its run, and the cards.
type WantedDeck = (i64, i64, Vec<RegisteredCard>);

/// The wanted decks by v1 match id, and each match's token.
type NewestDecks = (HashMap<i64, WantedDeck>, HashMap<i64, String>);

/// The newest registered deck of each v1 match (on a tie, the later 0.x
/// league's), by match id, with the match's token. Counts the decks left
/// out for want of a run or a match.
fn newest_decks_by_match(
    tx: &Transaction,
    decks: &[LegacyLimitedDeck],
    league_keys: Option<&[LeagueKey]>,
    counts: &mut LimitedImportCounts,
) -> Result<NewestDecks, StoreError> {
    let mut wanted: HashMap<i64, WantedDeck> = HashMap::new();
    let mut tokens: HashMap<i64, String> = HashMap::new();
    for deck in decks {
        let run = match run_imported_from(tx, deck.league_id, league_keys)? {
            Some(league) if run_facts(tx, league)?.is_some() => league,
            _ => {
                counts.decks_without_run += 1;
                continue;
            }
        };
        let token = match live_row_token(tx, &deck.match_token, deck.mtgo_match_id)? {
            Some(live) => live,
            None => deck.match_token.clone(),
        };
        let match_id: Option<i64> = tx
            .prepare_cached("SELECT id FROM matches WHERE token = ?1")?
            .query_row([&token], |row| row.get(0))
            .optional()?;
        let Some(match_id) = match_id else {
            counts.decks_without_match += 1;
            continue;
        };
        let newer = wanted
            .get(&match_id)
            .is_none_or(|(captured_at, league, _)| {
                (deck.captured_at_ms, run) >= (*captured_at, *league)
            });
        if newer {
            wanted.insert(
                match_id,
                (deck.captured_at_ms, run, clamp_cards(deck.cards.clone())),
            );
            tokens.insert(match_id, token);
        }
    }
    Ok((wanted, tokens))
}

/// Store one match's imported snapshot and count it as added, updated or
/// unchanged.
fn store_imported_snapshot(
    tx: &Transaction,
    match_id: i64,
    token: &str,
    captured_at: i64,
    cards: &[RegisteredCard],
    counts: &mut LimitedImportCounts,
) -> Result<(), StoreError> {
    let cards_text = cards_json(cards)?;
    let stored: Option<(String, i64)> = tx
        .prepare_cached(
            "SELECT cards, captured_at FROM limited_deck_snapshots
             WHERE match_id = ?1 AND source = 'imported'",
        )?
        .query_row([match_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    match stored {
        Some(stored) if stored == (cards_text.clone(), captured_at) => {
            counts.decks_unchanged += 1;
            return Ok(());
        }
        Some(_) => counts.decks_updated += 1,
        None => counts.decks_added += 1,
    }
    let signature = registered(token, captured_at, cards.to_vec()).signature;
    tx.prepare_cached(
        "INSERT INTO limited_deck_snapshots (match_id, source, cards, signature, captured_at)
         VALUES (?1, 'imported', ?2, ?3, ?4)
         ON CONFLICT (match_id, source) DO UPDATE SET
             cards = excluded.cards, signature = excluded.signature,
             captured_at = excluded.captured_at",
    )?
    .execute(params![match_id, cards_text, signature, captured_at])?;
    let catalog_ids: Vec<u32> = cards.iter().map(|card| card.catalog_id).collect();
    crate::cards::ensure_id_stubs(tx, &catalog_ids)?;
    Ok(())
}

/// Store the registered decks: per v1 match, the newest one (on a tie, the
/// later 0.x league's).
fn import_decks(
    tx: &Transaction,
    decks: &[LegacyLimitedDeck],
    league_keys: Option<&[LeagueKey]>,
    counts: &mut LimitedImportCounts,
) -> Result<(), StoreError> {
    let (wanted, tokens) = newest_decks_by_match(tx, decks, league_keys, counts)?;
    let mut ids: Vec<i64> = wanted.keys().copied().collect();
    ids.sort_unstable();
    for match_id in ids {
        let (captured_at, _, cards) = &wanted[&match_id];
        store_imported_snapshot(
            tx,
            match_id,
            &tokens[&match_id],
            *captured_at,
            cards,
            counts,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limited::{
        DraftNoteEdit, DraftNoteOutcome, MAX_CARD_QUANTITY, edit_draft_note, pick_notes,
        stored_draft, stored_registered_deck,
    };
    use crate::test_support::{db, dump};
    use limited::{DraftState, Pick, Reservation};

    const TOKEN: &str = "q-draft-token-1";

    /// Accounts Qlocal (1) and Qother (2), and 0.x draft league 29 of Qlocal
    /// (event 7001, course 35) imported as a run.
    fn setup(conn: &Connection) {
        conn.execute_batch(
            "INSERT INTO accounts (id, username, first_seen_at)
             VALUES (1, 'Qlocal', 0), (2, 'Qother', 0);",
        )
        .expect("setup");
        crate::test_support::insert_imported_run(
            conn,
            29,
            Some(1),
            &crate::league_sync::ImportedSeed {
                token: "q-league".to_string(),
                event_id: Some(7001),
                name: "Q Draft".to_string(),
                format: Some("Limited".to_string()),
                state: "complete".to_string(),
                started_at: 1,
                joined_at: None,
                completed_at: None,
                dropped_at: None,
                deck_version_id: None,
                kind: "draft".to_string(),
                set_code: Some("HOB".to_string()),
                mtgo_course_id: Some(35),
                legacy_id: Some(29),
            },
        );
    }

    fn pick(ordinal: u32) -> Pick {
        Pick {
            ordinal,
            pack_number: 1,
            pick_number: ordinal,
            pack_id: Some(9_000_000_000 + u64::from(ordinal)),
            direction: Some(0),
            cards_available: vec![153_998, 153_940, 154_118],
            picked_catalog_id: Some(153_940),
            picked_card_id: Some(5_000_000_000),
            reservations: vec![Reservation {
                catalog_id: 153_998,
                at_ms: 1_000 + i64::from(ordinal),
            }],
            shown_at_ms: Some(10_000),
            deadline_at_ms: Some(80_000),
            picked_at_ms: Some(36_000),
        }
    }

    fn draft() -> LegacyDraft {
        LegacyDraft {
            legacy_id: 3,
            league_id: Some(29),
            draft: Draft {
                token: TOKEN.to_string(),
                league_event_id: None,
                course_id: None,
                draft_id: Some(35_746_768),
                pod_token: Some("q-pod".to_string()),
                seat_index: Some(7),
                seat_count: 8,
                booster_catalog_id: Some(154_732),
                state: DraftState::Finished,
                pack_size: 14,
                picks_expected: 42,
                started_at_ms: Some(1_000),
                ended_at_ms: Some(9_000),
                picks: vec![pick(2), pick(1)],
            },
            notes: vec![(1, "  Q bomb first  ".to_string()), (2, "   ".to_string())],
        }
    }

    /// The stored draft of 0.x draft `legacy_id`: its account, the 0.x
    /// league it came from, and the draft.
    fn imported(conn: &Connection, legacy_id: i64) -> (Option<i64>, Option<i64>, Draft) {
        let (token, account, league): (String, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT draft_token, account_id, imported_league_from FROM drafts
                 WHERE source = 'imported' AND imported_from = ?1",
                [legacy_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("the imported draft");
        let draft = stored_draft(conn, &token).expect("read").expect("stored");
        (account, league, draft)
    }

    fn data() -> ImportedLimited {
        ImportedLimited {
            drafts: vec![draft()],
            decks: Vec::new(),
            league_keys: None,
        }
    }

    #[test]
    fn imported_drafts_picks_and_notes_round_trip() {
        let (_dir, mut conn) = db("import-limited-round-trip");
        setup(&conn);
        let counts = import_limited(&mut conn, &data(), 50).expect("import");
        assert_eq!(
            counts,
            LimitedImportCounts {
                drafts_added: 1,
                notes_added: 1,
                ..LimitedImportCounts::default()
            }
        );
        let (account, league, read) = imported(&conn, 3);
        let mut expected = draft().draft;
        expected.picks = vec![pick(1), pick(2)];
        expected.league_event_id = Some(7001);
        expected.course_id = Some(35);
        assert_eq!(read, expected, "every column, picks in ordinal order");
        assert_eq!((account, league), (Some(1), Some(29)));
        let drafts: i64 = conn
            .query_row("SELECT COUNT(*) FROM drafts", [], |row| row.get(0))
            .expect("count");
        assert_eq!(drafts, 1);
        assert_eq!(
            pick_notes(&conn, TOKEN).expect("notes"),
            BTreeMap::from([(1, "Q bomb first".to_string())])
        );
        // The imported pick takes a v1 note too.
        assert_eq!(
            edit_draft_note(
                &mut conn,
                &DraftNoteEdit::Set {
                    draft_token: TOKEN.to_string(),
                    ordinal: 2,
                    note: "Q wheel".to_string(),
                    now_ms: 60,
                },
            )
            .expect("edit"),
            DraftNoteOutcome::Done
        );
    }

    #[test]
    fn a_rerun_writes_nothing_and_keeps_v1_notes() {
        let (_dir, mut conn) = db("import-limited-rerun");
        setup(&conn);
        import_limited(&mut conn, &data(), 50).expect("import");
        conn.execute(
            "UPDATE draft_picks SET note = 'Q changed my mind', note_updated_at = 55
             WHERE ordinal = 1",
            [],
        )
        .expect("a v1 edit");
        let tables = ["drafts", "draft_picks", "limited_deck_snapshots"];
        let before: Vec<_> = tables.iter().map(|table| dump(&conn, table)).collect();
        let again = import_limited(&mut conn, &data(), 99).expect("rerun");
        assert_eq!(
            again,
            LimitedImportCounts {
                drafts_unchanged: 1,
                ..LimitedImportCounts::default()
            }
        );
        let after: Vec<_> = tables.iter().map(|table| dump(&conn, table)).collect();
        assert_eq!(after, before, "an unchanged rerun writes nothing");
    }

    #[test]
    fn a_draft_0x_changed_is_updated_with_its_picks_replaced() {
        let (_dir, mut conn) = db("import-limited-update");
        setup(&conn);
        import_limited(&mut conn, &data(), 50).expect("import");
        let mut changed = draft();
        changed.draft.picks = vec![pick(1)];
        changed.draft.state = DraftState::Abandoned;
        let counts = import_limited(
            &mut conn,
            &ImportedLimited {
                drafts: vec![changed],
                decks: Vec::new(),
                league_keys: None,
            },
            60,
        )
        .expect("rerun");
        assert_eq!(counts.drafts_updated, 1);
        let (_, _, read) = imported(&conn, 3);
        assert_eq!(read.picks, vec![pick(1)]);
        assert_eq!(read.state, DraftState::Abandoned);
    }

    #[test]
    fn a_draft_without_an_imported_run_takes_the_only_account_or_none() {
        let (_dir, mut conn) = db("import-limited-account");
        conn.execute_batch(
            "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);",
        )
        .expect("one account");
        let mut lone = draft();
        lone.league_id = Some(404);
        import_limited(
            &mut conn,
            &ImportedLimited {
                drafts: vec![lone.clone()],
                decks: Vec::new(),
                league_keys: None,
            },
            50,
        )
        .expect("import");
        let (account, _, read) = imported(&conn, 3);
        assert_eq!(account, Some(1));
        let run: Option<i64> = conn
            .query_row(
                "SELECT l.id FROM drafts d JOIN leagues l ON l.imported_from = d.imported_league_from",
                [],
                |row| row.get(0),
            )
            .optional()
            .expect("read");
        assert_eq!(run, None, "league 404 was not imported");
        assert_eq!(read.league_event_id, None);

        let (_dir2, mut two) = db("import-limited-no-account");
        two.execute_batch(
            "INSERT INTO accounts (id, username, first_seen_at)
             VALUES (1, 'Qlocal', 0), (2, 'Qother', 0);",
        )
        .expect("two accounts");
        let counts = import_limited(
            &mut two,
            &ImportedLimited {
                drafts: vec![lone],
                decks: Vec::new(),
                league_keys: None,
            },
            50,
        )
        .expect("import");
        assert_eq!(counts.drafts_without_account, 1);
        assert_eq!(imported(&two, 3).0, None);
    }

    /// v1 has the draft from the log: the import leaves that row and its
    /// picks as they are, and a draft v1 has no events for is stored as an
    /// imported one, on a rerun too.
    #[test]
    fn an_import_rerun_never_overwrites_a_log_draft() {
        let (_dir, mut conn) = db("import-limited-log-draft");
        setup(&conn);
        conn.execute_batch(&format!(
            "INSERT INTO drafts (id, draft_token, account_id, source, state, started_at)
             VALUES (70, '{TOKEN}', 1, 'log', 'picking', 500);
             INSERT INTO draft_picks (draft_id, ordinal, pack_number, pick_number,
                 cards_available, reservations, note, note_updated_at)
             VALUES (70, 1, 1, 1, '[7]', '[]', 'Q mine', 5);"
        ))
        .expect("a log draft");
        let mut other = draft();
        other.legacy_id = 4;
        other.draft.token = "q-draft-token-2".to_string();
        let both = ImportedLimited {
            drafts: vec![draft(), other],
            decks: Vec::new(),
            league_keys: None,
        };
        let before = (dump(&conn, "drafts"), dump(&conn, "draft_picks"));
        for now_ms in [50, 99] {
            import_limited(&mut conn, &both, now_ms).expect("import");
            let rows: Vec<(String, String, Option<i64>)> = conn
                .prepare("SELECT draft_token, source, imported_from FROM drafts ORDER BY id")
                .expect("prepare")
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .expect("query")
                .map(|row| row.expect("row"))
                .collect();
            assert_eq!(
                rows,
                vec![
                    (TOKEN.to_string(), "log".to_string(), None),
                    (
                        "q-draft-token-2".to_string(),
                        "imported".to_string(),
                        Some(4)
                    ),
                ]
            );
            let log_rows = (
                dump(&conn, "drafts")[..1].to_vec(),
                dump(&conn, "draft_picks")[..1].to_vec(),
            );
            assert_eq!(log_rows, before, "the log draft untouched ({now_ms})");
        }
    }

    // Issue #194: a second 0.x database whose draft 3 is the first one's
    // draft 4 (the same token) used to rename draft 3 onto that token and
    // fail. The first database's drafts stay; the shared one takes only a
    // note its pick lacks.
    #[test]
    fn a_second_0x_database_sharing_a_draft_token_keeps_the_first_ones_drafts() {
        let (_dir, mut conn) = db("import-limited-shared-token");
        setup(&conn);
        let mut other = draft();
        other.legacy_id = 4;
        other.draft.token = "q-draft-token-2".to_string();
        other.notes = Vec::new();
        let first = ImportedLimited {
            drafts: vec![draft(), other.clone()],
            decks: Vec::new(),
            league_keys: None,
        };
        import_limited(&mut conn, &first, 50).expect("first database");
        let before = dump(&conn, "drafts");

        let mut shared = other;
        shared.legacy_id = 3;
        shared.draft.state = DraftState::Abandoned;
        shared.draft.picks = vec![pick(1)];
        shared.notes = vec![(2, "Q from the second".to_string())];
        let second = ImportedLimited {
            drafts: vec![shared],
            decks: Vec::new(),
            league_keys: None,
        };
        let counts = import_limited(&mut conn, &second, 60).expect("second database");
        assert_eq!(
            (counts.drafts_unchanged, counts.notes_added),
            (1, 1),
            "kept, with the note its pick lacked"
        );
        assert_eq!(
            dump(&conn, "drafts"),
            before,
            "both drafts as the first left them"
        );
        assert_eq!(
            pick_notes(&conn, "q-draft-token-2").expect("notes"),
            BTreeMap::from([(2, "Q from the second".to_string())])
        );
        let again = import_limited(&mut conn, &second, 70).expect("again");
        assert_eq!((again.drafts_unchanged, again.notes_added), (1, 0));
    }

    // Final review F1: a draft pulled from the cloud first (a negative
    // synthetic id in both columns) is taken over by the same 0.x draft
    // imported later: one row, 0.x's ids, state and picks.
    #[test]
    fn a_0x_draft_takes_over_its_pulled_copy() {
        let (_dir, mut conn) = db("import-limited-pulled-takeover");
        setup(&conn);
        let mut pulled = draft();
        pulled.legacy_id = -77;
        pulled.league_id = None;
        pulled.draft.state = DraftState::Picking;
        pulled.draft.picks = vec![pick(1)];
        pulled.notes = Vec::new();
        let pull = ImportedLimited {
            drafts: vec![pulled],
            decks: Vec::new(),
            league_keys: None,
        };
        import_limited(&mut conn, &pull, 50).expect("pulled");
        let mut legacy = draft();
        legacy.league_id = None;
        let counts = import_limited(
            &mut conn,
            &ImportedLimited {
                drafts: vec![legacy],
                decks: Vec::new(),
                league_keys: None,
            },
            60,
        )
        .expect("0.x import");
        assert_eq!(counts.drafts_updated, 1, "taken over");
        let rows: Vec<(Option<i64>, Option<i64>, String)> = conn
            .prepare("SELECT imported_from, legacy_draft_id, state FROM drafts")
            .expect("prepare")
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect();
        assert_eq!(rows, vec![(Some(3), Some(3), "finished".to_string())]);
        let picks: i64 = conn
            .query_row("SELECT COUNT(*) FROM draft_picks", [], |row| row.get(0))
            .expect("picks");
        assert_eq!(picks, 2, "0.x's picks");
    }

    // Review I2: a second database's draft stored under a free id is that
    // database's own on a rerun (its token and 0.x id), so 0.x's changes
    // reach it; the first database's draft with that 0.x id stays.
    #[test]
    fn a_second_databases_collided_draft_follows_its_0x_on_a_rerun() {
        let (_dir, mut conn) = db("import-limited-collided-rerun");
        setup(&conn);
        import_limited(&mut conn, &data(), 50).expect("first database");
        let first = dump(&conn, "drafts");
        let mut theirs = draft();
        theirs.draft.token = "q-draft-token-b".to_string();
        theirs.league_id = None;
        let second = |state: DraftState| ImportedLimited {
            drafts: vec![LegacyDraft {
                draft: Draft {
                    state,
                    ..theirs.draft.clone()
                },
                ..theirs.clone()
            }],
            decks: Vec::new(),
            league_keys: None,
        };
        let added = import_limited(&mut conn, &second(DraftState::Picking), 60).expect("second");
        assert_eq!(added.drafts_added, 1);
        let row = |conn: &Connection| {
            conn.query_row(
                "SELECT imported_from || ':' || legacy_draft_id || ':' || state FROM drafts
                 WHERE draft_token = 'q-draft-token-b'",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("the second draft")
        };
        assert_eq!(row(&conn), "4:3:picking");

        let rerun = import_limited(&mut conn, &second(DraftState::Finished), 70).expect("rerun");
        assert_eq!(rerun.drafts_updated, 1);
        assert_eq!(row(&conn), "4:3:finished");
        assert_eq!(
            dump(&conn, "drafts")[..1],
            first[..],
            "the first database's draft"
        );
        let again = import_limited(&mut conn, &second(DraftState::Finished), 80).expect("again");
        assert_eq!(again.drafts_unchanged, 1);
    }

    #[test]
    fn a_note_for_a_log_pick_not_stored_yet_is_counted_as_skipped() {
        let (_dir, mut conn) = db("import-limited-note-no-pick");
        setup(&conn);
        conn.execute_batch(&format!(
            "INSERT INTO drafts (id, draft_token, account_id, source, state, started_at)
             VALUES (70, '{TOKEN}', 1, 'log', 'picking', 500);
             INSERT INTO draft_picks (draft_id, ordinal, pack_number, pick_number,
                 cards_available, reservations)
             VALUES (70, 1, 1, 1, '[7]', '[]');"
        ))
        .expect("a log draft of one pick");
        let mut legacy = draft();
        legacy.notes = vec![
            (1, "Q first".to_string()),
            (2, "Q second".to_string()),
            (3, "   ".to_string()),
        ];
        let counts = import_limited(
            &mut conn,
            &ImportedLimited {
                drafts: vec![legacy],
                decks: Vec::new(),
                league_keys: None,
            },
            50,
        )
        .expect("import");
        assert_eq!(
            (counts.notes_added, counts.notes_without_pick),
            (1, 1),
            "the note for pick 2 waits for the pick (a blank one is no note)"
        );
    }

    fn card(catalog_id: u32, quantity: u32, sideboard: bool) -> RegisteredCard {
        RegisteredCard {
            catalog_id,
            quantity,
            sideboard,
        }
    }

    #[test]
    fn registered_decks_go_under_their_run_and_the_live_token() {
        let (_dir, mut conn) = db("import-limited-decks");
        setup(&conn);
        // 0.x's first match, imported under its own token.
        conn.execute(
            "INSERT INTO matches (token, account_id, source, state, outcome, started_at,
                 last_activity_at, revision, derived_version)
             VALUES ('q-0x-a', 1, 'imported', 'complete', 'win', 1, 1, 1, 0)",
            [],
        )
        .expect("imported match");
        // v1 tracked the second match live, under its own token.
        conn.execute(
            "INSERT INTO matches (token, mtgo_match_id, account_id, source, state, outcome,
                 started_at, last_activity_at, revision, derived_version)
             VALUES ('q-live', 289328482, 1, 'log', 'complete', 'win', 1, 1, 1, 1)",
            [],
        )
        .expect("live match");
        let decks = vec![
            LegacyLimitedDeck {
                league_id: 29,
                match_token: "q-0x-a".to_string(),
                mtgo_match_id: Some(1),
                cards: vec![card(100, 4_000_000, false), card(200, 0, true)],
                captured_at_ms: 5_000,
            },
            LegacyLimitedDeck {
                league_id: 29,
                match_token: "q-0x-b".to_string(),
                mtgo_match_id: Some(289_328_482),
                cards: vec![card(300, 1, false)],
                captured_at_ms: 6_000,
            },
            LegacyLimitedDeck {
                league_id: 30,
                match_token: "q-0x-c".to_string(),
                mtgo_match_id: None,
                cards: vec![card(300, 1, false)],
                captured_at_ms: 7_000,
            },
        ];
        let limited = ImportedLimited {
            drafts: Vec::new(),
            decks,
            league_keys: None,
        };
        let counts = import_limited(&mut conn, &limited, 50).expect("import");
        assert_eq!(
            counts,
            LimitedImportCounts {
                decks_added: 2,
                decks_without_run: 1,
                ..LimitedImportCounts::default()
            }
        );
        let first = stored_registered_deck(&conn, "q-0x-a")
            .expect("read")
            .expect("the first match's deck");
        assert_eq!(
            first.cards,
            vec![card(100, MAX_CARD_QUANTITY, false)],
            "clamped, and the empty row dropped"
        );
        let live = stored_registered_deck(&conn, "q-live")
            .expect("read")
            .expect("under the live match");
        assert_eq!(live.cards, vec![card(300, 1, false)]);
        assert_eq!(stored_registered_deck(&conn, "q-0x-b").expect("read"), None);

        let before = dump(&conn, "limited_deck_snapshots");
        let again = import_limited(&mut conn, &limited, 99).expect("rerun");
        assert_eq!(
            again,
            LimitedImportCounts {
                decks_unchanged: 2,
                decks_without_run: 1,
                ..LimitedImportCounts::default()
            }
        );
        assert_eq!(dump(&conn, "limited_deck_snapshots"), before);
    }
}
