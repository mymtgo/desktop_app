//! The league sync (target schema sections 8 and 9).
//!
//! One sync folds one owner's league inputs (an account's, or the 0.x data
//! the import could give no account) with the rule engines
//! (`limited::fold_drafts`, then `leagues::fold_league_runs` once over
//! constructed and limited inputs together) and stores the result:
//! `drafts`, `draft_picks`, `limited_deck_snapshots`, `leagues`,
//! `tournaments`, `matches.league_id` and `matches.tournament_id`. It writes
//! only the rows and columns that changed, so a second sync writes nothing,
//! and the same inputs give the same rows whether they were synced one at a
//! time or all at once (live equals rebuild).
//!
//! The sync never writes an input column (`leagues.notes`,
//! `leagues.user_dropped_at`, `leagues.user_state`, `leagues.removed_at`,
//! `leagues.edited_at`, `leagues.imported_from`, `leagues.imported_seed`,
//! `matches.league_linked`, `matches.no_league`, `matches.edited_at`, a
//! linked match's `league_id`, `draft_picks.note`), never touches a manual
//! row, and of an imported draft writes only its state (the 30-minute rule),
//! `abandons_at` and `league_id`. A match the player linked to a run
//! (`league_linked`) is never folded: it is attached to its row by
//! `league_id`.
//!
//! Two exceptions move input with its row's identity: a run whose key
//! changed keeps its row (and so its notes, drop, player status and linked
//! matches), and a row retired by a late adoption or claimed by an import
//! hands its notes to the surviving row where that row has none, its player
//! status (`removed_at`, `user_state` with its drop time, `edited_at`, as
//! one unit) where newer, and its hand-linked matches.
//!
//! Rows keep their ids: a fold run takes the row with its key, else the row
//! of one of its `former_keys` (a late adoption), else the unmatched log row
//! that holds most of its matches (a hidden first match changes the key; a
//! row the player deleted is taken too and stays deleted), else a new row.
//! A row the fold no longer makes is deleted unless it carries input (a
//! note, a drop, a player status, a hand-linked match) or is imported; then
//! it is detached until the fold makes its key again.

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono_tz::Tz;
use leagues::{
    DraftInput, DraftTournament, FOLD_VERSION, ImportedRunSeed, LeagueFold, LeagueInput,
    LeagueKind, LimitedFacts, LimitedSeed, OrderedInput, PanelFormat, Run, RunState,
    TournamentDraft, fold_league_runs, is_limited_format_code, tournament_name,
};
use limited::{ABANDON_AFTER_MS, Draft, DraftState, Pick, RegisteredDeck, fold_drafts};
use mtgo_protocol::EventKind;
use mtgo_protocol::tournament::{TournamentFact, decode_fact};
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Transaction, params, params_from_iter};
use serde::{Deserialize, Serialize};

use crate::StoreError;
use crate::compact::{archived_events, compacted_at};
use crate::leagues::{
    DRAFT_POSITION, LeagueMatchRow, MATCH_POSITION, Scope, any_league_match, card_facts,
    imported_run_key, is_stale, last_activity, league_events, league_panels, match_rows,
    member_match, parse_state,
};
use crate::limited::{
    DRAFT_SELECT, cards_json, draft_at, draft_events, draft_state_str, parse_draft_state,
    parse_registered, registered, reservations_json, stored_cards, stored_picks,
};
use crate::write::decode_raw;

/// The derivation name the full sync records (`derivations.name`).
pub const LEAGUE_FOLD_DERIVATION: &str = "league_fold";
/// The setting holding the timezone (a JSON string) the full sync folded
/// with: run names and pick times depend on it.
pub const LEAGUE_FOLD_TIMEZONE: &str = "league_fold_timezone";

/// Whose runs one sync folds: one account's, or (`Nobody`) the 0.x runs and
/// drafts the import could give no account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LeagueOwner {
    Account(i64),
    Nobody,
}

impl LeagueOwner {
    /// The `account_id` of the owner's rows (bound to `account_id IS ?`).
    fn account(self) -> Option<i64> {
        match self {
            LeagueOwner::Account(id) => Some(id),
            LeagueOwner::Nobody => None,
        }
    }
}

/// What a sync changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    /// A league or tournament row, or a match's league or tournament, changed.
    pub leagues_changed: bool,
    /// A draft, pick or snapshot changed, or a draft or sealed run did.
    pub limited_changed: bool,
    /// The earliest `abandons_at` of the owners' attached drafts: when an
    /// open draft turns abandoned without a new event.
    pub next_abandon_at: Option<i64>,
    /// Matches whose archive could not be read for their registered deck:
    /// they have no snapshot (the sync skipped them; the caller logs them).
    pub skipped_snapshots: Vec<String>,
}

impl SyncOutcome {
    fn merge(&mut self, other: SyncOutcome) {
        self.leagues_changed |= other.leagues_changed;
        self.limited_changed |= other.limited_changed;
        self.next_abandon_at = match (self.next_abandon_at, other.next_abandon_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        self.skipped_snapshots.extend(other.skipped_snapshots);
    }
}

/// A 0.x run as the import left it (`leagues.imported_seed`, JSON): the
/// fold's seed for that run. The sync reads it and never writes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedSeed {
    pub token: String,
    pub event_id: Option<i64>,
    pub name: String,
    pub format: Option<String>,
    /// `active`, `complete`, `partial` or `dropped`.
    pub state: String,
    pub started_at: i64,
    pub joined_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub dropped_at: Option<i64>,
    pub deck_version_id: Option<i64>,
    /// `constructed`, `draft` or `sealed`.
    pub kind: String,
    pub set_code: Option<String>,
    pub mtgo_course_id: Option<i64>,
    /// The 0.x `leagues.id` the import took it from, by which a rerun of
    /// that 0.x database finds the run again whatever `imported_from` it got
    /// (issue #143). Every run the 0.x import or a pull stores has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_id: Option<i64>,
}

/// Fold one owner's league inputs and store the result, inside `tx`.
/// `tz` is the OS timezone (run names, pick times); `now_ms` decides which
/// open drafts are abandoned. An Arena account is left alone: its runs
/// belong to `arena_run_sync`, and this fold would detach or delete them.
pub fn sync_owner(
    tx: &Transaction,
    owner: LeagueOwner,
    tz: Tz,
    now_ms: i64,
) -> Result<SyncOutcome, StoreError> {
    if let LeagueOwner::Account(account_id) = owner {
        let arena_accounts: i64 = tx
            .prepare_cached("SELECT COUNT(*) FROM accounts WHERE id = ?1 AND platform = 'arena'")?
            .query_row([account_id], |row| row.get(0))?;
        if arena_accounts > 0 {
            return Ok(SyncOutcome::default());
        }
    }
    let mut out = SyncOutcome::default();
    let sources = read_sources(tx, owner)?;
    let mut leagues_changed = false;

    // 1. Drafts, and the tournaments a draft no league holds may belong to
    // (their matches' tournament tokens first: the tokens find the
    // tournaments' own lines).
    let drafts = sync_drafts(tx, owner, tz, now_ms, &mut out)?;
    let tournaments = match owner {
        LeagueOwner::Account(account_id) => {
            leagues_changed |= link_tournament_tokens(tx, account_id)?;
            draft_tournaments(tx, account_id)?
        }
        LeagueOwner::Nobody => LimitedTournaments::default(),
    };

    // 2. Registered decks of the limited matches, league and tournament.
    let mut snapshot_tokens = sources.snapshot_tokens();
    snapshot_tokens.extend(tournaments.match_tokens.iter().cloned());
    let registered = sync_snapshots(tx, &snapshot_tokens, &mut out)?;

    // 3. The fold, with card facts for the decks and the picks.
    let mut catalog_ids: BTreeSet<u32> = BTreeSet::new();
    for deck in registered.values() {
        for card in &deck.cards {
            catalog_ids.insert(card.catalog_id);
        }
    }
    for input in &drafts {
        for pick in &input.draft.picks {
            if let Some(catalog_id) = pick.picked_catalog_id {
                catalog_ids.insert(catalog_id);
            }
        }
    }
    let facts = LimitedFacts {
        drafts,
        registered,
        cards: card_facts(tx, &catalog_ids)?,
        panels: sources.panels.clone(),
        seeds: sources.limited_seeds.clone(),
        tournaments: tournaments.inputs.clone(),
    };
    let mut folded = fold(tx, &sources.inputs, &facts, tz)?;

    // 4. Rows. A row that took a new key by its matches carries its drop
    // under its old key, which the fold did not see: fold once more.
    let mut assigned = assign_rows(tx, owner, &folded.runs, now_ms)?;
    if assigned.refold {
        folded = fold(tx, &sources.inputs, &facts, tz)?;
        let again = assign_rows(tx, owner, &folded.runs, now_ms)?;
        assigned.changed |= again.changed;
        assigned.ids = again.ids;
    }
    let runs = folded.runs;
    leagues_changed |= assigned.changed;
    let mut limited_changed = assigned.changed;
    let (constructed, limited) = write_runs(tx, owner, &runs, &assigned.ids)?;
    leagues_changed |= constructed || limited;
    limited_changed |= limited;
    if remove_leftovers(tx, owner, &assigned.ids, now_ms)? {
        leagues_changed = true;
        limited_changed = true;
    }

    // 5. Matches and drafts point at their run.
    if let LeagueOwner::Account(account_id) = owner {
        let (constructed, limited) = write_match_leagues(tx, account_id, &runs, &assigned.ids)?;
        leagues_changed |= constructed || limited;
        limited_changed |= limited;
    }
    if write_draft_leagues(tx, owner, &runs, &assigned.ids)? {
        leagues_changed = true;
        limited_changed = true;
    }

    // 6. Tournaments, then the drafts played in them.
    if let LeagueOwner::Account(account_id) = owner {
        leagues_changed |= sync_tournaments(tx, account_id, tz)?;
    }
    if write_draft_tournaments(tx, owner, &folded.tournament_drafts, &tournaments.names)? {
        limited_changed = true;
    }

    out.leagues_changed |= leagues_changed;
    out.limited_changed |= limited_changed;
    out.next_abandon_at = tx
        .prepare_cached(
            "SELECT MIN(abandons_at) FROM drafts WHERE account_id IS ?1 AND detached_at IS NULL",
        )?
        .query_row([owner.account()], |row| row.get(0))?;
    Ok(out)
}

/// Sync every account, then the account-less 0.x data, in one transaction,
/// and record the fold version and timezone (the full backfill).
pub fn sync_all(conn: &mut Connection, tz: Tz, now_ms: i64) -> Result<SyncOutcome, StoreError> {
    let tx = conn.transaction()?;
    let out = sync_every_owner(&tx, tz, now_ms)?;
    if !league_fold_current(&tx, tz)? {
        tx.execute(
            "INSERT INTO derivations (name, version, rebuilt_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (name) DO UPDATE SET version = excluded.version,
                 rebuilt_at = excluded.rebuilt_at",
            params![LEAGUE_FOLD_DERIVATION, FOLD_VERSION, now_ms],
        )?;
        tx.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![LEAGUE_FOLD_TIMEZONE, timezone_json(tz)?],
        )?;
    }
    tx.commit()?;
    Ok(out)
}

/// Sync every account, then the account-less 0.x data, inside `tx` (what
/// `sync_all` runs, without recording the fold version).
pub fn sync_every_owner(tx: &Transaction, tz: Tz, now_ms: i64) -> Result<SyncOutcome, StoreError> {
    let mut accounts: Vec<i64> = Vec::new();
    {
        let mut stmt = tx.prepare_cached("SELECT id FROM accounts ORDER BY id")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        for row in rows {
            accounts.push(row?);
        }
    }
    let mut out = SyncOutcome::default();
    for account_id in accounts {
        out.merge(sync_owner(
            tx,
            LeagueOwner::Account(account_id),
            tz,
            now_ms,
        )?);
    }
    out.merge(sync_owner(tx, LeagueOwner::Nobody, tz, now_ms)?);
    Ok(out)
}

/// Whether the stored runs were folded at `leagues::FOLD_VERSION` (or later)
/// in timezone `tz`. When not, every owner needs a sync (`sync_all`).
pub fn league_fold_current(conn: &Connection, tz: Tz) -> Result<bool, StoreError> {
    let version: Option<i64> = conn
        .query_row(
            "SELECT version FROM derivations WHERE name = ?1",
            [LEAGUE_FOLD_DERIVATION],
            |row| row.get(0),
        )
        .optional()?;
    let zone: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [LEAGUE_FOLD_TIMEZONE],
            |row| row.get(0),
        )
        .optional()?;
    Ok(version.is_some_and(|version| version >= FOLD_VERSION)
        && zone.as_deref() == Some(timezone_json(tz)?.as_str()))
}

/// A timezone as the settings table stores it (a JSON string).
fn timezone_json(tz: Tz) -> Result<String, StoreError> {
    Ok(serde_json::to_string(tz.name())?)
}

// ---- Values ------------------------------------------------------------------

fn int(value: i64) -> Value {
    Value::Integer(value)
}

fn opt_int(value: Option<i64>) -> Value {
    value.map_or(Value::Null, Value::Integer)
}

fn opt_u32(value: Option<u32>) -> Value {
    opt_int(value.map(i64::from))
}

/// An unsigned id; one too large for SQLite's integers is stored as none.
fn opt_u64(value: Option<u64>) -> Value {
    opt_int(value.and_then(|value| i64::try_from(value).ok()))
}

fn text(value: &str) -> Value {
    Value::Text(value.to_string())
}

fn opt_text(value: Option<&str>) -> Value {
    value.map_or(Value::Null, text)
}

/// `count` values of a row, from column `start`.
fn values(row: &rusqlite::Row, start: usize, count: usize) -> rusqlite::Result<Vec<Value>> {
    let mut found = Vec::with_capacity(count);
    for index in start..start + count {
        found.push(row.get(index)?);
    }
    Ok(found)
}

/// Write the `columns` of one row whose desired value differs from the
/// stored one. `key` names the row (`id = ?`, `draft_id = ? AND ordinal = ?`).
/// Returns whether anything was written.
fn write_changed(
    tx: &Transaction,
    table: &str,
    key: &[(&str, Value)],
    columns: &[&str],
    stored: &[Value],
    desired: &[Value],
) -> Result<bool, StoreError> {
    let mut sets = Vec::new();
    let mut bound: Vec<Value> = Vec::new();
    for ((column, have), want) in columns.iter().zip(stored).zip(desired) {
        if have != want {
            bound.push(want.clone());
            sets.push(format!("{column} = ?{}", bound.len()));
        }
    }
    if sets.is_empty() {
        return Ok(false);
    }
    let mut filters = Vec::new();
    for (column, value) in key {
        bound.push(value.clone());
        filters.push(format!("{column} = ?{}", bound.len()));
    }
    let sql = format!(
        "UPDATE {table} SET {} WHERE {}",
        sets.join(", "),
        filters.join(" AND ")
    );
    tx.prepare_cached(&sql)?.execute(params_from_iter(bound))?;
    Ok(true)
}

/// `INSERT INTO {table} ({columns}) VALUES (...)`, returning the row id.
fn insert(
    tx: &Transaction,
    table: &str,
    columns: &[&str],
    values: Vec<Value>,
) -> Result<i64, StoreError> {
    let placeholders: Vec<String> = (1..=columns.len()).map(|n| format!("?{n}")).collect();
    let sql = format!(
        "INSERT INTO {table} ({}) VALUES ({})",
        columns.join(", "),
        placeholders.join(", ")
    );
    tx.prepare_cached(&sql)?.execute(params_from_iter(values))?;
    Ok(tx.last_insert_rowid())
}

// ---- League inputs -----------------------------------------------------------

/// What the fold reads besides drafts, decks and card facts.
#[derive(Default)]
struct Sources {
    /// League matches, events and 0.x seeds (manual drops are added per fold).
    inputs: Vec<OrderedInput>,
    /// Every match among them: plain ones and the seeds' members.
    matches: Vec<LeagueMatchRow>,
    panels: Vec<PanelFormat>,
    limited_seeds: HashMap<String, LimitedSeed>,
    /// Members of draft and sealed seeds (their decks are read whatever
    /// their format).
    limited_members: BTreeSet<String>,
}

impl Sources {
    /// The matches whose registered deck the limited rules read: limited
    /// ones, and every member of a limited seed.
    fn snapshot_tokens(&self) -> Vec<String> {
        let mut tokens = Vec::new();
        let mut seen = HashSet::new();
        for row in &self.matches {
            let limited_format = row.format.as_deref().is_some_and(is_limited_format_code);
            if (limited_format || self.limited_members.contains(&row.token))
                && seen.insert(row.token.clone())
            {
                tokens.push(row.token.clone());
            }
        }
        tokens
    }
}

/// League matches that join a run by their token (leagues spec 4): log and
/// manual matches of the account with a league token, not tournament
/// matches, reached in progress or later, not linked by hand or by the 0.x
/// import (those are input: `league_linked`), and not taken out of every
/// league by the player (`no_league`, self-management spec 4.2).
pub(crate) fn plain_scope() -> Scope {
    Scope {
        from: "listed_matches m",
        filter: "m.account_id = ?1 AND m.league_token IS NOT NULL AND m.league_linked = 0
             AND m.no_league = 0
             AND m.source IN ('log', 'manual')
             AND m.tournament_event_id IS NULL
             AND m.state IN ('in_progress', 'ended', 'complete', 'abandoned')"
            .to_string(),
        order: "ORDER BY m.started_at, m.id",
        limit: "",
        group: "NULL",
    }
}

/// The linked members of the owner's 0.x runs, by run row.
pub(crate) fn members_scope() -> Scope {
    Scope {
        from: "leagues l JOIN listed_matches m ON m.league_id = l.id",
        filter: "l.source = 'imported' AND l.account_id IS ?1 AND m.league_linked = 1
             AND m.no_league = 0"
            .to_string(),
        order: "ORDER BY m.started_at, m.id",
        limit: "",
        group: "l.id",
    }
}

/// The owner's 0.x run rows (`?1` is the account id, or NULL for none):
/// their id, key, 0.x id and seed.
pub(crate) const IMPORTED_SEEDS: &str =
    "SELECT id, run_key, imported_from, imported_seed FROM leagues
     WHERE source = 'imported' AND account_id IS ?1";

/// A 0.x run row: its id, key, 0.x id and seed.
struct SeedRow {
    id: i64,
    run_key: String,
    imported_from: i64,
    seed: ImportedSeed,
}

fn read_sources(tx: &Transaction, owner: LeagueOwner) -> Result<Sources, StoreError> {
    let mut sources = Sources::default();
    if let LeagueOwner::Account(account_id) = owner {
        for (_, row) in match_rows(tx, &plain_scope(), &[&account_id])? {
            if let Some(played) = any_league_match(&row) {
                let (file, offset) = MATCH_POSITION;
                sources
                    .inputs
                    .push((played.started_at, file, offset, LeagueInput::Match(played)));
                sources.matches.push(row);
            }
        }
        sources.inputs.extend(league_events(tx, account_id)?);
        sources.panels = league_panels(tx, account_id)?;
    }

    let mut seeds: Vec<SeedRow> = Vec::new();
    {
        let mut stmt = tx.prepare_cached(IMPORTED_SEEDS)?;
        let rows = stmt.query_map([owner.account()], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (id, run_key, imported_from, json) = row?;
            seeds.push(SeedRow {
                id,
                run_key,
                imported_from,
                seed: serde_json::from_str(&json)?,
            });
        }
    }
    // Constructed seeds first, each kind newest first (as the reads before
    // the stored runs gave them to the fold).
    seeds.sort_by(|a, b| {
        (a.seed.kind != "constructed")
            .cmp(&(b.seed.kind != "constructed"))
            .then(b.seed.started_at.cmp(&a.seed.started_at))
            .then(b.imported_from.cmp(&a.imported_from))
    });
    let mut members: HashMap<i64, Vec<LeagueMatchRow>> = HashMap::new();
    if !seeds.is_empty() {
        for (group, row) in match_rows(tx, &members_scope(), &[&owner.account()])? {
            if let Some(group) = group {
                members.entry(group).or_default().push(row);
            }
        }
    }
    for row in seeds {
        let seed = row.seed;
        let run_members = members.remove(&row.id).unwrap_or_default();
        let kind = match seed.kind.as_str() {
            "draft" => Some(LeagueKind::Draft),
            "sealed" => Some(LeagueKind::Sealed),
            _ => None,
        };
        if let Some(kind) = kind {
            for member in &run_members {
                sources.limited_members.insert(member.token.clone());
            }
            sources.limited_seeds.insert(
                row.run_key.clone(),
                LimitedSeed {
                    kind,
                    set_code: seed.set_code.clone().filter(|code| !code.trim().is_empty()),
                    course_id: seed.mtgo_course_id,
                },
            );
        }
        let deck_version_id = match seed.deck_version_id {
            Some(id) if version_exists(tx, id)? => Some(id),
            _ => None,
        };
        let input = ImportedRunSeed {
            key: row.run_key,
            token: seed.token.clone(),
            event_id: seed.event_id,
            joined_at: seed.joined_at,
            name: seed.name.clone(),
            format: seed
                .format
                .clone()
                .filter(|format| !format.trim().is_empty()),
            deck_version_id,
            started_at: seed.started_at,
            state: parse_state(&seed.state)?,
            dropped_at: seed.dropped_at,
            completed_at: seed.completed_at,
            matches: run_members
                .iter()
                .map(|member| member_match(member, &seed.token))
                .collect(),
        };
        let (file, offset) = MATCH_POSITION;
        sources.inputs.push((
            input.started_at,
            file,
            offset,
            LeagueInput::ImportedRun(input),
        ));
        sources.matches.extend(run_members);
    }
    Ok(sources)
}

/// What the sync folds for `owner`, for the tests of the reads: the
/// inputs `read_sources` gives plus the manual drops `fold` adds, every
/// match among them, and the facts the limited rules take from the same
/// read (the panels and the limited seeds).
#[cfg(test)]
pub(crate) struct FoldInputs {
    pub(crate) inputs: Vec<OrderedInput>,
    pub(crate) matches: Vec<LeagueMatchRow>,
    pub(crate) facts: LimitedFacts,
}

#[cfg(test)]
impl FoldInputs {
    /// The constructed runs the sync's fold makes of these inputs. The
    /// drafts, registered decks and card facts the sync also reads shape
    /// only limited runs, so they are left out.
    pub(crate) fn constructed_runs(&self, tz: Tz) -> Vec<Run> {
        fold_league_runs(self.inputs.clone(), self.facts.clone(), tz)
            .runs
            .into_iter()
            .filter(|run| run.kind == LeagueKind::Constructed)
            .collect()
    }
}

/// `read_sources` and the manual drops for `owner`, read in a transaction
/// that is rolled back (it only reads).
#[cfg(test)]
pub(crate) fn fold_inputs(conn: &Connection, owner: LeagueOwner) -> Result<FoldInputs, StoreError> {
    let tx = conn.unchecked_transaction()?;
    let sources = read_sources(&tx, owner)?;
    let mut inputs = sources.inputs;
    inputs.extend(manual_drops(&tx)?);
    Ok(FoldInputs {
        inputs,
        matches: sources.matches,
        facts: LimitedFacts {
            panels: sources.panels,
            seeds: sources.limited_seeds,
            ..LimitedFacts::default()
        },
    })
}

/// Whether a deck version still exists (a 0.x seed may name a pruned one).
fn version_exists(tx: &Transaction, id: i64) -> Result<bool, StoreError> {
    Ok(tx
        .prepare_cached("SELECT 1 FROM deck_versions WHERE id = ?1")?
        .query_row([id], |_| Ok(()))
        .optional()?
        .is_some())
}

/// Every row's manual drop, at its time (a user record, so file 0, offset
/// 0): `user_dropped_at` where the player's status is Dropped
/// (self-management spec 3.2). Another status keeps the time, which the fold
/// then ignores. A drop whose run is not in this fold is ignored by it.
fn manual_drops(tx: &Transaction) -> Result<Vec<OrderedInput>, StoreError> {
    let mut stmt = tx.prepare_cached(
        "SELECT run_key, user_dropped_at FROM leagues
         WHERE user_dropped_at IS NOT NULL AND user_state = 'dropped'
             AND source <> 'manual'",
    )?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get(1)?)))?;
    let mut drops = Vec::new();
    for row in rows {
        let (run_key, at_ms): (String, i64) = row?;
        drops.push((at_ms, 0, 0, LeagueInput::ManualDrop { run_key, at_ms }));
    }
    Ok(drops)
}

/// The fold over the inputs plus the drops stored now.
fn fold(
    tx: &Transaction,
    inputs: &[OrderedInput],
    facts: &LimitedFacts,
    tz: Tz,
) -> Result<LeagueFold, StoreError> {
    let mut all = inputs.to_vec();
    all.extend(manual_drops(tx)?);
    Ok(fold_league_runs(all, facts.clone(), tz))
}

// ---- Drafts ------------------------------------------------------------------

/// The draft columns the sync writes for a log draft, in this order.
const DRAFT_COLUMNS: [&str; 17] = [
    "account_id",
    "source",
    "league_event_id",
    "mtgo_course_id",
    "mtgo_draft_id",
    "pod_token",
    "seat_count",
    "seat_index",
    "booster_catalog_id",
    "state",
    "pack_size",
    "picks_expected",
    "started_at",
    "ended_at",
    "abandons_at",
    "detached_at",
    "imported_from",
];

/// The pick columns the sync writes, in this order (never `note`).
const PICK_COLUMNS: [&str; 12] = [
    "pack_number",
    "pick_number",
    "pack_id",
    "direction",
    "cards_available",
    "picked_catalog_id",
    "picked_card_id",
    "reservations",
    "shown_at",
    "deadline_at",
    "picked_at",
    "detached_at",
];

fn is_open(state: DraftState) -> bool {
    matches!(state, DraftState::Connecting | DraftState::Picking)
}

/// Fold the owner's draft events, store the drafts, and return every draft
/// of the owner as a fold input: the log's at their first line, then the
/// 0.x drafts whose token has no events, at their start.
fn sync_drafts(
    tx: &Transaction,
    owner: LeagueOwner,
    tz: Tz,
    now_ms: i64,
    out: &mut SyncOutcome,
) -> Result<Vec<DraftInput>, StoreError> {
    let mut inputs = Vec::new();
    if let LeagueOwner::Account(account_id) = owner {
        let events = draft_events(tx, account_id)?;
        // The first line of each draft (its place in the fold) and its first
        // line the draft fold acts on (its last activity before any pick).
        let mut first_line: HashMap<String, i64> = HashMap::new();
        let mut first_activity: HashMap<String, i64> = HashMap::new();
        for event in &events {
            if let Some(token) = &event.draft_token {
                first_line
                    .entry(token.clone())
                    .or_insert(event.logged_at_ms);
                if event.kind != EventKind::DraftPackOpened {
                    first_activity
                        .entry(token.clone())
                        .or_insert(event.logged_at_ms);
                }
            }
        }
        let mut folded: HashSet<String> = HashSet::new();
        for draft in fold_drafts(&events, tz, now_ms) {
            let abandons_at = is_open(draft.state).then(|| {
                last_activity(&draft)
                    .or_else(|| first_activity.get(&draft.token).copied())
                    .unwrap_or(0)
                    + ABANDON_AFTER_MS
            });
            if write_log_draft(tx, account_id, &draft, abandons_at, now_ms)? {
                out.limited_changed = true;
            }
            folded.insert(draft.token.clone());
            let at = first_line
                .get(&draft.token)
                .copied()
                .or(draft.started_at_ms)
                .unwrap_or(0);
            let (file, offset) = DRAFT_POSITION;
            inputs.push(DraftInput {
                order_ms: at,
                file_id: file,
                offset,
                draft,
                run_key: None,
            });
        }
        if retire_log_drafts(tx, account_id, &folded, now_ms)? {
            out.limited_changed = true;
        }
    }

    for imported in drafts_from_0x(tx, owner)? {
        let mut draft = imported.draft;
        if is_stale(&draft, now_ms) {
            draft.state = DraftState::Abandoned;
        }
        let abandons_at = if is_open(draft.state) {
            last_activity(&draft).map(|at| at + ABANDON_AFTER_MS)
        } else {
            None
        };
        let state = draft_state_str(draft.state);
        if state != imported.state || abandons_at != imported.abandons_at {
            tx.prepare_cached("UPDATE drafts SET state = ?1, abandons_at = ?2 WHERE id = ?3")?
                .execute(params![state, abandons_at, imported.id])?;
            out.limited_changed = true;
        }
        let run_key = match imported.league_from {
            Some(id) if imported_run_exists(tx, id)? => Some(imported_run_key(id)),
            _ => None,
        };
        let at = draft
            .started_at_ms
            .or_else(|| last_activity(&draft))
            .unwrap_or(0);
        inputs.push(DraftInput {
            order_ms: at,
            file_id: 0,
            offset: u64::try_from(imported.legacy_id).unwrap_or(0),
            draft,
            run_key,
        });
    }
    Ok(inputs)
}

fn imported_run_exists(tx: &Transaction, imported_from: i64) -> Result<bool, StoreError> {
    Ok(tx
        .prepare_cached("SELECT 1 FROM leagues WHERE imported_from = ?1")?
        .query_row([imported_from], |_| Ok(()))
        .optional()?
        .is_some())
}

fn draft_values(account_id: i64, draft: &Draft, abandons_at: Option<i64>) -> Vec<Value> {
    vec![
        int(account_id),
        text("log"),
        opt_u64(draft.league_event_id),
        opt_u64(draft.course_id),
        opt_u64(draft.draft_id),
        opt_text(draft.pod_token.as_deref()),
        int(i64::from(draft.seat_count)),
        opt_u32(draft.seat_index),
        opt_u32(draft.booster_catalog_id),
        text(draft_state_str(draft.state)),
        int(i64::from(draft.pack_size)),
        int(i64::from(draft.picks_expected)),
        opt_int(draft.started_at_ms),
        opt_int(draft.ended_at_ms),
        opt_int(abandons_at),
        Value::Null,
        Value::Null,
    ]
}

/// Store one draft folded from the log by its token (a 0.x draft of the
/// same token becomes this one, keeping its id and pick notes), with its
/// picks. Returns whether anything was written.
fn write_log_draft(
    tx: &Transaction,
    account_id: i64,
    draft: &Draft,
    abandons_at: Option<i64>,
    now_ms: i64,
) -> Result<bool, StoreError> {
    let desired = draft_values(account_id, draft, abandons_at);
    let stored: Option<(i64, Vec<Value>)> = tx
        .prepare_cached(&format!(
            "SELECT id, {} FROM drafts WHERE draft_token = ?1",
            DRAFT_COLUMNS.join(", ")
        ))?
        .query_row([&draft.token], |row| {
            Ok((row.get(0)?, values(row, 1, DRAFT_COLUMNS.len())?))
        })
        .optional()?;
    let (id, mut changed) = match stored {
        Some((id, stored)) => (
            id,
            write_changed(
                tx,
                "drafts",
                &[("id", int(id))],
                &DRAFT_COLUMNS,
                &stored,
                &desired,
            )?,
        ),
        None => {
            let mut columns = vec!["draft_token"];
            columns.extend(DRAFT_COLUMNS);
            let mut row = vec![text(&draft.token)];
            row.extend(desired);
            (insert(tx, "drafts", &columns, row)?, true)
        }
    };
    changed |= write_picks(tx, id, &draft.picks, now_ms)?;
    Ok(changed)
}

fn pick_values(pick: &Pick) -> Result<Vec<Value>, StoreError> {
    Ok(vec![
        int(i64::from(pick.pack_number)),
        int(i64::from(pick.pick_number)),
        opt_u64(pick.pack_id),
        opt_u32(pick.direction),
        Value::Text(serde_json::to_string(&pick.cards_available)?),
        opt_u32(pick.picked_catalog_id),
        opt_u64(pick.picked_card_id),
        Value::Text(reservations_json(&pick.reservations)?),
        opt_int(pick.shown_at_ms),
        opt_int(pick.deadline_at_ms),
        opt_int(pick.picked_at_ms),
        Value::Null,
    ])
}

/// Store a draft's picks by ordinal. A stored pick the fold no longer makes
/// is deleted, or detached when it carries a note.
fn write_picks(
    tx: &Transaction,
    draft_id: i64,
    picks: &[Pick],
    now_ms: i64,
) -> Result<bool, StoreError> {
    let mut stored: HashMap<i64, (Vec<Value>, bool)> = HashMap::new();
    {
        let mut stmt = tx.prepare_cached(&format!(
            "SELECT ordinal, note IS NOT NULL, {} FROM draft_picks WHERE draft_id = ?1",
            PICK_COLUMNS.join(", ")
        ))?;
        let rows = stmt.query_map([draft_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                (values(row, 2, PICK_COLUMNS.len())?, row.get::<_, bool>(1)?),
            ))
        })?;
        for row in rows {
            let (ordinal, found) = row?;
            stored.insert(ordinal, found);
        }
    }
    let mut changed = false;
    for pick in picks {
        let ordinal = i64::from(pick.ordinal);
        let desired = pick_values(pick)?;
        let wrote = match stored.remove(&ordinal) {
            Some((have, _)) => write_changed(
                tx,
                "draft_picks",
                &[("draft_id", int(draft_id)), ("ordinal", int(ordinal))],
                &PICK_COLUMNS,
                &have,
                &desired,
            )?,
            None => {
                let mut columns = vec!["draft_id", "ordinal"];
                columns.extend(PICK_COLUMNS);
                let mut row = vec![int(draft_id), int(ordinal)];
                row.extend(desired);
                insert(tx, "draft_picks", &columns, row)?;
                true
            }
        };
        if wrote {
            // The pack's cards and the pick get `cards` rows (0.x
            // `ApplyDraftEvent`).
            let mut ids = pick.cards_available.clone();
            ids.extend(pick.picked_catalog_id);
            crate::cards::ensure_id_stubs(tx, &ids)?;
        }
        changed |= wrote;
    }
    for (ordinal, (have, noted)) in stored {
        if noted {
            // `detached_at` is the last pick column.
            if have.last() == Some(&Value::Null) {
                tx.prepare_cached(
                    "UPDATE draft_picks SET detached_at = ?1 WHERE draft_id = ?2 AND ordinal = ?3",
                )?
                .execute(params![now_ms, draft_id, ordinal])?;
                changed = true;
            }
        } else {
            tx.prepare_cached("DELETE FROM draft_picks WHERE draft_id = ?1 AND ordinal = ?2")?
                .execute(params![draft_id, ordinal])?;
            changed = true;
        }
    }
    Ok(changed)
}

/// The account's log drafts the fold no longer makes: deleted, or detached
/// when a pick carries a note.
fn retire_log_drafts(
    tx: &Transaction,
    account_id: i64,
    folded: &HashSet<String>,
    now_ms: i64,
) -> Result<bool, StoreError> {
    let mut gone: Vec<(i64, bool, bool)> = Vec::new();
    {
        let mut stmt = tx.prepare_cached(
            "SELECT d.id, d.draft_token, d.detached_at IS NOT NULL,
                 EXISTS (SELECT 1 FROM draft_picks p WHERE p.draft_id = d.id
                         AND p.note IS NOT NULL)
             FROM drafts d WHERE d.source = 'log' AND d.account_id = ?1",
        )?;
        let rows = stmt.query_map([account_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
                row.get::<_, bool>(3)?,
            ))
        })?;
        for row in rows {
            let (id, token, detached, noted) = row?;
            if !folded.contains(&token) {
                gone.push((id, detached, noted));
            }
        }
    }
    let mut changed = false;
    for (id, detached, noted) in gone {
        if noted {
            if !detached {
                tx.prepare_cached(
                    "UPDATE drafts SET detached_at = ?1, abandons_at = NULL WHERE id = ?2",
                )?
                .execute(params![now_ms, id])?;
                changed = true;
            }
        } else {
            tx.prepare_cached("DELETE FROM drafts WHERE id = ?1")?
                .execute([id])?;
            changed = true;
        }
    }
    Ok(changed)
}

/// A stored 0.x draft, as a fold seed.
struct ImportedDraftRow {
    id: i64,
    /// The 0.x `drafts.id`.
    legacy_id: i64,
    /// The 0.x league it belonged to.
    league_from: Option<i64>,
    state: String,
    abandons_at: Option<i64>,
    draft: Draft,
}

/// The owner's 0.x drafts (v1 has no events for their token), with their
/// picks in ordinal order.
fn drafts_from_0x(
    tx: &Transaction,
    owner: LeagueOwner,
) -> Result<Vec<ImportedDraftRow>, StoreError> {
    let mut drafts: Vec<ImportedDraftRow> = Vec::new();
    {
        let mut stmt = tx.prepare_cached(&format!(
            "SELECT d.id, d.imported_from, d.imported_league_from, d.abandons_at, {DRAFT_SELECT}
             FROM drafts d WHERE d.source = 'imported' AND d.account_id IS ?1
             ORDER BY d.started_at, d.imported_from"
        ))?;
        let rows = stmt.query_map([owner.account()], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                draft_at(row, 4)?,
            ))
        })?;
        for row in rows {
            let (id, legacy_id, league_from, abandons_at, (mut draft, state)) = row?;
            draft.state = parse_draft_state(&state)?;
            drafts.push(ImportedDraftRow {
                id,
                legacy_id,
                league_from,
                state,
                abandons_at,
                draft,
            });
        }
    }
    if drafts.is_empty() {
        return Ok(drafts);
    }
    let account = owner.account();
    let mut picks = stored_picks(
        tx,
        "d.source = 'imported' AND d.account_id IS ?1",
        &[&account],
    )?;
    for found in &mut drafts {
        found.draft.picks = picks.remove(&found.id).unwrap_or_default();
    }
    Ok(drafts)
}

// ---- Registered decks --------------------------------------------------------

/// A registered deck read from a log line, with the line's position.
struct FoundDeck {
    file: i64,
    offset: i64,
    deck: RegisteredDeck,
}

/// Store each match's newest readable registered deck and return the deck
/// the fold reads for each (`registered`, else 0.x's `imported`), by token.
fn sync_snapshots(
    tx: &Transaction,
    tokens: &[String],
    out: &mut SyncOutcome,
) -> Result<HashMap<String, RegisteredDeck>, StoreError> {
    let mut decks = HashMap::new();
    for token in tokens {
        let match_id: Option<i64> = tx
            .prepare_cached("SELECT id FROM matches WHERE token = ?1")?
            .query_row([token], |row| row.get(0))
            .optional()?;
        let Some(match_id) = match_id else {
            continue;
        };
        let stored: Option<(i64, i64)> = tx
            .prepare_cached(
                "SELECT event_file_id, event_offset FROM limited_deck_snapshots
                 WHERE match_id = ?1 AND source = 'registered'",
            )?
            .query_row([match_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()?;
        let mut found = newest_live_deck(tx, token, stored, &mut out.skipped_snapshots)?;
        let archive = if found.is_none() && stored.is_none() {
            compacted_at(tx, token)?
        } else {
            None
        };
        if let Some(archive) = archive {
            let checked: Option<i64> = tx
                .prepare_cached("SELECT registered_deck_checked_at FROM matches WHERE id = ?1")?
                .query_row([match_id], |row| row.get(0))?;
            // Read once: from then on the row (or the checked mark, when the
            // archive has no registered line) stands for the archive.
            if checked != Some(archive) {
                match newest_archived_deck(tx, token) {
                    Ok(Some(deck)) => found = Some(deck),
                    Ok(None) => {
                        tx.prepare_cached(
                            "UPDATE matches SET registered_deck_checked_at = ?2 WHERE id = ?1",
                        )?
                        .execute(params![match_id, archive])?;
                    }
                    Err(StoreError::Sqlite(error)) => return Err(StoreError::Sqlite(error)),
                    Err(_) => {
                        if !out.skipped_snapshots.contains(token) {
                            out.skipped_snapshots.push(token.clone());
                        }
                    }
                }
            }
        }
        if let Some(found) = found {
            tx.prepare_cached(
                "INSERT INTO limited_deck_snapshots (match_id, source, cards, signature,
                     captured_at, event_file_id, event_offset)
                 VALUES (?1, 'registered', ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (match_id, source) DO UPDATE SET cards = excluded.cards,
                     signature = excluded.signature, captured_at = excluded.captured_at,
                     event_file_id = excluded.event_file_id,
                     event_offset = excluded.event_offset",
            )?
            .execute(params![
                match_id,
                cards_json(&found.deck.cards)?,
                found.deck.signature,
                found.deck.captured_at_ms,
                found.file,
                found.offset
            ])?;
            let ids: Vec<u32> = found
                .deck
                .cards
                .iter()
                .map(|card| card.catalog_id)
                .collect();
            crate::cards::ensure_id_stubs(tx, &ids)?;
            out.limited_changed = true;
        }
        let snapshot: Option<(String, i64)> = tx
            .prepare_cached(
                "SELECT cards, captured_at FROM limited_deck_snapshots WHERE match_id = ?1
                 ORDER BY source = 'registered' DESC LIMIT 1",
            )?
            .query_row([match_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()?;
        if let Some((cards, captured_at)) = snapshot {
            decks.insert(
                token.clone(),
                registered(token, captured_at, stored_cards(&cards)?),
            );
        }
    }
    Ok(decks)
}

/// The newest readable `match_deck_registered` line of the match still in
/// `log_events` that is newer than `after` (the stored row's line). A line
/// whose stored text cannot be decoded is skipped like an unreadable one,
/// and the match is added to `skipped` (the caller logs it).
fn newest_live_deck(
    tx: &Transaction,
    token: &str,
    after: Option<(i64, i64)>,
    skipped: &mut Vec<String>,
) -> Result<Option<FoundDeck>, StoreError> {
    let mut stmt = tx.prepare_cached(
        "SELECT log_file_id, byte_offset, logged_at, raw, compressed FROM log_events
         WHERE match_token = ?1 AND kind = 'match_deck_registered' AND quarantined = 0
         ORDER BY log_file_id DESC, byte_offset DESC",
    )?;
    let mut rows = stmt.query([token])?;
    while let Some(row) = rows.next()? {
        let file: i64 = row.get(0)?;
        let offset: i64 = row.get(1)?;
        if after.is_some_and(|stored| (file, offset) <= stored) {
            break;
        }
        let raw = match decode_raw(row.get(3)?, row.get(4)?) {
            Ok(raw) => raw,
            Err(StoreError::Sqlite(error)) => return Err(StoreError::Sqlite(error)),
            Err(_) => {
                if !skipped.iter().any(|known| known == token) {
                    skipped.push(token.to_string());
                }
                continue;
            }
        };
        if let Some(cards) = parse_registered(&raw, token) {
            return Ok(Some(FoundDeck {
                file,
                offset,
                deck: registered(token, row.get(2)?, cards),
            }));
        }
    }
    Ok(None)
}

/// The newest readable registered deck line in a compacted match's archive
/// (its live lines are read by `newest_live_deck`). An unreadable archive is
/// an error.
fn newest_archived_deck(tx: &Transaction, token: &str) -> Result<Option<FoundDeck>, StoreError> {
    let mut lines: Vec<(i64, i64, i64, String)> = Vec::new();
    for event in archived_events(tx, token)? {
        lines.push((
            event.log_file_id,
            i64::try_from(event.byte_offset).unwrap_or(i64::MAX),
            event.logged_at_ms,
            event.raw,
        ));
    }
    lines.sort_by_key(|(file, offset, _, _)| std::cmp::Reverse((*file, *offset)));
    for (file, offset, at, raw) in lines {
        if let Some(cards) = parse_registered(&raw, token) {
            return Ok(Some(FoundDeck {
                file,
                offset,
                deck: registered(token, at, cards),
            }));
        }
    }
    Ok(None)
}

// ---- League rows -------------------------------------------------------------

/// The league columns the sync writes, in this order.
const LEAGUE_COLUMNS: [&str; 16] = [
    "kind",
    "state",
    "token",
    "event_id",
    "mtgo_course_id",
    "name",
    "format",
    "set_code",
    "deck_version_id",
    "deck_id",
    "started_at",
    "joined_at",
    "completed_at",
    "dropped_at",
    "detached_at",
    "rounds",
];

pub(crate) fn state_str(state: RunState) -> &'static str {
    match state {
        RunState::Active => "active",
        RunState::Complete => "complete",
        RunState::Partial => "partial",
        RunState::Dropped => "dropped",
    }
}

fn run_values(tx: &Transaction, run: &Run) -> Result<Vec<Value>, StoreError> {
    let deck_id: Option<i64> = match run.deck_version_id {
        Some(version) => tx
            .prepare_cached("SELECT deck_id FROM deck_versions WHERE id = ?1")?
            .query_row([version], |row| row.get(0))
            .optional()?,
        None => None,
    };
    Ok(vec![
        text(run.kind.as_str()),
        text(state_str(run.state)),
        text(&run.token),
        opt_int(run.event_id),
        opt_int(run.course_id),
        text(&run.name),
        opt_text(run.format.as_deref()),
        opt_text(run.set_code.as_deref()),
        opt_int(run.deck_version_id),
        opt_int(deck_id),
        int(run.started_at),
        opt_int(run.joined_at),
        opt_int(run.completed_at),
        opt_int(run.dropped_at),
        Value::Null,
        opt_int(run.rounds.map(|rounds| rounds as i64)),
    ])
}

/// The player's status columns of a row (self-management spec 3.2): moved
/// together between two rows of one run, never one alone. The drop time
/// travels with the status it belongs to.
#[derive(Debug, Clone, Default, PartialEq)]
struct PlayerStatus {
    removed_at: Option<i64>,
    /// `leagues.user_state`; a drop time counts only with `dropped`.
    user_state: Option<String>,
    user_dropped_at: Option<i64>,
    edited_at: Option<i64>,
}

impl PlayerStatus {
    fn is_set(&self) -> bool {
        self.removed_at.is_some()
            || self.user_state.is_some()
            || self.user_dropped_at.is_some()
            || self.edited_at.is_some()
    }

    /// Whether this status replaces `other` when two rows of one run meet:
    /// the other has none, or this one was edited later (NULL is oldest).
    fn beats(&self, other: &PlayerStatus) -> bool {
        self.is_set() && (!other.is_set() || self.edited_at > other.edited_at)
    }
}

/// One of the owner's stored runs (never a manual one).
struct LeagueRow {
    id: i64,
    run_key: String,
    source: String,
    notes: Option<String>,
    /// When the player last edited the note in v1 (`None`: never, or 0.x's).
    notes_edited_at: Option<i64>,
    detached_at: Option<i64>,
    status: PlayerStatus,
    /// A match is linked to this row by hand (`league_linked = 1`).
    linked: bool,
    /// A listed match (not hidden, not detached) is linked to this row by
    /// hand: the row has a member, so it stays attached without a fold run.
    listed_link: bool,
    /// `LEAGUE_COLUMNS`.
    values: Vec<Value>,
    /// Deleted by a hand-over.
    gone: bool,
}

impl LeagueRow {
    /// Whether the row holds player input, so it is detached instead of
    /// deleted when no run takes it: a note, a drop, a status, or a
    /// hand-linked match (deleting the row would `SET NULL` the link, which
    /// the baseline CHECK refuses).
    fn carries_input(&self) -> bool {
        self.notes.is_some() || self.status.is_set() || self.linked
    }
}

/// The ids of the rows a match is linked to by hand, each with whether one
/// of those matches is listed (not hidden, not detached). One scan of
/// `matches` (there is no index on `league_id`).
fn linked_rows(tx: &Transaction) -> Result<HashMap<i64, bool>, StoreError> {
    let mut stmt = tx.prepare_cached(
        "SELECT league_id, MAX(hidden_at IS NULL AND detached_at IS NULL) FROM matches
         WHERE league_linked = 1 GROUP BY league_id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, bool>(1)?))
    })?;
    let mut found = HashMap::new();
    for row in rows {
        let (id, listed) = row?;
        found.insert(id, listed);
    }
    Ok(found)
}

fn owner_rows(tx: &Transaction, owner: LeagueOwner) -> Result<Vec<LeagueRow>, StoreError> {
    let linked = linked_rows(tx)?;
    let mut stmt = tx.prepare_cached(&format!(
        "SELECT id, run_key, source, notes, user_dropped_at, detached_at, notes_edited_at,
             removed_at, user_state, edited_at, {}
         FROM leagues WHERE source <> 'manual' AND account_id IS ?1 ORDER BY id",
        LEAGUE_COLUMNS.join(", ")
    ))?;
    let rows = stmt.query_map([owner.account()], |row| {
        let id: i64 = row.get(0)?;
        Ok(LeagueRow {
            id,
            run_key: row.get(1)?,
            source: row.get(2)?,
            notes: row.get(3)?,
            detached_at: row.get(5)?,
            notes_edited_at: row.get(6)?,
            status: PlayerStatus {
                removed_at: row.get(7)?,
                user_state: row.get(8)?,
                user_dropped_at: row.get(4)?,
                edited_at: row.get(9)?,
            },
            linked: linked.contains_key(&id),
            listed_link: linked.get(&id).copied().unwrap_or(false),
            values: values(row, 10, LEAGUE_COLUMNS.len())?,
            gone: false,
        })
    })?;
    let mut found = Vec::new();
    for row in rows {
        found.push(row?);
    }
    Ok(found)
}

/// Which row each fold run took.
struct Assignment {
    /// By run, in fold order; `None` for a 0.x run whose row is gone.
    ids: Vec<Option<i64>>,
    /// A row took a new key by its matches while carrying a drop.
    refold: bool,
    /// A row was rekeyed, inserted, handed input or removed.
    changed: bool,
}

fn rekey(tx: &Transaction, row: &mut LeagueRow, run_key: &str) -> Result<(), StoreError> {
    tx.prepare_cached("UPDATE leagues SET run_key = ?1 WHERE id = ?2")?
        .execute(params![run_key, row.id])?;
    row.run_key = run_key.to_string();
    Ok(())
}

/// The loser's player status goes to the winner when it is newer (or the
/// winner has none), as one unit with its drop time; the loser's is cleared
/// either way, so a retired row never keeps a stale status. Returns whether
/// a row changed.
fn move_status(
    tx: &Transaction,
    rows: &mut [LeagueRow],
    winner: usize,
    loser: usize,
) -> Result<bool, StoreError> {
    if !rows[loser].status.is_set() {
        return Ok(false);
    }
    let status = std::mem::take(&mut rows[loser].status);
    tx.prepare_cached(
        "UPDATE leagues SET removed_at = NULL, user_state = NULL, user_dropped_at = NULL,
             edited_at = NULL
         WHERE id = ?1",
    )?
    .execute([rows[loser].id])?;
    if status.beats(&rows[winner].status) {
        tx.prepare_cached(
            "UPDATE leagues SET removed_at = ?1, user_state = ?2, user_dropped_at = ?3,
                 edited_at = ?4
             WHERE id = ?5",
        )?
        .execute(params![
            status.removed_at,
            status.user_state,
            status.user_dropped_at,
            status.edited_at,
            rows[winner].id
        ])?;
        rows[winner].status = status;
    }
    Ok(true)
}

/// The loser's hand-linked matches move to the winner (self-management spec
/// 4.1). Returns whether any moved.
fn move_links(
    tx: &Transaction,
    rows: &mut [LeagueRow],
    winner: usize,
    loser: usize,
) -> Result<bool, StoreError> {
    if !rows[loser].linked {
        return Ok(false);
    }
    tx.prepare_cached(
        "UPDATE matches SET league_id = ?1 WHERE league_id = ?2 AND league_linked = 1",
    )?
    .execute(params![rows[winner].id, rows[loser].id])?;
    rows[loser].linked = false;
    rows[winner].linked = true;
    rows[winner].listed_link |= rows[loser].listed_link;
    rows[loser].listed_link = false;
    Ok(true)
}

/// The hidden matches deleted from the loser row (`deleted_from_league_id`,
/// league self-management spec section 13) now name the winner, so a
/// Restore or the Deleted list still finds their league once the loser row
/// is gone (its delete would clear the column).
fn move_deleted_from(
    tx: &Transaction,
    rows: &[LeagueRow],
    winner: usize,
    loser: usize,
) -> Result<(), StoreError> {
    tx.prepare_cached(
        "UPDATE matches SET deleted_from_league_id = ?1 WHERE deleted_from_league_id = ?2",
    )?
    .execute(params![rows[winner].id, rows[loser].id])?;
    Ok(())
}

/// A row retired by a late adoption hands its notes to the adopting run's
/// row where that row has none, and its player status (with its drop, when
/// newer), hand-linked matches and the record of the matches deleted from it
/// always; it is then deleted, or detached when it still carries input (both
/// rows had notes).
fn hand_over(
    tx: &Transaction,
    rows: &mut [LeagueRow],
    winner: usize,
    loser: usize,
    now_ms: i64,
) -> Result<bool, StoreError> {
    let mut changed = false;
    if rows[winner].notes.is_none() && rows[loser].notes.is_some() {
        let notes = rows[loser].notes.take();
        let edited = rows[loser].notes_edited_at.take();
        tx.prepare_cached("UPDATE leagues SET notes = NULL, notes_edited_at = NULL WHERE id = ?1")?
            .execute([rows[loser].id])?;
        tx.prepare_cached("UPDATE leagues SET notes = ?1, notes_edited_at = ?2 WHERE id = ?3")?
            .execute(params![notes, edited, rows[winner].id])?;
        rows[winner].notes = notes;
        rows[winner].notes_edited_at = edited;
        changed = true;
    }
    changed |= move_status(tx, rows, winner, loser)?;
    changed |= move_links(tx, rows, winner, loser)?;
    move_deleted_from(tx, rows, winner, loser)?;
    changed |= take_synced_id(tx, rows[winner].id, rows[loser].id)?;
    let loser_row = &mut rows[loser];
    if loser_row.carries_input() {
        if loser_row.detached_at.is_none() {
            tx.prepare_cached("UPDATE leagues SET detached_at = ?1 WHERE id = ?2")?
                .execute(params![now_ms, loser_row.id])?;
            loser_row.detached_at = Some(now_ms);
            changed = true;
        }
    } else {
        tx.prepare_cached("DELETE FROM leagues WHERE id = ?1")?
            .execute([loser_row.id])?;
        loser_row.gone = true;
        changed = true;
    }
    Ok(changed)
}

/// The log row of a run an import claimed gives its notes, player status
/// and hand-linked matches to the imported row. A note only fills an
/// imported row that holds no v1-typed note (`notes_edited_at` unset): a v1
/// note beats 0.x's (the import is a one-time migration). Of several log
/// rows, the most recently edited note wins (`carried` holds each winner's
/// current pick, `(edited at, row id)`).
/// A blank note counts as none. Its player status (`removed_at`,
/// `user_state` with its drop time, `edited_at`) goes with it when newer than
/// the imported row's (NULL is oldest, so of two unedited statuses the
/// imported row keeps its own), and its hand-linked matches and the record
/// of the matches deleted from it always. The log row is then deleted.
fn carry_to_imported(
    tx: &Transaction,
    rows: &mut [LeagueRow],
    carried: &mut HashMap<usize, (i64, i64)>,
    winner: usize,
    loser: usize,
) -> Result<(), StoreError> {
    let note = rows[loser]
        .notes
        .as_deref()
        .is_some_and(|notes| !notes.trim().is_empty());
    let rank = (rows[loser].notes_edited_at.unwrap_or(0), rows[loser].id);
    let typed_in_v1 = rows[winner].notes_edited_at.is_some() && !carried.contains_key(&winner);
    let newer = carried.get(&winner).is_none_or(|best| rank > *best);
    if note && !typed_in_v1 && newer {
        let notes = rows[loser].notes.clone();
        let edited = rows[loser].notes_edited_at;
        tx.prepare_cached("UPDATE leagues SET notes = ?1, notes_edited_at = ?2 WHERE id = ?3")?
            .execute(params![notes, edited, rows[winner].id])?;
        rows[winner].notes = notes;
        rows[winner].notes_edited_at = edited;
        carried.insert(winner, rank);
    }
    move_status(tx, rows, winner, loser)?;
    move_links(tx, rows, winner, loser)?;
    move_deleted_from(tx, rows, winner, loser)?;
    take_synced_id(tx, rows[winner].id, rows[loser].id)?;
    tx.prepare_cached("DELETE FROM leagues WHERE id = ?1")?
        .execute([rows[loser].id])?;
    rows[loser].gone = true;
    Ok(())
}

/// The sync client id of league row `league_id` (`leagues.sync_client_id`).
fn sync_client_id(tx: &Transaction, league_id: i64) -> Result<Option<String>, StoreError> {
    Ok(tx
        .prepare_cached("SELECT sync_client_id FROM leagues WHERE id = ?1")?
        .query_row([league_id], |row| row.get(0))?)
}

/// Whether league client id `client_id` has a server copy: a `synced_rows`
/// row with a hash (sent or pulled). A row the sync only built, or that
/// only holds a carry, has no hash and does not count.
fn is_uploaded(tx: &Transaction, client_id: &str) -> Result<bool, StoreError> {
    Ok(tx
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM synced_rows
                            WHERE type = 'league' AND client_id = ?1 AND hash IS NOT NULL)",
        )?
        .query_row([client_id], |row| row.get(0))?)
}

/// Whether league row `league_id` came from a 0.x database (`imported`
/// with a positive 0.x id; a pulled run's is negative).
fn is_0x_row(tx: &Transaction, league_id: i64) -> Result<bool, StoreError> {
    Ok(tx
        .prepare_cached(
            "SELECT source = 'imported' AND COALESCE(imported_from, 0) > 0
             FROM leagues WHERE id = ?1",
        )?
        .query_row([league_id], |row| row.get(0))?)
}

/// When two rows of one run merge (sync identity spec 2.4), the winner
/// takes the loser's sync client id only when the loser's id was uploaded
/// (`is_uploaded`) and the winner's was not, so the league keeps the server
/// copy it already has. A 0.x row never takes one: it keeps the id 0.x
/// uploaded it under, whose synced rows the 0.x import brings in after the
/// league sync. The loser takes the winner's old id in
/// exchange: a loser kept detached (`hand_over`) then never shares an id
/// with the winner, and the id it holds has no server copy. Runs before the
/// loser is deleted or detached. Returns whether the ids moved.
fn take_synced_id(tx: &Transaction, winner_id: i64, loser_id: i64) -> Result<bool, StoreError> {
    let Some(loser_client_id) = sync_client_id(tx, loser_id)? else {
        return Ok(false);
    };
    let Some(winner_client_id) = sync_client_id(tx, winner_id)? else {
        // A winner with no token is never sent, so it takes no id.
        return Ok(false);
    };
    if winner_client_id == loser_client_id
        || is_0x_row(tx, winner_id)?
        || !is_uploaded(tx, &loser_client_id)?
        || is_uploaded(tx, &winner_client_id)?
    {
        return Ok(false);
    }
    let mut set = tx.prepare_cached("UPDATE leagues SET sync_client_id = ?2 WHERE id = ?1")?;
    set.execute(params![winner_id, loser_client_id])?;
    set.execute(params![loser_id, winner_client_id])?;
    Ok(true)
}

/// Give each fold run its row: the row with its key, else the row of one of
/// its former keys, else the unmatched log row holding most of its matches
/// (lowest id on a tie; a row the player deleted too, which stays deleted),
/// else a new row.
fn assign_rows(
    tx: &Transaction,
    owner: LeagueOwner,
    runs: &[Run],
    now_ms: i64,
) -> Result<Assignment, StoreError> {
    let mut rows = owner_rows(tx, owner)?;
    let mut by_key: HashMap<String, usize> = HashMap::new();
    let mut by_id: HashMap<i64, usize> = HashMap::new();
    for (index, row) in rows.iter().enumerate() {
        by_key.insert(row.run_key.clone(), index);
        by_id.insert(row.id, index);
    }
    let mut claims: Vec<Option<usize>> = vec![None; runs.len()];
    let mut taken: HashSet<usize> = HashSet::new();
    let mut changed = false;
    let mut refold = false;

    // The same key.
    for (run_index, run) in runs.iter().enumerate() {
        if let Some(&row) = by_key.get(&run.key) {
            claims[run_index] = Some(row);
            taken.insert(row);
        }
    }

    // A former key: a late adoption.
    for (run_index, run) in runs.iter().enumerate() {
        for key in &run.former_keys {
            let Some(&row) = by_key.get(key) else {
                continue;
            };
            // Former keys are `draft:` keys of log rows; an imported row is
            // never rekeyed, handed over or deleted here.
            if rows[row].source != "log" || !taken.insert(row) {
                continue;
            }
            match claims[run_index] {
                None => {
                    rekey(tx, &mut rows[row], &run.key)?;
                    claims[run_index] = Some(row);
                    changed = true;
                }
                Some(winner) => {
                    if rows[row].source == "log" && hand_over(tx, &mut rows, winner, row, now_ms)? {
                        changed = true;
                        // A drop or status that moved keys may fold
                        // differently under the winner's key: fold again.
                        refold = true;
                    }
                }
            }
        }
    }

    // A 0.x run that claimed a log run's matches: the log row's note,
    // player status and hand-linked matches move to the imported row
    // (`carry_to_imported`). The log row is the run keyed by its league
    // token and one of the 0.x run's matches, when the fold made no run of
    // that key (it is the run the claim replaced).
    let mut carried: HashMap<usize, (i64, i64)> = HashMap::new();
    for (run_index, run) in runs.iter().enumerate() {
        let Some(winner) = claims[run_index] else {
            continue;
        };
        if !run.key.starts_with("0x:") || rows[winner].source != "imported" {
            continue;
        }
        for token in &run.match_tokens {
            let key = format!("{}:{}", run.token, token);
            let Some(&row) = by_key.get(&key) else {
                continue;
            };
            if rows[row].source != "log"
                || rows[row].gone
                || taken.contains(&row)
                || !rows[row].carries_input()
                || runs.iter().any(|other| other.key == key)
            {
                continue;
            }
            taken.insert(row);
            carry_to_imported(tx, &mut rows, &mut carried, winner, row)?;
            changed = true;
            refold = true;
        }
    }

    // The unmatched log row holding most of the run's matches.
    let mut made: HashSet<&str> = HashSet::new();
    for run in runs {
        made.insert(run.key.as_str());
        for key in &run.former_keys {
            made.insert(key.as_str());
        }
    }
    let mut league_of: HashMap<String, i64> = HashMap::new();
    if runs.iter().zip(&claims).any(|(_, claim)| claim.is_none()) {
        let mut stmt = tx.prepare_cached(
            "SELECT token, league_id FROM matches
             WHERE league_id IS NOT NULL AND league_linked = 0 AND account_id IS ?1",
        )?;
        let found = stmt.query_map([owner.account()], |row| Ok((row.get(0)?, row.get(1)?)))?;
        for row in found {
            let (token, league_id) = row?;
            league_of.insert(token, league_id);
        }
    }
    for (run_index, run) in runs.iter().enumerate() {
        if claims[run_index].is_some() {
            continue;
        }
        let mut counts: HashMap<usize, usize> = HashMap::new();
        for token in &run.match_tokens {
            let Some(row) = league_of.get(token).and_then(|id| by_id.get(id)).copied() else {
                continue;
            };
            let candidate = &rows[row];
            // A row the player deleted is taken too and stays deleted (spec
            // 4.4): only its own kept matches point at it, so the run is
            // that league.
            if !taken.contains(&row)
                && !candidate.gone
                && candidate.source == "log"
                && !made.contains(candidate.run_key.as_str())
            {
                *counts.entry(row).or_insert(0) += 1;
            }
        }
        let best = counts
            .into_iter()
            .max_by(|(a, a_count), (b, b_count)| {
                a_count.cmp(b_count).then(rows[*b].id.cmp(&rows[*a].id))
            })
            .map(|(row, _)| row);
        if let Some(row) = best {
            taken.insert(row);
            rekey(tx, &mut rows[row], &run.key)?;
            if rows[row].status.user_dropped_at.is_some() {
                refold = true;
            }
            claims[run_index] = Some(row);
            changed = true;
        }
    }

    // A new row.
    let mut ids = Vec::with_capacity(runs.len());
    for (run_index, run) in runs.iter().enumerate() {
        match claims[run_index] {
            Some(row) => ids.push(Some(rows[row].id)),
            // A 0.x run's row is the import's; the sync never makes one.
            None if run.key.starts_with("0x:") => ids.push(None),
            None if let Some(id) = claim_other_owners_row(tx, owner, &run.key)? => {
                ids.push(Some(id));
                changed = true;
            }
            None => {
                let mut columns = vec!["run_key", "account_id", "source", "created_at"];
                columns.extend(LEAGUE_COLUMNS);
                let mut row = vec![
                    text(&run.key),
                    opt_int(owner.account()),
                    text("log"),
                    int(now_ms),
                ];
                row.extend(run_values(tx, run)?);
                ids.push(Some(insert(tx, "leagues", &columns, row)?));
                changed = true;
            }
        }
    }
    Ok(Assignment {
        ids,
        refold,
        changed,
    })
}

/// The log row of another owner that holds `run_key` (keys are unique over
/// every owner), moved to `owner`. It happens when a run follows its draft
/// to another owner: an account-less 0.x draft minted `draft:{token}` in the
/// `Nobody` sync, then the account's log has events for that token.
fn claim_other_owners_row(
    tx: &Transaction,
    owner: LeagueOwner,
    run_key: &str,
) -> Result<Option<i64>, StoreError> {
    let found: Option<i64> = tx
        .prepare_cached(
            "SELECT id FROM leagues WHERE run_key = ?1 AND source = 'log'
                 AND account_id IS NOT ?2",
        )?
        .query_row(params![run_key, owner.account()], |row| row.get(0))
        .optional()?;
    if let Some(id) = found {
        tx.prepare_cached("UPDATE leagues SET account_id = ?1 WHERE id = ?2")?
            .execute(params![owner.account(), id])?;
    }
    Ok(found)
}

/// Write each run's columns into its row where they changed. Returns
/// whether a constructed run changed and whether a limited one did.
fn write_runs(
    tx: &Transaction,
    owner: LeagueOwner,
    runs: &[Run],
    ids: &[Option<i64>],
) -> Result<(bool, bool), StoreError> {
    let stored: HashMap<i64, Vec<Value>> = owner_rows(tx, owner)?
        .into_iter()
        .map(|row| (row.id, row.values))
        .collect();
    let mut constructed = false;
    let mut limited = false;
    for (run, id) in runs.iter().zip(ids) {
        let Some(id) = id else {
            continue;
        };
        let Some(have) = stored.get(id) else {
            continue;
        };
        let desired = run_values(tx, run)?;
        if write_changed(
            tx,
            "leagues",
            &[("id", int(*id))],
            &LEAGUE_COLUMNS,
            have,
            &desired,
        )? {
            if run.kind.is_limited() {
                limited = true;
            } else {
                constructed = true;
            }
        }
    }
    Ok((constructed, limited))
}

/// The owner's rows no run took: deleted, or detached when they carry input
/// (a note, a drop, a player status, a hand-linked match) or are imported.
/// A log row with a listed hand-linked match is neither: the match is its
/// member by `league_id` (spec 4.1), so the row stays attached (or is
/// attached again) without a fold run. Otherwise a log league whose matches
/// were restored before it, or whose folded matches all moved away, would
/// stay detached and out of every league read. With no fold run to write
/// its `state`, such a row left `active` takes `partial` (issue #415): a
/// stale `active` would make it the run in progress and refuse Delete, and
/// the app does not know how it ended. Any other state, and the player's
/// `user_state`, is left alone.
fn remove_leftovers(
    tx: &Transaction,
    owner: LeagueOwner,
    ids: &[Option<i64>],
    now_ms: i64,
) -> Result<bool, StoreError> {
    let kept: HashSet<i64> = ids.iter().flatten().copied().collect();
    let mut changed = false;
    for row in owner_rows(tx, owner)? {
        if kept.contains(&row.id) {
            continue;
        }
        if row.source == "log" && row.listed_link {
            if row.detached_at.is_some() {
                tx.prepare_cached("UPDATE leagues SET detached_at = NULL WHERE id = ?1")?
                    .execute([row.id])?;
                changed = true;
            }
            let ended = tx
                .prepare_cached(
                    "UPDATE leagues SET state = 'partial' WHERE id = ?1 AND state = 'active'",
                )?
                .execute([row.id])?;
            if ended > 0 {
                changed = true;
            }
            continue;
        }
        let keep = row.source == "imported" || row.carries_input();
        if keep {
            if row.detached_at.is_none() {
                tx.prepare_cached("UPDATE leagues SET detached_at = ?1 WHERE id = ?2")?
                    .execute(params![now_ms, row.id])?;
                changed = true;
            }
        } else {
            tx.prepare_cached("DELETE FROM leagues WHERE id = ?1")?
                .execute([row.id])?;
            changed = true;
        }
    }
    Ok(changed)
}

/// `matches.league_id` of the account's unlinked matches: the row of the run
/// holding the match, else none. Returns whether a constructed match changed
/// and whether a limited one did.
fn write_match_leagues(
    tx: &Transaction,
    account_id: i64,
    runs: &[Run],
    ids: &[Option<i64>],
) -> Result<(bool, bool), StoreError> {
    let mut wanted: HashMap<&str, i64> = HashMap::new();
    for (run, id) in runs.iter().zip(ids) {
        if let Some(id) = id {
            for token in &run.match_tokens {
                wanted.insert(token.as_str(), *id);
            }
        }
    }
    let mut changes: Vec<(i64, Option<i64>, bool)> = Vec::new();
    {
        let mut stmt = tx.prepare_cached(
            "SELECT id, token, league_id, format FROM matches
             WHERE account_id = ?1 AND league_linked = 0",
        )?;
        let rows = stmt.query_map([account_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?;
        for row in rows {
            let (id, token, league_id, format) = row?;
            let desired = wanted.get(token.as_str()).copied();
            if desired != league_id {
                let limited = format.as_deref().is_some_and(is_limited_format_code);
                changes.push((id, desired, limited));
            }
        }
    }
    let mut constructed = false;
    let mut limited = false;
    for (id, league_id, is_limited) in changes {
        tx.prepare_cached("UPDATE matches SET league_id = ?1 WHERE id = ?2")?
            .execute(params![league_id, id])?;
        if is_limited {
            limited = true;
        } else {
            constructed = true;
        }
    }
    Ok((constructed, limited))
}

/// `drafts.league_id` of the owner's drafts: the row of the run holding the
/// draft, else none.
fn write_draft_leagues(
    tx: &Transaction,
    owner: LeagueOwner,
    runs: &[Run],
    ids: &[Option<i64>],
) -> Result<bool, StoreError> {
    let mut wanted: HashMap<&str, i64> = HashMap::new();
    for (run, id) in runs.iter().zip(ids) {
        if let (Some(id), Some(token)) = (id, &run.draft_token) {
            wanted.insert(token.as_str(), *id);
        }
    }
    let mut changes: Vec<(i64, Option<i64>)> = Vec::new();
    {
        let mut stmt = tx.prepare_cached(
            "SELECT id, draft_token, league_id, detached_at IS NOT NULL FROM drafts
             WHERE account_id IS ?1",
        )?;
        let rows = stmt.query_map([owner.account()], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, bool>(3)?,
            ))
        })?;
        for row in rows {
            let (id, token, league_id, detached) = row?;
            let desired = if detached {
                None
            } else {
                wanted.get(token.as_str()).copied()
            };
            if desired != league_id {
                changes.push((id, desired));
            }
        }
    }
    // A run holds one draft (a unique index): clear first, then set.
    for (id, _) in &changes {
        tx.prepare_cached("UPDATE drafts SET league_id = NULL WHERE id = ?1")?
            .execute([id])?;
    }
    for (id, league_id) in &changes {
        if league_id.is_some() {
            tx.prepare_cached("UPDATE drafts SET league_id = ?1 WHERE id = ?2")?
                .execute(params![league_id, id])?;
        }
    }
    Ok(!changes.is_empty())
}

// ---- Tournaments -------------------------------------------------------------

/// A round info naming a match counts from this long before its start
/// (MTGO logs the pairing before the match begins).
pub const ROUND_INFO_BEFORE_MS: i64 = 24 * 3_600_000;
/// ... and until this long after its end (or last activity).
pub const ROUND_INFO_AFTER_MS: i64 = 3_600_000;

/// 0.x `LinkMatchToTournament`: each log tournament match of the account
/// without a `tournament_token` takes the token of the newest
/// `tournament_round_info` event that names it, logged between a day before
/// its start and an hour after its end. Not found stays null (a later event
/// links it at a later sync). A token, once written, stays: the facts that
/// gave it do not change, so a sync per event and one sync at the end agree.
fn link_tournament_tokens(tx: &Transaction, account_id: i64) -> Result<bool, StoreError> {
    let mut unlinked: Vec<(i64, String, i64, i64)> = Vec::new();
    {
        let mut stmt = tx.prepare_cached(
            "SELECT id, token, started_at, COALESCE(ended_at, last_activity_at) FROM matches
             WHERE account_id = ?1 AND source = 'log' AND tournament_event_id IS NOT NULL
                 AND tournament_token IS NULL
             ORDER BY id",
        )?;
        let rows = stmt.query_map([account_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
        for row in rows {
            unlinked.push(row?);
        }
    }
    let mut changed = false;
    for (id, token, started_at, ended_at) in unlinked {
        let from = started_at.saturating_sub(ROUND_INFO_BEFORE_MS);
        let to = ended_at.max(started_at).saturating_add(ROUND_INFO_AFTER_MS);
        let Some(tournament_token) = newest_round_info_token(tx, &token, from, to)? else {
            continue;
        };
        tx.prepare_cached("UPDATE matches SET tournament_token = ?1 WHERE id = ?2")?
            .execute(params![tournament_token, id])?;
        changed = true;
    }
    Ok(changed)
}

/// The token of the newest round info event in `[from, to]` whose raw text
/// names `match_token` (0.x `raw_text LIKE %token%`, newest first) and whose
/// tournament token reads (a damaged newer line is passed over); `None` when
/// there is none.
fn newest_round_info_token(
    tx: &Transaction,
    match_token: &str,
    from: i64,
    to: i64,
) -> Result<Option<String>, StoreError> {
    // The first `kind IN` lets SQLite walk `log_events_tournament_kinds`.
    let mut stmt = tx.prepare_cached(
        "SELECT raw, compressed FROM log_events
         WHERE kind IN ('tournament_sync', 'tournament_state_changed', 'tournament_round_result',
                 'tournament_round_info', 'tournament_player_eliminated', 'tournament_ended')
             AND kind = 'tournament_round_info' AND logged_at BETWEEN ?1 AND ?2
         ORDER BY logged_at DESC, log_file_id DESC, byte_offset DESC",
    )?;
    let mut rows = stmt.query(params![from, to])?;
    while let Some(row) = rows.next()? {
        let raw: Vec<u8> = row.get(0)?;
        let compressed: bool = row.get(1)?;
        let text = decode_raw(raw, compressed)?;
        if text.contains(match_token)
            && let Some(token) = mymtgo_api::round_info_tournament_token(&text)
        {
            return Ok(Some(token));
        }
    }
    Ok(None)
}

/// The account of the lowest listed match id of a tournament event: the
/// owner of its one row when two accounts played in it.
fn event_owner(tx: &Transaction, event_id: i64) -> Result<Option<i64>, StoreError> {
    Ok(tx
        .prepare_cached(
            "SELECT account_id FROM listed_matches
             WHERE tournament_event_id = ?1 AND state <> 'pending'
             ORDER BY id LIMIT 1",
        )?
        .query_row([event_id], |row| row.get(0))
        .optional()?)
}

/// One row per tournament of the account's listed matches (its first match's
/// format and start; the synthesized name while `name_synthesized = 1`), and
/// each of the account's matches pointed at its tournament's row. It never
/// writes `name_checked_at` (the API worker's, 07); a synthesized row it
/// deletes takes its check with it, so a tournament that comes back is asked
/// about again.
fn sync_tournaments(tx: &Transaction, account_id: i64, tz: Tz) -> Result<bool, StoreError> {
    // The first match of each tournament, in start order, and the token of
    // its first match that has one.
    let mut firsts: Vec<(i64, Option<String>, i64)> = Vec::new();
    let mut tokens: HashMap<i64, String> = HashMap::new();
    {
        let mut seen: HashSet<i64> = HashSet::new();
        let mut stmt = tx.prepare_cached(
            "SELECT m.tournament_event_id, m.format, m.started_at, m.tournament_token
             FROM listed_matches m
             WHERE m.account_id = ?1 AND m.tournament_event_id IS NOT NULL
                 AND m.state <> 'pending'
             ORDER BY m.started_at, m.id",
        )?;
        let rows = stmt.query_map([account_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?;
        for row in rows {
            let (event_id, format, started_at, token) = row?;
            if seen.insert(event_id) {
                firsts.push((event_id, format, started_at));
            }
            if let Some(token) = token {
                tokens.entry(event_id).or_insert(token);
            }
        }
    }

    // (id, account, name, format, started_at, synthesized, token) by event id.
    type TournamentRow = (
        i64,
        Option<i64>,
        String,
        Option<String>,
        i64,
        bool,
        Option<String>,
    );
    let mut stored: HashMap<i64, TournamentRow> = HashMap::new();
    {
        let mut stmt = tx.prepare_cached(
            "SELECT mtgo_event_id, id, account_id, name, format, started_at, name_synthesized,
                 token
             FROM tournaments
             WHERE account_id = ?1 OR mtgo_event_id IN (
                 SELECT tournament_event_id FROM matches
                 WHERE account_id = ?1 AND tournament_event_id IS NOT NULL)",
        )?;
        let rows = stmt.query_map([account_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                (
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ),
            ))
        })?;
        for row in rows {
            let (event_id, found) = row?;
            stored.insert(event_id, found);
        }
    }

    let mut changed = false;
    let mut ids: HashMap<i64, i64> = HashMap::new();
    let mut wanted: HashSet<i64> = HashSet::new();
    for (event_id, format, started_at) in firsts {
        wanted.insert(event_id);
        let name = tournament_name(event_id, started_at, tz);
        let token = tokens.get(&event_id).cloned();
        match stored.get(&event_id) {
            Some((
                id,
                account,
                stored_name,
                stored_format,
                stored_start,
                synthesized,
                stored_token,
            )) => {
                // A tournament seen by two accounts has one row: it stays
                // with the account of the event's lowest match id, so the
                // two syncs do not take it from each other on every pass.
                if *account != Some(account_id)
                    && account.is_some()
                    && event_owner(tx, event_id)? != Some(account_id)
                {
                    ids.insert(event_id, *id);
                    continue;
                }
                let desired_name = if *synthesized { &name } else { stored_name };
                if *account != Some(account_id)
                    || desired_name != stored_name
                    || *stored_format != format
                    || *stored_start != started_at
                    || *stored_token != token
                {
                    tx.prepare_cached(
                        "UPDATE tournaments SET account_id = ?1, name = ?2, format = ?3,
                             started_at = ?4, token = ?5
                         WHERE id = ?6",
                    )?
                    .execute(params![
                        account_id,
                        desired_name,
                        format,
                        started_at,
                        token,
                        id
                    ])?;
                    changed = true;
                }
                ids.insert(event_id, *id);
            }
            None => {
                tx.prepare_cached(
                    "INSERT INTO tournaments (mtgo_event_id, account_id, name, format, started_at,
                         token)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )?
                .execute(params![
                    event_id, account_id, name, format, started_at, token
                ])?;
                ids.insert(event_id, tx.last_insert_rowid());
                changed = true;
            }
        }
    }

    // Each of the account's matches points at its tournament's row.
    let mut changes: Vec<(i64, Option<i64>)> = Vec::new();
    {
        let mut stmt = tx.prepare_cached(
            "SELECT id, tournament_event_id, tournament_id FROM matches
             WHERE account_id = ?1 AND (tournament_event_id IS NOT NULL
                 OR tournament_id IS NOT NULL)",
        )?;
        let rows = stmt.query_map([account_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })?;
        for row in rows {
            let (id, event_id, tournament_id) = row?;
            let desired = event_id.and_then(|event_id| ids.get(&event_id).copied());
            if desired != tournament_id {
                changes.push((id, desired));
            }
        }
    }
    for (id, tournament_id) in changes {
        tx.prepare_cached("UPDATE matches SET tournament_id = ?1 WHERE id = ?2")?
            .execute(params![tournament_id, id])?;
        changed = true;
    }

    // A synthesized row whose tournament has no listed match left goes.
    for (event_id, (id, account, _, _, _, synthesized, _)) in &stored {
        if *account == Some(account_id) && *synthesized && !wanted.contains(event_id) {
            tx.prepare_cached("DELETE FROM tournaments WHERE id = ?1")?
                .execute([id])?;
            changed = true;
        }
    }
    Ok(changed)
}

// ---- Tournament drafts ------------------------------------------------------

/// The account's tournaments played in a draft format, as the fold reads
/// them, with what the sync needs besides.
#[derive(Default)]
struct LimitedTournaments {
    inputs: Vec<DraftTournament>,
    /// Each event's name from its `EventSyncData_t` line, by event id.
    names: HashMap<i64, String>,
    /// Their matches, for their registered decks.
    match_tokens: Vec<String>,
}

/// The account's draft tournaments (never sealed: no draft to place, and
/// their sync lines are not decoded every pass): each event's first listed
/// match (its format and start) and, read from the newest `EventSyncData_t` line of its
/// tournament token, the event's name and the draft it names.
fn draft_tournaments(tx: &Transaction, account_id: i64) -> Result<LimitedTournaments, StoreError> {
    let mut found = LimitedTournaments::default();
    let mut tokens: HashMap<i64, String> = HashMap::new();
    let mut seen: HashSet<i64> = HashSet::new();
    let mut drafted: HashSet<i64> = HashSet::new();
    {
        let mut stmt = tx.prepare_cached(
            "SELECT m.tournament_event_id, m.format, m.started_at, m.tournament_token, m.token
             FROM listed_matches m
             WHERE m.account_id = ?1 AND m.tournament_event_id IS NOT NULL
                 AND m.state <> 'pending'
             ORDER BY m.started_at, m.id",
        )?;
        let rows = stmt.query_map([account_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        for row in rows {
            let (event_id, format, started_at, tournament_token, token) = row?;
            // The first match (in start order) says whether the event is a
            // draft, and gives its format and start.
            if seen.insert(event_id)
                && LeagueKind::from_format_code(format.as_deref()) == LeagueKind::Draft
            {
                drafted.insert(event_id);
                found.inputs.push(DraftTournament {
                    event_id,
                    format,
                    first_match_at: started_at,
                    draft_token: None,
                    draft_id: None,
                });
            }
            if !drafted.contains(&event_id) {
                continue;
            }
            found.match_tokens.push(token);
            if let Some(tournament_token) = tournament_token {
                tokens.entry(event_id).or_insert(tournament_token);
            }
        }
    }
    for input in &mut found.inputs {
        let Some(token) = tokens.get(&input.event_id) else {
            continue;
        };
        let mut stmt = tx.prepare_cached(
            "SELECT raw, compressed FROM log_events
             WHERE tournament_token = ?1 AND kind = 'tournament_sync' AND quarantined = 0
             ORDER BY logged_at DESC, log_file_id DESC, byte_offset DESC",
        )?;
        let mut rows = stmt.query([token])?;
        // The newest snapshot names the draft (a fired draft never unfires,
        // and every line carries both), so the draft fields come from it
        // alone; older lines are read only while the event's name is missing.
        let mut newest_read = false;
        while let Some(row) = rows.next()? {
            let text = decode_raw(row.get(0)?, row.get(1)?)?;
            let Some(TournamentFact::Snapshot(snapshot)) =
                decode_fact(EventKind::TournamentSync, &text, None)
            else {
                continue;
            };
            if snapshot.event_id.is_some_and(|id| id != input.event_id) {
                continue;
            }
            if !newest_read {
                newest_read = true;
                input.draft_token = snapshot.draft_token;
                input.draft_id = snapshot.draft_id;
            }
            if let Some(name) = snapshot.name {
                found.names.insert(input.event_id, name);
                break;
            }
        }
    }
    Ok(found)
}

/// `drafts.tournament_id` and `tournament_name` of the owner's drafts: the
/// tournament row and event name of the tournament holding the draft, else
/// none. Returns whether anything changed.
fn write_draft_tournaments(
    tx: &Transaction,
    owner: LeagueOwner,
    links: &[TournamentDraft],
    names: &HashMap<i64, String>,
) -> Result<bool, StoreError> {
    let mut wanted: HashMap<&str, (i64, Option<&str>)> = HashMap::new();
    for link in links {
        let id: Option<i64> = tx
            .prepare_cached("SELECT id FROM tournaments WHERE mtgo_event_id = ?1")?
            .query_row([link.event_id], |row| row.get(0))
            .optional()?;
        if let Some(id) = id {
            let name = names.get(&link.event_id).map(String::as_str);
            wanted.insert(link.draft_token.as_str(), (id, name));
        }
    }
    let mut changes: Vec<(i64, Option<i64>, Option<String>)> = Vec::new();
    {
        let mut stmt = tx.prepare_cached(
            "SELECT id, draft_token, tournament_id, tournament_name,
                 detached_at IS NOT NULL OR league_id IS NOT NULL
             FROM drafts WHERE account_id IS ?1",
        )?;
        let rows = stmt.query_map([owner.account()], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, bool>(4)?,
            ))
        })?;
        for row in rows {
            let (id, token, tournament_id, tournament_name, elsewhere) = row?;
            let desired = if elsewhere {
                None
            } else {
                wanted.get(token.as_str()).copied()
            };
            let desired_id = desired.map(|(id, _)| id);
            let desired_name = desired.and_then(|(_, name)| name).map(str::to_string);
            if desired_id != tournament_id || desired_name != tournament_name {
                changes.push((id, desired_id, desired_name));
            }
        }
    }
    // A tournament holds one draft (a unique index): clear first, then set.
    for (id, _, _) in &changes {
        tx.prepare_cached(
            "UPDATE drafts SET tournament_id = NULL, tournament_name = NULL WHERE id = ?1",
        )?
        .execute([id])?;
    }
    for (id, tournament_id, tournament_name) in &changes {
        if tournament_id.is_some() {
            tx.prepare_cached(
                "UPDATE drafts SET tournament_id = ?1, tournament_name = ?2 WHERE id = ?3",
            )?
            .execute(params![tournament_id, tournament_name, id])?;
        }
    }
    Ok(!changes.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compact::compact_match;
    use crate::limited::registered_deck;
    use crate::test_support::*;
    use crate::write::{MatchWrite, NewEvent, WriteBatch, commit, insert_log_file};
    use fixture_tools::{fixture_names, load_fixture, prepared_events};
    use leagues::{RunState, tournament_name};
    use mtgo_match::{MatchView, Projector};
    use mtgo_protocol::{EventKind, Subjects};
    use rusqlite::params;

    const UTC: Tz = chrono_tz::UTC;
    /// The hobbit fixture's league and draft.
    const HOBBIT_LEAGUE: &str = "48a2e914-f2ee-4fce-a4ad-47e396488889";
    const HOBBIT_DRAFT: &str = "791bacca-caea-4d88-b6c7-3bc067d412c2";
    const LEAGUE: &str = "q-league-0001";
    const LEAGUE_2: &str = "q-league-0002";
    const DRAFT: &str = "q-draft-0001";
    const EVENT: u64 = 11039;
    const COURSE: i64 = 35_746_768;
    const MINUTE: i64 = 60_000;
    const HOUR: i64 = 60 * MINUTE;
    const BASE: i64 = 1_787_400_000_000;

    /// The timezone the tests sync in: the first log file's (a fixture's
    /// `meta.json` sets it), UTC when there is none.
    fn test_tz(conn: &Connection) -> Tz {
        conn.query_row(
            "SELECT timezone FROM log_files ORDER BY id LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .and_then(|name| name.parse().ok())
        .unwrap_or(UTC)
    }

    fn sync(conn: &mut Connection, now_ms: i64) -> SyncOutcome {
        let tz = test_tz(conn);
        sync_all(conn, tz, now_ms).expect("sync")
    }

    fn sync_one(conn: &mut Connection, owner: LeagueOwner, now_ms: i64) -> SyncOutcome {
        let tz = test_tz(conn);
        let tx = conn.transaction().expect("tx");
        let outcome = sync_owner(&tx, owner, tz, now_ms).expect("sync owner");
        tx.commit().expect("commit");
        outcome
    }

    fn account_id(conn: &Connection, username: &str) -> i64 {
        conn.query_row(
            "SELECT id FROM accounts WHERE username = ?1",
            [username],
            |row| row.get(0),
        )
        .expect("account")
    }

    fn rows_of(conn: &Connection, sql: &str) -> Vec<String> {
        let mut stmt = conn.prepare(sql).expect(sql);
        let columns = stmt.column_count();
        stmt.query_map([], |row| {
            let mut values = Vec::new();
            for index in 0..columns {
                let value: rusqlite::types::Value = row.get(index)?;
                values.push(format!("{value:?}"));
            }
            Ok(values.join(" | "))
        })
        .expect("query")
        .map(|row| row.expect("row"))
        .collect()
    }

    /// Everything the sync writes, with ids replaced by keys (run keys, draft
    /// tokens, match tokens, event ids), so two databases compare.
    fn synced(conn: &Connection) -> Vec<String> {
        let mut all = Vec::new();
        all.extend(rows_of(
            conn,
            "SELECT run_key, account_id, source, kind, state, token, event_id, mtgo_course_id,
                 name, format, set_code, deck_version_id, deck_id, started_at, joined_at,
                 completed_at, dropped_at, detached_at, user_dropped_at, notes, imported_from,
                 imported_seed
             FROM leagues ORDER BY run_key",
        ));
        all.extend(rows_of(
            conn,
            "SELECT d.draft_token, d.account_id, d.source, l.run_key, d.league_event_id,
                 d.mtgo_course_id, d.mtgo_draft_id, d.pod_token, d.seat_count, d.seat_index,
                 d.booster_catalog_id, d.state, d.pack_size, d.picks_expected, d.started_at,
                 d.ended_at, d.abandons_at, d.detached_at, d.imported_from
             FROM drafts d LEFT JOIN leagues l ON l.id = d.league_id ORDER BY d.draft_token",
        ));
        all.extend(rows_of(
            conn,
            "SELECT d.draft_token, p.ordinal, p.pack_number, p.pick_number, p.pack_id,
                 p.direction, p.cards_available, p.picked_catalog_id, p.picked_card_id,
                 p.reservations, p.shown_at, p.deadline_at, p.picked_at, p.detached_at, p.note
             FROM draft_picks p JOIN drafts d ON d.id = p.draft_id
             ORDER BY d.draft_token, p.ordinal",
        ));
        all.extend(rows_of(
            conn,
            "SELECT m.token, s.source, s.cards, s.signature, s.captured_at, s.event_file_id,
                 s.event_offset
             FROM limited_deck_snapshots s JOIN matches m ON m.id = s.match_id
             ORDER BY m.token, s.source",
        ));
        all.extend(rows_of(
            conn,
            "SELECT mtgo_event_id, account_id, token, name, format, started_at, name_synthesized
             FROM tournaments ORDER BY mtgo_event_id",
        ));
        all.extend(rows_of(
            conn,
            "SELECT m.token, l.run_key, m.league_linked, t.mtgo_event_id
             FROM matches m LEFT JOIN leagues l ON l.id = m.league_id
             LEFT JOIN tournaments t ON t.id = m.tournament_id ORDER BY m.token",
        ));
        all
    }

    /// Whether an event can change what the sync writes.
    fn fold_relevant(kind: EventKind) -> bool {
        crate::limited::DRAFT_KINDS.contains(&kind)
            || matches!(
                kind,
                EventKind::LeagueJoined
                    | EventKind::LeagueDropped
                    | EventKind::MatchDeckRegistered
                    | EventKind::Login
                    | EventKind::TournamentRoundInfo
            )
    }

    fn upserts(projector: &Projector, tokens: Vec<String>, revision: &mut u64) -> Vec<MatchWrite> {
        let mut writes = Vec::new();
        for token in tokens {
            *revision += 1;
            match projector.view(&token) {
                Some(view) => writes.push(MatchWrite::Upsert {
                    view: Box::new(view.clone()),
                    revision: *revision,
                }),
                None => writes.push(MatchWrite::Remove { token }),
            }
        }
        writes
    }

    /// Store a fixture log the way the tracker does (events and projected
    /// matches, one event per batch), syncing after every event that can
    /// change a run (`each`) or once at the end. Returns the local account.
    fn ingest_fixture(conn: &mut Connection, name: &str, each: bool) -> i64 {
        let fixture = load_fixture(name);
        let tz = fixture.meta.tz();
        let now = fixture.meta.now_ms();
        let mut file = new_log_file("Q/mtgo.log");
        file.inherited_username = Some(fixture.meta.local_username.clone());
        file.timezone = fixture.meta.timezone.clone();
        let file_id = insert_log_file(conn, &file).expect("file");
        // The tracker registers the local player from its Login.
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at) VALUES (?1, 0)",
            [&fixture.meta.local_username],
        )
        .expect("account");
        let mut projector = Projector::new(vec![fixture.meta.local_username.clone()]);
        let mut revision = 0;
        for event in prepared_events(&fixture) {
            let changed = projector.apply(&event);
            let relevant = fold_relevant(event.kind) || !changed.is_empty();
            let batch = WriteBatch {
                events: vec![NewEvent {
                    log_file_id: file_id,
                    byte_offset: event.offset,
                    kind: Some(event.kind),
                    subjects: event.subjects.clone(),
                    logged_at_ms: event.at_ms,
                    raw: event.text.clone(),
                    quarantined: false,
                }],
                matches: upserts(&projector, changed, &mut revision),
                now_ms: event.at_ms,
                ..WriteBatch::default()
            };
            commit(conn, &batch).expect("commit");
            if each && relevant {
                sync_all(conn, tz, now).expect("sync");
            }
        }
        let finals = projector.finalize(now);
        let batch = WriteBatch {
            matches: upserts(&projector, finals, &mut revision),
            now_ms: now,
            ..WriteBatch::default()
        };
        commit(conn, &batch).expect("finalize");
        sync_all(conn, tz, now).expect("sync");
        account_id(conn, &fixture.meta.local_username)
    }

    fn file(conn: &Connection) -> i64 {
        insert_log_file(conn, &new_log_file("Q/mtgo.log")).expect("file")
    }

    /// A complete league match of Qlocal on `league`, in `format`.
    fn league_view(token: &str, league: &str, format: Option<&str>, started_at: i64) -> MatchView {
        let mut played = view(token, "Qopponent", started_at);
        played.league_token = Some(league.to_string());
        played.format = format.map(str::to_string);
        played.mtgo_match_id = None;
        for game in &mut played.games {
            game.mtgo_game_id = None;
        }
        played
    }

    fn store_views(conn: &mut Connection, views: Vec<MatchView>) {
        let matches = views
            .into_iter()
            .map(|played| MatchWrite::Upsert {
                view: Box::new(played),
                revision: 1,
            })
            .collect();
        commit(
            conn,
            &WriteBatch {
                matches,
                ..WriteBatch::default()
            },
        )
        .expect("matches");
    }

    fn joined(file_id: i64, offset: u64, league: &str, at_ms: i64) -> NewEvent {
        let mut event = event_with(
            file_id,
            offset,
            EventKind::LeagueJoined,
            Subjects {
                league_token: Some(league.to_string()),
                league_event_id: Some(EVENT),
                ..Subjects::default()
            },
            "12:00:00 [INF] (UI|Qleague)",
        );
        event.logged_at_ms = at_ms;
        event
    }

    fn registered_line(match_token: &str, cards: impl IntoIterator<Item = u32>) -> String {
        let rows: Vec<String> = cards
            .into_iter()
            .map(|catalog_id| {
                format!("{{\"CatalogID\":{catalog_id},\"Quantity\":1,\"InSideboard\":false}}")
            })
            .collect();
        format!(
            "12:07:09 [INF] (BaseClient|Inbound: FlsMatchDeckGetRespMessage) \
             {{\"MatchToken\":\"{match_token}\",\"MatchID\":7,\"Cards\":[{}]}}",
            rows.join(",")
        )
    }

    fn registered_event(file_id: i64, offset: u64, match_token: &str, raw: String) -> NewEvent {
        event_with(
            file_id,
            offset,
            EventKind::MatchDeckRegistered,
            Subjects {
                match_token: Some(match_token.to_string()),
                ..Subjects::default()
            },
            &raw,
        )
    }

    fn league_row(conn: &Connection, run_key: &str) -> (i64, String, Option<String>, Option<i64>) {
        conn.query_row(
            "SELECT id, state, notes, detached_at FROM leagues WHERE run_key = ?1",
            [run_key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap_or_else(|error| panic!("{run_key}: {error}"))
    }

    #[test]
    fn an_mtgo_sync_leaves_an_arena_run_alone() {
        let (_dir, mut conn) = db("league-sync-arena-guard");
        store_views(&mut conn, vec![view("q-a1", "Qopponent", BASE)]);
        move_match_to_arena(&conn, "q-a1");
        let arena = arena_account(&conn);
        conn.execute(
            "INSERT INTO leagues (run_key, account_id, source, kind, state, token, name,
                 started_at, created_at)
             VALUES ('arena:q-course', ?1, 'log', 'sealed', 'active', 'arena:q-course',
                 'Arena sealed', ?2, ?2)",
            params![arena, BASE],
        )
        .expect("the arena run");
        let (run_id, _, _, _) = league_row(&conn, "arena:q-course");
        conn.execute(
            "UPDATE matches SET league_id = ?1 WHERE token = 'q-a1'",
            [run_id],
        )
        .expect("link");

        sync(&mut conn, BASE + HOUR);
        let tx = conn.transaction().expect("tx");
        let outcome =
            sync_owner(&tx, LeagueOwner::Account(arena), UTC, BASE + HOUR).expect("sync owner");
        tx.commit().expect("commit");

        assert!(!outcome.leagues_changed && !outcome.limited_changed);
        assert_eq!(
            league_row(&conn, "arena:q-course"),
            (run_id, "active".to_string(), None, None)
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM leagues WHERE removed_at IS NOT NULL"
            ),
            0
        );
        assert_eq!(
            number(&conn, "SELECT league_id FROM matches WHERE token = 'q-a1'"),
            run_id
        );
    }

    /// `{run key} {kind} {state}` of every row.
    fn keys(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT run_key || ' ' || kind || ' ' || state FROM leagues ORDER BY run_key")
            .expect("prepare");
        stmt.query_map([], |row| row.get(0))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect()
    }

    fn members(conn: &Connection, run_key: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare(
                "SELECT m.token FROM matches m JOIN leagues l ON l.id = m.league_id
                 WHERE l.run_key = ?1 ORDER BY m.started_at",
            )
            .expect("prepare");
        stmt.query_map([run_key], |row| row.get(0))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect()
    }

    fn unhide(conn: &Connection, token: &str) {
        conn.execute(
            "UPDATE matches SET hidden_at = NULL WHERE token = ?1",
            [token],
        )
        .expect("unhide");
    }

    /// A sealed league of three matches: a panel view, and each match with
    /// its registered deck, stored one event or match at a time.
    fn sealed_steps(conn: &mut Connection, each: bool) {
        let file_id = file(conn);
        let now = BASE + 10 * HOUR;
        store_events(conn, vec![joined(file_id, 10, LEAGUE, BASE - MINUTE)]);
        if each {
            sync(conn, now);
        }
        for index in 0..3u64 {
            let token = format!("q-sealed-{index}");
            let at = BASE + index as i64 * HOUR;
            store_views(conn, vec![league_view(&token, LEAGUE, Some("S6FRA"), at)]);
            if each {
                sync(conn, now);
            }
            let raw = registered_line(&token, 1000..1000 + 23 + index as u32);
            store_events(
                conn,
                vec![registered_event(file_id, 100 + index * 100, &token, raw)],
            );
            if each {
                sync(conn, now);
            }
        }
        sync(conn, now);
    }

    /// The player entering `league`: the join request and the panel MTGO
    /// logs with it, stating `format_description`.
    fn entered(
        file_id: i64,
        offset: u64,
        league: &str,
        at_ms: i64,
        format_description: &str,
    ) -> Vec<NewEvent> {
        let mut request = event_with(
            file_id,
            offset,
            EventKind::LeagueJoinRequest,
            Subjects::default(),
            "12:24:23 [INF] (DEFAULT|) Send Class: FlsLeagueUserJoinReqMessage",
        );
        request.logged_at_ms = at_ms;
        let mut panel = event_with(
            file_id,
            offset + 1,
            EventKind::LeagueJoined,
            Subjects {
                league_token: Some(league.to_string()),
                league_event_id: Some(EVENT),
                ..Subjects::default()
            },
            &format!(
                "12:24:23 [INF] (UI|Creating GameDetailsView) League\nEventToken={league}\n\
                 EventId={EVENT}\nFormatDescription={format_description}\nPlayFormatCd=S6FRA"
            ),
        );
        panel.logged_at_ms = at_ms;
        vec![request, panel]
    }

    #[test]
    fn a_league_entry_runs_its_stated_matches_and_the_next_entry_starts_a_new_run() {
        let (_dir, mut conn) = db("league-sync-entries");
        let file_id = file(&conn);
        store_events(
            &mut conn,
            entered(file_id, 10, LEAGUE, BASE - MINUTE, "Play Up to 6 Matches"),
        );
        let first: Vec<MatchView> = (0..6)
            .map(|index| {
                league_view(
                    &format!("q-first-{index}"),
                    LEAGUE,
                    Some("S6FRA"),
                    BASE + index * HOUR,
                )
            })
            .collect();
        store_views(&mut conn, first);
        store_events(
            &mut conn,
            entered(
                file_id,
                900,
                LEAGUE,
                BASE + 7 * HOUR,
                "Play Up to 6 Matches",
            ),
        );
        store_views(
            &mut conn,
            vec![league_view(
                "q-second-0",
                LEAGUE,
                Some("S6FRA"),
                BASE + 8 * HOUR,
            )],
        );
        sync(&mut conn, BASE + 10 * HOUR);

        let first_key = format!("{LEAGUE}:q-first-0");
        assert_eq!(members(&conn, &first_key).len(), 6);
        assert_eq!(league_row(&conn, &first_key).1, "complete");
        let rounds: Option<i64> = conn
            .query_row(
                "SELECT rounds FROM leagues WHERE run_key = ?1",
                [&first_key],
                |row| row.get(0),
            )
            .expect("rounds");
        assert_eq!(rounds, Some(6), "the league's stated length is stored");
        let second_key = format!("{LEAGUE}:q-second-0");
        assert_eq!(members(&conn, &second_key), vec!["q-second-0"]);
        assert_eq!(league_row(&conn, &second_key).1, "active");
    }

    #[test]
    fn league_sync_once_per_event_equals_one_sync_at_the_end() {
        for name in ["league_join_drop", "draft_hobbit"] {
            let (_live_dir, mut live) = db(&format!("league-sync-live-{name}"));
            ingest_fixture(&mut live, name, true);
            let (_end_dir, mut end) = db(&format!("league-sync-end-{name}"));
            ingest_fixture(&mut end, name, false);
            let rows = synced(&end);
            assert!(
                rows.iter().any(|row| row.contains("\"log\"")),
                "{name}: {rows:#?}"
            );
            assert_eq!(synced(&live), rows, "{name}");
        }

        let (_live_dir, mut live) = db("league-sync-live-sealed");
        sealed_steps(&mut live, true);
        let (_end_dir, mut end) = db("league-sync-end-sealed");
        sealed_steps(&mut end, false);
        assert_eq!(synced(&live), synced(&end));
        let key = format!("{LEAGUE}:q-sealed-0");
        assert_eq!(members(&end, &key).len(), 3);
        assert_eq!(
            number(&end, "SELECT COUNT(*) FROM limited_deck_snapshots"),
            3
        );
        assert_eq!(
            number(&end, "SELECT COUNT(*) FROM leagues WHERE kind = 'sealed'"),
            1
        );
    }

    #[test]
    fn the_hobbit_draft_is_stored_as_one_draft_run_with_its_draft_and_decks() {
        let (_dir, mut conn) = db("league-sync-hobbit");
        ingest_fixture(&mut conn, "draft_hobbit", false);
        let key = format!("draft:{HOBBIT_DRAFT}");
        let (id, state, _, detached) = league_row(&conn, &key);
        assert_eq!(state, "complete");
        assert_eq!(detached, None);
        assert_eq!(members(&conn, &key).len(), 3);
        let (kind, token, set_code, event_id, course): (String, String, String, i64, i64) = conn
            .query_row(
                "SELECT kind, token, set_code, event_id, mtgo_course_id FROM leagues WHERE id = ?1",
                [id],
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
            .expect("run");
        assert_eq!(
            (
                kind.as_str(),
                token.as_str(),
                set_code.as_str(),
                event_id,
                course
            ),
            ("draft", HOBBIT_LEAGUE, "HOB", 11039, COURSE)
        );
        let (draft_league, state, picks): (i64, String, i64) = conn
            .query_row(
                "SELECT d.league_id, d.state, (SELECT COUNT(*) FROM draft_picks p
                     WHERE p.draft_id = d.id)
                 FROM drafts d WHERE d.draft_token = ?1",
                [HOBBIT_DRAFT],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("draft");
        assert_eq!((draft_league, state.as_str(), picks), (id, "finished", 42));
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM limited_deck_snapshots WHERE source = 'registered'"
            ),
            3
        );
    }

    #[test]
    fn the_league_sync_stubs_every_pack_pick_and_snapshot_card() {
        let (_dir, mut conn) = db("league-sync-stubs");
        ingest_fixture(&mut conn, "draft_hobbit", false);
        assert!(number(&conn, "SELECT COUNT(*) FROM cards") > 0);
        // The full scan finds nothing left to stub.
        let tx = conn.transaction().expect("tx");
        assert_eq!(crate::cards::scan_missing(&tx).expect("scan"), 0);
    }

    #[test]
    fn a_second_league_sync_writes_nothing() {
        let (_dir, mut conn) = db("league-sync-twice");
        let account = ingest_fixture(&mut conn, "draft_hobbit", false);
        let now = load_fixture("draft_hobbit").meta.now_ms();
        let before = conn.total_changes();
        let outcome = sync_one(&mut conn, LeagueOwner::Account(account), now);
        assert!(!outcome.leagues_changed && !outcome.limited_changed);
        sync_one(&mut conn, LeagueOwner::Nobody, now);
        assert_eq!(conn.total_changes(), before);
        let outcome =
            sync_all(&mut conn, load_fixture("draft_hobbit").meta.tz(), now).expect("sync");
        assert!(!outcome.leagues_changed && !outcome.limited_changed);
        assert_eq!(conn.total_changes(), before);
    }

    #[test]
    fn the_fold_is_current_after_a_full_sync_in_the_same_zone() {
        let (_dir, mut conn) = db("league-sync-current");
        assert!(!league_fold_current(&conn, UTC).expect("current"));
        sync(&mut conn, BASE);
        assert!(league_fold_current(&conn, UTC).expect("current"));
        assert!(!league_fold_current(&conn, chrono_tz::Europe::London).expect("current"));
        conn.execute(
            "UPDATE derivations SET version = 0 WHERE name = 'league_fold'",
            [],
        )
        .expect("age");
        assert!(!league_fold_current(&conn, UTC).expect("current"));
    }

    #[test]
    fn stored_constructed_runs_equal_fold_runs_on_every_fixture() {
        for name in fixture_names() {
            let fixture = load_fixture(&name);
            let (_dir, mut conn) = db(&format!("league-sync-parity-{name}"));
            let account = ingest_fixture(&mut conn, &name, false);
            let folded = fold_inputs(&conn, LeagueOwner::Account(account))
                .expect("inputs")
                .constructed_runs(fixture.meta.tz());
            let mut expected: Vec<String> = folded
                .iter()
                .map(|run| {
                    let state = match run.state {
                        RunState::Active => "active",
                        RunState::Complete => "complete",
                        RunState::Partial => "partial",
                        RunState::Dropped => "dropped",
                    };
                    format!(
                        "{} {} {:?} {} {:?} {} {:?} {:?} {:?} {:?}",
                        run.key,
                        run.token,
                        run.event_id,
                        run.name,
                        run.format,
                        state,
                        run.started_at,
                        run.joined_at,
                        run.completed_at,
                        run.dropped_at,
                    ) + &format!(" {:?}", run.match_tokens)
                })
                .collect();
            expected.sort();
            let mut stored = Vec::new();
            let mut stmt = conn
                .prepare(
                    "SELECT run_key, token, event_id, name, format, state, started_at,
                         joined_at, completed_at, dropped_at
                     FROM leagues WHERE kind = 'constructed' AND detached_at IS NULL",
                )
                .expect("prepare");
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        format!(
                            "{} {} {:?} {} {:?} {} {:?} {:?} {:?} {:?}",
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<i64>>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, i64>(6)?,
                            row.get::<_, Option<i64>>(7)?,
                            row.get::<_, Option<i64>>(8)?,
                            row.get::<_, Option<i64>>(9)?,
                        ),
                    ))
                })
                .expect("query")
                .map(|row| row.expect("row"))
                .collect::<Vec<_>>();
            for (key, text) in rows {
                stored.push(text + &format!(" {:?}", members(&conn, &key)));
            }
            stored.sort();
            assert_eq!(stored, expected, "{name}");
        }
    }

    #[test]
    fn the_format_less_then_limited_case_is_one_limited_run() {
        let (_dir, mut conn) = db("league-sync-format-less");
        store_views(
            &mut conn,
            vec![
                league_view("q-m1", LEAGUE, None, BASE),
                league_view("q-m2", LEAGUE, Some("DHOBHOBHOB"), BASE + HOUR),
            ],
        );
        sync(&mut conn, BASE + 2 * HOUR);
        assert_eq!(keys(&conn), vec![format!("{LEAGUE}:q-m1 draft active")]);
        assert_eq!(
            members(&conn, &format!("{LEAGUE}:q-m1")),
            vec!["q-m1", "q-m2"]
        );
    }

    /// Every input column the sync must leave alone, and every manual row.
    fn inputs(conn: &Connection) -> Vec<String> {
        let mut all = rows_of(
            conn,
            "SELECT run_key, user_dropped_at, notes, imported_from, imported_seed, removed_at,
                 user_state, edited_at
             FROM leagues ORDER BY id",
        );
        all.extend(rows_of(
            conn,
            "SELECT * FROM leagues WHERE source = 'manual' ORDER BY id",
        ));
        all.extend(rows_of(
            conn,
            "SELECT token, notes, hidden_at, league_linked,
                 CASE WHEN league_linked = 1 THEN league_id END, no_league, edited_at
             FROM matches ORDER BY token",
        ));
        all.extend(rows_of(
            conn,
            "SELECT d.draft_token, d.imported_league_from, p.ordinal, p.note, p.note_updated_at
             FROM draft_picks p JOIN drafts d ON d.id = p.draft_id ORDER BY d.id, p.ordinal",
        ));
        all
    }

    /// A 0.x run `0x:{id}` as the import stores it.
    fn imported_run(conn: &Connection, id: i64, seed: &ImportedSeed) {
        conn.execute(
            "INSERT INTO leagues (run_key, account_id, source, kind, state, token, event_id,
                 name, format, started_at, created_at, imported_from, imported_seed)
             VALUES (?1, (SELECT id FROM accounts WHERE username = 'Qlocal'), 'imported', ?2,
                 ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?10)",
            params![
                format!("0x:{id}"),
                seed.kind,
                seed.state,
                seed.token,
                seed.event_id,
                seed.name,
                seed.format,
                seed.started_at,
                id,
                serde_json::to_string(seed).expect("seed"),
            ],
        )
        .expect("imported run");
    }

    fn seed(token: &str, state: &str, started_at: i64) -> ImportedSeed {
        ImportedSeed {
            token: token.to_string(),
            event_id: Some(777),
            name: "Q 0.x League".to_string(),
            format: Some("CMODERN".to_string()),
            state: state.to_string(),
            started_at,
            joined_at: Some(started_at),
            completed_at: None,
            dropped_at: None,
            deck_version_id: None,
            kind: "constructed".to_string(),
            set_code: None,
            mtgo_course_id: None,
            legacy_id: None,
        }
    }

    fn link(conn: &Connection, token: &str, run_key: &str) {
        conn.execute(
            "UPDATE matches SET league_linked = 1,
                 league_id = (SELECT id FROM leagues WHERE run_key = ?2)
             WHERE token = ?1",
            params![token, run_key],
        )
        .expect("link");
    }

    #[test]
    fn the_league_sync_never_writes_input_columns_or_manual_rows() {
        let (_dir, mut conn) = db("league-sync-inputs");
        store_views(
            &mut conn,
            vec![
                league_view("q-m1", LEAGUE, Some("CMODERN"), BASE),
                league_view("q-m2", LEAGUE, Some("CMODERN"), BASE + HOUR),
                league_view("q-x1", LEAGUE_2, None, BASE - 10 * HOUR),
                league_view("q-manual", "q-hand", Some("CMODERN"), BASE - 20 * HOUR),
            ],
        );
        conn.execute_batch(
            "UPDATE matches SET notes = 'Q match note' WHERE token = 'q-m2';
             INSERT INTO leagues (run_key, account_id, source, kind, state, name, started_at,
                 created_at, notes, token)
             VALUES ('manual:1', 1, 'manual', 'constructed', 'complete', 'Q by hand',
                 1, 2, 'Q manual note', 'manual_Q');",
        )
        .expect("inputs");
        link(&conn, "q-manual", "manual:1");
        imported_run(&conn, 7, &seed(LEAGUE_2, "active", BASE - 11 * HOUR));
        link(&conn, "q-x1", "0x:7");
        conn.execute_batch(&format!(
            "UPDATE leagues SET notes = 'Q 0.x note', user_state = 'dropped', user_dropped_at = {}
                 WHERE run_key = '0x:7';
             INSERT INTO drafts (id, draft_token, account_id, source, state, started_at,
                 imported_from, imported_league_from)
             VALUES (5, '{DRAFT}', 1, 'imported', 'finished', {BASE}, 50, 7);
             INSERT INTO draft_picks (draft_id, ordinal, pack_number, pick_number,
                 cards_available, reservations, note, note_updated_at)
             VALUES (5, 1, 1, 1, '[1,2]', '[]', 'Q pick note', 9);",
            BASE - 9 * HOUR
        ))
        .expect("more inputs");
        sync(&mut conn, BASE + 2 * HOUR);
        let key = format!("{LEAGUE}:q-m1");
        conn.execute(
            "UPDATE leagues SET notes = 'Q run note', user_state = 'dropped', user_dropped_at = ?2 WHERE run_key = ?1",
            params![key, BASE + 90 * MINUTE],
        )
        .expect("run inputs");
        hide(&conn, "q-m2");
        let before = inputs(&conn);

        sync(&mut conn, BASE + 3 * HOUR);
        store_views(
            &mut conn,
            vec![league_view(
                "q-m3",
                LEAGUE,
                Some("CMODERN"),
                BASE + 2 * HOUR,
            )],
        );
        sync(&mut conn, BASE + 4 * HOUR);
        let mut after = inputs(&conn);
        // The new match is the only change: an unlinked row, no input.
        after.retain(|row| !row.contains("q-m3"));
        assert_eq!(after, before);
        assert_eq!(league_row(&conn, &key).1, "dropped");
        assert_eq!(league_row(&conn, "0x:7").1, "dropped");
    }

    #[test]
    fn a_hidden_first_match_keeps_the_runs_id_notes_and_drop() {
        let (_dir, mut conn) = db("league-sync-hidden-first");
        store_views(
            &mut conn,
            vec![
                league_view("q-m1", LEAGUE, Some("CMODERN"), BASE),
                league_view("q-m2", LEAGUE, Some("CMODERN"), BASE + HOUR),
                league_view("q-m3", LEAGUE, Some("CMODERN"), BASE + 2 * HOUR),
            ],
        );
        sync(&mut conn, BASE + 3 * HOUR);
        let first_key = format!("{LEAGUE}:q-m1");
        let (id, state, _, _) = league_row(&conn, &first_key);
        assert_eq!(state, "active");
        conn.execute(
            "UPDATE leagues SET notes = 'Q run note', user_state = 'dropped', user_dropped_at = ?2 WHERE id = ?1",
            params![id, BASE + 4 * HOUR],
        )
        .expect("inputs");
        sync(&mut conn, BASE + 5 * HOUR);
        assert_eq!(league_row(&conn, &first_key).1, "dropped");

        hide(&conn, "q-m1");
        sync(&mut conn, BASE + 6 * HOUR);
        let second_key = format!("{LEAGUE}:q-m2");
        assert_eq!(
            league_row(&conn, &second_key),
            (
                id,
                "dropped".to_string(),
                Some("Q run note".to_string()),
                None
            )
        );
        assert_eq!(members(&conn, &second_key), vec!["q-m2", "q-m3"]);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM leagues"), 1);

        unhide(&conn, "q-m1");
        sync(&mut conn, BASE + 7 * HOUR);
        assert_eq!(
            league_row(&conn, &first_key),
            (
                id,
                "dropped".to_string(),
                Some("Q run note".to_string()),
                None
            )
        );
        assert_eq!(members(&conn, &first_key), vec!["q-m1", "q-m2", "q-m3"]);
    }

    /// A finished 0.x draft of `DRAFT` (league event `EVENT`, course
    /// `COURSE`) picking 1000..=1041, started at `started_at`.
    fn imported_draft(conn: &Connection, started_at: i64) {
        conn.execute(
            "INSERT INTO drafts (id, draft_token, account_id, source, league_event_id,
                 mtgo_course_id, mtgo_draft_id, state, started_at, ended_at, imported_from)
             VALUES (5, ?1, 1, 'imported', ?2, ?3, 6781001, 'finished', ?4, ?5, 50)",
            params![
                DRAFT,
                EVENT as i64,
                COURSE,
                started_at,
                started_at + 25 * MINUTE
            ],
        )
        .expect("draft");
        for ordinal in 1..=42u32 {
            conn.execute(
                "INSERT INTO draft_picks (draft_id, ordinal, pack_number, pick_number,
                     cards_available, picked_catalog_id, picked_card_id, reservations)
                 VALUES (5, ?1, ?2, ?3, ?4, ?5, ?6, '[]')",
                params![
                    ordinal,
                    (ordinal - 1) / 14 + 1,
                    (ordinal - 1) % 14 + 1,
                    format!("[{}]", 999 + ordinal),
                    999 + ordinal,
                    1_000_999 + ordinal,
                ],
            )
            .expect("pick");
        }
    }

    fn creatures(conn: &Connection, ids: impl IntoIterator<Item = u32>) {
        for id in ids {
            conn.execute(
                "INSERT INTO cards (catalog_id, name, type) VALUES (?1, ?2, 'Creature')",
                params![id, format!("Q card {id}")],
            )
            .expect("card");
        }
    }

    #[test]
    fn a_late_adoption_keeps_the_retired_runs_notes() {
        let (_dir, mut conn) = db("league-sync-adoption");
        conn.execute_batch(
            "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);",
        )
        .expect("account");
        let file_id = file(&conn);
        store_events(
            &mut conn,
            vec![joined(file_id, 10, LEAGUE_2, BASE - 4 * HOUR)],
        );
        imported_draft(&conn, BASE - 3 * HOUR);
        creatures(&conn, 1000..=1041);
        sync(&mut conn, BASE - 150 * MINUTE);
        let draft_key = format!("draft:{DRAFT}");
        let (id, _, _, _) = league_row(&conn, &draft_key);
        conn.execute(
            "UPDATE leagues SET notes = 'Q draft note' WHERE id = ?1",
            [id],
        )
        .expect("notes");

        let match_at = BASE - 2 * HOUR;
        store_events(
            &mut conn,
            vec![joined(file_id, 20, LEAGUE, match_at - MINUTE)],
        );
        store_views(
            &mut conn,
            vec![league_view("q-m1", LEAGUE, Some("DHOBHOBHOB"), match_at)],
        );
        store_events(
            &mut conn,
            vec![registered_event(
                file_id,
                30,
                "q-m1",
                registered_line("q-m1", 1000..=1022),
            )],
        );
        sync(&mut conn, BASE);
        let key = format!("{LEAGUE}:q-m1");
        assert_eq!(
            league_row(&conn, &key),
            (
                id,
                "active".to_string(),
                Some("Q draft note".to_string()),
                None
            )
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM leagues"), 1);
        assert_eq!(
            number(&conn, "SELECT league_id FROM drafts WHERE id = 5"),
            id
        );
    }

    #[test]
    fn a_vanished_run_with_notes_is_detached_and_comes_back() {
        let (_dir, mut conn) = db("league-sync-vanished");
        store_views(
            &mut conn,
            vec![
                league_view("q-m1", LEAGUE, Some("CMODERN"), BASE),
                league_view("q-n1", LEAGUE_2, Some("CMODERN"), BASE + HOUR),
            ],
        );
        sync(&mut conn, BASE + 2 * HOUR);
        let key = format!("{LEAGUE}:q-m1");
        let (id, _, _, _) = league_row(&conn, &key);
        conn.execute("UPDATE leagues SET notes = 'Q keep me' WHERE id = ?1", [id])
            .expect("notes");

        hide(&conn, "q-m1");
        hide(&conn, "q-n1");
        sync(&mut conn, BASE + 3 * HOUR);
        let (_, _, notes, detached) = league_row(&conn, &key);
        assert_eq!(notes.as_deref(), Some("Q keep me"));
        assert_eq!(detached, Some(BASE + 3 * HOUR));
        // The run without input is gone.
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM leagues"), 1);
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE league_id IS NOT NULL"
            ),
            0
        );

        unhide(&conn, "q-m1");
        sync(&mut conn, BASE + 4 * HOUR);
        assert_eq!(
            league_row(&conn, &key),
            (
                id,
                "active".to_string(),
                Some("Q keep me".to_string()),
                None
            )
        );
        assert_eq!(members(&conn, &key), vec!["q-m1"]);
    }

    #[test]
    fn an_active_imported_run_refolds_the_same_after_a_member_is_hidden_and_unhidden() {
        let (_dir, mut conn) = db("league-sync-imported");
        store_views(
            &mut conn,
            vec![
                league_view("q-x1", LEAGUE, None, BASE),
                league_view("q-x2", LEAGUE, None, BASE + HOUR),
                league_view("q-m3", LEAGUE, Some("CMODERN"), BASE + 2 * HOUR),
            ],
        );
        let the_seed = seed(LEAGUE, "active", BASE - MINUTE);
        imported_run(&conn, 7, &the_seed);
        link(&conn, "q-x1", "0x:7");
        link(&conn, "q-x2", "0x:7");
        sync(&mut conn, BASE + 3 * HOUR);
        assert_eq!(members(&conn, "0x:7"), vec!["q-x1", "q-x2", "q-m3"]);
        assert_eq!(league_row(&conn, "0x:7").1, "active");
        let first = synced(&conn);

        hide(&conn, "q-x2");
        sync(&mut conn, BASE + 4 * HOUR);
        assert_eq!(league_row(&conn, "0x:7").1, "active");
        unhide(&conn, "q-x2");
        sync(&mut conn, BASE + 5 * HOUR);
        assert_eq!(synced(&conn), first);
        let stored: String = conn
            .query_row(
                "SELECT imported_seed FROM leagues WHERE run_key = '0x:7'",
                [],
                |row| row.get(0),
            )
            .expect("seed");
        assert_eq!(
            serde_json::from_str::<ImportedSeed>(&stored).expect("json"),
            the_seed
        );
    }

    /// The hobbit fixture's draft events up to (not including) the first
    /// event at or after `cut_ms`, with every Login, in file `file_id`.
    fn hobbit_draft_events(file_id: i64, cut_ms: Option<i64>) -> Vec<NewEvent> {
        let fixture = load_fixture("draft_hobbit");
        prepared_events(&fixture)
            .into_iter()
            .filter(|event| {
                crate::limited::DRAFT_KINDS.contains(&event.kind) || event.kind == EventKind::Login
            })
            .filter(|event| cut_ms.is_none_or(|cut| event.at_ms < cut))
            .map(|event| NewEvent {
                log_file_id: file_id,
                byte_offset: event.offset,
                kind: Some(event.kind),
                subjects: event.subjects,
                logged_at_ms: event.at_ms,
                raw: event.text,
                quarantined: false,
            })
            .collect()
    }

    fn hobbit_file(conn: &Connection) -> i64 {
        let fixture = load_fixture("draft_hobbit");
        let mut new = new_log_file("Q/mtgo.log");
        new.inherited_username = Some(fixture.meta.local_username.clone());
        let file_id = insert_log_file(conn, &new).expect("file");
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at) VALUES (?1, 0)",
            [&fixture.meta.local_username],
        )
        .expect("account");
        file_id
    }

    #[test]
    fn an_open_draft_turns_abandoned_at_abandons_at() {
        let (_dir, mut conn) = db("league-sync-abandon");
        let file_id = hobbit_file(&conn);
        let all = hobbit_draft_events(file_id, None);
        // Half way through the picks.
        let picks: Vec<i64> = all
            .iter()
            .filter(|event| event.kind == Some(EventKind::DraftPendingPick))
            .map(|event| event.logged_at_ms)
            .collect();
        let cut = picks[picks.len() / 2];
        let events = hobbit_draft_events(file_id, Some(cut));
        store_events(&mut conn, events);
        let first_pick = picks[0];
        let outcome = sync(&mut conn, first_pick);
        let (state, abandons_at): (String, Option<i64>) = conn
            .query_row(
                "SELECT state, abandons_at FROM drafts WHERE draft_token = ?1",
                [HOBBIT_DRAFT],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("draft");
        assert_eq!(state, "picking");
        let abandons_at = abandons_at.expect("a deadline");
        let last: i64 = conn
            .query_row(
                "SELECT MAX(MAX(COALESCE(shown_at, 0), COALESCE(picked_at, 0)))
                 FROM draft_picks p JOIN drafts d ON d.id = p.draft_id
                 WHERE d.draft_token = ?1",
                [HOBBIT_DRAFT],
                |row| row.get(0),
            )
            .expect("last");
        assert_eq!(abandons_at, last + limited::ABANDON_AFTER_MS);
        assert_eq!(outcome.next_abandon_at, Some(abandons_at));

        sync(&mut conn, abandons_at);
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM drafts WHERE state = 'picking'"),
            1
        );
        let outcome = sync(&mut conn, abandons_at + 1);
        assert!(outcome.limited_changed);
        assert_eq!(outcome.next_abandon_at, None);
        let (state, abandons_at): (String, Option<i64>) = conn
            .query_row(
                "SELECT state, abandons_at FROM drafts WHERE draft_token = ?1",
                [HOBBIT_DRAFT],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("draft");
        assert_eq!((state.as_str(), abandons_at), ("abandoned", None));
    }

    #[test]
    fn an_imported_draft_becomes_log_when_its_token_has_events_and_keeps_pick_notes() {
        let (_dir, mut conn) = db("league-sync-imported-draft");
        let file_id = hobbit_file(&conn);
        conn.execute_batch(&format!(
            "INSERT INTO drafts (id, draft_token, account_id, source, state, started_at,
                 imported_from)
             VALUES (5, '{HOBBIT_DRAFT}', 1, 'imported', 'picking', 1, 50);
             INSERT INTO draft_picks (draft_id, ordinal, pack_number, pick_number,
                 cards_available, reservations, note, note_updated_at)
             VALUES (5, 1, 1, 1, '[1,2]', '[]', 'Q first pick', 9),
                    (5, 43, 4, 1, '[3]', '[]', 'Q a pick 0.x had', 9),
                    (5, 44, 4, 2, '[4]', '[]', NULL, NULL);"
        ))
        .expect("imported draft");
        // Long idle: the 30-minute rule abandons it.
        sync(&mut conn, 10 * HOUR);
        assert_eq!(
            rows_of(&conn, "SELECT source, state, abandons_at FROM drafts"),
            vec!["Text(\"imported\") | Text(\"abandoned\") | Null"]
        );

        store_events(&mut conn, hobbit_draft_events(file_id, None));
        sync(&mut conn, load_fixture("draft_hobbit").meta.now_ms());
        let (id, source, imported_from, state): (i64, String, Option<i64>, String) = conn
            .query_row(
                "SELECT id, source, imported_from, state FROM drafts WHERE draft_token = ?1",
                [HOBBIT_DRAFT],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("draft");
        assert_eq!(
            (id, source.as_str(), imported_from, state.as_str()),
            (5, "log", None, "finished")
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM draft_picks WHERE detached_at IS NULL"
            ),
            42
        );
        assert_eq!(
            rows_of(
                &conn,
                "SELECT ordinal, note, detached_at IS NOT NULL FROM draft_picks
                 WHERE note IS NOT NULL ORDER BY ordinal"
            ),
            vec![
                "Integer(1) | Text(\"Q first pick\") | Integer(0)",
                "Integer(43) | Text(\"Q a pick 0.x had\") | Integer(1)",
            ]
        );
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM draft_picks WHERE ordinal = 44"),
            0
        );
    }

    fn snapshots_match_registered_decks(conn: &Connection) {
        let mut stmt = conn
            .prepare(
                "SELECT m.token, s.cards, s.signature, s.captured_at
                 FROM limited_deck_snapshots s JOIN matches m ON m.id = s.match_id
                 WHERE s.source = 'registered' ORDER BY m.token",
            )
            .expect("prepare");
        let rows: Vec<(String, String, String, i64)> = stmt
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .expect("query")
            .map(|row| row.expect("row"))
            .collect();
        assert_eq!(rows.len(), 3);
        for (token, cards, signature, captured_at) in rows {
            let deck = registered_deck(conn, &token)
                .expect("deck")
                .expect("a registered deck");
            assert_eq!(
                cards,
                crate::limited::cards_json(&deck.cards).expect("json")
            );
            assert_eq!(signature, deck.signature);
            assert_eq!(captured_at, deck.captured_at_ms);
        }
    }

    fn limited_tokens(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT token FROM matches WHERE format LIKE 'DHOB%' ORDER BY token")
            .expect("prepare");
        stmt.query_map([], |row| row.get(0))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect()
    }

    #[test]
    fn snapshots_equal_registered_deck_live_and_after_compaction() {
        let (_dir, mut conn) = db("league-sync-snapshots");
        ingest_fixture(&mut conn, "draft_hobbit", false);
        let now = load_fixture("draft_hobbit").meta.now_ms();
        snapshots_match_registered_decks(&conn);
        let before = synced(&conn);

        for token in limited_tokens(&conn) {
            compact_match(&conn, &token, now).expect("compact");
        }
        let changes = conn.total_changes();
        sync(&mut conn, now);
        assert_eq!(
            conn.total_changes(),
            changes,
            "a compacted match keeps its row"
        );
        snapshots_match_registered_decks(&conn);

        conn.execute("DELETE FROM limited_deck_snapshots", [])
            .expect("forget");
        sync(&mut conn, now);
        snapshots_match_registered_decks(&conn);
        assert_eq!(synced(&conn), before);
    }

    #[test]
    fn a_compacted_match_without_a_registered_deck_is_read_once() {
        let (_dir, mut conn) = db("league-sync-no-registered");
        ingest_fixture(&mut conn, "draft_hobbit", false);
        let now = load_fixture("draft_hobbit").meta.now_ms();
        let tokens = limited_tokens(&conn);
        conn.execute(
            "DELETE FROM log_events WHERE kind = 'match_deck_registered' AND match_token = ?1",
            [&tokens[0]],
        )
        .expect("no registered line");
        for token in &tokens {
            compact_match(&conn, token, now).expect("compact");
        }
        conn.execute("DELETE FROM limited_deck_snapshots", [])
            .expect("forget");
        let first = sync(&mut conn, now);
        assert!(first.skipped_snapshots.is_empty());
        // The archive is not read again: an unreadable one would be skipped.
        conn.execute(
            "UPDATE match_event_archives SET raw = x'00' WHERE match_token = ?1",
            [&tokens[0]],
        )
        .expect("corrupt");
        let second = sync(&mut conn, now);
        assert!(
            second.skipped_snapshots.is_empty(),
            "the second sync read the archive again: {:?}",
            second.skipped_snapshots
        );
        // A recompaction (a new archive) is read again.
        conn.execute(
            "UPDATE match_event_archives SET compacted_at = compacted_at + 1 WHERE match_token = ?1",
            [&tokens[0]],
        )
        .expect("recompacted");
        let third = sync(&mut conn, now);
        assert_eq!(third.skipped_snapshots, vec![tokens[0].clone()]);
    }

    #[test]
    fn a_corrupt_archive_skips_that_snapshot() {
        let (_dir, mut conn) = db("league-sync-corrupt");
        ingest_fixture(&mut conn, "draft_hobbit", false);
        let now = load_fixture("draft_hobbit").meta.now_ms();
        let tokens = limited_tokens(&conn);
        for token in &tokens {
            compact_match(&conn, token, now).expect("compact");
        }
        conn.execute("DELETE FROM limited_deck_snapshots", [])
            .expect("forget");
        conn.execute(
            "UPDATE match_event_archives SET raw = x'00' WHERE match_token = ?1",
            [&tokens[0]],
        )
        .expect("corrupt");
        let outcome = sync(&mut conn, now);
        assert_eq!(outcome.skipped_snapshots, vec![tokens[0].clone()]);
        let snapshotted: Vec<String> = rows_of(
            &conn,
            "SELECT m.token FROM limited_deck_snapshots s JOIN matches m ON m.id = s.match_id
             ORDER BY m.token",
        );
        assert_eq!(snapshotted.len(), 2);
        assert!(!snapshotted.iter().any(|row| row.contains(&tokens[0])));
    }

    const SWISS: i64 = 19_990_050;

    /// Round `round` of Swiss Draft `SWISS`, played by `user` at `at`.
    fn swiss_round(
        token: &str,
        user: &str,
        round: u32,
        outcome: mtgo_match::Outcome,
        at: i64,
    ) -> MatchView {
        let mut played = view(token, "Qopponent", at);
        played.mtgo_match_id = None;
        for game in &mut played.games {
            game.mtgo_game_id = None;
        }
        played.local_username = user.to_string();
        played.tournament_event_id = Some(SWISS as u64);
        played.tournament_round = Some(round);
        played.format = Some("DFRAFRAFRA".to_string());
        played.game_structure = Some("FRAx3".to_string());
        played.match_type = None;
        played.outcome = outcome;
        played
    }

    /// The three rounds of Swiss Draft `SWISS`, from `first_at`, an hour
    /// apart: won, lost, won.
    fn swiss_rounds(conn: &mut Connection, first_at: i64) {
        let mut views = Vec::new();
        for (round, outcome) in [
            (1, mtgo_match::Outcome::Win),
            (2, mtgo_match::Outcome::Loss),
            (3, mtgo_match::Outcome::Win),
        ] {
            views.push(swiss_round(
                &format!("q-swiss-{round}"),
                "Qlocal",
                round,
                outcome,
                first_at + i64::from(round - 1) * HOUR,
            ));
        }
        store_views(conn, views);
    }

    /// A finished draft of no league (`DRAFT`, as the 0.x import stores one),
    /// 25 minutes long from `started_at`.
    fn leagueless_draft(conn: &Connection, started_at: i64) {
        leagueless_draft_of(conn, 5, DRAFT, "Qlocal", started_at);
    }

    fn leagueless_draft_of(conn: &Connection, id: i64, token: &str, user: &str, started_at: i64) {
        let account = account_id(conn, user);
        conn.execute(
            "INSERT INTO drafts (id, draft_token, account_id, source, state, started_at, ended_at,
                 imported_from)
             VALUES (?1, ?2, ?3, 'imported', 'finished', ?4, ?5, ?6)",
            params![
                id,
                token,
                account,
                started_at,
                started_at + 25 * MINUTE,
                id * 10
            ],
        )
        .expect("draft");
    }

    fn draft_tournament(conn: &Connection) -> (Option<i64>, Option<String>) {
        conn.query_row(
            "SELECT t.mtgo_event_id, d.tournament_name FROM drafts d
             LEFT JOIN tournaments t ON t.id = d.tournament_id WHERE d.draft_token = ?1",
            [DRAFT],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("draft")
    }

    #[test]
    fn a_swiss_draft_in_progress_is_dropped_by_a_later_match_outside_it() {
        let (_dir, mut conn) = db("league-sync-swiss-state");
        // Round 1 only, 12 minutes after the draft ends.
        store_views(
            &mut conn,
            vec![swiss_round(
                "q-swiss-1",
                "Qlocal",
                1,
                mtgo_match::Outcome::Win,
                BASE + 37 * MINUTE,
            )],
        );
        leagueless_draft(&conn, BASE);
        sync(&mut conn, BASE + 5 * HOUR);
        let account = account_id(&conn, "Qlocal");
        let filter = crate::leagues::RunFilter {
            owners: vec![LeagueOwner::Account(account)],
            ..crate::leagues::RunFilter::default()
        };
        let state = |conn: &Connection| {
            let read = crate::leagues::stored_tournament_drafts(conn, &filter).expect("read");
            (read[0].run.state, read[0].run.completed_at)
        };
        assert_eq!(state(&conn), (RunState::Active, None), "one round played");

        // A constructed match after round 1: the player left the event.
        store_views(
            &mut conn,
            vec![view("q-later", "Qopponent", BASE + 3 * HOUR)],
        );
        sync(&mut conn, BASE + 5 * HOUR);
        assert_eq!(state(&conn), (RunState::Dropped, None));
    }

    #[test]
    fn two_accounts_drafting_one_swiss_event_each_keep_their_draft() {
        let (_dir, mut conn) = db("league-sync-swiss-two-accounts");
        let mut views = Vec::new();
        for (user, prefix) in [("Qlocal", "q-a"), ("Qsecond", "q-b")] {
            for round in 1..=3 {
                views.push(swiss_round(
                    &format!("{prefix}-{round}"),
                    user,
                    round,
                    mtgo_match::Outcome::Win,
                    BASE + 37 * MINUTE + i64::from(round - 1) * HOUR,
                ));
            }
        }
        store_views(&mut conn, views);
        leagueless_draft_of(&conn, 5, DRAFT, "Qlocal", BASE);
        leagueless_draft_of(&conn, 6, "q-draft-0002", "Qsecond", BASE);
        sync(&mut conn, BASE + 5 * HOUR);
        assert_eq!(
            rows_of(
                &conn,
                "SELECT d.draft_token, t.mtgo_event_id FROM drafts d
                 JOIN tournaments t ON t.id = d.tournament_id ORDER BY d.id"
            ),
            vec![
                format!("Text(\"{DRAFT}\") | Integer({SWISS})"),
                format!("Text(\"q-draft-0002\") | Integer({SWISS})"),
            ]
        );
        let again = sync(&mut conn, BASE + 5 * HOUR);
        assert!(!again.limited_changed);
    }

    #[test]
    fn an_unlinked_swiss_draft_links_to_its_tournament_when_the_fold_version_rises() {
        let (_dir, mut conn) = db("league-sync-swiss-refold");
        // Round 1 is paired 12 minutes after the draft ends.
        swiss_rounds(&mut conn, BASE + 37 * MINUTE);
        leagueless_draft(&conn, BASE);
        sync(&mut conn, BASE + 5 * HOUR);
        assert_eq!(draft_tournament(&conn), (Some(SWISS), None));

        // As an older fold left the database: the draft unlinked.
        conn.execute_batch(&format!(
            "UPDATE drafts SET tournament_id = NULL;
             UPDATE derivations SET version = {} WHERE name = '{LEAGUE_FOLD_DERIVATION}';",
            FOLD_VERSION - 1
        ))
        .expect("an older fold");
        let filter = |conn: &Connection| crate::leagues::RunFilter {
            owners: vec![LeagueOwner::Account(account_id(conn, "Qlocal"))],
            ..crate::leagues::RunFilter::default()
        };
        let unlinked = |conn: &Connection| {
            crate::leagues::stored_limited_runs(conn, &filter(conn))
                .expect("read")
                .unlinked_drafts
        };
        assert_eq!(unlinked(&conn).len(), 1);
        assert!(!league_fold_current(&conn, UTC).expect("current"));

        let outcome = sync(&mut conn, BASE + 5 * HOUR);
        assert!(outcome.limited_changed);
        assert!(league_fold_current(&conn, UTC).expect("current"));
        assert_eq!(draft_tournament(&conn), (Some(SWISS), None));
        assert!(unlinked(&conn).is_empty());
        let read = crate::leagues::stored_tournament_drafts(&conn, &filter(&conn)).expect("read");
        assert_eq!(read.len(), 1);
        let run = &read[0];
        assert_eq!(run.run.key, "tournament:19990050");
        assert_eq!(run.run.kind, LeagueKind::Draft);
        assert_eq!(run.run.set_code.as_deref(), Some("FRA"));
        assert_eq!(run.run.state, RunState::Complete);
        assert_eq!(
            run.run.completed_at,
            Some(BASE + 37 * MINUTE + 2 * HOUR + 4_000)
        );
        assert_eq!(
            run.draft.as_ref().map(|draft| draft.token.as_str()),
            Some(DRAFT)
        );
        let tokens: Vec<&str> = run.matches.iter().map(|row| row.token.as_str()).collect();
        assert_eq!(tokens, vec!["q-swiss-1", "q-swiss-2", "q-swiss-3"]);
        assert!(
            crate::leagues::stored_limited_runs(&conn, &filter(&conn))
                .expect("read")
                .runs
                .is_empty(),
            "not a league run"
        );
        let by_key = crate::leagues::RunFilter {
            run_key: Some("tournament:19990050".to_string()),
            ..filter(&conn)
        };
        assert_eq!(
            crate::leagues::stored_tournament_drafts(&conn, &by_key)
                .expect("read")
                .len(),
            1
        );
        // A tournament, not a league: no league row, no match in a league.
        assert_eq!(
            rows_of(&conn, "SELECT COUNT(*) FROM leagues"),
            vec!["Integer(0)"]
        );
        assert_eq!(
            rows_of(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE league_id IS NOT NULL"
            ),
            vec!["Integer(0)"]
        );
        // A second sync writes nothing.
        let again = sync(&mut conn, BASE + 5 * HOUR);
        assert!(!again.limited_changed && !again.leagues_changed);
    }

    #[test]
    fn a_swiss_draft_links_by_the_draft_its_tournament_line_names() {
        let (_dir, mut conn) = db("league-sync-swiss-named");
        // Days after the draft: only the tournament's own line can link it.
        swiss_rounds(&mut conn, BASE + 3 * 24 * HOUR);
        leagueless_draft(&conn, BASE);
        conn.execute("UPDATE matches SET tournament_token = 'q-swiss-event'", [])
            .expect("tokens");
        let file = insert_log_file(&conn, &new_log_file("Q/mtgo.log")).expect("file");
        let line = format!(
            "15:27:13 [INF] (Game Management|Processing Registered Handler for EventSyncData_t \
             in TournamentUninitializedState) Message: {{\"EventToken\":\"q-swiss-event\",\
             \"EventID\":{SWISS},\"GameStructureCd\":\"DFRAFRAFRA\",\
             \"Description\":\"Q Fracture Swiss Draft\",\"TournamentSyncData\":\
             {{\"DraftToken\":\"{DRAFT}\",\"DraftID\":7999556}}}} Receiver: Q"
        );
        store_events(
            &mut conn,
            vec![event_with(
                file,
                10,
                EventKind::TournamentSync,
                Subjects {
                    tournament_token: Some("q-swiss-event".to_string()),
                    ..Subjects::default()
                },
                &line,
            )],
        );
        sync(&mut conn, BASE + 4 * 24 * HOUR);
        assert_eq!(
            draft_tournament(&conn),
            (Some(SWISS), Some("Q Fracture Swiss Draft".to_string()))
        );
    }

    #[test]
    fn a_draft_with_no_tournament_soon_after_it_stays_unlinked() {
        let (_dir, mut conn) = db("league-sync-swiss-far");
        swiss_rounds(&mut conn, BASE + 3 * 24 * HOUR);
        leagueless_draft(&conn, BASE);
        sync(&mut conn, BASE + 4 * 24 * HOUR);
        assert_eq!(draft_tournament(&conn), (None, None));
    }

    #[test]
    fn tournaments_are_one_row_per_event_with_the_synthesized_name() {
        let (_dir, mut conn) = db("league-sync-tournaments");
        let mut views = Vec::new();
        for (token, event, at, format) in [
            ("q-t1", 77, BASE, "CMODERN"),
            ("q-t2", 77, BASE + HOUR, "CPAUPER"),
            ("q-t3", 78, BASE + 30 * HOUR, "CLEGACY"),
        ] {
            let mut played = view(token, "Qopponent", at);
            played.mtgo_match_id = None;
            for game in &mut played.games {
                game.mtgo_game_id = None;
            }
            played.tournament_event_id = Some(event);
            played.tournament_round = Some(1);
            played.format = Some(format.to_string());
            views.push(played);
        }
        store_views(&mut conn, views);
        sync(&mut conn, BASE + 40 * HOUR);
        assert_eq!(
            rows_of(
                &conn,
                "SELECT mtgo_event_id, name, format, started_at, name_synthesized
                 FROM tournaments ORDER BY mtgo_event_id"
            ),
            vec![
                format!(
                    "Integer(77) | Text(\"{}\") | Text(\"CMODERN\") | Integer({BASE}) | Integer(1)",
                    tournament_name(77, BASE, UTC)
                ),
                format!(
                    "Integer(78) | Text(\"{}\") | Text(\"CLEGACY\") | Integer({}) | Integer(1)",
                    tournament_name(78, BASE + 30 * HOUR, UTC),
                    BASE + 30 * HOUR
                ),
            ]
        );
        assert_eq!(
            rows_of(
                &conn,
                "SELECT m.token, t.mtgo_event_id FROM matches m
                 JOIN tournaments t ON t.id = m.tournament_id ORDER BY m.token"
            ),
            vec![
                "Text(\"q-t1\") | Integer(77)",
                "Text(\"q-t2\") | Integer(77)",
                "Text(\"q-t3\") | Integer(78)",
            ]
        );

        // A synthesized name follows the first match; a real one stays.
        hide(&conn, "q-t1");
        conn.execute(
            "UPDATE tournaments SET name = 'Q Challenge', name_synthesized = 0
             WHERE mtgo_event_id = 78",
            [],
        )
        .expect("named");
        sync(&mut conn, BASE + 40 * HOUR);
        assert_eq!(
            rows_of(
                &conn,
                "SELECT mtgo_event_id, name, format, started_at FROM tournaments
                 ORDER BY mtgo_event_id"
            ),
            vec![
                format!(
                    "Integer(77) | Text(\"{}\") | Text(\"CPAUPER\") | Integer({})",
                    tournament_name(77, BASE + HOUR, UTC),
                    BASE + HOUR
                ),
                format!(
                    "Integer(78) | Text(\"Q Challenge\") | Text(\"CLEGACY\") | Integer({})",
                    BASE + 30 * HOUR
                ),
            ]
        );
    }

    #[test]
    fn a_tournament_two_accounts_played_stays_with_the_lowest_match_id() {
        let (_dir, mut conn) = db("league-sync-tournament-two-accounts");
        let mut views = Vec::new();
        // The second account's match is stored first (lower id) but starts later.
        for (token, user, at) in [("q-b1", "Qsecond", BASE + HOUR), ("q-a1", "Qlocal", BASE)] {
            let mut played = view(token, "Qopponent", at);
            played.mtgo_match_id = None;
            for game in &mut played.games {
                game.mtgo_game_id = None;
            }
            played.local_username = user.to_string();
            played.tournament_event_id = Some(77);
            played.tournament_round = Some(1);
            views.push(played);
        }
        store_views(&mut conn, views);
        let second = account_id(&conn, "Qsecond");
        let local = account_id(&conn, "Qlocal");
        let owner = |conn: &Connection| -> Option<i64> {
            conn.query_row(
                "SELECT account_id FROM tournaments WHERE mtgo_event_id = 77",
                [],
                |row| row.get(0),
            )
            .optional()
            .expect("read")
        };

        // Whichever account syncs first, then the other, again and again.
        sync_one(&mut conn, LeagueOwner::Account(local), BASE + 2 * HOUR);
        assert_eq!(owner(&conn), Some(local), "the first sync creates it");
        let outcome = sync_one(&mut conn, LeagueOwner::Account(second), BASE + 2 * HOUR);
        assert_eq!(owner(&conn), Some(second), "the lowest match id takes it");
        assert!(outcome.leagues_changed);
        for _ in 0..2 {
            let a = sync_one(&mut conn, LeagueOwner::Account(local), BASE + 2 * HOUR);
            let b = sync_one(&mut conn, LeagueOwner::Account(second), BASE + 2 * HOUR);
            assert_eq!(owner(&conn), Some(second));
            assert!(!a.leagues_changed && !b.leagues_changed, "no flip");
        }
        assert_eq!(
            rows_of(&conn, "SELECT COUNT(*) FROM tournaments"),
            vec!["Integer(1)"]
        );
        let linked = rows_of(
            &conn,
            "SELECT COUNT(*) FROM matches WHERE tournament_id IS NOT NULL",
        );
        assert_eq!(linked, vec!["Integer(2)"]);
    }

    // A tournament row inserted without an account (the 0.x import's, the
    // API worker's) is adopted by the next league sync, not duplicated.
    #[test]
    fn a_tournament_row_without_an_account_is_adopted_not_duplicated() {
        let (_dir, mut conn) = db("league-sync-tournament-adopt");
        conn.execute(
            "INSERT INTO tournaments (mtgo_event_id, name, started_at) VALUES (77, 'Qimported', 5)",
            [],
        )
        .expect("a row with no account");
        tournament_views(&mut conn, 1);
        sync(&mut conn, BASE + 2 * HOUR);
        let rows = rows_of(
            &conn,
            "SELECT mtgo_event_id, account_id IS NOT NULL FROM tournaments
             ORDER BY mtgo_event_id",
        );
        assert_eq!(
            rows,
            vec!["Integer(77) | Integer(1)", "Integer(78) | Integer(1)"],
            "one row per event, the first adopted"
        );
        let linked: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM matches m JOIN tournaments t ON t.id = m.tournament_id
                 WHERE t.mtgo_event_id = 77",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(linked, 1);
    }

    /// One tournament match per (token, event), stored at `revision`.
    fn tournament_views(conn: &mut Connection, revision: u64) {
        let mut matches = Vec::new();
        for (token, event, at) in [("q-n1", 77, BASE), ("q-n2", 78, BASE + HOUR)] {
            let mut played = view(token, "Qopponent", at);
            played.mtgo_match_id = None;
            for game in &mut played.games {
                game.mtgo_game_id = None;
            }
            played.tournament_event_id = Some(event);
            played.tournament_round = Some(1);
            played.format = Some("CMODERN".to_string());
            matches.push(MatchWrite::Upsert {
                view: Box::new(played),
                revision,
            });
        }
        commit(
            conn,
            &WriteBatch {
                matches,
                ..WriteBatch::default()
            },
        )
        .expect("matches");
    }

    fn tournament_row(conn: &Connection, event: i64) -> Option<(String, bool, Option<i64>)> {
        conn.query_row(
            "SELECT name, name_synthesized, name_checked_at FROM tournaments
             WHERE mtgo_event_id = ?1",
            [event],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .expect("read")
    }

    // 07 spec section 8: a row the API named keeps its name and its check
    // across a league sync and a rebuild (matches derived again, the fold
    // rerun in another timezone); the sync never writes `name_checked_at`.
    #[test]
    fn the_league_sync_keeps_an_api_name() {
        let (_dir, mut conn) = db("league-sync-api-name");
        tournament_views(&mut conn, 1);
        sync(&mut conn, BASE + 2 * HOUR);
        let synthesized = tournament_name(78, BASE + HOUR, UTC);
        assert_eq!(
            tournament_row(&conn, 78),
            Some((synthesized.clone(), true, None))
        );
        conn.execute(
            "UPDATE tournaments SET name = 'Qlegacy Challenge 32', name_synthesized = 0,
                 name_checked_at = 5 WHERE mtgo_event_id = 77",
            [],
        )
        .expect("named by the API");
        conn.execute(
            "UPDATE tournaments SET name_checked_at = 6 WHERE mtgo_event_id = 78",
            [],
        )
        .expect("a 404");
        let named = Some(("Qlegacy Challenge 32".to_string(), false, Some(5)));
        let not_found = Some((synthesized, true, Some(6)));

        sync(&mut conn, BASE + 3 * HOUR);
        assert_eq!(tournament_row(&conn, 77), named);
        assert_eq!(tournament_row(&conn, 78), not_found);

        tournament_views(&mut conn, 2);
        conn.execute("DELETE FROM derivations", [])
            .expect("stale fold");
        sync_all(&mut conn, chrono_tz::Europe::London, BASE + 4 * HOUR).expect("rebuild");
        assert_eq!(tournament_row(&conn, 77), named);
        let renamed = tournament_name(78, BASE + HOUR, chrono_tz::Europe::London);
        assert_eq!(
            tournament_row(&conn, 78),
            Some((renamed, true, Some(6))),
            "a synthesized name follows the fold; its check stays"
        );
    }

    // A synthesized row the sync deletes takes its check with it: the
    // tournament is asked about again if it comes back.
    #[test]
    fn a_deleted_synthesized_row_is_asked_about_again() {
        let (_dir, mut conn) = db("league-sync-api-name-gone");
        tournament_views(&mut conn, 1);
        sync(&mut conn, BASE + 2 * HOUR);
        conn.execute(
            "UPDATE tournaments SET name_checked_at = 6 WHERE mtgo_event_id = 78",
            [],
        )
        .expect("a 404");
        hide(&conn, "q-n2");
        sync(&mut conn, BASE + 3 * HOUR);
        assert_eq!(tournament_row(&conn, 78), None);
        conn.execute(
            "UPDATE matches SET hidden_at = NULL WHERE token = 'q-n2'",
            [],
        )
        .expect("unhidden");
        sync(&mut conn, BASE + 4 * HOUR);
        let back = tournament_row(&conn, 78).expect("back");
        assert_eq!((back.1, back.2), (true, None));
    }

    #[test]
    fn an_account_less_imported_run_is_synced_by_nobody() {
        let (_dir, mut conn) = db("league-sync-nobody");
        store_views(&mut conn, vec![league_view("q-x1", LEAGUE, None, BASE)]);
        let mut the_seed = seed(LEAGUE, "complete", BASE - MINUTE);
        the_seed.completed_at = Some(BASE + HOUR);
        imported_run(&conn, 7, &the_seed);
        conn.execute("UPDATE leagues SET account_id = NULL", [])
            .expect("no account");
        link(&conn, "q-x1", "0x:7");
        let outcome = sync_one(&mut conn, LeagueOwner::Nobody, BASE + 2 * HOUR);
        assert!(outcome.leagues_changed);
        assert_eq!(league_row(&conn, "0x:7").1, "complete");
        assert_eq!(members(&conn, "0x:7"), vec!["q-x1"]);
        let completed: Option<i64> = conn
            .query_row(
                "SELECT completed_at FROM leagues WHERE run_key = '0x:7'",
                [],
                |row| row.get(0),
            )
            .expect("completed");
        assert_eq!(completed, Some(BASE + HOUR));
    }

    #[test]
    fn an_account_less_minted_draft_run_moves_to_the_account_when_its_draft_does() {
        let (_dir, mut conn) = db("league-sync-cross-owner");
        let file_id = hobbit_file(&conn);
        let account = number(&conn, "SELECT id FROM accounts");
        conn.execute(
            "INSERT INTO drafts (id, draft_token, account_id, source, league_event_id, state,
                 started_at, imported_from)
             VALUES (5, ?1, NULL, 'imported', 777, 'finished', 1, 50)",
            [HOBBIT_DRAFT],
        )
        .expect("an account-less 0.x draft");
        sync(&mut conn, 10 * HOUR);
        let key = format!("draft:{HOBBIT_DRAFT}");
        let (id, _, _, _) = league_row(&conn, &key);
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM leagues WHERE account_id IS NULL"
            ),
            1
        );

        store_events(&mut conn, hobbit_draft_events(file_id, None));
        sync_all(&mut conn, UTC, load_fixture("draft_hobbit").meta.now_ms())
            .expect("the full sync succeeds");
        let (moved, _, _, detached) = league_row(&conn, &key);
        assert_eq!((moved, detached), (id, None));
        let owner: Option<i64> = conn
            .query_row(
                "SELECT account_id FROM leagues WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .expect("owner");
        assert_eq!(owner, Some(account));
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM leagues"), 1);
        assert_eq!(
            number(&conn, "SELECT league_id FROM drafts WHERE id = 5"),
            id
        );
        // And it stays put.
        let changes = conn.total_changes();
        sync_all(&mut conn, UTC, load_fixture("draft_hobbit").meta.now_ms()).expect("again");
        assert_eq!(conn.total_changes(), changes);
    }

    #[test]
    fn a_corrupt_live_registered_line_is_skipped_not_fatal() {
        let (_dir, mut conn) = db("league-sync-corrupt-line");
        let file_id = file(&conn);
        store_views(
            &mut conn,
            vec![league_view("q-s1", LEAGUE, Some("S6FRA"), BASE)],
        );
        store_events(
            &mut conn,
            vec![registered_event(
                file_id,
                100,
                "q-s1",
                registered_line("q-s1", 1000..1023),
            )],
        );
        conn.execute(
            "INSERT INTO log_events (log_file_id, byte_offset, kind, match_token, logged_at, raw,
                 compressed)
             VALUES (?1, 200, 'match_deck_registered', 'q-s1', ?2, x'00', 1)",
            params![file_id, BASE + MINUTE],
        )
        .expect("a corrupt compressed line");
        let outcome = sync(&mut conn, BASE + HOUR);
        assert_eq!(outcome.skipped_snapshots, vec!["q-s1".to_string()]);
        assert_eq!(
            rows_of(
                &conn,
                "SELECT event_offset FROM limited_deck_snapshots WHERE source = 'registered'"
            ),
            vec!["Integer(100)"]
        );
    }

    // ---- Tournament tokens (0.x LinkMatchToTournament) ---------------------

    const MATCH_A: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    const MATCH_B: &str = "dddddddd-dddd-dddd-dddd-dddddddddddd";
    const TOUR_A: &str = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
    const TOUR_B: &str = "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee";

    /// 0.x `roundInfoRaw`.
    fn round_info_raw(tournament: &str, match_token: &str) -> String {
        format!(
            "15:43:44 [INF] (Game Management|Processing Registered Handler for FlsTournamentRoundInfoMessage in TournamentNotJoinedFiredState) Processor: TournamentNotJoinedFiredState Message: {{\"Token\":\"{tournament}\",\"ID\":12840476,\"Round\":{{\"Number\":1,\"Matches\":[{{\"EventToken\":\"{match_token}\",\"ParentToken\":\"{tournament}\",\"MatchCreateInfo\":{{\"MatchToken\":\"{match_token}\",\"MatchID\":286470836}}}}],\"ByeList\":[],\"Results\":[]}}}} Receiver: WotC.MtGO.Client.Model.Play.TournamentEvent.Tournament"
        )
    }

    fn round_info(file_id: i64, offset: u64, raw: &str, at_ms: i64) -> NewEvent {
        let mut event = event_with(
            file_id,
            offset,
            EventKind::TournamentRoundInfo,
            Subjects {
                tournament_token: Some("q-tour".to_string()),
                ..Subjects::default()
            },
            raw,
        );
        event.logged_at_ms = at_ms;
        event
    }

    /// A complete tournament match of Qlocal (it ends 4 seconds after it
    /// starts).
    fn tournament_match(token: &str, event: Option<u64>, started_at: i64) -> MatchView {
        let mut played = view(token, "Qopponent", started_at);
        played.mtgo_match_id = None;
        for game in &mut played.games {
            game.mtgo_game_id = None;
        }
        played.tournament_event_id = event;
        played.tournament_round = event.map(|_| 1);
        played
    }

    fn tournament_token(conn: &Connection, token: &str) -> Option<String> {
        conn.query_row(
            "SELECT tournament_token FROM matches WHERE token = ?1",
            [token],
            |row| row.get(0),
        )
        .expect("match")
    }

    // 0.x LinkMatchToTournamentTest: "populates tournament_token when a
    // round_info log event mentions the match" (the round info arrives
    // before the match, as MTGO logs it).
    #[test]
    fn a_round_info_before_the_match_gives_its_tournament_token() {
        let (_dir, mut conn) = db("league-sync-token-before");
        let file_id = file(&conn);
        store_events(
            &mut conn,
            vec![round_info(
                file_id,
                10,
                &round_info_raw(TOUR_A, MATCH_A),
                BASE - 10 * MINUTE,
            )],
        );
        store_views(&mut conn, vec![tournament_match(MATCH_A, Some(77), BASE)]);
        sync(&mut conn, BASE + HOUR);
        assert_eq!(tournament_token(&conn, MATCH_A).as_deref(), Some(TOUR_A));
    }

    // The "match created before round_info" order (0.x's backfill pass).
    #[test]
    fn a_round_info_after_the_match_gives_its_tournament_token() {
        let (_dir, mut conn) = db("league-sync-token-after");
        let file_id = file(&conn);
        store_views(&mut conn, vec![tournament_match(MATCH_A, Some(77), BASE)]);
        sync(&mut conn, BASE + HOUR);
        assert_eq!(tournament_token(&conn, MATCH_A), None);
        store_events(
            &mut conn,
            vec![round_info(
                file_id,
                10,
                &round_info_raw(TOUR_A, MATCH_A),
                BASE + 30 * MINUTE,
            )],
        );
        sync(&mut conn, BASE + HOUR);
        assert_eq!(tournament_token(&conn, MATCH_A).as_deref(), Some(TOUR_A));
    }

    // The newest round info that names the match may not carry a readable
    // tournament token (a damaged line): the newest readable one is taken,
    // so a live sync and a rebuild agree.
    #[test]
    fn the_newest_readable_round_info_gives_the_tournament_token() {
        let (_dir, mut conn) = db("league-sync-token-readable");
        let file_id = file(&conn);
        store_events(
            &mut conn,
            vec![
                round_info(
                    file_id,
                    10,
                    &round_info_raw(TOUR_A, MATCH_A),
                    BASE - 10 * MINUTE,
                ),
                round_info(
                    file_id,
                    20,
                    &format!("15:50:00 [INF] damaged round info for {MATCH_A}"),
                    BASE - 5 * MINUTE,
                ),
            ],
        );
        store_views(&mut conn, vec![tournament_match(MATCH_A, Some(77), BASE)]);
        sync(&mut conn, BASE + HOUR);
        assert_eq!(tournament_token(&conn, MATCH_A).as_deref(), Some(TOUR_A));
    }

    // 0.x "is a no-op when the match is already linked".
    #[test]
    fn a_linked_match_keeps_its_tournament_token() {
        let (_dir, mut conn) = db("league-sync-token-linked");
        let file_id = file(&conn);
        store_views(&mut conn, vec![tournament_match(MATCH_A, Some(123), BASE)]);
        conn.execute(
            "UPDATE matches SET tournament_token = 'cccccccc-cccc-cccc-cccc-cccccccccccc'",
            [],
        )
        .expect("linked");
        store_events(
            &mut conn,
            vec![round_info(
                file_id,
                10,
                &round_info_raw(TOUR_A, MATCH_A),
                BASE,
            )],
        );
        sync(&mut conn, BASE + HOUR);
        assert_eq!(
            tournament_token(&conn, MATCH_A).as_deref(),
            Some("cccccccc-cccc-cccc-cccc-cccccccccccc")
        );
    }

    // 0.x "skips matches with no tournament_event_id (non-tournament
    // matches)" and "does nothing when no round_info mentions this match";
    // round infos outside the match's window (a day before its start, an
    // hour after its end) do not count.
    #[test]
    fn only_a_tournament_match_named_by_a_round_info_in_its_window_is_linked() {
        let (_dir, mut conn) = db("league-sync-token-none");
        let file_id = file(&conn);
        let end = BASE + 4_000;
        store_events(
            &mut conn,
            vec![
                round_info(file_id, 10, &round_info_raw(TOUR_A, MATCH_A), BASE),
                round_info(file_id, 20, &round_info_raw(TOUR_A, "q-other"), BASE),
                round_info(
                    file_id,
                    30,
                    &round_info_raw(TOUR_B, "q-early"),
                    BASE - 25 * HOUR,
                ),
                round_info(
                    file_id,
                    40,
                    &round_info_raw(TOUR_B, "q-late"),
                    end + HOUR + 1,
                ),
                round_info(
                    file_id,
                    50,
                    &round_info_raw(TOUR_B, "q-edge-start"),
                    BASE - 24 * HOUR,
                ),
                round_info(
                    file_id,
                    60,
                    &round_info_raw(TOUR_B, "q-edge-end"),
                    end + HOUR,
                ),
            ],
        );
        store_views(
            &mut conn,
            vec![
                tournament_match(MATCH_A, None, BASE),
                tournament_match(MATCH_B, Some(77), BASE),
                tournament_match("q-early", Some(77), BASE),
                tournament_match("q-late", Some(77), BASE),
                tournament_match("q-edge-start", Some(77), BASE),
                tournament_match("q-edge-end", Some(77), BASE),
            ],
        );
        sync(&mut conn, BASE + 3 * HOUR);
        for token in [MATCH_A, MATCH_B, "q-early", "q-late"] {
            assert_eq!(tournament_token(&conn, token), None, "{token}");
        }
        assert_eq!(
            tournament_token(&conn, "q-edge-start").as_deref(),
            Some(TOUR_B)
        );
        assert_eq!(
            tournament_token(&conn, "q-edge-end").as_deref(),
            Some(TOUR_B)
        );
    }

    // 0.x "survives malformed round_info JSON (missing outer closing
    // brace)": the token is read with a pattern, not a JSON parser.
    #[test]
    fn a_malformed_round_info_still_gives_the_token() {
        let (_dir, mut conn) = db("league-sync-token-malformed");
        let file_id = file(&conn);
        let raw = round_info_raw(TOUR_A, MATCH_A).replace("\"Results\":[]}}", "\"Results\":[]}");
        assert!(
            serde_json::from_str::<serde_json::Value>(
                raw.split("Message: ")
                    .nth(1)
                    .expect("json")
                    .trim_end_matches(
                        " Receiver: WotC.MtGO.Client.Model.Play.TournamentEvent.Tournament"
                    )
            )
            .is_err()
        );
        store_events(&mut conn, vec![round_info(file_id, 10, &raw, BASE)]);
        store_views(&mut conn, vec![tournament_match(MATCH_A, Some(77), BASE)]);
        sync(&mut conn, BASE + HOUR);
        assert_eq!(tournament_token(&conn, MATCH_A).as_deref(), Some(TOUR_A));
    }

    // 0.x `orderByDesc('id')`: the newest round info naming the match wins;
    // one whose token does not read is passed over for the newest readable
    // one (Q8 M2: live and rebuild agree), and with none the match stays
    // unlinked.
    #[test]
    fn the_newest_round_info_naming_the_match_decides() {
        let (_dir, mut conn) = db("league-sync-token-newest");
        let file_id = file(&conn);
        store_events(
            &mut conn,
            vec![
                round_info(file_id, 10, &round_info_raw(TOUR_A, MATCH_A), BASE - HOUR),
                round_info(file_id, 20, &round_info_raw(TOUR_B, MATCH_A), BASE - MINUTE),
                round_info(file_id, 30, &round_info_raw(TOUR_A, MATCH_B), BASE - MINUTE),
                round_info(
                    file_id,
                    40,
                    &round_info_raw("not-a-guid", MATCH_B),
                    BASE - 30 * 1_000,
                ),
            ],
        );
        store_views(
            &mut conn,
            vec![
                tournament_match(MATCH_A, Some(77), BASE),
                tournament_match(MATCH_B, Some(77), BASE),
            ],
        );
        sync(&mut conn, BASE + HOUR);
        assert_eq!(tournament_token(&conn, MATCH_A).as_deref(), Some(TOUR_B));
        assert_eq!(
            tournament_token(&conn, MATCH_B).as_deref(),
            Some(TOUR_A),
            "the unreadable newer one is passed over"
        );
    }

    // `tournaments.token` is the token of the tournament's first match that
    // has one.
    #[test]
    fn a_tournament_takes_the_token_of_its_first_linked_match() {
        let (_dir, mut conn) = db("league-sync-token-tournament");
        let file_id = file(&conn);
        store_views(
            &mut conn,
            vec![
                tournament_match("q-first", Some(77), BASE),
                tournament_match(MATCH_A, Some(77), BASE + HOUR),
                tournament_match(MATCH_B, Some(77), BASE + 2 * HOUR),
            ],
        );
        sync(&mut conn, BASE + 3 * HOUR);
        assert_eq!(
            rows_of(&conn, "SELECT token FROM tournaments"),
            vec!["Null"]
        );
        store_events(
            &mut conn,
            vec![
                round_info(
                    file_id,
                    10,
                    &round_info_raw(TOUR_B, MATCH_B),
                    BASE + 2 * HOUR,
                ),
                round_info(file_id, 20, &round_info_raw(TOUR_A, MATCH_A), BASE + HOUR),
            ],
        );
        sync(&mut conn, BASE + 3 * HOUR);
        assert_eq!(
            rows_of(&conn, "SELECT token FROM tournaments"),
            vec![format!("Text(\"{TOUR_A}\")")]
        );
    }

    /// The steps of two tournament matches and their round infos, one
    /// batch each; a sync after each (`each`) or once at the end.
    fn tournament_steps(conn: &mut Connection, each: bool) {
        let file_id = file(conn);
        let steps: Vec<(Vec<NewEvent>, Vec<MatchView>)> = vec![
            (
                vec![round_info(
                    file_id,
                    10,
                    &round_info_raw(TOUR_A, MATCH_A),
                    BASE - MINUTE,
                )],
                Vec::new(),
            ),
            (Vec::new(), vec![tournament_match(MATCH_A, Some(77), BASE)]),
            (
                Vec::new(),
                vec![tournament_match(MATCH_B, Some(78), BASE + HOUR)],
            ),
            (
                vec![round_info(
                    file_id,
                    20,
                    &round_info_raw(TOUR_B, "q-elsewhere"),
                    BASE + HOUR,
                )],
                Vec::new(),
            ),
            (
                vec![round_info(
                    file_id,
                    30,
                    &round_info_raw(TOUR_B, MATCH_B),
                    BASE + HOUR + 10 * MINUTE,
                )],
                Vec::new(),
            ),
        ];
        for (events, views) in steps {
            let matches = views
                .into_iter()
                .map(|played| MatchWrite::Upsert {
                    view: Box::new(played),
                    revision: 1,
                })
                .collect();
            let batch = WriteBatch {
                events,
                matches,
                ..WriteBatch::default()
            };
            commit(conn, &batch).expect("commit");
            if each {
                sync(conn, BASE + 3 * HOUR);
            }
        }
        sync(conn, BASE + 3 * HOUR);
    }

    // Live equals rebuild: a sync after each step gives the same tokens as
    // one sync at the end, and a second sync writes nothing.
    #[test]
    fn tournament_tokens_live_equal_rebuild() {
        let (_live_dir, mut live) = db("league-sync-tokens-live");
        tournament_steps(&mut live, true);
        let (_end_dir, mut end) = db("league-sync-tokens-end");
        tournament_steps(&mut end, false);
        let tokens = |conn: &Connection| {
            let mut all = rows_of(
                conn,
                "SELECT token, tournament_token FROM matches ORDER BY token",
            );
            all.extend(synced(conn));
            all
        };
        assert_eq!(
            rows_of(
                &end,
                "SELECT token, tournament_token FROM matches ORDER BY token"
            ),
            vec![
                format!("Text(\"{MATCH_A}\") | Text(\"{TOUR_A}\")"),
                format!("Text(\"{MATCH_B}\") | Text(\"{TOUR_B}\")"),
            ]
        );
        assert_eq!(tokens(&live), tokens(&end));
        let before = end.total_changes();
        let outcome = sync(&mut end, BASE + 3 * HOUR);
        assert!(!outcome.leagues_changed);
        assert_eq!(end.total_changes(), before);
    }
    /// A v1 log run `LEAGUE:q-x1` (matches q-x1, q-x2) with `inputs`, then a
    /// 0.x run `0x:7` claiming both matches, as the import leaves them.
    fn claimed_by_import(
        conn: &mut Connection,
        v1_note: Option<&str>,
        v1_drop: Option<i64>,
        zero_x_note: Option<&str>,
        zero_x_drop: Option<i64>,
    ) {
        store_views(
            conn,
            vec![
                league_view("q-x1", LEAGUE, Some("CMODERN"), BASE),
                league_view("q-x2", LEAGUE, Some("CMODERN"), BASE + HOUR),
            ],
        );
        sync(conn, BASE + 2 * HOUR);
        conn.execute(
            "UPDATE leagues SET notes = ?1, user_dropped_at = ?2,
                 user_state = CASE WHEN ?2 IS NULL THEN NULL ELSE 'dropped' END
             WHERE run_key = ?3",
            params![v1_note, v1_drop, format!("{LEAGUE}:q-x1")],
        )
        .expect("v1 inputs");
        let the_seed = seed(LEAGUE, "active", BASE - MINUTE);
        imported_run(conn, 7, &the_seed);
        conn.execute(
            "UPDATE leagues SET notes = ?1, user_dropped_at = ?2,
                 user_state = CASE WHEN ?2 IS NULL THEN NULL ELSE 'dropped' END
             WHERE run_key = '0x:7'",
            params![zero_x_note, zero_x_drop],
        )
        .expect("0.x inputs");
        link(conn, "q-x1", "0x:7");
        link(conn, "q-x2", "0x:7");
        sync(conn, BASE + 3 * HOUR);
    }

    fn inputs_of(conn: &Connection, run_key: &str) -> (Option<String>, Option<i64>) {
        conn.query_row(
            "SELECT notes, user_dropped_at FROM leagues WHERE run_key = ?1",
            [run_key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("row")
    }

    #[test]
    fn an_import_claiming_a_run_receives_the_log_rows_note_and_drop() {
        let (_dir, mut conn) = db("league-sync-carry");
        claimed_by_import(
            &mut conn,
            Some("v1 note"),
            Some(BASE + 5 * HOUR),
            None,
            None,
        );
        assert_eq!(
            inputs_of(&conn, "0x:7"),
            (Some("v1 note".to_string()), Some(BASE + 5 * HOUR))
        );
        assert_eq!(league_row(&conn, "0x:7").1, "dropped");
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM leagues"), 1);
    }

    #[test]
    fn an_import_claiming_a_run_carries_a_drop_with_its_own_status() {
        // The log row was dropped, then set Complete: the imported row that
        // claims it follows that status, not a forced Dropped.
        let (_dir2, mut conn2) = db("league-sync-carry-status-complete");
        store_views(
            &mut conn2,
            vec![
                league_view("q-x1", LEAGUE, Some("CMODERN"), BASE),
                league_view("q-x2", LEAGUE, Some("CMODERN"), BASE + HOUR),
            ],
        );
        sync(&mut conn2, BASE + 2 * HOUR);
        conn2
            .execute(
                "UPDATE leagues SET user_dropped_at = ?1, user_state = 'complete'
                 WHERE run_key = ?2",
                params![BASE + 5 * HOUR, format!("{LEAGUE}:q-x1")],
            )
            .expect("dropped then complete");
        let the_seed = seed(LEAGUE, "active", BASE - MINUTE);
        imported_run(&conn2, 7, &the_seed);
        link(&conn2, "q-x1", "0x:7");
        link(&conn2, "q-x2", "0x:7");
        sync(&mut conn2, BASE + 3 * HOUR);
        assert_eq!(
            rows_of(
                &conn2,
                "SELECT user_state, user_dropped_at FROM leagues WHERE run_key = '0x:7'"
            ),
            vec![format!("Text(\"complete\") | Integer({})", BASE + 5 * HOUR)]
        );
        assert_ne!(league_row(&conn2, "0x:7").1, "dropped");
    }

    /// Both drops are unedited (`edited_at` NULL): a tie, so the imported
    /// row keeps its own drop while the log row's note fills it.
    #[test]
    fn a_v1_note_beats_the_imported_one_and_of_two_unedited_drops_the_imported_one_stays() {
        let (_dir, mut conn) = db("league-sync-carry-wins");
        claimed_by_import(
            &mut conn,
            Some("v1 note"),
            Some(BASE + 9 * HOUR),
            Some("0.x note"),
            Some(BASE + 5 * HOUR),
        );
        assert_eq!(
            inputs_of(&conn, "0x:7"),
            (Some("v1 note".to_string()), Some(BASE + 5 * HOUR))
        );
    }

    #[test]
    fn an_empty_log_note_never_overwrites_the_imported_one() {
        let (_dir, mut conn) = db("league-sync-carry-empty");
        claimed_by_import(&mut conn, None, None, Some("0.x note"), None);
        assert_eq!(
            inputs_of(&conn, "0x:7"),
            (Some("0.x note".to_string()), None)
        );
    }
    /// A log run `LEAGUE:q-x1` and a second log row `LEAGUE:q-x2` with the
    /// given (note, edited at) each, and the imported row's (note, edited at),
    /// then a 0.x run `0x:7` claims both matches and the sync runs.
    fn two_log_rows_claimed(
        conn: &mut Connection,
        first: (Option<&str>, Option<i64>),
        second: (Option<&str>, Option<i64>),
        imported: (Option<&str>, Option<i64>),
    ) {
        store_views(
            conn,
            vec![
                league_view("q-x1", LEAGUE, Some("CMODERN"), BASE),
                league_view("q-x2", LEAGUE, Some("CMODERN"), BASE + HOUR),
            ],
        );
        sync(conn, BASE + 2 * HOUR);
        conn.execute(
            "INSERT INTO leagues (run_key, account_id, source, kind, state, token, name,
                 started_at, created_at)
             SELECT ?1, account_id, 'log', kind, state, token, name, started_at, created_at
             FROM leagues WHERE run_key = ?2",
            params![format!("{LEAGUE}:q-x2"), format!("{LEAGUE}:q-x1")],
        )
        .expect("second log row");
        for (key, (note, at)) in [
            (format!("{LEAGUE}:q-x1"), first),
            (format!("{LEAGUE}:q-x2"), second),
        ] {
            conn.execute(
                "UPDATE leagues SET notes = ?1, notes_edited_at = ?2 WHERE run_key = ?3",
                params![note, at, key],
            )
            .expect("note");
        }
        imported_run(conn, 7, &seed(LEAGUE, "active", BASE - MINUTE));
        conn.execute(
            "UPDATE leagues SET notes = ?1, notes_edited_at = ?2 WHERE run_key = '0x:7'",
            params![imported.0, imported.1],
        )
        .expect("imported note");
        link(conn, "q-x1", "0x:7");
        link(conn, "q-x2", "0x:7");
        sync(conn, BASE + 3 * HOUR);
    }

    #[test]
    fn the_most_recently_edited_log_note_wins_the_carry() {
        let (_dir, mut conn) = db("league-sync-carry-newest");
        two_log_rows_claimed(
            &mut conn,
            (Some("older"), Some(BASE + 5 * HOUR)),
            (Some("newest"), Some(BASE + 7 * HOUR)),
            (Some("0.x note"), None),
        );
        assert_eq!(inputs_of(&conn, "0x:7").0.as_deref(), Some("newest"));
        let (_dir, mut conn) = db("league-sync-carry-newest-first");
        two_log_rows_claimed(
            &mut conn,
            (Some("newest"), Some(BASE + 7 * HOUR)),
            (Some("older"), Some(BASE + 5 * HOUR)),
            (None, None),
        );
        assert_eq!(inputs_of(&conn, "0x:7").0.as_deref(), Some("newest"));
    }

    #[test]
    fn a_v1_note_on_the_imported_row_is_kept() {
        let (_dir, mut conn) = db("league-sync-carry-keeps-v1");
        two_log_rows_claimed(
            &mut conn,
            (Some("log note"), Some(BASE + 9 * HOUR)),
            (None, None),
            (Some("typed after the import"), Some(BASE + 4 * HOUR)),
        );
        assert_eq!(
            inputs_of(&conn, "0x:7").0.as_deref(),
            Some("typed after the import")
        );
    }

    #[test]
    fn a_blank_log_note_never_erases_the_imported_one() {
        let (_dir, mut conn) = db("league-sync-carry-blank");
        two_log_rows_claimed(
            &mut conn,
            (Some("   "), Some(BASE + 9 * HOUR)),
            (None, None),
            (Some("0.x note"), None),
        );
        assert_eq!(inputs_of(&conn, "0x:7").0.as_deref(), Some("0.x note"));
    }

    /// "Remove from league" as the league edit does it (spec 4.2).
    fn remove_from_leagues(conn: &Connection, token: &str) {
        conn.execute(
            "UPDATE matches SET no_league = 1, league_linked = 0, league_id = NULL
             WHERE token = ?1",
            [token],
        )
        .expect("no league");
    }

    fn three_league_matches() -> Vec<MatchView> {
        vec![
            league_view("q-m1", LEAGUE, Some("CMODERN"), BASE),
            league_view("q-m2", LEAGUE, Some("CMODERN"), BASE + HOUR),
            league_view("q-m3", LEAGUE, Some("CMODERN"), BASE + 2 * HOUR),
        ]
    }

    #[test]
    fn a_drop_counts_only_while_the_players_status_is_dropped() {
        let (_dir, mut conn) = db("league-sync-drop-status");
        store_views(
            &mut conn,
            vec![
                league_view("q-m1", LEAGUE, Some("CMODERN"), BASE),
                league_view("q-m2", LEAGUE, Some("CMODERN"), BASE + HOUR),
            ],
        );
        sync(&mut conn, BASE + 2 * HOUR);
        let key = format!("{LEAGUE}:q-m1");
        conn.execute(
            "UPDATE leagues SET user_dropped_at = ?2 WHERE run_key = ?1",
            params![key, BASE + 2 * HOUR],
        )
        .expect("a drop time alone");
        sync(&mut conn, BASE + 3 * HOUR);
        assert_eq!(
            league_row(&conn, &key).1,
            "active",
            "a drop time without the Dropped status is no drop"
        );

        conn.execute(
            "UPDATE leagues SET user_state = 'dropped' WHERE run_key = ?1",
            [&key],
        )
        .expect("dropped");
        sync(&mut conn, BASE + 4 * HOUR);
        assert_eq!(league_row(&conn, &key).1, "dropped");

        // The next match of the token starts a new league (decision 2).
        store_views(
            &mut conn,
            vec![league_view(
                "q-m3",
                LEAGUE,
                Some("CMODERN"),
                BASE + 5 * HOUR,
            )],
        );
        sync(&mut conn, BASE + 6 * HOUR);
        assert_eq!(
            keys(&conn),
            vec![
                format!("{LEAGUE}:q-m1 constructed dropped"),
                format!("{LEAGUE}:q-m3 constructed active"),
            ]
        );

        // Another status leaves the drop time, which the fold then ignores:
        // the run is Active again and takes the later match back.
        conn.execute(
            "UPDATE leagues SET user_state = 'complete' WHERE run_key = ?1",
            [&key],
        )
        .expect("complete");
        sync(&mut conn, BASE + 7 * HOUR);
        assert_eq!(
            keys(&conn),
            vec![format!("{LEAGUE}:q-m1 constructed active")]
        );
        assert_eq!(members(&conn, &key), vec!["q-m1", "q-m2", "q-m3"]);
    }

    #[test]
    fn a_match_removed_from_every_league_is_in_no_run() {
        let (_dir, mut conn) = db("league-sync-no-league");
        store_views(&mut conn, three_league_matches());
        sync(&mut conn, BASE + 3 * HOUR);
        let key = format!("{LEAGUE}:q-m1");
        assert_eq!(members(&conn, &key), vec!["q-m1", "q-m2", "q-m3"]);

        remove_from_leagues(&conn, "q-m2");
        sync(&mut conn, BASE + 4 * HOUR);
        assert_eq!(members(&conn, &key), vec!["q-m1", "q-m3"]);
        assert_eq!(
            rows_of(
                &conn,
                "SELECT no_league, league_linked, league_id FROM matches WHERE token = 'q-m2'"
            ),
            vec!["Integer(1) | Integer(0) | Null"]
        );
    }

    #[test]
    fn removing_a_runs_first_match_from_leagues_live_equals_rebuild() {
        let (_live_dir, mut live) = db("league-sync-no-league-live");
        store_views(&mut live, three_league_matches());
        sync(&mut live, BASE + 3 * HOUR);
        remove_from_leagues(&live, "q-m1");
        sync(&mut live, BASE + 4 * HOUR);

        let (_rebuilt_dir, mut rebuilt) = db("league-sync-no-league-rebuild");
        store_views(&mut rebuilt, three_league_matches());
        remove_from_leagues(&rebuilt, "q-m1");
        sync(&mut rebuilt, BASE + 4 * HOUR);

        assert_eq!(synced(&live), synced(&rebuilt));
        assert_eq!(
            keys(&live),
            vec![format!("{LEAGUE}:q-m2 constructed active")]
        );
    }

    #[test]
    fn a_tournament_match_removed_from_leagues_stays_in_its_tournament() {
        let (_dir, mut conn) = db("league-sync-no-league-tournament");
        let mut played = view("q-t1", "Qopponent", BASE);
        played.mtgo_match_id = None;
        for game in &mut played.games {
            game.mtgo_game_id = None;
        }
        played.tournament_event_id = Some(77);
        played.tournament_round = Some(1);
        store_views(&mut conn, vec![played]);
        remove_from_leagues(&conn, "q-t1");
        sync(&mut conn, BASE + HOUR);
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches m JOIN tournaments t ON t.id = m.tournament_id
                 WHERE m.token = 'q-t1' AND t.mtgo_event_id = 77"
            ),
            1
        );
    }

    /// Spec 4.1: a match linked into a log run is attached by its row, never
    /// folded: its own token's run is not made, and the run's lifecycle
    /// state is the fold's. (Passes before this task: it pins the rule the
    /// row survival in Task 3 builds on.)
    #[test]
    fn a_match_linked_into_a_log_run_is_attached_by_row_not_folded() {
        let (_dir, mut conn) = db("league-sync-linked-log-run");
        let mut views = vec![league_view(
            "q-x1",
            LEAGUE_2,
            Some("CMODERN"),
            BASE - 10 * HOUR,
        )];
        for index in 0..4 {
            views.push(league_view(
                &format!("q-m{index}"),
                LEAGUE,
                Some("CMODERN"),
                BASE + index * HOUR,
            ));
        }
        store_views(&mut conn, views);
        sync(&mut conn, BASE + 5 * HOUR);
        let key = format!("{LEAGUE}:q-m0");
        link(&conn, "q-x1", &key);
        sync(&mut conn, BASE + 6 * HOUR);

        assert_eq!(
            members(&conn, &key),
            vec!["q-x1", "q-m0", "q-m1", "q-m2", "q-m3"]
        );
        assert_eq!(
            keys(&conn),
            vec![format!("{key} constructed active")],
            "q-x1's own run is gone; five members but four folded: still Active"
        );
        assert_eq!(
            number(
                &conn,
                "SELECT league_linked FROM matches WHERE token = 'q-x1'"
            ),
            1
        );
    }

    /// `(removed_at, user_state, edited_at)` of the row `run_key`.
    fn status_of(conn: &Connection, run_key: &str) -> (Option<i64>, Option<String>, Option<i64>) {
        conn.query_row(
            "SELECT removed_at, user_state, edited_at FROM leagues WHERE run_key = ?1",
            [run_key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap_or_else(|error| panic!("{run_key}: {error}"))
    }

    /// A log row whose fold matches are all gone but which still holds a
    /// listed hand-linked match stays listed (attached): the linked match is
    /// its member by `league_id` (spec 4.1). Once that match is hidden too,
    /// the row is detached, never deleted (deleting would SET NULL the link,
    /// which the baseline CHECK refuses).
    #[test]
    fn a_log_run_holding_only_a_linked_member_stays_listed() {
        let (_dir, mut conn) = db("league-sync-linked-detach");
        store_views(
            &mut conn,
            vec![
                league_view("q-m1", LEAGUE, Some("CMODERN"), BASE),
                league_view("q-x1", LEAGUE_2, Some("CMODERN"), BASE + HOUR),
            ],
        );
        sync(&mut conn, BASE + 2 * HOUR);
        let key = format!("{LEAGUE}:q-m1");
        let (id, _, _, _) = league_row(&conn, &key);
        link(&conn, "q-x1", &key);
        sync(&mut conn, BASE + 3 * HOUR);

        // The fold no longer makes the run: its only folded match is hidden.
        // The linked match still holds the row: listed, not detached, and
        // Partial, as no fold run writes its state (issue #415).
        hide(&conn, "q-m1");
        sync(&mut conn, BASE + 4 * HOUR);
        assert_eq!(
            league_row(&conn, &key),
            (id, "partial".to_string(), None, None)
        );
        // Only a stale Active becomes Partial: Complete and Dropped stay.
        for state in ["complete", "dropped", "partial"] {
            conn.execute(
                "UPDATE leagues SET state = ?1 WHERE id = ?2",
                params![state, id],
            )
            .expect("state");
            sync(&mut conn, BASE + 4 * HOUR);
            assert_eq!(league_row(&conn, &key).1, state);
        }
        assert_eq!(members(&conn, &key), vec!["q-x1"]);
        assert_eq!(
            number(
                &conn,
                "SELECT league_linked FROM matches WHERE token = 'q-x1'"
            ),
            1
        );

        // The linked match hidden too: detached, not deleted.
        hide(&conn, "q-x1");
        sync(&mut conn, BASE + 5 * HOUR);
        assert_eq!(
            league_row(&conn, &key),
            (id, "partial".to_string(), None, Some(BASE + 5 * HOUR))
        );

        // The linked match back: attached again without a fold run.
        unhide(&conn, "q-x1");
        sync(&mut conn, BASE + 6 * HOUR);
        assert_eq!(
            league_row(&conn, &key),
            (id, "partial".to_string(), None, None)
        );

        // Its folded match back: the fold writes the run's state again.
        unhide(&conn, "q-m1");
        sync(&mut conn, BASE + 7 * HOUR);
        assert_eq!(
            league_row(&conn, &key),
            (id, "active".to_string(), None, None)
        );
        assert_eq!(members(&conn, &key), vec!["q-m1", "q-x1"]);
    }

    #[test]
    fn a_removed_league_survives_every_sync_and_comes_back_on_restore() {
        let (_dir, mut conn) = db("league-sync-removed");
        store_views(
            &mut conn,
            vec![
                league_view("q-m1", LEAGUE, Some("CMODERN"), BASE),
                league_view("q-m2", LEAGUE, Some("CMODERN"), BASE + HOUR),
            ],
        );
        sync(&mut conn, BASE + 2 * HOUR);
        let key = format!("{LEAGUE}:q-m1");
        let (id, _, _, _) = league_row(&conn, &key);

        // Deleted, matches kept: the fold still makes the run, so its row
        // stays attached and the sync never writes `removed_at`.
        let deleted_at = BASE + 3 * HOUR;
        conn.execute(
            "UPDATE leagues SET removed_at = ?2, edited_at = ?2 WHERE id = ?1",
            params![id, deleted_at],
        )
        .expect("deleted");
        sync(&mut conn, BASE + 4 * HOUR);
        assert_eq!(
            league_row(&conn, &key),
            (id, "active".to_string(), None, None)
        );
        assert_eq!(
            status_of(&conn, &key),
            (Some(deleted_at), None, Some(deleted_at))
        );
        assert_eq!(members(&conn, &key), vec!["q-m1", "q-m2"]);

        // Its matches deleted too: no fold run, the row is detached, not
        // deleted.
        for token in ["q-m1", "q-m2"] {
            conn.execute(
                "UPDATE matches SET hidden_at = ?2, edited_at = ?2 WHERE token = ?1",
                params![token, deleted_at],
            )
            .expect("hidden");
        }
        sync(&mut conn, BASE + 5 * HOUR);
        assert_eq!(
            league_row(&conn, &key),
            (id, "active".to_string(), None, Some(BASE + 5 * HOUR))
        );
        assert_eq!(
            status_of(&conn, &key),
            (Some(deleted_at), None, Some(deleted_at))
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM leagues"), 1);

        // Undo: the league and its matches back; the fold makes its key
        // again and takes the same row.
        conn.execute(
            "UPDATE leagues SET removed_at = NULL, edited_at = ?2 WHERE id = ?1",
            params![id, BASE + 6 * HOUR],
        )
        .expect("restored");
        unhide(&conn, "q-m1");
        unhide(&conn, "q-m2");
        sync(&mut conn, BASE + 7 * HOUR);
        assert_eq!(
            league_row(&conn, &key),
            (id, "active".to_string(), None, None)
        );
        assert_eq!(members(&conn, &key), vec!["q-m1", "q-m2"]);
    }

    /// Spec 4.4 (ruling after LF9's review): a league deleted keeping its
    /// matches keeps its run when the run's key changes. Its first match
    /// deleted on its own, the fold's key moves to the second match and the
    /// unmatched-row fallback takes the deleted row (its kept matches point
    /// at it): rekeyed, still deleted, no new listed league. Its Restore
    /// then shows the kept matches in it again (spec 4.5).
    #[test]
    fn a_removed_row_keeps_its_run_when_the_key_changes() {
        let (_dir, mut conn) = db("league-sync-removed-fallback");
        store_views(&mut conn, three_league_matches());
        sync(&mut conn, BASE + 3 * HOUR);
        let old_key = format!("{LEAGUE}:q-m1");
        let (old_id, _, _, _) = league_row(&conn, &old_key);
        let deleted_at = BASE + 4 * HOUR;
        conn.execute(
            "UPDATE leagues SET removed_at = ?2, edited_at = ?2 WHERE id = ?1",
            params![old_id, deleted_at],
        )
        .expect("deleted, matches kept");

        // The first match goes: the fold's key changes to the second match.
        hide(&conn, "q-m1");
        sync(&mut conn, BASE + 5 * HOUR);
        let new_key = format!("{LEAGUE}:q-m2");
        let (new_id, _, _, detached) = league_row(&conn, &new_key);
        assert_eq!(new_id, old_id, "the deleted row, rekeyed");
        assert_eq!(detached, None);
        assert_eq!(
            status_of(&conn, &new_key).0,
            Some(deleted_at),
            "still deleted"
        );
        assert_eq!(members(&conn, &new_key), vec!["q-m2", "q-m3"]);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM leagues"), 1);
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM listed_leagues"),
            0,
            "no new listed league"
        );

        let restore = crate::leagues::LeagueEdit::Restore {
            run_key: new_key.clone(),
            now_ms: BASE + 6 * HOUR,
        };
        assert_eq!(
            crate::leagues::edit_league(&mut conn, &restore, chrono_tz::UTC).expect("restore"),
            crate::leagues::LeagueEditOutcome::Done
        );
        assert_eq!(status_of(&conn, &new_key).0, None);
        assert_eq!(members(&conn, &new_key), vec!["q-m2", "q-m3"]);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM listed_leagues"), 1);
    }

    #[test]
    fn a_hand_over_moves_the_player_status_and_the_linked_matches() {
        let (_dir, mut conn) = db("league-sync-hand-over-status");
        conn.execute_batch(
            "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);",
        )
        .expect("account");
        let file_id = file(&conn);
        store_events(
            &mut conn,
            vec![joined(file_id, 10, LEAGUE_2, BASE - 4 * HOUR)],
        );
        imported_draft(&conn, BASE - 3 * HOUR);
        creatures(&conn, 1000..=1041);
        sync(&mut conn, BASE - 150 * MINUTE);
        let draft_key = format!("draft:{DRAFT}");
        let (draft_id, _, _, _) = league_row(&conn, &draft_key);

        // The player's status and a hand-linked match on the draft's run.
        store_views(
            &mut conn,
            vec![league_view(
                "q-c1",
                "q-league-0003",
                Some("CMODERN"),
                BASE - 5 * HOUR,
            )],
        );
        link(&conn, "q-c1", &draft_key);
        // And a match deleted from the draft's run (spec section 13).
        store_views(
            &mut conn,
            vec![league_view(
                "q-c2",
                "q-league-0004",
                Some("CMODERN"),
                BASE - 5 * HOUR + MINUTE,
            )],
        );
        conn.execute(
            "UPDATE matches SET league_id = ?1, hidden_at = ?2 WHERE token = 'q-c2'",
            params![draft_id, BASE - 145 * MINUTE],
        )
        .expect("deleted from the draft's run");
        let edited = BASE - 140 * MINUTE;
        conn.execute(
            "UPDATE leagues SET removed_at = ?2, user_state = 'complete', edited_at = ?2
             WHERE id = ?1",
            params![draft_id, edited],
        )
        .expect("status");

        // The adopting run's own row already exists when the late adoption
        // comes, so the draft's row is handed over, not rekeyed.
        let key = format!("{LEAGUE}:q-m1");
        let match_at = BASE - 2 * HOUR;
        conn.execute(
            "INSERT INTO leagues (run_key, account_id, source, kind, state, token, name,
                 started_at, created_at)
             VALUES (?1, 1, 'log', 'draft', 'active', ?2, 'Q run', ?3, ?3)",
            params![key, LEAGUE, match_at],
        )
        .expect("the run's row");
        let (run_id, _, _, _) = league_row(&conn, &key);
        store_events(
            &mut conn,
            vec![joined(file_id, 20, LEAGUE, match_at - MINUTE)],
        );
        store_views(
            &mut conn,
            vec![league_view("q-m1", LEAGUE, Some("DHOBHOBHOB"), match_at)],
        );
        store_events(
            &mut conn,
            vec![registered_event(
                file_id,
                30,
                "q-m1",
                registered_line("q-m1", 1000..=1022),
            )],
        );
        sync(&mut conn, BASE);

        assert_eq!(
            status_of(&conn, &key),
            (Some(edited), Some("complete".to_string()), Some(edited))
        );
        assert_eq!(
            number(&conn, "SELECT league_id FROM matches WHERE token = 'q-c1'"),
            run_id
        );
        assert_eq!(
            number(
                &conn,
                "SELECT deleted_from_league_id FROM matches WHERE token = 'q-c2'"
            ),
            run_id,
            "the deleted match names the surviving row"
        );
        assert_eq!(
            number(
                &conn,
                &format!("SELECT COUNT(*) FROM leagues WHERE id = {draft_id}")
            ),
            0,
            "the retired row carried nothing else, so it is deleted"
        );
        assert_eq!(
            number(&conn, "SELECT league_id FROM drafts WHERE id = 5"),
            run_id
        );
    }

    #[test]
    fn an_import_claiming_a_run_receives_its_status_and_linked_matches() {
        let (_dir, mut conn) = db("league-sync-carry-status");
        store_views(
            &mut conn,
            vec![
                league_view("q-x1", LEAGUE, Some("CMODERN"), BASE),
                league_view("q-x2", LEAGUE, Some("CMODERN"), BASE + HOUR),
                league_view("q-y1", LEAGUE_2, Some("CMODERN"), BASE + 2 * HOUR),
                league_view("q-x3", LEAGUE, Some("CMODERN"), BASE + 2 * HOUR),
            ],
        );
        sync(&mut conn, BASE + 3 * HOUR);
        let log_key = format!("{LEAGUE}:q-x1");
        link(&conn, "q-y1", &log_key);
        // A match deleted from the log run (spec section 13).
        conn.execute(
            "UPDATE matches SET hidden_at = ?1 WHERE token = 'q-x3'",
            [BASE + 3 * HOUR],
        )
        .expect("deleted");
        let (log_id, _, _, _) = league_row(&conn, &log_key);
        assert_eq!(
            number(
                &conn,
                "SELECT deleted_from_league_id FROM matches WHERE token = 'q-x3'"
            ),
            log_id
        );
        conn.execute(
            "UPDATE leagues SET user_state = 'partial', edited_at = ?2 WHERE run_key = ?1",
            params![log_key, BASE + 4 * HOUR],
        )
        .expect("status");
        imported_run(&conn, 7, &seed(LEAGUE, "active", BASE - MINUTE));
        link(&conn, "q-x1", "0x:7");
        link(&conn, "q-x2", "0x:7");
        sync(&mut conn, BASE + 5 * HOUR);

        assert_eq!(
            status_of(&conn, "0x:7"),
            (None, Some("partial".to_string()), Some(BASE + 4 * HOUR))
        );
        assert_eq!(members(&conn, "0x:7"), vec!["q-x1", "q-x2", "q-y1"]);
        assert_eq!(keys(&conn), vec!["0x:7 constructed active"]);
        let (imported_id, _, _, _) = league_row(&conn, "0x:7");
        assert_eq!(
            number(
                &conn,
                "SELECT deleted_from_league_id FROM matches WHERE token = 'q-x3'"
            ),
            imported_id,
            "the deleted match names the imported row"
        );
    }

    #[test]
    fn the_newer_status_wins_a_carry_and_the_loser_is_cleared() {
        let (_dir, mut conn) = db("league-sync-carry-newest-status");
        store_views(
            &mut conn,
            vec![
                league_view("q-x1", LEAGUE, Some("CMODERN"), BASE),
                league_view("q-x2", LEAGUE, Some("CMODERN"), BASE + HOUR),
            ],
        );
        sync(&mut conn, BASE + 2 * HOUR);
        conn.execute(
            "UPDATE leagues SET user_state = 'partial', edited_at = ?2 WHERE run_key = ?1",
            params![format!("{LEAGUE}:q-x1"), BASE + 3 * HOUR],
        )
        .expect("an older log status");
        imported_run(&conn, 7, &seed(LEAGUE, "active", BASE - MINUTE));
        conn.execute(
            "UPDATE leagues SET user_state = 'complete', edited_at = ?1 WHERE run_key = '0x:7'",
            [BASE + 4 * HOUR],
        )
        .expect("a newer imported status");
        link(&conn, "q-x1", "0x:7");
        link(&conn, "q-x2", "0x:7");
        sync(&mut conn, BASE + 5 * HOUR);

        assert_eq!(
            status_of(&conn, "0x:7"),
            (None, Some("complete".to_string()), Some(BASE + 4 * HOUR))
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM leagues"), 1);
    }

    /// Review focus 2: a full refold (a fold version bump: the derivation
    /// record is gone, so `sync_all` folds every owner again) after a
    /// status, a removal from leagues, a move into a log run, a deleted
    /// league and a deleted match gives the same rows as before it.
    #[test]
    fn a_full_refold_keeps_every_player_edit() {
        let (_dir, mut conn) = db("league-sync-refold-player-edits");
        let mut views = three_league_matches();
        views.push(league_view(
            "q-y1",
            LEAGUE_2,
            Some("CMODERN"),
            BASE + 4 * HOUR,
        ));
        views.push(league_view(
            "q-y2",
            LEAGUE_2,
            Some("CMODERN"),
            BASE + 5 * HOUR,
        ));
        store_views(&mut conn, views);
        sync(&mut conn, BASE + 6 * HOUR);
        let first = format!("{LEAGUE}:q-m1");
        let second = format!("{LEAGUE_2}:q-y1");

        conn.execute(
            "UPDATE leagues SET user_state = 'partial', edited_at = ?2 WHERE run_key = ?1",
            params![first, BASE + 7 * HOUR],
        )
        .expect("a status");
        remove_from_leagues(&conn, "q-m2");
        link(&conn, "q-m3", &second);
        conn.execute(
            "UPDATE matches SET edited_at = ?2 WHERE token = ?1",
            params!["q-m3", BASE + 7 * HOUR],
        )
        .expect("a move");
        conn.execute(
            "UPDATE leagues SET removed_at = ?2, edited_at = ?2 WHERE run_key = ?1",
            params![second, BASE + 8 * HOUR],
        )
        .expect("a deleted league, matches kept");
        conn.execute(
            "UPDATE matches SET hidden_at = ?2, edited_at = ?2 WHERE token = ?1",
            params!["q-y2", BASE + 9 * HOUR],
        )
        .expect("a deleted match");
        sync(&mut conn, BASE + 10 * HOUR);
        assert_eq!(members(&conn, &first), vec!["q-m1"]);
        assert_eq!(members(&conn, &second), vec!["q-m3", "q-y1"]);
        let before = (synced(&conn), inputs(&conn));

        conn.execute("DELETE FROM derivations", [])
            .expect("a stale fold");
        sync(&mut conn, BASE + 11 * HOUR);
        assert_eq!((synced(&conn), inputs(&conn)), before);
    }

    // ---- Sync identity (sync identity and cursor spec, section 2.4) ------

    /// A winner row (token `q-winner`) and a loser log row (`q-loser`) of
    /// no owner, with the sync ids the triggers give them. An imported
    /// winner has 0.x id `imported_from` (positive for 0.x, negative for a
    /// pulled run). The loser carries a note when `kept` (so a hand-over
    /// detaches it instead of deleting it).
    fn merge_rows(
        conn: &Connection,
        winner_source: &str,
        imported_from: Option<i64>,
        kept: bool,
    ) -> (String, String) {
        let run_key = match imported_from {
            Some(from) => format!("0x:{from}"),
            None => "q:winner".to_string(),
        };
        conn.execute(
            "INSERT INTO leagues (run_key, source, kind, state, token, name, started_at,
                 created_at, imported_seed, imported_from)
             VALUES (?3, ?1, 'constructed', 'active', 'q-winner', 'Q', ?2, ?2,
                 CASE WHEN ?1 = 'imported' THEN '{}' END, ?4)",
            params![winner_source, BASE, run_key, imported_from],
        )
        .expect("the winner");
        conn.execute(
            "INSERT INTO leagues (run_key, source, kind, state, token, name, started_at,
                 created_at, notes)
             VALUES ('q:loser', 'log', 'constructed', 'active', 'q-loser', 'Q', ?1, ?1, ?2)",
            params![BASE + HOUR, kept.then_some("Q kept")],
        )
        .expect("the loser");
        (
            sync_id_of(conn, "q-winner").expect("the winner's id"),
            sync_id_of(conn, "q-loser").expect("the loser's id"),
        )
    }

    /// The sync id of the row with league token `token`.
    fn sync_id_of(conn: &Connection, token: &str) -> Option<String> {
        conn.query_row(
            "SELECT sync_client_id FROM leagues WHERE token = ?1",
            [token],
            |row| row.get(0),
        )
        .optional()
        .expect("lookup")
        .flatten()
    }

    fn uploaded(conn: &Connection, client_id: &str) {
        conn.execute(
            "INSERT INTO synced_rows (type, client_id, hash) VALUES ('league', ?1, 'h')",
            [client_id],
        )
        .expect("a synced row");
    }

    /// Merge the loser into the winner by `hand_over`, or by
    /// `carry_to_imported` when `carry`.
    fn merge(conn: &mut Connection, carry: bool) {
        let key_of = |token: &str| -> String {
            conn.query_row(
                "SELECT run_key FROM leagues WHERE token = ?1",
                [token],
                |row| row.get(0),
            )
            .expect("run key")
        };
        let (winner_key, loser_key) = (key_of("q-winner"), key_of("q-loser"));
        let tx = conn.transaction().expect("transaction");
        let mut rows = owner_rows(&tx, LeagueOwner::Nobody).expect("rows");
        let winner = rows
            .iter()
            .position(|row| row.run_key == winner_key)
            .expect("winner");
        let loser = rows
            .iter()
            .position(|row| row.run_key == loser_key)
            .expect("loser");
        if carry {
            let mut carried = HashMap::new();
            carry_to_imported(&tx, &mut rows, &mut carried, winner, loser).expect("carry");
        } else {
            hand_over(&tx, &mut rows, winner, loser, BASE).expect("hand over");
        }
        tx.commit().expect("commit");
    }

    #[test]
    fn a_hand_over_keeps_the_synced_id() {
        let (_dir, mut conn) = db("league-sync-hand-over-synced-id");
        let (_, loser_id) = merge_rows(&conn, "log", None, false);
        uploaded(&conn, &loser_id);
        merge(&mut conn, false);
        assert_eq!(sync_id_of(&conn, "q-winner"), Some(loser_id));
        assert_eq!(sync_id_of(&conn, "q-loser"), None, "deleted");
    }

    // League run identity (server spec 2026-10-23): when the winner takes
    // the loser's id, its own matches' bundles and reports named the old
    // one. Their touch keys move, so the bundles rebuild, and this device's
    // report is dropped, so the next enqueue reports the match again.
    #[test]
    fn a_taken_id_rebuilds_the_winners_match_bundles_and_reports() {
        let (_dir, mut conn) = db("league-sync-hand-over-requeues");
        let (winner_id, loser_id) = merge_rows(&conn, "log", None, false);
        uploaded(&conn, &loser_id);
        conn.execute_batch(
            "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);
             INSERT INTO matches (token, account_id, source, format, match_type, state,
                 outcome, started_at, last_activity_at, revision, derived_version, league_id)
             SELECT 'q-member', 1, 'log', 'CMODERN', 'Modern', 'complete', 'win', 10, 12, 1,
                 4, id FROM leagues WHERE run_key = 'q:winner';
             INSERT INTO outbox (kind, subject, payload, status, attempts, sent_at, created_at)
             VALUES ('match_report', 'q-member', X'', 'sent', 1, 13, 13);",
        )
        .expect("a reported member");
        let match_id = number(&conn, "SELECT id FROM matches WHERE token = 'q-member'");
        let before = crate::sync::match_touch_key(&conn, match_id).expect("key");

        merge(&mut conn, false);

        assert_eq!(sync_id_of(&conn, "q-winner"), Some(loser_id));
        assert_ne!(sync_id_of(&conn, "q-winner"), Some(winner_id));
        assert_ne!(
            crate::sync::match_touch_key(&conn, match_id).expect("key"),
            before,
            "the bundle rebuilds with the taken id"
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM outbox WHERE kind = 'match_report' AND subject = 'q-member'"
            ),
            0,
            "reported again"
        );
    }

    #[test]
    fn a_hand_over_keeps_the_winners_id_when_it_is_synced_or_the_losers_is_not() {
        for (winner_synced, loser_synced) in [(true, true), (true, false), (false, false)] {
            let (_dir, mut conn) = db("league-sync-hand-over-own-id");
            let (winner_id, loser_id) = merge_rows(&conn, "log", None, false);
            if winner_synced {
                uploaded(&conn, &winner_id);
            }
            if loser_synced {
                uploaded(&conn, &loser_id);
            }
            merge(&mut conn, false);
            assert_eq!(sync_id_of(&conn, "q-winner"), Some(winner_id));
        }
    }

    // A loser kept detached (it carries input) takes the winner's old id,
    // which has no server copy, so two rows never share one id.
    #[test]
    fn a_hand_over_to_a_kept_loser_swaps_the_ids() {
        let (_dir, mut conn) = db("league-sync-hand-over-kept-loser");
        let (winner_id, loser_id) = merge_rows(&conn, "log", None, true);
        uploaded(&conn, &loser_id);
        conn.execute(
            "UPDATE leagues SET notes = 'Q winner note' WHERE run_key = 'q:winner'",
            [],
        )
        .expect("the winner keeps its note, so the loser keeps its own");
        merge(&mut conn, false);
        assert!(
            number(
                &conn,
                "SELECT detached_at IS NOT NULL FROM leagues WHERE run_key = 'q:loser'"
            ) == 1,
            "kept detached"
        );
        assert_eq!(sync_id_of(&conn, "q-winner"), Some(loser_id));
        assert_eq!(sync_id_of(&conn, "q-loser"), Some(winner_id));
    }

    // Only an uploaded id counts: a row the sync only built (no hash) has
    // no server copy to keep.
    #[test]
    fn a_hand_over_takes_the_uploaded_id_over_one_only_built() {
        let (_dir, mut conn) = db("league-sync-hand-over-built-id");
        let (winner_id, loser_id) = merge_rows(&conn, "log", None, false);
        uploaded(&conn, &loser_id);
        conn.execute(
            "INSERT INTO synced_rows (type, client_id, built_hash) VALUES ('league', ?1, 'b')",
            [&winner_id],
        )
        .expect("built, never sent");
        merge(&mut conn, false);
        assert_eq!(sync_id_of(&conn, "q-winner"), Some(loser_id));
    }

    // 0.x parity: a 0.x imported row keeps the id 0.x uploaded it under
    // (its synced rows come in after the league sync), so it never takes a
    // log row's id.
    #[test]
    fn a_carry_into_a_0x_row_keeps_its_own_id() {
        let (_dir, mut conn) = db("league-sync-carry-0x-id");
        let (winner_id, loser_id) = merge_rows(&conn, "imported", Some(7), false);
        uploaded(&conn, &loser_id);
        merge(&mut conn, true);
        assert_eq!(sync_id_of(&conn, "q-winner"), Some(winner_id));
    }

    #[test]
    fn a_carry_to_imported_keeps_the_synced_id() {
        let (_dir, mut conn) = db("league-sync-carry-synced-id");
        let (_, loser_id) = merge_rows(&conn, "imported", Some(-7), false);
        uploaded(&conn, &loser_id);
        merge(&mut conn, true);
        assert_eq!(sync_id_of(&conn, "q-winner"), Some(loser_id));
        assert_eq!(sync_id_of(&conn, "q-loser"), None, "deleted");
    }

    #[test]
    fn a_carry_to_imported_keeps_the_winners_id_when_it_is_synced_or_the_losers_is_not() {
        for (winner_synced, loser_synced) in [(true, true), (true, false), (false, false)] {
            let (_dir, mut conn) = db("league-sync-carry-own-id");
            let (winner_id, loser_id) = merge_rows(&conn, "imported", Some(-7), false);
            if winner_synced {
                uploaded(&conn, &winner_id);
            }
            if loser_synced {
                uploaded(&conn, &loser_id);
            }
            merge(&mut conn, true);
            assert_eq!(sync_id_of(&conn, "q-winner"), Some(winner_id));
        }
    }
}
