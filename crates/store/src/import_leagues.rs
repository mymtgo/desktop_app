//! 0.x leagues imported into v1 (leagues spec sections 5 and 9, limited spec
//! section 7). Runs after the matches and the decks, so member tokens and
//! deck versions exist.
//!
//! - A 0.x manual constructed league becomes a v1 manual league (a `leagues`
//!   row with `source = 'manual'`, found again by `imported_from`), its
//!   matches becoming links (`matches.league_id` with `league_linked = 1`).
//! - Any other 0.x league becomes an imported run (`source = 'imported'`,
//!   run key `0x:{imported_from}`, `imported_from` = the 0.x id, or the next
//!   free id when another 0.x database took it): the league as 0.x
//!   left it is its seed (`imported_seed`, `league_sync::ImportedSeed`), and
//!   its matches are linked to it. A draft or sealed league keeps its kind,
//!   set code and course id: it seeds the limited runs, never the
//!   constructed ones. A new row takes the seed's values; from then on the
//!   league sync writes its folded columns.
//! - A member goes in under the 0.x match's token, or the live token when v1
//!   tracked that match live (`live_row_token`, the rule notes and deck links
//!   follow). A 0.x match v1 has no row for (still in progress in 0.x, say)
//!   is left out. A history import a member names stays one; "Delete all
//!   imported" keeps it while the league holds it.
//! - An imported run holds its live members too (0.x and v1 run side by
//!   side, so a run's later matches are often live in v1): the fold seeds the
//!   run with them, and they leave the plain fold inputs, so the run shows
//!   once with 0.x's record. A manual league leaves out a match already in a
//!   derived run (a live match with a league token). Both leave out a match
//!   already in another league, since a match is in at most one.
//! - Account: the members' account, else the deck's, else the only account
//!   when v1 has exactly one, else NULL (counted in `without_account`).
//! - Notes go into the row's `notes` (trimmed, at most 5000 characters), never
//!   over a note v1 already has.
//!
//! A 0.x run is found again by the 0.x id its seed keeps and its start
//! (`find_imported_run`; 0.x may heal a placeholder token). A run with its
//! natural key (0.x `LeagueClientId`: token and start) under another 0.x id
//! came from another 0.x database (issue #143): it is kept as it is (only a
//! missing note is filled), so importing a second database never
//! overwrites or loses the first one's leagues. A manual league is found by
//! its 0.x id and `manual_` token.
//!
//! A rerun updates an imported run (its seed, account and links) from 0.x,
//! never clearing an account v1 has. A
//! manual league is the user's in v1 once imported (they link and unlink its
//! matches), so a rerun takes 0.x's name and start time, fills an account or
//! deck v1 lacks, and leaves its links alone. Notes and drops made in v1 are
//! never touched.
//!
//! A match the player decided (league self-management spec 4.6: moved,
//! removed from leagues, deleted or restored, so `edited_at` is set, or out
//! of every league, `no_league`) is never linked or unlinked by an import,
//! and a rerun compares a run's links without it or a hidden match. An
//! import never writes a league's `removed_at`, `user_state` or
//! `edited_at`; a removed row is found again like any other.

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::StoreError;
use crate::import::live_row_token;
use crate::league_sync::ImportedSeed;
use crate::leagues::{
    MANUAL_MAX_MATCHES, MAX_NOTES_CHARS, ManualRow, imported_run_key, insert_manual_row,
    manual_version,
};

/// The deck version a 0.x league was played with, by the deck's MTGO id and
/// the version's signature (as `ImportedLink` names it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeagueDeck {
    pub deck_mtgo_id: String,
    pub signature: String,
}

/// One 0.x match of a league (`matches.league_id`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeagueMember {
    pub token: String,
    pub mtgo_match_id: Option<i64>,
}

/// A 0.x league's id with its natural key (0.x `LeagueClientId`): what the
/// Limited import finds the league's imported run by, since a second 0.x
/// database reuses the ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeagueKey {
    pub legacy_id: i64,
    pub token: String,
    pub started_at_ms: i64,
}

impl LeagueKey {
    pub fn of(league: &ImportedLeague) -> LeagueKey {
        LeagueKey {
            legacy_id: league.legacy_id,
            token: league.token.clone(),
            started_at_ms: league.started_at_ms,
        }
    }
}

/// The 0.x league kinds v1 imports (`leagues.kind`).
pub const LEAGUE_KINDS: [&str; 3] = ["constructed", "draft", "sealed"];

/// One 0.x league, not deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedLeague {
    /// The 0.x `leagues.id`.
    pub legacy_id: i64,
    /// Made by hand in 0.x (`leagues.manual`). Only a constructed league
    /// becomes a manual league; a limited one is always a run.
    pub manual: bool,
    /// `constructed`, `draft` or `sealed` (`LEAGUE_KINDS`).
    pub kind: String,
    /// 0.x `leagues.set_code` (`HOB`), limited only.
    pub set_code: Option<String>,
    /// 0.x `leagues.mtgo_course_id`, limited only.
    pub course_id: Option<i64>,
    pub token: String,
    pub event_id: Option<i64>,
    pub name: String,
    pub format: String,
    /// `active`, `complete`, `partial` or `dropped`.
    pub state: String,
    pub started_at_ms: i64,
    pub joined_at_ms: Option<i64>,
    pub completed_at_ms: Option<i64>,
    pub dropped_at_ms: Option<i64>,
    pub created_at_ms: Option<i64>,
    /// `None` when 0.x has no version, or it was not imported (Limited,
    /// unreadable).
    pub deck: Option<LeagueDeck>,
    pub notes: Option<String>,
    /// In 0.x start order.
    pub matches: Vec<LeagueMember>,
}

/// What one league import did, manual leagues and imported runs together.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LeagueImportCounts {
    pub added: u32,
    pub updated: u32,
    pub unchanged: u32,
    /// Stored with no account: none of the rules found one. Such a league is
    /// not listed under any account.
    pub without_account: u32,
}

/// Write the leagues, their members and their notes in one transaction.
pub fn import_leagues(
    conn: &mut Connection,
    leagues: &[ImportedLeague],
    now_ms: i64,
) -> Result<LeagueImportCounts, StoreError> {
    // Immediate: the write lock is taken (waiting out the busy timeout)
    // before the first read, so a commit by another connection between the
    // reads and the first write cannot fail it with SQLITE_BUSY_SNAPSHOT.
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let counts = import_league_rows(&tx, leagues, None, now_ms)?;
    tx.commit()?;
    Ok(counts)
}

/// `import_leagues` inside a transaction the caller owns. A pulled league
/// passes the client id its bundle came with (`pulled_client_id`), which a
/// new row stores as its sync client id (sync identity spec 2.2).
fn import_league_rows(
    tx: &Transaction,
    leagues: &[ImportedLeague],
    pulled_client_id: Option<&str>,
    now_ms: i64,
) -> Result<LeagueImportCounts, StoreError> {
    let only_account = only_account(tx)?;
    let mut counts = LeagueImportCounts::default();
    for league in leagues {
        let members = members(tx, &league.matches)?;
        let version = deck_version(tx, league.deck.as_ref())?;
        let account_id = members_account(&members)
            .or(version.as_ref().and_then(|version| version.account_id))
            .or(only_account);
        let (change, stored_account) = if league.manual && league.kind == "constructed" {
            import_manual(tx, league, &members, version.as_ref(), account_id, now_ms)?
        } else {
            import_run(tx, league, &members, version.as_ref(), account_id, now_ms)?
        };
        if let (Change::Added(id), Some(client_id)) = (&change, pulled_client_id) {
            store_pulled_client_id(tx, *id, client_id)?;
        }
        match change {
            Change::Added(_) => counts.added += 1,
            Change::Updated => counts.updated += 1,
            Change::Unchanged => counts.unchanged += 1,
        }
        if stored_account.is_none() {
            counts.without_account += 1;
        }
    }
    Ok(counts)
}

/// A league pulled from the cloud (07 spec section 10): imported like a
/// 0.x league under its synthetic 0.x id (`league.legacy_id`), unless this
/// device already has the run (its client id, or the same token and start)
/// from elsewhere: its own runs and 0.x's win. A pulled run keeps the
/// matches linked to it so far (pulled matches link themselves). Returns
/// whether it was imported.
pub(crate) fn import_pulled_league(
    tx: &Transaction,
    league: &ImportedLeague,
    client_id: &str,
    now_ms: i64,
) -> Result<bool, StoreError> {
    let elsewhere: bool = tx
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM leagues
                            WHERE (sync_client_id = ?4 OR (token = ?1 AND started_at = ?2))
                              AND imported_from IS NOT ?3)",
        )?
        .query_row(
            params![
                league.token,
                league.started_at_ms,
                league.legacy_id,
                client_id
            ],
            |row| row.get(0),
        )?;
    if elsewhere {
        return Ok(false);
    }
    let mut members: Vec<LeagueMember> = Vec::new();
    {
        let mut stmt = tx.prepare_cached(
            "SELECT m.token, m.mtgo_match_id FROM matches m
             JOIN leagues l ON l.id = m.league_id
             WHERE l.imported_from = ?1 AND m.league_linked = 1 ORDER BY m.started_at, m.id",
        )?;
        let rows = stmt.query_map([league.legacy_id], |row| {
            Ok(LeagueMember {
                token: row.get(0)?,
                mtgo_match_id: row.get(1)?,
            })
        })?;
        for row in rows {
            members.push(row?);
        }
    }
    let wanted = ImportedLeague {
        matches: members,
        ..league.clone()
    };
    import_league_rows(tx, std::slice::from_ref(&wanted), Some(client_id), now_ms)?;
    Ok(true)
}

/// A pulled league's new row stores the client id its bundle came with, as
/// given: the device that sent it may have rekeyed its run, so the id can
/// disagree with the formula for the pulled start (sync identity spec 2.2).
/// The insert trigger filled the formula a moment before, in the same
/// transaction; this is the row's only id from then on.
fn store_pulled_client_id(tx: &Transaction, id: i64, client_id: &str) -> Result<(), StoreError> {
    tx.prepare_cached("UPDATE leagues SET sync_client_id = ?2 WHERE id = ?1")?
        .execute(params![id, client_id])?;
    Ok(())
}

/// A pulled manual league this device holds as the server sent it, made the
/// server's copy again (a later pull, so another device's edit lands): its
/// name, notes and deck. A deck this device does not have keeps the row's
/// own; its members link themselves as their matches are pulled.
pub(crate) fn replace_manual_league(
    tx: &Transaction,
    league_id: i64,
    league: &ImportedLeague,
) -> Result<(), StoreError> {
    let notes: Option<String> = league
        .notes
        .as_deref()
        .map(str::trim)
        .filter(|notes| !notes.is_empty())
        .map(|notes| notes.chars().take(MAX_NOTES_CHARS).collect());
    tx.prepare_cached("UPDATE leagues SET name = ?2, notes = ?3 WHERE id = ?1")?
        .execute(params![league_id, league.name, notes])?;
    match (&league.deck, deck_version(tx, league.deck.as_ref())?) {
        (None, _) => {
            tx.prepare_cached(
                "UPDATE leagues SET deck_id = NULL, deck_version_id = NULL WHERE id = ?1",
            )?
            .execute([league_id])?;
        }
        (Some(_), Some(version)) => {
            tx.prepare_cached(
                "UPDATE leagues SET deck_id = ?2, deck_version_id = ?3, format = ?4 WHERE id = ?1",
            )?
            .execute(params![
                league_id,
                version.deck_id,
                version.id,
                version.format
            ])?;
        }
        (Some(_), None) => {}
    }
    Ok(())
}

/// What happened to one league.
enum Change {
    /// A new row, with its id.
    Added(i64),
    Updated,
    Unchanged,
}

/// A v1 match a 0.x league member became.
struct Member {
    token: String,
    account_id: i64,
    started_at: i64,
    /// A live match with a league token: it is in its derived run unless an
    /// imported run claims it, so a manual league leaves it out.
    in_derived_run: bool,
}

/// The v1 version a league's 0.x version became, with its deck.
struct DeckVersion {
    id: i64,
    deck_id: i64,
    account_id: Option<i64>,
    /// The deck's format code.
    format: String,
}

/// A log or manual match in a derived run, as `leagues::IN_A_LEAGUE` has it.
const IN_DERIVED_RUN: &str =
    "(source IN ('log', 'manual') AND trim(COALESCE(league_token, '')) <> ''
     AND tournament_event_id IS NULL AND state <> 'pending')";

/// The account when v1 has exactly one.
pub(crate) fn only_account(tx: &Transaction) -> Result<Option<i64>, StoreError> {
    let mut stmt = tx.prepare("SELECT id FROM accounts LIMIT 2")?;
    let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row?);
    }
    Ok(if ids.len() == 1 { Some(ids[0]) } else { None })
}

/// The v1 matches of a league's 0.x matches, each once: the live row with its
/// token or MTGO match ID, else the imported row with its token. A 0.x match
/// v1 has no row for is left out.
fn members(tx: &Transaction, matches: &[LeagueMember]) -> Result<Vec<Member>, StoreError> {
    let mut found: Vec<Member> = Vec::new();
    for member in matches {
        let token = match live_row_token(tx, &member.token, member.mtgo_match_id)? {
            Some(live) => live,
            None => member.token.clone(),
        };
        if found.iter().any(|known| known.token == token) {
            continue;
        }
        let row = tx
            .prepare_cached(&format!(
                "SELECT account_id, started_at, {IN_DERIVED_RUN} FROM matches WHERE token = ?1"
            ))?
            .query_row([&token], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .optional()?;
        if let Some((account_id, started_at, in_derived_run)) = row {
            found.push(Member {
                token,
                account_id,
                started_at,
                in_derived_run,
            });
        }
    }
    Ok(found)
}

/// The account of the league's first match.
fn members_account(members: &[Member]) -> Option<i64> {
    let mut first: Option<&Member> = None;
    for member in members {
        if first.is_none_or(|kept| member.started_at < kept.started_at) {
            first = Some(member);
        }
    }
    first.map(|member| member.account_id)
}

fn deck_version(
    tx: &Transaction,
    deck: Option<&LeagueDeck>,
) -> Result<Option<DeckVersion>, StoreError> {
    let Some(deck) = deck else {
        return Ok(None);
    };
    Ok(tx
        .prepare_cached(
            "SELECT v.id, v.deck_id, d.account_id, d.format FROM deck_versions v
             JOIN decks d ON d.id = v.deck_id
             WHERE d.mtgo_id = ?1 AND v.signature = ?2",
        )?
        .query_row(params![deck.deck_mtgo_id, deck.signature], |row| {
            Ok(DeckVersion {
                id: row.get(0)?,
                deck_id: row.get(1)?,
                account_id: row.get(2)?,
                format: row.get(3)?,
            })
        })
        .optional()?)
}

/// Whether the match is linked (by hand or by an import) to a league other
/// than row `league_id` (`None`: any league).
fn in_other_league(
    tx: &Transaction,
    token: &str,
    league_id: Option<i64>,
) -> Result<bool, StoreError> {
    Ok(tx
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM matches
                            WHERE token = ?1 AND league_linked = 1 AND league_id IS NOT ?2)",
        )?
        .query_row(params![token, league_id], |row| row.get(0))?)
}

/// What the player decided about one match (league self-management spec
/// 4.6).
struct PlayerMarks {
    /// Moved, removed from leagues, deleted or restored by the player
    /// (`edited_at`), or out of every league (`no_league`): no import links
    /// or unlinks it.
    kept: bool,
    /// Deleted ("Delete match", `hidden_at`): left out of a rerun's
    /// comparison.
    hidden: bool,
}

/// The player's marks on the stored match `token` (a member always has a
/// row: `members` leaves out the others).
fn player_marks(tx: &Transaction, token: &str) -> Result<PlayerMarks, StoreError> {
    Ok(tx
        .prepare_cached(
            "SELECT edited_at IS NOT NULL OR no_league = 1, hidden_at IS NOT NULL
             FROM matches WHERE token = ?1",
        )?
        .query_row([token], |row| {
            Ok(PlayerMarks {
                kept: row.get(0)?,
                hidden: row.get(1)?,
            })
        })?)
}

/// Where a 0.x league's imported run is: `(row id, imported_from, own)`.
/// Its own run is the one imported from this 0.x league: the seed's 0.x id
/// (`ImportedSeed::legacy_id`) and start, with the seed's token the same or
/// a 0.x placeholder (`draft-...`, which 0.x `HealPlaceholderLeague`
/// rewrites at the first match). Else a run with its natural key (0.x
/// `LeagueClientId`: token and start) that another 0.x database brought
/// (`own` false). A pulled run (negative `imported_from`) is only ever its
/// own league's, found by that id alone.
pub(crate) fn find_imported_run(
    tx: &Transaction,
    key: &LeagueKey,
) -> Result<Option<(i64, i64, bool)>, StoreError> {
    // A pulled run is its league's whatever its start: the device that
    // sent it may rekey its run, which moves the start and keeps the client
    // id its 0.x id comes from (sync identity spec 2.2).
    if key.legacy_id < 0 {
        let pulled = tx
            .prepare_cached(
                "SELECT id, imported_from FROM leagues
                 WHERE source = 'imported' AND imported_from = ?1",
            )?
            .query_row([key.legacy_id], |row| Ok((row.get(0)?, row.get(1)?, true)))
            .optional()?;
        if pulled.is_some() {
            return Ok(pulled);
        }
    }
    let own = tx
        .prepare_cached(
            "SELECT id, imported_from FROM leagues
             WHERE source = 'imported'
               AND json_extract(imported_seed, '$.started_at') = ?3
               AND (json_extract(imported_seed, '$.token') = ?2
                    OR json_extract(imported_seed, '$.token') LIKE 'draft-%')
               AND json_extract(imported_seed, '$.legacy_id') = ?1
             ORDER BY imported_from = ?1 DESC, id LIMIT 1",
        )?
        .query_row(
            params![key.legacy_id, key.token, key.started_at_ms],
            |row| Ok((row.get(0)?, row.get(1)?, true)),
        )
        .optional()?;
    if own.is_some() {
        return Ok(own);
    }
    Ok(tx
        .prepare_cached(
            "SELECT id, imported_from FROM leagues
             WHERE source = 'imported' AND imported_from > 0
               AND json_extract(imported_seed, '$.token') = ?2
               AND json_extract(imported_seed, '$.started_at') = ?3
             ORDER BY imported_from = ?1 DESC, id LIMIT 1",
        )?
        .query_row(
            params![key.legacy_id, key.token, key.started_at_ms],
            |row| Ok((row.get(0)?, row.get(1)?, false)),
        )
        .optional()?)
}

/// The `imported_from` a new league row takes: its 0.x id, unless a league
/// from another 0.x database has it (issue #143), then the next free id.
fn free_imported_from(tx: &Transaction, legacy_id: i64) -> Result<i64, StoreError> {
    let taken: bool = tx
        .prepare_cached("SELECT EXISTS (SELECT 1 FROM leagues WHERE imported_from = ?1)")?
        .query_row([legacy_id], |row| row.get(0))?;
    if !taken {
        return Ok(legacy_id);
    }
    let highest: i64 = tx
        .prepare_cached("SELECT MAX(imported_from) FROM leagues")?
        .query_row([], |row| row.get(0))?;
    Ok(highest.max(legacy_id) + 1)
}

/// An imported run as stored: its row, account and seed.
struct StoredRun {
    id: i64,
    account_id: Option<i64>,
    seed: ImportedSeed,
}

/// Where a 0.x league already is in v1.
enum FoundRun {
    /// Its own row: imported from this league (or pulled under its id).
    Own(Box<StoredRun>),
    /// A row with its natural key that another 0.x database brought: kept
    /// as it is.
    Other(i64),
    None,
}

/// Add or update the imported run and its links; returns the change and the
/// run's account.
fn import_run(
    tx: &Transaction,
    league: &ImportedLeague,
    members: &[Member],
    version: Option<&DeckVersion>,
    account_id: Option<i64>,
    now_ms: i64,
) -> Result<(Change, Option<i64>), StoreError> {
    let wanted = ImportedSeed {
        token: league.token.clone(),
        event_id: league.event_id,
        name: league.name.clone(),
        format: Some(league.format.clone()),
        state: league.state.clone(),
        started_at: league.started_at_ms,
        joined_at: league.joined_at_ms,
        completed_at: league.completed_at_ms,
        dropped_at: league.dropped_at_ms,
        deck_version_id: version.map(|version| version.id),
        kind: league.kind.clone(),
        set_code: league.set_code.clone(),
        mtgo_course_id: league.course_id,
        legacy_id: Some(league.legacy_id),
    };
    let stored = match stored_run(tx, league)? {
        FoundRun::Own(run) => Some(*run),
        FoundRun::Other(id) => {
            // Another 0.x database's copy of this run: it stays as it is.
            insert_note(tx, id, league.notes.as_deref())?;
            let account_id: Option<i64> = tx.query_row(
                "SELECT account_id FROM leagues WHERE id = ?1",
                [id],
                |row| row.get(0),
            )?;
            return Ok((Change::Unchanged, account_id));
        }
        FoundRun::None => None,
    };
    let stored_id = stored.as_ref().map(|run| run.id);
    // An account v1 gave the run (a manual match placed in it, issue #162)
    // stays when 0.x finds none.
    let account_id = account_id.or(stored.as_ref().and_then(|run| run.account_id));
    // The members the import links, and the ones a rerun compares with the
    // stored links (`linked_members`): neither has a match the player
    // decided, and the comparison has no hidden match either.
    let mut wanted_tokens: Vec<String> = Vec::new();
    let mut compared_tokens: Vec<String> = Vec::new();
    for member in members {
        if in_other_league(tx, &member.token, stored_id)? {
            continue;
        }
        let marks = player_marks(tx, &member.token)?;
        if marks.kept {
            continue;
        }
        wanted_tokens.push(member.token.clone());
        if !marks.hidden {
            compared_tokens.push(member.token.clone());
        }
    }
    compared_tokens.sort();

    let (id, change) = match stored {
        None => {
            let id = insert_run(tx, league, &wanted, version, account_id, now_ms)?;
            (id, Change::Added(id))
        }
        Some(stored) => {
            if stored.account_id == account_id
                && stored.seed == wanted
                && linked_members(tx, stored.id)? == compared_tokens
            {
                (stored.id, Change::Unchanged)
            } else {
                tx.execute(
                    "UPDATE leagues SET account_id = ?2, imported_seed = ?3 WHERE id = ?1",
                    params![stored.id, account_id, serde_json::to_string(&wanted)?],
                )?;
                (stored.id, Change::Updated)
            }
        }
    };
    if !matches!(change, Change::Unchanged) {
        // A match the player moved here (or deleted) keeps its link.
        tx.execute(
            "UPDATE matches SET league_id = NULL, league_linked = 0
             WHERE league_id = ?1 AND league_linked = 1 AND edited_at IS NULL
               AND no_league = 0",
            [id],
        )?;
        for token in &wanted_tokens {
            link_member(tx, token, id)?;
        }
    }
    insert_note(tx, id, league.notes.as_deref())?;
    Ok((change, account_id))
}

/// The stored run of this 0.x league (`find_imported_run`).
fn stored_run(tx: &Transaction, league: &ImportedLeague) -> Result<FoundRun, StoreError> {
    let Some((id, _, own)) = find_imported_run(tx, &LeagueKey::of(league))? else {
        return Ok(FoundRun::None);
    };
    if !own {
        return Ok(FoundRun::Other(id));
    }
    let (account_id, json): (Option<i64>, String) = tx.query_row(
        "SELECT account_id, imported_seed FROM leagues WHERE id = ?1",
        [id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(FoundRun::Own(Box::new(StoredRun {
        id,
        account_id,
        seed: serde_json::from_str(&json)?,
    })))
}

/// A new imported run row: the seed's values, which the league sync folds
/// from then on.
fn insert_run(
    tx: &Transaction,
    league: &ImportedLeague,
    seed: &ImportedSeed,
    version: Option<&DeckVersion>,
    account_id: Option<i64>,
    now_ms: i64,
) -> Result<i64, StoreError> {
    let format = Some(league.format.trim()).filter(|format| !format.is_empty());
    let imported_from = free_imported_from(tx, league.legacy_id)?;
    Ok(tx.query_row(
        "INSERT INTO leagues (run_key, account_id, source, kind, state, token, event_id,
             mtgo_course_id, name, format, set_code, deck_version_id, deck_id, started_at,
             joined_at, completed_at, dropped_at, created_at, imported_from, imported_seed)
         VALUES (?1, ?2, 'imported', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
             ?16, ?17, ?18, ?19)
         RETURNING id",
        params![
            imported_run_key(imported_from),
            account_id,
            league.kind,
            league.state,
            league.token,
            league.event_id,
            league.course_id,
            league.name,
            format,
            league.set_code,
            version.map(|version| version.id),
            version.map(|version| version.deck_id),
            league.started_at_ms,
            league.joined_at_ms,
            league.completed_at_ms,
            league.dropped_at_ms,
            league.created_at_ms.unwrap_or(now_ms),
            imported_from,
            serde_json::to_string(seed)?
        ],
        |row| row.get(0),
    )?)
}

/// The tokens of the matches linked to row `id` that a rerun compares,
/// sorted: not decided by the player (`edited_at`, `no_league`) and not
/// hidden.
fn linked_members(tx: &Transaction, id: i64) -> Result<Vec<String>, StoreError> {
    let mut stmt = tx.prepare_cached(
        "SELECT token FROM matches
         WHERE league_id = ?1 AND league_linked = 1 AND edited_at IS NULL AND no_league = 0
           AND hidden_at IS NULL
         ORDER BY token",
    )?;
    let rows = stmt.query_map([id], |row| row.get(0))?;
    let mut tokens = Vec::new();
    for row in rows {
        tokens.push(row?);
    }
    Ok(tokens)
}

/// The columns of a manual league the import compares and writes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ManualFields {
    account_id: Option<i64>,
    deck_id: Option<i64>,
    name: String,
    started_at: i64,
}

/// Add the manual league with its links, or bring a stored one up to date
/// (name and start from 0.x, account and deck only where v1 has none, links
/// untouched); returns the change and the league's account.
fn import_manual(
    tx: &Transaction,
    league: &ImportedLeague,
    members: &[Member],
    version: Option<&DeckVersion>,
    account_id: Option<i64>,
    now_ms: i64,
) -> Result<(Change, Option<i64>), StoreError> {
    let deck_id = version.map(|version| version.deck_id);
    let token = manual_token(&league.token);
    let stored = tx
        .query_row(
            "SELECT id, account_id, deck_id, name, started_at, created_at FROM leagues
             WHERE source = 'manual' AND imported_from = ?1 AND token = ?2",
            params![league.legacy_id, token],
            |row| {
                let id: i64 = row.get(0)?;
                let fields = ManualFields {
                    account_id: row.get(1)?,
                    deck_id: row.get(2)?,
                    name: row.get(3)?,
                    started_at: row.get(4)?,
                };
                Ok((id, fields, row.get::<_, i64>(5)?))
            },
        )
        .optional()?;
    if stored.is_none() {
        // Another 0.x database's copy of this manual league (its `manual_`
        // token under another 0.x id) stays as it is.
        let other: Option<(i64, Option<i64>)> = tx
            .prepare_cached(
                "SELECT id, account_id FROM leagues
                 WHERE source = 'manual' AND token = ?1 AND imported_from > 0
                 ORDER BY id LIMIT 1",
            )?
            .query_row([&token], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()?;
        if let Some((id, account)) = other {
            insert_note(tx, id, league.notes.as_deref())?;
            return Ok((Change::Unchanged, account));
        }
    }

    let (id, change, account) = match stored {
        Some((id, stored, created_at)) => {
            let merged = ManualFields {
                account_id: stored.account_id.or(account_id),
                deck_id: stored.deck_id.or(deck_id),
                name: league.name.clone(),
                started_at: league.started_at_ms,
            };
            let change = if merged == stored {
                Change::Unchanged
            } else {
                tx.execute(
                    "UPDATE leagues SET account_id = ?2, deck_id = ?3, name = ?4, started_at = ?5
                     WHERE id = ?1",
                    params![
                        id,
                        merged.account_id,
                        merged.deck_id,
                        merged.name,
                        merged.started_at
                    ],
                )?;
                // A deck found now brings its format and version, as when
                // the league is made.
                if stored.deck_id.is_none()
                    && let (Some(deck_id), Some(version)) = (merged.deck_id, version)
                {
                    tx.execute(
                        "UPDATE leagues SET format = ?2, deck_version_id = ?3 WHERE id = ?1",
                        params![id, version.format, manual_version(tx, deck_id, created_at)?],
                    )?;
                }
                Change::Updated
            };
            (id, change, merged.account_id)
        }
        None => {
            let id = insert_manual_row(
                tx,
                &ManualRow {
                    account_id,
                    deck_id,
                    format: version.map(|version| version.format.clone()),
                    name: league.name.clone(),
                    token,
                    started_at: league.started_at_ms,
                    created_at: league.created_at_ms.unwrap_or(now_ms),
                    imported_from: Some(free_imported_from(tx, league.legacy_id)?),
                },
            )?;
            let mut linked = 0;
            for member in members {
                if linked == MANUAL_MAX_MATCHES {
                    break;
                }
                if member.in_derived_run
                    || in_other_league(tx, &member.token, None)?
                    || player_marks(tx, &member.token)?.kept
                {
                    continue;
                }
                link_member(tx, &member.token, id)?;
                linked += 1;
            }
            (id, Change::Added(id), account_id)
        }
    };
    insert_note(tx, id, league.notes.as_deref())?;
    Ok((change, account))
}

/// Link the match to league row `id`, unless the player decided its league
/// (`edited_at`, `no_league`; league self-management spec 4.6). A history
/// import stays one: "Delete all imported" keeps it while a 0.x league holds
/// it (issue #180, `history_import::HELD_BY_0X_LEAGUE`).
fn link_member(tx: &Transaction, token: &str, id: i64) -> Result<(), StoreError> {
    tx.prepare_cached(
        "UPDATE matches SET league_id = ?2, league_linked = 1
         WHERE token = ?1 AND edited_at IS NULL AND no_league = 0",
    )?
    .execute(params![token, id])?;
    Ok(())
}

/// A 0.x manual league keeps its token (07 ruling 13), which 0.x made
/// `manual_` and 24 random characters; one without that start gets it.
fn manual_token(token: &str) -> String {
    if token.starts_with("manual_") {
        token.to_string()
    } else {
        format!("manual_{token}")
    }
}

/// Store a non-blank 0.x note on row `id`, unless v1 already has one (a note
/// written in v1 is never overwritten). Trimmed, as a v1 edit is, and cut to
/// `MAX_NOTES_CHARS`.
fn insert_note(tx: &Transaction, id: i64, notes: Option<&str>) -> Result<(), StoreError> {
    let Some(notes) = notes.map(str::trim).filter(|notes| !notes.is_empty()) else {
        return Ok(());
    };
    let notes: String = notes.chars().take(MAX_NOTES_CHARS).collect();
    tx.prepare_cached("UPDATE leagues SET notes = ?2 WHERE id = ?1 AND notes IS NULL")?
        .execute(params![id, notes])?;
    Ok(())
}
