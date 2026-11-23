//! Arena limited runs, stored (Arena limited runs spec section 4): reads an
//! Arena account's course observations and limited matches, folds them with
//! `leagues::fold_arena_runs` and writes the runs into `leagues` (run key and
//! token `arena:<course id>`), `matches.league_id` and the matches'
//! `registered` `limited_deck_snapshots` rows.
//!
//! Only the account's rows whose run key starts with `arena:` are written or
//! deleted, so an MTGO run or another account's row is never touched. MTGO's
//! `league_sync::sync_owner` leaves Arena accounts alone in turn.
//!
//! Across devices (cross-device sync spec section 8, lead ruling R5): the
//! fold also takes the course runs other devices pushed
//! (`synced_arena_runs`) and the pulled limited matches that name a course
//! run (`synced_sources.league_client_id`); each course run's record is the
//! newest snapshot of either. Each course run with a registered deck gets
//! its course deck (`limited:arena_<course id>`), whose slot gates the run
//! and its matches, and its matches are linked to that deck's versions.

use std::collections::{HashMap, HashSet};

use leagues::{
    ArenaRun, ArenaRunMatch, CourseRecord, CourseSnapshot, CourseState, MatchDeck, PulledCourseRun,
    PulledRunMatch, RunState, arena_course_id, fold_arena_runs_merged, join_records, same_pool,
};
use limited::RegisteredCard;
use rusqlite::{OptionalExtension, Transaction, params};
use sync_bundles::league::{ARENA_COURSE_DECK_PREFIX, ArenaRunBundle};

use crate::StoreError;
use crate::arena_courses::{CourseObservation, latest_pool, observations};

/// Sync every Arena account's runs.
pub fn sync_all_arena_runs(tx: &Transaction, now_ms: i64) -> Result<(), StoreError> {
    for account_id in arena_account_ids(tx)? {
        sync_arena_runs(tx, account_id, now_ms)?;
    }
    Ok(())
}

/// Fold one Arena account's courses and limited matches and store the runs.
/// Any other account is left alone.
pub fn sync_arena_runs(tx: &Transaction, account_id: i64, now_ms: i64) -> Result<(), StoreError> {
    if !is_arena_account(tx, account_id)? {
        return Ok(());
    }
    let snapshots = course_snapshots(&observations(tx, account_id)?);
    let matches = limited_matches(tx, account_id)?;
    let pulled_runs = pulled_course_runs(tx, account_id)?;
    let pulled_matches = pulled_run_matches(tx, account_id)?;
    let runs = fold_arena_runs_merged(&snapshots, &matches, &pulled_runs, &pulled_matches);

    let mut league_by_token: HashMap<String, i64> = HashMap::new();
    let mut kept_keys: HashSet<String> = HashSet::new();
    let mut decks: Vec<&MatchDeck> = Vec::new();
    let mut stored_runs: Vec<(i64, &ArenaRun)> = Vec::new();
    for run in &runs {
        let Some(league_id) = upsert_run(tx, account_id, run, now_ms)? else {
            continue;
        };
        kept_keys.insert(run.run_key.clone());
        for token in &run.match_tokens {
            league_by_token.insert(token.clone(), league_id);
        }
        decks.extend(run.decks.iter());
        stored_runs.push((league_id, run));
    }
    delete_gone_runs(tx, account_id, &kept_keys)?;
    write_match_leagues(tx, account_id, &league_by_token)?;

    let observed_at: HashMap<(i64, i64), i64> = snapshots
        .iter()
        .map(|snapshot| ((snapshot.file_id, snapshot.offset), snapshot.observed_at_ms))
        .collect();
    let started_at: HashMap<&str, i64> = matches
        .iter()
        .map(|played| (played.token.as_str(), played.started_at_ms))
        .collect();
    write_snapshots(tx, account_id, &decks, &started_at, &observed_at)?;
    for (league_id, run) in stored_runs {
        link_course_deck(tx, account_id, league_id, run, &observed_at, now_ms)?;
    }
    Ok(())
}

/// An Arena run as the `arena-seed` dev command prints it.
pub struct ArenaRunSummary {
    pub name: String,
    pub wins: i64,
    pub losses: i64,
    pub state: String,
}

/// Every `arena:` run with the wins and losses of its completed linked
/// matches, oldest first.
pub fn arena_run_summaries(
    conn: &rusqlite::Connection,
) -> Result<Vec<ArenaRunSummary>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT l.name,
                COALESCE(SUM(m.outcome = 'win'), 0),
                COALESCE(SUM(m.outcome = 'loss'), 0),
                l.state
         FROM leagues l
         LEFT JOIN matches m ON m.league_id = l.id AND m.state = 'complete'
         WHERE l.run_key LIKE 'arena:%'
         GROUP BY l.id
         ORDER BY l.started_at, l.id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ArenaRunSummary {
            name: row.get(0)?,
            wins: row.get(1)?,
            losses: row.get(2)?,
            state: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// `sync_arena_runs` in a savepoint, for the tracker's batch: a run sync
/// that fails (data that does not read back, a row a CHECK refuses) is
/// rolled back, logged and skipped, so it never fails the batch's matches
/// and stops tracking. A failure of the database itself (busy, full disk,
/// IO) still fails the caller, as the Arena game sync's does.
pub(crate) fn sync_arena_runs_guarded(
    tx: &Transaction,
    account_id: i64,
    now_ms: i64,
) -> Result<(), StoreError> {
    tx.execute_batch("SAVEPOINT arena_run_sync")?;
    match sync_arena_runs(tx, account_id, now_ms) {
        Ok(()) => {
            tx.execute_batch("RELEASE arena_run_sync")?;
            Ok(())
        }
        Err(StoreError::Sqlite(error)) if is_database_failure(&error) => {
            Err(StoreError::Sqlite(error))
        }
        Err(error) => {
            tx.execute_batch("ROLLBACK TO arena_run_sync; RELEASE arena_run_sync")?;
            tracing::warn!(account_id, %error, "an Arena account's limited runs could not be synced; skipped");
            Ok(())
        }
    }
}

/// `sync_arena_runs_guarded` for every Arena account.
pub(crate) fn sync_all_arena_runs_guarded(tx: &Transaction, now_ms: i64) -> Result<(), StoreError> {
    for account_id in arena_account_ids(tx)? {
        sync_arena_runs_guarded(tx, account_id, now_ms)?;
    }
    Ok(())
}

fn is_database_failure(error: &rusqlite::Error) -> bool {
    use rusqlite::ErrorCode;
    matches!(
        error.sqlite_error_code(),
        Some(
            ErrorCode::DatabaseBusy
                | ErrorCode::DatabaseLocked
                | ErrorCode::DiskFull
                | ErrorCode::SystemIoFailure
                | ErrorCode::OutOfMemory
                | ErrorCode::ReadOnly
        )
    )
}

fn arena_account_ids(tx: &Transaction) -> Result<Vec<i64>, StoreError> {
    let mut stmt =
        tx.prepare_cached("SELECT id FROM accounts WHERE platform = 'arena' ORDER BY id")?;
    let rows = stmt.query_map([], |row| row.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn is_arena_account(tx: &Transaction, account_id: i64) -> Result<bool, StoreError> {
    Ok(tx
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM accounts WHERE id = ?1 AND platform = 'arena')",
        )?
        .query_row([account_id], |row| row.get(0))?)
}

/// The observations grouped into snapshots by log place. A snapshot whose
/// only row is the marker (course id '') has no course.
fn course_snapshots(rows: &[CourseObservation]) -> Vec<CourseSnapshot> {
    let mut snapshots: Vec<CourseSnapshot> = Vec::new();
    for row in rows {
        let same_place = snapshots
            .last()
            .is_some_and(|last| last.file_id == row.file_id && last.offset == row.byte_offset);
        if !same_place {
            snapshots.push(CourseSnapshot {
                file_id: row.file_id,
                offset: row.byte_offset,
                observed_at_ms: row.observed_at_ms,
                courses: Vec::new(),
            });
        }
        if row.course_id.is_empty() {
            continue;
        }
        if let Some(snapshot) = snapshots.last_mut() {
            snapshot.courses.push(CourseState {
                course_id: row.course_id.clone(),
                event_name: row.event_name.clone(),
                module: row.module.clone(),
                wins: row.wins,
                losses: row.losses,
                main: row.main.clone(),
                side: row.side.clone(),
                pool: row.pool.clone(),
            });
        }
    }
    snapshots
}

/// The account's listed limited matches that the player kept in leagues,
/// each at the log place of its first stored Arena event. A match with no
/// stored event has no place in the log and is left out; one with no stored
/// event name goes under its format code.
fn limited_matches(tx: &Transaction, account_id: i64) -> Result<Vec<ArenaRunMatch>, StoreError> {
    let mut stmt = tx.prepare_cached(
        "SELECT m.token, e.event_name, COALESCE(m.format, ''), m.started_at, m.ended_at,
             (SELECT arena_log_file_id || ':' || byte_offset FROM arena_log_events
              WHERE match_token = m.token
              ORDER BY arena_log_file_id, byte_offset, seq LIMIT 1)
         FROM listed_matches m LEFT JOIN arena_match_events e ON e.match_token = m.token
         WHERE m.account_id = ?1 AND m.match_type = 'limited' AND m.no_league = 0
         ORDER BY m.id",
    )?;
    let rows = stmt.query_map([account_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })?;
    let mut cards = first_game_cards_of(tx, account_id, None)?;
    let mut matches = Vec::new();
    for row in rows {
        let (token, event_name, format_code, started_at_ms, ended_at_ms, place) = row?;
        let Some((file_id, offset)) = place.as_deref().and_then(parse_place) else {
            continue;
        };
        matches.push(ArenaRunMatch {
            event_name: event_name.unwrap_or_else(|| format_code.clone()),
            format_code,
            file_id,
            offset,
            started_at_ms,
            ended_at_ms,
            cards: cards.remove(&token).unwrap_or_default(),
            token,
        });
    }
    Ok(matches)
}

/// The local deck and sideboard cards (catalog ids) of game 1 of each of
/// the account's limited matches that has game rows, by match token; only
/// `token`'s when given. In limited that is the pool, which tells two entries
/// of one event apart when no course snapshot covers their matches. Basic
/// lands are left out: every list has them, so they only blur the
/// comparison (a basic whose card data is unknown stays).
fn first_game_cards_of(
    tx: &Transaction,
    account_id: i64,
    token: Option<&str>,
) -> Result<HashMap<String, Vec<u32>>, StoreError> {
    let mut stmt = tx.prepare_cached(
        "SELECT m.token, c.catalog_id
         FROM listed_matches m
         JOIN games g ON g.match_id = m.id AND g.game_number = 1
         JOIN game_cards c ON c.game_id = g.id AND c.side = 'local'
         LEFT JOIN cards k ON k.catalog_id = c.catalog_id
         WHERE m.account_id = ?1 AND m.match_type = 'limited'
             AND (?2 IS NULL OR m.token = ?2)
             AND (c.main_qty > 0 OR c.side_qty > 0)
             AND COALESCE(k.type, '') NOT LIKE 'Basic%Land%'",
    )?;
    let rows = stmt.query_map(params![account_id, token], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut cards: HashMap<String, Vec<u32>> = HashMap::new();
    for row in rows {
        let (token, catalog_id) = row?;
        if let Ok(catalog_id) = u32::try_from(catalog_id) {
            cards.entry(token).or_default().push(catalog_id);
        }
    }
    Ok(cards)
}

/// The account whose runs a limited match's new game rows can move, or
/// `None` when they cannot: the match end only folds again when the match
/// sits in no run, in a course-less run (its pool may start a new one), or
/// in a course whose stated pool its game 1 cards do not fit. Any other
/// match's place is already decided by its course, so the whole account is
/// not read again after every match.
pub(crate) fn account_to_fold_at_match_end(
    tx: &Transaction,
    token: &str,
) -> Result<Option<i64>, StoreError> {
    let row: Option<(i64, Option<String>)> = tx
        .prepare_cached(
            "SELECT m.account_id, l.run_key FROM listed_matches m
             LEFT JOIN listed_leagues l ON l.id = m.league_id
             WHERE m.token = ?1 AND m.match_type = 'limited'",
        )?
        .query_row([token], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let Some((account_id, run_key)) = row else {
        return Ok(None);
    };
    let Some(course_id) = run_key.as_deref().and_then(arena_course_id) else {
        return Ok(Some(account_id));
    };
    let Some(pool) = latest_pool(tx, course_id)? else {
        return Ok(None);
    };
    let cards = first_game_cards_of(tx, account_id, Some(token))?
        .remove(token)
        .unwrap_or_default();
    let fits = same_pool(&pool, &cards) != Some(false);
    Ok((!fits).then_some(account_id))
}

/// `file:offset` as two numbers.
fn parse_place(place: &str) -> Option<(i64, i64)> {
    let (file_id, offset) = place.split_once(':')?;
    Some((file_id.parse().ok()?, offset.parse().ok()?))
}

/// A run's sync client id: `arena_<course id>` for a course run, the same
/// on every device (cross-device sync spec section 8). An orphan run has
/// none: it is never synced.
fn run_client_id(run_key: &str) -> Option<String> {
    arena_course_id(run_key).map(|course_id| format!("arena_{course_id}"))
}

/// Insert or update the run's row by its run key and return its id. The
/// player's columns are never written. A key held by another account's row
/// is left alone (`None`).
fn upsert_run(
    tx: &Transaction,
    account_id: i64,
    run: &ArenaRun,
    now_ms: i64,
) -> Result<Option<i64>, StoreError> {
    let existing: Option<(i64, Option<i64>)> = tx
        .prepare_cached("SELECT id, account_id FROM leagues WHERE run_key = ?1")?
        .query_row([&run.run_key], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let kind = run.kind.as_str();
    let state = crate::league_sync::state_str(run.state);
    let record = run.record.as_ref();
    let wins = record.map(|record| record.wins);
    let losses = record.map(|record| record.losses);
    let observed_at = record.map(|record| record.observed_at_ms);
    match existing {
        Some((id, owner)) if owner == Some(account_id) => {
            tx.prepare_cached(
                "UPDATE leagues SET kind = ?2, state = ?3, name = ?4, format = ?5,
                     set_code = ?6, started_at = ?7, completed_at = ?8, course_wins = ?9,
                     course_losses = ?10, course_observed_at = ?11
                 WHERE id = ?1",
            )?
            .execute(params![
                id,
                kind,
                state,
                run.name,
                run.format_code,
                run.set_code,
                run.started_at_ms,
                run.completed_at_ms,
                wins,
                losses,
                observed_at
            ])?;
            Ok(Some(id))
        }
        Some(_) => {
            tracing::warn!(run_key = %run.run_key, "an Arena run's key belongs to another account");
            Ok(None)
        }
        None => {
            let id = tx
                .prepare_cached(
                    "INSERT INTO leagues (run_key, account_id, source, kind, state, token, name,
                         format, set_code, started_at, completed_at, created_at, sync_client_id,
                         course_wins, course_losses, course_observed_at)
                     VALUES (?1, ?2, 'log', ?3, ?4, ?1, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                         ?14)
                     RETURNING id",
                )?
                .query_row(
                    params![
                        run.run_key,
                        account_id,
                        kind,
                        state,
                        run.name,
                        run.format_code,
                        run.set_code,
                        run.started_at_ms,
                        run.completed_at_ms,
                        now_ms,
                        run_client_id(&run.run_key),
                        wins,
                        losses,
                        observed_at
                    ],
                    |row| row.get(0),
                )?;
            Ok(Some(id))
        }
    }
}

/// Delete the account's `arena:` rows the fold no longer makes (an orphan
/// that joined its course), their matches' links cleared first.
fn delete_gone_runs(
    tx: &Transaction,
    account_id: i64,
    kept_keys: &HashSet<String>,
) -> Result<(), StoreError> {
    let rows: Vec<(i64, String)> = {
        let mut stmt = tx.prepare_cached(
            "SELECT id, run_key FROM leagues
             WHERE account_id = ?1 AND run_key LIKE 'arena:%' ORDER BY id",
        )?;
        let rows = stmt.query_map([account_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    for (id, run_key) in rows {
        if kept_keys.contains(&run_key) {
            continue;
        }
        tx.prepare_cached(
            "UPDATE matches SET league_id = NULL, league_linked = 0 WHERE league_id = ?1",
        )?
        .execute([id])?;
        tx.prepare_cached("DELETE FROM leagues WHERE id = ?1")?
            .execute([id])?;
    }
    Ok(())
}

/// `matches.league_id` of the account's limited matches and of any match in
/// one of the account's `arena:` rows: the row of the run holding the
/// match, else none. A match linked by hand, or in a row of another kind,
/// keeps its link.
fn write_match_leagues(
    tx: &Transaction,
    account_id: i64,
    league_by_token: &HashMap<String, i64>,
) -> Result<(), StoreError> {
    let changes: Vec<(i64, Option<i64>)> = {
        let mut stmt = tx.prepare_cached(
            "SELECT id, token, league_id FROM matches
             WHERE account_id = ?1 AND league_linked = 0
               AND ((league_id IS NULL AND match_type = 'limited')
                    OR league_id IN (SELECT id FROM leagues
                                     WHERE account_id = ?1 AND run_key LIKE 'arena:%'))",
        )?;
        let rows = stmt.query_map([account_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })?;
        let mut changes = Vec::new();
        for row in rows {
            let (id, token, league_id) = row?;
            let wanted = league_by_token.get(&token).copied();
            if wanted != league_id {
                changes.push((id, wanted));
            }
        }
        changes
    };
    for (id, league_id) in changes {
        tx.prepare_cached("UPDATE matches SET league_id = ?1 WHERE id = ?2")?
            .execute(params![league_id, id])?;
    }
    Ok(())
}

/// Each run match's `registered` snapshot: its course deck, rewritten only
/// when the observation it came from changed. Any other of the account's
/// matches (no run holds it, or its run has no deck for it) has none.
fn write_snapshots(
    tx: &Transaction,
    account_id: i64,
    decks: &[&MatchDeck],
    started_at: &HashMap<&str, i64>,
    observed_at: &HashMap<(i64, i64), i64>,
) -> Result<(), StoreError> {
    let deck_by_token: HashMap<&str, &MatchDeck> = decks
        .iter()
        .map(|deck| (deck.token.as_str(), *deck))
        .collect();
    let stored: HashMap<String, (i64, i64, i64)> = {
        let mut stmt = tx.prepare_cached(
            "SELECT m.token, m.id, s.event_file_id, s.event_offset
             FROM limited_deck_snapshots s JOIN matches m ON m.id = s.match_id
             WHERE m.account_id = ?1 AND s.source = 'registered'",
        )?;
        let rows = stmt.query_map([account_id], |row| {
            Ok((row.get(0)?, (row.get(1)?, row.get(2)?, row.get(3)?)))
        })?;
        rows.collect::<Result<_, _>>()?
    };
    for (token, (match_id, _, _)) in &stored {
        if !deck_by_token.contains_key(token.as_str()) {
            tx.prepare_cached(
                "DELETE FROM limited_deck_snapshots WHERE match_id = ?1 AND source = 'registered'",
            )?
            .execute([match_id])?;
        }
    }
    for deck in decks {
        if let Some((_, file_id, offset)) = stored.get(&deck.token)
            && (*file_id, *offset) == (deck.file_id, deck.offset)
        {
            continue;
        }
        let match_id: i64 = tx
            .prepare_cached("SELECT id FROM matches WHERE token = ?1")?
            .query_row([&deck.token], |row| row.get(0))?;
        let captured_at = observed_at
            .get(&(deck.file_id, deck.offset))
            .or_else(|| started_at.get(deck.token.as_str()))
            .copied()
            .unwrap_or_default();
        let registered = crate::limited::registered(&deck.token, captured_at, deck_cards(deck));
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
            crate::limited::cards_json(&registered.cards)?,
            registered.signature,
            registered.captured_at_ms,
            deck.file_id,
            deck.offset
        ])?;
        let ids: Vec<u32> = registered
            .cards
            .iter()
            .map(|card| card.catalog_id)
            .collect();
        crate::cards::ensure_id_stubs(tx, &ids)?;
    }
    Ok(())
}

/// The course runs other devices pushed for the account.
fn pulled_course_runs(
    tx: &Transaction,
    account_id: i64,
) -> Result<Vec<PulledCourseRun>, StoreError> {
    let mut stmt = tx.prepare_cached(
        "SELECT course_id, event_name, wins, losses, state, observed_at
         FROM synced_arena_runs WHERE account_id = ?1 ORDER BY course_id",
    )?;
    let rows = stmt.query_map([account_id], |row| {
        let state: String = row.get(4)?;
        Ok(PulledCourseRun {
            course_id: row.get(0)?,
            event_name: row.get(1)?,
            record: CourseRecord {
                wins: row.get(2)?,
                losses: row.get(3)?,
                state: snapshot_state(&state),
                observed_at_ms: row.get(5)?,
            },
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// A snapshot's stored or bundled state: `complete`, else active.
fn snapshot_state(state: &str) -> RunState {
    if state == "complete" {
        RunState::Complete
    } else {
        RunState::Active
    }
}

/// The account's listed limited matches pulled from another device whose
/// bundle named a course run (`arena_<course id>`), the player keeping them
/// in leagues.
fn pulled_run_matches(
    tx: &Transaction,
    account_id: i64,
) -> Result<Vec<PulledRunMatch>, StoreError> {
    let mut stmt = tx.prepare_cached(
        "SELECT m.token, s.league_client_id, m.started_at, m.ended_at
         FROM listed_matches m JOIN synced_sources s ON s.match_id = m.id
         WHERE m.account_id = ?1 AND m.match_type = 'limited' AND m.no_league = 0
           AND s.league_client_id IS NOT NULL
         ORDER BY m.id",
    )?;
    let rows = stmt.query_map([account_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, Option<i64>>(3)?,
        ))
    })?;
    let mut matches = Vec::new();
    for row in rows {
        let (token, client_id, started_at_ms, ended_at_ms) = row?;
        let Some(course_id) = client_id.strip_prefix("arena_") else {
            continue;
        };
        if course_id.is_empty() {
            continue;
        }
        matches.push(PulledRunMatch {
            token,
            course_id: course_id.to_string(),
            started_at_ms,
            ended_at_ms,
        });
    }
    Ok(matches)
}

/// Keep a pulled course run for the fold: its row in `synced_arena_runs`,
/// joined with the one held (`join_records`: the newer counts, complete when
/// either is), so a device never goes back to an older record. Returns
/// whether the stored row changed.
pub(crate) fn store_pulled_run(
    tx: &Transaction,
    account_id: i64,
    run: &ArenaRunBundle,
) -> Result<bool, StoreError> {
    let pulled = CourseRecord {
        wins: run.snapshot.wins,
        losses: run.snapshot.losses,
        state: snapshot_state(&run.snapshot.state),
        observed_at_ms: run.snapshot.observed_at_ms,
    };
    keep_pulled_record(
        tx,
        account_id,
        &run.course_id,
        &run.event_name,
        &run.kind,
        &pulled,
    )
}

/// `store_pulled_run` for one record of a course.
fn keep_pulled_record(
    tx: &Transaction,
    account_id: i64,
    course_id: &str,
    event_name: &str,
    kind: &str,
    pulled: &CourseRecord,
) -> Result<bool, StoreError> {
    let stored = stored_pulled_record(tx, account_id, course_id)?;
    let joined = match &stored {
        Some(stored) => join_records(stored, pulled),
        None => pulled.clone(),
    };
    if stored.as_ref() == Some(&joined) {
        return Ok(false);
    }
    let state = if joined.state == RunState::Complete {
        "complete"
    } else {
        "active"
    };
    tx.prepare_cached(
        "INSERT INTO synced_arena_runs (account_id, course_id, event_name, kind, wins, losses,
             state, observed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (account_id, course_id) DO UPDATE SET event_name = excluded.event_name,
             kind = excluded.kind, wins = excluded.wins, losses = excluded.losses,
             state = excluded.state, observed_at = excluded.observed_at",
    )?
    .execute(params![
        account_id,
        course_id,
        event_name,
        kind,
        joined.wins,
        joined.losses,
        state,
        joined.observed_at_ms
    ])?;
    Ok(true)
}

fn stored_pulled_record(
    tx: &Transaction,
    account_id: i64,
    course_id: &str,
) -> Result<Option<CourseRecord>, StoreError> {
    Ok(tx
        .prepare_cached(
            "SELECT wins, losses, state, observed_at FROM synced_arena_runs
             WHERE account_id = ?1 AND course_id = ?2",
        )?
        .query_row(params![account_id, course_id], |row| {
            let state: String = row.get(2)?;
            Ok(CourseRecord {
                wins: row.get(0)?,
                losses: row.get(1)?,
                state: snapshot_state(&state),
                observed_at_ms: row.get(3)?,
            })
        })
        .optional()?)
}

/// A pulled match names the course run `course_id` for `account_id`, the
/// account that played it: a pulled record of that course held under another
/// Arena account (filed under the current account when the run came first)
/// moves to it, and so does that account's row of the run, whole (its id,
/// the player's notes and status with their times, its links; #828), before
/// the other account's runs are folded again. `run_key` is unique, so the
/// account cannot hold a row of the run already. Rare; the caller's fold
/// after the pull brings the row up to date here.
pub(crate) fn move_pulled_run_to(
    tx: &Transaction,
    account_id: i64,
    course_id: &str,
    now_ms: i64,
) -> Result<(), StoreError> {
    let others: Vec<(i64, String, String)> = {
        let mut stmt = tx.prepare_cached(
            "SELECT account_id, event_name, kind FROM synced_arena_runs
             WHERE course_id = ?1 AND account_id <> ?2 ORDER BY account_id",
        )?;
        let rows = stmt.query_map(params![course_id, account_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        rows.collect::<Result<_, _>>()?
    };
    let run_key = format!("arena:{course_id}");
    for (other, event_name, kind) in others {
        if let Some(record) = stored_pulled_record(tx, other, course_id)? {
            keep_pulled_record(tx, account_id, course_id, &event_name, &kind, &record)?;
        }
        tx.prepare_cached(
            "DELETE FROM synced_arena_runs WHERE account_id = ?1 AND course_id = ?2",
        )?
        .execute(params![other, course_id])?;
        tx.prepare_cached(
            "UPDATE leagues SET account_id = ?1 WHERE account_id = ?2 AND run_key = ?3",
        )?
        .execute(params![account_id, other, run_key])?;
        sync_arena_runs_guarded(tx, other, now_ms)?;
    }
    Ok(())
}

/// Account `from` is merged into `to` (#538): each pulled record of a course
/// both hold is joined into `to`'s (`join_records`, as `keep_pulled_record`
/// does for a pull) and `from`'s row deleted, so the move that follows never
/// meets the `(account_id, course_id)` key.
pub(crate) fn merge_pulled_runs(tx: &Transaction, from: i64, to: i64) -> Result<(), StoreError> {
    let shared: Vec<(String, String, String)> = {
        let mut stmt = tx.prepare_cached(
            "SELECT f.course_id, f.event_name, f.kind FROM synced_arena_runs f
             JOIN synced_arena_runs t ON t.account_id = ?2 AND t.course_id = f.course_id
             WHERE f.account_id = ?1 ORDER BY f.course_id",
        )?;
        let rows = stmt.query_map(params![from, to], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        rows.collect::<Result<_, _>>()?
    };
    for (course_id, event_name, kind) in shared {
        if let Some(record) = stored_pulled_record(tx, from, &course_id)? {
            keep_pulled_record(tx, to, &course_id, &event_name, &kind, &record)?;
        }
        tx.prepare_cached(
            "DELETE FROM synced_arena_runs WHERE account_id = ?1 AND course_id = ?2",
        )?
        .execute(params![from, course_id])?;
    }
    Ok(())
}

/// The Arena account a pulled course run belongs to: the account whose
/// pulled matches name it, else the one whose run row holds its client id,
/// else the current Arena account. `None` when there is none: the run waits
/// for the next pull.
pub(crate) fn account_for_pulled_run(
    tx: &rusqlite::Connection,
    client_id: &str,
) -> Result<Option<i64>, StoreError> {
    let named: Option<i64> = tx
        .prepare_cached(
            "SELECT m.account_id FROM synced_sources s
             JOIN matches m ON m.id = s.match_id
             JOIN accounts a ON a.id = m.account_id AND a.platform = 'arena'
             WHERE s.league_client_id = ?1
             ORDER BY m.id LIMIT 1",
        )?
        .query_row([client_id], |row| row.get(0))
        .optional()?;
    if named.is_some() {
        return Ok(named);
    }
    let held: Option<i64> = tx
        .prepare_cached(
            "SELECT l.account_id FROM leagues l
             JOIN accounts a ON a.id = l.account_id AND a.platform = 'arena'
             WHERE l.sync_client_id = ?1 AND l.run_key LIKE 'arena:%'
             ORDER BY l.id LIMIT 1",
        )?
        .query_row([client_id], |row| row.get(0))
        .optional()?;
    if held.is_some() {
        return Ok(held);
    }
    crate::accounts::current_account_id(tx, crate::Platform::Arena)
}

/// The course deck of a course run (lead ruling R5): `limited:arena_<course
/// id>` under the account, made from the run's registered decks (a version
/// per registered deck, the newest the run's), or the one pulled from
/// another device. The run's row names the deck and its newest version, and
/// each run match without a deck plays the version of its registered deck
/// (else the newest); a match already on a version of this deck moves with
/// its registered deck. A course run with neither, an orphan, or a deck key
/// held by another account is left as it is.
fn link_course_deck(
    tx: &Transaction,
    account_id: i64,
    league_id: i64,
    run: &ArenaRun,
    observed_at: &HashMap<(i64, i64), i64>,
    now_ms: i64,
) -> Result<(), StoreError> {
    let Some(course_id) = &run.course_id else {
        return Ok(());
    };
    let mtgo_id = format!("{ARENA_COURSE_DECK_PREFIX}{course_id}");
    let existing: Option<(i64, Option<i64>)> = tx
        .prepare_cached("SELECT id, account_id FROM decks WHERE mtgo_id = ?1")?
        .query_row([&mtgo_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let deck_id = match existing {
        Some((_, Some(owner))) if owner != account_id => return Ok(()),
        Some((deck_id, owner)) => {
            if owner.is_none() {
                tx.prepare_cached("UPDATE decks SET account_id = ?2 WHERE id = ?1")?
                    .execute(params![deck_id, account_id])?;
            }
            deck_id
        }
        None if run.decks.is_empty() => return Ok(()),
        None => tx
            .prepare_cached(
                "INSERT INTO decks (mtgo_id, account_id, name, format, created_at, updated_at,
                     source)
                 VALUES (?1, ?2, ?3, 'Limited', ?4, ?4, 'file')
                 RETURNING id",
            )?
            .query_row(params![mtgo_id, account_id, run.name, now_ms], |row| {
                row.get(0)
            })?,
    };

    let mut version_by_token: HashMap<&str, i64> = HashMap::new();
    let mut newest: Option<i64> = None;
    for deck in &run.decks {
        let captured_at = observed_at
            .get(&(deck.file_id, deck.offset))
            .copied()
            .unwrap_or_default();
        let signature =
            crate::limited::registered(&deck.token, captured_at, deck_cards(deck)).signature;
        let version_id: i64 = tx
            .prepare_cached(
                "INSERT INTO deck_versions (deck_id, signature, modified_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT (deck_id, signature) DO UPDATE
                     SET modified_at = MIN(modified_at, excluded.modified_at)
                 RETURNING id",
            )?
            .query_row(params![deck_id, signature, captured_at], |row| row.get(0))?;
        version_by_token.insert(deck.token.as_str(), version_id);
        newest = Some(version_id);
    }
    if newest.is_none() {
        newest = tx
            .prepare_cached(
                "SELECT id FROM deck_versions WHERE deck_id = ?1
                 ORDER BY modified_at DESC, id DESC LIMIT 1",
            )?
            .query_row([deck_id], |row| row.get(0))
            .optional()?;
    }
    tx.prepare_cached(
        "UPDATE leagues SET deck_id = ?2, deck_version_id = ?3
         WHERE id = ?1 AND (deck_id IS NOT ?2 OR deck_version_id IS NOT ?3)",
    )?
    .execute(params![league_id, deck_id, newest])?;

    for token in &run.match_tokens {
        let (version, may_move) = match version_by_token.get(token.as_str()) {
            Some(version) => (Some(*version), true),
            None => (newest, false),
        };
        let Some(version) = version else {
            continue;
        };
        tx.prepare_cached(
            "UPDATE matches SET deck_version_id = ?2
             WHERE token = ?1 AND deck_version_id IS NOT ?2
               AND (deck_version_id IS NULL
                    OR (?3 AND deck_version_id IN (SELECT id FROM deck_versions
                                                   WHERE deck_id = ?4)))",
        )?
        .execute(params![token, version, may_move, deck_id])?;
    }
    Ok(())
}

/// A course deck as registered rows: the main deck, then the sideboard.
fn deck_cards(deck: &MatchDeck) -> Vec<RegisteredCard> {
    let main = deck
        .main
        .iter()
        .map(|&(catalog_id, quantity)| RegisteredCard {
            catalog_id,
            quantity,
            sideboard: false,
        });
    let side = deck
        .side
        .iter()
        .map(|&(catalog_id, quantity)| RegisteredCard {
            catalog_id,
            quantity,
            sideboard: true,
        });
    main.chain(side).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Platform;
    use crate::arena::PlatformAccount;
    use crate::test_support::db;
    use crate::write::{WriteBatch, commit};
    use rusqlite::Connection;

    const PLAYER: &str = "Qplayer#12345";
    const DRAFT: &str = "PremierDraft_FRA_20260929";

    fn setup(label: &str) -> (fixture_tools::TestDir, Connection, i64) {
        let (dir, mut conn) = db(label);
        crate::test_support::arena_log_file(&conn);
        commit(
            &mut conn,
            &WriteBatch {
                platform_accounts: vec![PlatformAccount {
                    platform: Platform::Arena,
                    username: PLAYER.to_string(),
                    user_id: Some("QUSERID1".to_string()),
                    display_name: None,
                    seen_at_ms: 1,
                }],
                ..WriteBatch::default()
            },
        )
        .expect("account");
        let account_id = conn
            .query_row(
                "SELECT id FROM accounts WHERE username = ?1",
                [PLAYER],
                |row| row.get(0),
            )
            .expect("account id");
        (dir, conn, account_id)
    }

    fn course(byte_offset: i64, course_id: &str, module: &str) -> CourseObservation {
        CourseObservation {
            file_id: 1,
            byte_offset,
            observed_at_ms: 1_000 + byte_offset,
            account_username: PLAYER.to_string(),
            course_id: course_id.to_string(),
            event_name: DRAFT.to_string(),
            module: module.to_string(),
            wins: 0,
            losses: 0,
            deck_id: "q-deck".to_string(),
            deck_name: "Draft Deck".to_string(),
            main: vec![(2_000_002, 1), (2_000_001, 2)],
            side: vec![(2_000_003, 1)],
            pool: vec![2_000_001, 2_000_001, 2_000_002, 2_000_003],
        }
    }

    fn observe(conn: &mut Connection, rows: Vec<CourseObservation>) {
        commit(
            conn,
            &WriteBatch {
                arena_course_observations: rows,
                ..WriteBatch::default()
            },
        )
        .expect("observations");
    }

    /// A limited match `token` of `account_id` whose first Arena event is at
    /// `byte_offset` of file 1, played in `event_name` when given.
    fn limited_match(
        conn: &Connection,
        account_id: i64,
        token: &str,
        byte_offset: i64,
        event_name: Option<&str>,
    ) {
        conn.execute(
            "INSERT INTO matches (token, account_id, source, format, match_type, state, outcome,
                 started_at, ended_at, last_activity_at, revision, derived_version)
             VALUES (?1, ?2, 'log', 'DFRAFRAFRA', 'limited', 'complete', 'win', ?3, ?4, ?4, 1, 0)",
            params![
                token,
                account_id,
                10_000 + byte_offset,
                20_000 + byte_offset
            ],
        )
        .expect("match");
        for (offset, seq) in [(byte_offset + 5, 0), (byte_offset, 1)] {
            conn.execute(
                "INSERT INTO arena_log_events (arena_log_file_id, byte_offset, seq, kind,
                     match_token, logged_at, raw)
                 VALUES (1, ?1, ?2, 'room_state', ?3, 1, x'')",
                params![offset, seq, token],
            )
            .expect("event");
        }
        if let Some(event_name) = event_name {
            conn.execute(
                "INSERT INTO arena_match_events (match_token, event_name) VALUES (?1, ?2)",
                params![token, event_name],
            )
            .expect("event name");
        }
    }

    fn sync(conn: &mut Connection, account_id: i64, now_ms: i64) {
        let tx = conn.transaction().expect("tx");
        sync_arena_runs(&tx, account_id, now_ms).expect("sync");
        tx.commit().expect("commit");
    }

    fn texts(conn: &Connection, sql: &str) -> Vec<String> {
        let mut stmt = conn.prepare(sql).expect("prepare");
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query");
        rows.collect::<Result<_, _>>().expect("rows")
    }

    /// Every row of `table` as text, every column, sorted.
    fn dump(conn: &Connection, table: &str) -> Vec<String> {
        dump_where(conn, table, "1")
    }

    /// The rows of `table` that `filter` keeps, as `dump` gives them.
    fn dump_where(conn: &Connection, table: &str, filter: &str) -> Vec<String> {
        let columns = texts(
            conn,
            &format!("SELECT 'quote(\"' || name || '\")' FROM pragma_table_info('{table}')"),
        );
        let mut rows = texts(
            conn,
            &format!(
                "SELECT {} FROM {table} WHERE {filter}",
                columns.join(" || '|' || ")
            ),
        );
        rows.sort();
        rows
    }

    #[test]
    fn sync_writes_a_run_its_links_and_snapshots() {
        let (_dir, mut conn, account_id) = setup("arena-run-sync-writes");
        observe(&mut conn, vec![course(100, "q-course", "CreateMatch")]);
        limited_match(&conn, account_id, "q-m1", 200, Some(DRAFT));
        limited_match(&conn, account_id, "q-m2", 300, Some(DRAFT));
        // A constructed match and a match taken out of every league stay out.
        conn.execute(
            "INSERT INTO matches (token, account_id, source, format, match_type, state, outcome,
                 started_at, last_activity_at, revision, derived_version)
             VALUES ('q-constructed', ?1, 'log', 'CSTANDARD', 'constructed', 'complete', 'win',
                 1, 2, 1, 0)",
            [account_id],
        )
        .expect("constructed");
        limited_match(&conn, account_id, "q-out", 400, Some(DRAFT));
        conn.execute("UPDATE matches SET no_league = 1 WHERE token = 'q-out'", [])
            .expect("no league");

        sync(&mut conn, account_id, 99_000);

        assert_eq!(
            texts(
                &conn,
                "SELECT run_key || '|' || token || '|' || account_id || '|' || source || '|'
                     || kind || '|' || state || '|' || name || '|' || format || '|' || set_code
                     || '|' || started_at || '|' || COALESCE(completed_at, '-') || '|'
                     || created_at || '|' || COALESCE(mtgo_course_id, '-') || '|'
                     || COALESCE(event_id, '-') || '|' || (sync_client_id IS NOT NULL)
                 FROM leagues"
            ),
            vec![format!(
                "arena:q-course|arena:q-course|{account_id}|log|draft|active|Premier Draft \u{b7} FRA|DFRAFRAFRA|FRA|10200|-|99000|-|-|1"
            )]
        );
        assert_eq!(
            texts(
                &conn,
                "SELECT m.token || ' ' || COALESCE(l.run_key, '-') FROM matches m
                 LEFT JOIN leagues l ON l.id = m.league_id ORDER BY m.token"
            ),
            vec![
                "q-constructed -".to_string(),
                "q-m1 arena:q-course".to_string(),
                "q-m2 arena:q-course".to_string(),
                "q-out -".to_string(),
            ]
        );
        let expected_cards = vec![
            RegisteredCard {
                catalog_id: 2_000_002,
                quantity: 1,
                sideboard: false,
            },
            RegisteredCard {
                catalog_id: 2_000_001,
                quantity: 2,
                sideboard: false,
            },
            RegisteredCard {
                catalog_id: 2_000_003,
                quantity: 1,
                sideboard: true,
            },
        ];
        let signature = crate::limited::registered("q-m1", 0, expected_cards.clone()).signature;
        assert_eq!(
            texts(
                &conn,
                "SELECT m.token || '|' || s.source || '|' || s.cards || '|' || s.signature || '|'
                     || s.captured_at || '|' || s.event_file_id || '|' || s.event_offset
                 FROM limited_deck_snapshots s JOIN matches m ON m.id = s.match_id
                 ORDER BY m.token"
            ),
            ["q-m1", "q-m2"]
                .iter()
                .map(|token| format!(
                    "{token}|registered|{}|{signature}|1100|1|100",
                    crate::limited::cards_json(&expected_cards).expect("json")
                ))
                .collect::<Vec<_>>()
        );
        let stored: String = conn
            .query_row(
                "SELECT cards FROM limited_deck_snapshots LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("cards");
        let value: serde_json::Value = serde_json::from_str(&stored).expect("json");
        assert_eq!(
            limited::registered_cards_from_json(&value),
            expected_cards,
            "the registered JSON shape"
        );
    }

    #[test]
    fn sync_is_idempotent() {
        let (_dir, mut conn, account_id) = setup("arena-run-sync-idempotent");
        observe(&mut conn, vec![course(100, "q-course", "CreateMatch")]);
        limited_match(&conn, account_id, "q-m1", 200, Some(DRAFT));
        observe(&mut conn, vec![course(300, "q-course", "Complete")]);

        sync(&mut conn, account_id, 99_000);
        let tables = ["leagues", "matches", "limited_deck_snapshots"];
        let first: Vec<Vec<String>> = tables.iter().map(|table| dump(&conn, table)).collect();
        assert_eq!(
            texts(&conn, "SELECT state || ' ' || completed_at FROM leagues"),
            vec!["complete 20200".to_string()],
            "completed at the later of the observation and the last match end"
        );
        sync(&mut conn, account_id, 123_000);
        let second: Vec<Vec<String>> = tables.iter().map(|table| dump(&conn, table)).collect();
        assert_eq!(first, second, "the same rows and ids");
    }

    #[test]
    fn an_orphan_that_joins_its_course_is_replaced() {
        let (_dir, mut conn, account_id) = setup("arena-run-sync-orphan");
        limited_match(&conn, account_id, "q-m1", 200, Some(DRAFT));
        sync(&mut conn, account_id, 99_000);
        assert_eq!(
            texts(
                &conn,
                "SELECT l.run_key FROM matches m JOIN leagues l ON l.id = m.league_id"
            ),
            vec![format!("arena:{DRAFT}:q-m1")]
        );

        observe(&mut conn, vec![course(300, "q-course", "CreateMatch")]);
        sync(&mut conn, account_id, 99_500);
        assert_eq!(
            texts(&conn, "SELECT run_key FROM leagues"),
            vec!["arena:q-course".to_string()],
            "the orphan run is gone"
        );
        assert_eq!(
            texts(
                &conn,
                "SELECT l.run_key FROM matches m JOIN leagues l ON l.id = m.league_id"
            ),
            vec!["arena:q-course".to_string()]
        );
    }

    #[test]
    fn a_course_run_takes_its_course_client_id_and_an_orphan_none() {
        let (_dir, mut conn, account_id) = setup("arena-run-sync-client-ids");
        observe(&mut conn, vec![course(100, "q-course", "CreateMatch")]);
        limited_match(&conn, account_id, "q-m1", 200, Some(DRAFT));
        // A match of another event with no course: an orphan run.
        limited_match(
            &conn,
            account_id,
            "q-m2",
            300,
            Some("QuickDraft_FDN_20261101"),
        );
        sync(&mut conn, account_id, 99_000);
        assert_eq!(
            texts(
                &conn,
                "SELECT run_key || ' ' || COALESCE(sync_client_id, '-') FROM leagues
                 ORDER BY run_key"
            ),
            vec![
                "arena:QuickDraft_FDN_20261101:q-m2 -".to_string(),
                "arena:q-course arena_q-course".to_string(),
            ]
        );
    }

    /// Game `game_number` of match `token`, its local deck `main` and
    /// sideboard `side` as catalog ids.
    fn game_with_cards(
        conn: &Connection,
        token: &str,
        game_number: i64,
        main: &[i64],
        side: &[i64],
    ) {
        let game_id: i64 = conn
            .query_row(
                "INSERT INTO games (match_id, game_number, result, end_reason)
                 SELECT id, ?2, 'win', 'win' FROM matches WHERE token = ?1 RETURNING id",
                params![token, game_number],
                |row| row.get(0),
            )
            .expect("game");
        for (catalog_id, main_qty, side_qty) in main
            .iter()
            .map(|id| (*id, 1, 0))
            .chain(side.iter().map(|id| (*id, 0, 1)))
        {
            conn.execute(
                "INSERT INTO game_cards (game_id, side, catalog_id, main_qty, side_qty)
                 VALUES (?1, 'local', ?2, ?3, ?4)",
                params![game_id, catalog_id, main_qty, side_qty],
            )
            .expect("card");
        }
        // The opponent's cards are never the local pool.
        conn.execute(
            "INSERT INTO game_cards (game_id, side, catalog_id, seen) VALUES (?1, 'opponent', 7, 1)",
            [game_id],
        )
        .expect("opponent card");
    }

    #[test]
    fn course_less_matches_split_by_their_first_games_pool() {
        let (_dir, mut conn, account_id) = setup("arena-run-sync-pools");
        let pool_a: Vec<i64> = (2_000_100..2_000_140).collect();
        let pool_b: Vec<i64> = (2_000_500..2_000_540).collect();
        for (token, offset, pool) in [
            ("q-m1", 100, &pool_a),
            ("q-m2", 200, &pool_a),
            ("q-m3", 300, &pool_b),
        ] {
            limited_match(&conn, account_id, token, offset, Some(DRAFT));
            game_with_cards(&conn, token, 1, &pool[..23], &pool[23..]);
        }
        // A later game of a match never decides its pool.
        game_with_cards(&conn, "q-m2", 2, &pool_b, &[]);

        sync(&mut conn, account_id, 99_000);

        assert_eq!(
            texts(
                &conn,
                "SELECT m.token || ' ' || l.run_key FROM matches m
                 JOIN leagues l ON l.id = m.league_id ORDER BY m.token"
            ),
            vec![
                format!("q-m1 arena:{DRAFT}:q-m1"),
                format!("q-m2 arena:{DRAFT}:q-m1"),
                format!("q-m3 arena:{DRAFT}:q-m3"),
            ]
        );
    }

    #[test]
    fn basic_lands_never_count_toward_a_pool() {
        let (_dir, mut conn, account_id) = setup("arena-run-sync-basics");
        let pool: Vec<i64> = (2_000_100..2_000_140).collect();
        let basics: Vec<i64> = (2_000_900..2_000_930).collect();
        for basic in &basics {
            conn.execute(
                "INSERT INTO cards (catalog_id, name, type) VALUES (?1, 'Qplains', 'Basic Land - Plains')",
                [basic],
            )
            .expect("basic");
        }
        limited_match(&conn, account_id, "q-m1", 100, Some(DRAFT));
        game_with_cards(&conn, "q-m1", 1, &pool[..23], &pool[23..]);
        // The same pool's deck, short and padded with basic lands: counted
        // with its basics it would look like another pool.
        limited_match(&conn, account_id, "q-m2", 200, Some(DRAFT));
        game_with_cards(&conn, "q-m2", 1, &pool[..10], &basics);

        sync(&mut conn, account_id, 99_000);

        assert_eq!(
            texts(&conn, "SELECT run_key FROM leagues"),
            vec![format!("arena:{DRAFT}:q-m1")]
        );
    }

    /// Each match's run key (or `-`) and registered snapshot place (or `-`).
    fn links(conn: &Connection) -> Vec<String> {
        texts(
            conn,
            "SELECT m.token || ' ' || COALESCE(l.run_key, '-') || ' '
                 || COALESCE(s.event_file_id || ':' || s.event_offset, '-')
             FROM matches m LEFT JOIN leagues l ON l.id = m.league_id
             LEFT JOIN limited_deck_snapshots s ON s.match_id = m.id AND s.source = 'registered'
             ORDER BY m.token",
        )
    }

    #[test]
    fn a_match_that_leaves_the_fold_loses_its_link_and_snapshot() {
        for change in [
            "no_league = 1",
            "hidden_at = 5",
            "detached_at = 5",
            "match_type = 'constructed'",
        ] {
            let (_dir, mut conn, account_id) = setup("arena-run-sync-leaves");
            observe(&mut conn, vec![course(100, "q-course", "CreateMatch")]);
            limited_match(&conn, account_id, "q-m1", 200, Some(DRAFT));
            limited_match(&conn, account_id, "q-m2", 300, Some(DRAFT));
            sync(&mut conn, account_id, 99_000);
            assert_eq!(
                links(&conn),
                vec![
                    "q-m1 arena:q-course 1:100".to_string(),
                    "q-m2 arena:q-course 1:100".to_string(),
                ]
            );

            conn.execute(
                &format!("UPDATE matches SET {change} WHERE token = 'q-m2'"),
                [],
            )
            .expect("change");
            sync(&mut conn, account_id, 99_500);
            assert_eq!(
                links(&conn),
                vec![
                    "q-m1 arena:q-course 1:100".to_string(),
                    "q-m2 - -".to_string(),
                ],
                "{change}"
            );
        }
    }

    #[test]
    fn a_run_key_of_another_account_is_left_alone() {
        let (_dir, mut conn, account_id) = setup("arena-run-sync-other-key");
        let other_id: i64 = conn
            .query_row(
                "INSERT INTO accounts (username, first_seen_at) VALUES ('Qother', 0) RETURNING id",
                [],
                |row| row.get(0),
            )
            .expect("other account");
        conn.execute(
            "INSERT INTO leagues (run_key, account_id, source, kind, state, token, name,
                 started_at, created_at)
             VALUES ('arena:q-course', ?1, 'log', 'draft', 'active', 'arena:q-course', 'Qother',
                 1, 1)",
            [other_id],
        )
        .expect("other row");
        observe(&mut conn, vec![course(100, "q-course", "CreateMatch")]);
        limited_match(&conn, account_id, "q-m1", 200, Some(DRAFT));
        let before = dump(&conn, "leagues");

        sync(&mut conn, account_id, 99_000);
        assert_eq!(dump(&conn, "leagues"), before);
        assert_eq!(
            links(&conn),
            vec!["q-m1 - -".to_string()],
            "no link, no snapshot"
        );
    }

    #[test]
    fn a_snapshot_is_rewritten_only_when_its_observation_moves() {
        let (_dir, mut conn, account_id) = setup("arena-run-sync-rewrite");
        observe(&mut conn, vec![course(100, "q-course", "CreateMatch")]);
        limited_match(&conn, account_id, "q-m1", 200, Some(DRAFT));
        sync(&mut conn, account_id, 99_000);
        // A mark the sync would overwrite if it rewrote the row.
        conn.execute("UPDATE limited_deck_snapshots SET captured_at = 7", [])
            .expect("mark");
        sync(&mut conn, account_id, 99_100);
        let snapshot = |conn: &Connection| {
            texts(
                conn,
                "SELECT captured_at || ' ' || event_offset || ' ' || cards
                 FROM limited_deck_snapshots",
            )
        };
        assert_eq!(
            snapshot(&conn),
            vec![r#"7 100 [{"catalog_id":2000002,"quantity":1,"sideboard":false},{"catalog_id":2000001,"quantity":2,"sideboard":false},{"catalog_id":2000003,"quantity":1,"sideboard":true}]"#.to_string()],
            "the same place is not rewritten"
        );

        observe(
            &mut conn,
            vec![CourseObservation {
                main: vec![(2_000_004, 3)],
                side: Vec::new(),
                ..course(150, "q-course", "CreateMatch")
            }],
        );
        sync(&mut conn, account_id, 99_200);
        assert_eq!(
            snapshot(&conn),
            vec![r#"1150 150 [{"catalog_id":2000004,"quantity":3,"sideboard":false}]"#.to_string()],
            "a new place rewrites the cards"
        );
    }

    #[test]
    fn a_failed_run_sync_leaves_the_rest_of_the_batch_committed() {
        for resync in [false, true] {
            let (_dir, mut conn, account_id) = setup("arena-run-sync-guarded");
            observe(&mut conn, vec![course(100, "q-course", "CreateMatch")]);
            limited_match(&conn, account_id, "q-m1", 200, Some(DRAFT));
            sync(&mut conn, account_id, 99_000);
            let before = (
                dump(&conn, "leagues"),
                dump(&conn, "limited_deck_snapshots"),
            );
            // A stored observation that does not read back fails the sync
            // after a new match was linked in the savepoint.
            limited_match(&conn, account_id, "q-m2", 300, Some(DRAFT));
            conn.execute(
                "INSERT INTO arena_course_observations (arena_log_file_id, byte_offset,
                     observed_at, account_id, course_id, event_name, module, wins, losses,
                     deck_id, deck_name, cards)
                 VALUES (1, 250, 1, ?1, 'q-broken', ?2, 'CreateMatch', 0, 0, '', '', 'not json')",
                params![account_id, DRAFT],
            )
            .expect("broken row");

            commit(
                &mut conn,
                &WriteBatch {
                    arena_run_accounts: if resync {
                        Vec::new()
                    } else {
                        vec![PLAYER.to_string()]
                    },
                    arena_runs_resync: resync,
                    arena_match_events: vec![("q-late".to_string(), DRAFT.to_string())],
                    settings: vec![("q_setting".to_string(), "1".to_string())],
                    now_ms: 99_500,
                    ..WriteBatch::default()
                },
            )
            .expect("the batch commits");

            assert_eq!(
                texts(&conn, "SELECT value FROM settings WHERE key = 'q_setting'"),
                vec!["1".to_string()],
                "resync {resync}"
            );
            assert_eq!(
                texts(
                    &conn,
                    "SELECT event_name FROM arena_match_events WHERE match_token = 'q-late'"
                ),
                vec![DRAFT.to_string()]
            );
            assert_eq!(
                (
                    dump(&conn, "leagues"),
                    dump(&conn, "limited_deck_snapshots")
                ),
                before,
                "the failed sync is rolled back"
            );
        }
    }

    #[test]
    fn the_arena_sync_leaves_mtgo_rows_alone() {
        let (_dir, mut conn, account_id) = setup("arena-run-sync-mtgo");
        let mtgo_id: i64 = conn
            .query_row(
                "INSERT INTO accounts (username, first_seen_at) VALUES ('Qmtgo', 0) RETURNING id",
                [],
                |row| row.get(0),
            )
            .expect("mtgo account");
        conn.execute_batch(&format!(
            "INSERT INTO leagues (id, run_key, account_id, source, kind, state, token, name,
                 started_at, created_at)
             VALUES (50, 'q-league:q-mtgo', {mtgo_id}, 'log', 'draft', 'active', 'q-league',
                 'Qleague', 1, 1),
                 (51, 'arena:q-other', {mtgo_id}, 'log', 'draft', 'active', 'arena:q-other',
                 'Qother', 1, 1),
                 (52, 'q-league:q-arena', {account_id}, 'log', 'draft', 'active', 'q-league',
                 'Qleague', 1, 1);
             INSERT INTO matches (token, account_id, source, format, match_type, state, outcome,
                 started_at, last_activity_at, revision, derived_version, league_id)
             VALUES ('q-mtgo', {mtgo_id}, 'log', 'DFRAFRAFRA', 'limited', 'complete', 'win',
                 1, 2, 1, 0, 50);
             INSERT INTO limited_deck_snapshots (match_id, source, cards, signature,
                 captured_at, event_file_id, event_offset)
             SELECT id, 'registered', '[]', 'Q-sig', 1, 7, 8 FROM matches;"
        ))
        .expect("mtgo rows");
        // The Arena account has a run of its own to write.
        observe(&mut conn, vec![course(100, "q-course", "CreateMatch")]);
        limited_match(&conn, account_id, "q-m1", 200, Some(DRAFT));
        let others = [
            ("leagues", "id IN (50, 51, 52)"),
            ("matches", "token = 'q-mtgo'"),
            (
                "limited_deck_snapshots",
                "match_id = (SELECT id FROM matches WHERE token = 'q-mtgo')",
            ),
        ];
        let rows = |conn: &Connection| -> Vec<Vec<String>> {
            others
                .iter()
                .map(|(table, filter)| dump_where(conn, table, filter))
                .collect()
        };
        let before = rows(&conn);

        sync(&mut conn, account_id, 99_000);
        sync(&mut conn, mtgo_id, 99_000);
        let tx = conn.transaction().expect("tx");
        sync_all_arena_runs(&tx, 99_000).expect("sync all");
        tx.commit().expect("commit");

        assert_eq!(before, rows(&conn), "no other row changed");
        assert_eq!(
            texts(
                &conn,
                "SELECT l.run_key FROM matches m JOIN leagues l ON l.id = m.league_id
                 WHERE m.token = 'q-m1'"
            ),
            vec!["arena:q-course".to_string()],
            "the Arena run is written"
        );
    }

    #[test]
    fn arena_runs_are_read_on_arena_only() {
        use crate::Platform;
        use crate::league_sync::LeagueOwner;
        use crate::leagues::{RunFilter, account_ids, stored_limited_runs};

        let (_dir, mut conn, arena) = setup("arena-runs-read-on-arena");
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at, platform) VALUES ('Qmtgo', 0, 'mtgo')",
            [],
        )
        .expect("mtgo account");
        let mtgo: i64 = conn.last_insert_rowid();
        let mut empty = course(100, "q-empty", "CreateMatch");
        empty.event_name = "QuickDraft_FDN_20260101".to_string();
        empty.pool = Vec::new();
        observe(
            &mut conn,
            vec![course(100, "q-course", "CreateMatch"), empty],
        );
        limited_match(&conn, arena, "q-m1", 200, Some(DRAFT));
        sync(&mut conn, arena, 99_000);

        assert_eq!(
            account_ids(&conn, Platform::Arena).expect("ids"),
            vec![arena]
        );
        assert_eq!(account_ids(&conn, Platform::Mtgo).expect("ids"), vec![mtgo]);

        let filter = |owners: Vec<LeagueOwner>| RunFilter {
            owners,
            kinds: Vec::new(),
            deck_id: None,
            run_key: None,
        };
        let on_arena =
            stored_limited_runs(&conn, &filter(vec![LeagueOwner::Account(arena)])).expect("read");
        assert_eq!(on_arena.runs.len(), 2);
        let empty = on_arena
            .runs
            .iter()
            .find(|run| run.run.key == "arena:q-empty")
            .expect("the empty course");
        assert_eq!(empty.stated_pool, None, "an empty stated pool is none");
        let run = on_arena
            .runs
            .iter()
            .find(|run| run.run.key == "arena:q-course")
            .expect("the course");
        assert_eq!(run.run.key, "arena:q-course");
        assert!(run.is_arena());
        assert_eq!(
            run.stated_pool,
            Some(vec![2_000_001, 2_000_001, 2_000_002, 2_000_003])
        );
        assert!(run.run_input().uncapped);

        let on_mtgo = stored_limited_runs(
            &conn,
            &filter(vec![LeagueOwner::Account(mtgo), LeagueOwner::Nobody]),
        )
        .expect("read");
        assert!(on_mtgo.runs.is_empty());
    }

    // ---- Runs across devices (cross-device sync spec section 8) ----------

    use crate::import_decks::{ImportedDeck, ImportedVersion};
    use crate::sync::{
        BuiltRow, PulledDeckParts, SyncEdit, arena_run_bundle_input, deck_bundle_input,
        deck_candidates, edit_sync, is_league_pending, is_pending, is_pending_key,
        league_candidates, match_bundle_input, match_candidates, synced_rows,
    };
    use mymtgo_api::{SyncType, Tier};
    use std::collections::BTreeMap;
    use sync_bundles::league::{arena_run_bundle, read_arena_run_bundle};
    use sync_bundles::{bundle_hash, canonical_json};

    /// Late enough that every test match is past its send hold.
    const LATER: i64 = 1_000_000_000;

    /// The cloud: each row's hash and what a device pulls of it.
    #[derive(Default)]
    struct Cloud {
        decks: BTreeMap<String, (String, serde_json::Value)>,
        runs: BTreeMap<String, (String, serde_json::Value)>,
        matches: BTreeMap<String, (String, crate::import::PulledMatch)>,
    }

    /// `course` with the record `wins`-`losses`.
    fn course_at(byte_offset: i64, wins: u32, losses: u32, module: &str) -> CourseObservation {
        let mut row = course(byte_offset, "q-course", module);
        row.wins = wins;
        row.losses = losses;
        row
    }

    /// Mark the course deck's slot, as the deck sync's ledger does.
    fn enable_course_deck(conn: &Connection) {
        conn.execute(
            "UPDATE decks SET cloud_sync_enabled = 1 WHERE mtgo_id = 'limited:arena_q-course'",
            [],
        )
        .expect("slot");
    }

    fn run_bundle(conn: &Connection, league_id: i64) -> serde_json::Value {
        let input = arena_run_bundle_input(conn, league_id)
            .expect("input")
            .expect("an Arena run");
        arena_run_bundle(&input, None)
    }

    /// The runner's push: build every owned row (recording the build), and
    /// send it when it differs from what this device last knew the server
    /// held. A row the server changed since is left to the pull.
    fn push(conn: &mut Connection, cloud: &mut Cloud) {
        enable_course_deck(conn);
        let decks = synced_rows(conn, SyncType::Deck).expect("rows");
        let mut built = Vec::new();
        let mut stored = Vec::new();
        for candidate in deck_candidates(conn, Tier::Supporter).expect("decks") {
            if !candidate.owned {
                continue;
            }
            let bundle = deck_bundle_of(conn, &candidate);
            let hash = bundle_hash(&canonical_json(&bundle));
            built.push(BuiltRow {
                client_id: candidate.client_id.clone(),
                built_hash: hash.clone(),
                touch_key: candidate.touch_key.clone(),
                changed_at: 1,
            });
            let known = decks
                .get(&candidate.client_id)
                .and_then(|row| row.hash.clone());
            let server = cloud
                .decks
                .get(&candidate.client_id)
                .map(|(hash, _)| hash.clone());
            if known.as_ref() != Some(&hash) && server == known {
                cloud
                    .decks
                    .insert(candidate.client_id.clone(), (hash.clone(), bundle));
                stored.push((candidate.client_id, hash));
            }
        }
        record(conn, SyncType::Deck, built, stored);

        let leagues = synced_rows(conn, SyncType::League).expect("rows");
        let mut built = Vec::new();
        let mut stored = Vec::new();
        for candidate in league_candidates(conn, Tier::Supporter).expect("leagues") {
            assert!(candidate.owned, "{}", candidate.client_id);
            let bundle = run_bundle(conn, candidate.row_id);
            let hash = bundle_hash(&canonical_json(&bundle));
            built.push(BuiltRow {
                client_id: candidate.client_id.clone(),
                built_hash: hash.clone(),
                touch_key: candidate.touch_key.clone(),
                changed_at: 1,
            });
            let known = leagues
                .get(&candidate.client_id)
                .and_then(|row| row.hash.clone());
            let server = cloud
                .runs
                .get(&candidate.client_id)
                .map(|(hash, _)| hash.clone());
            if known.as_ref() != Some(&hash) && server == known {
                cloud
                    .runs
                    .insert(candidate.client_id.clone(), (hash.clone(), bundle));
                stored.push((candidate.client_id, hash));
            }
        }
        record(conn, SyncType::League, built, stored);

        let matches = synced_rows(conn, SyncType::Match).expect("rows");
        let mut built = Vec::new();
        let mut stored = Vec::new();
        for candidate in match_candidates(conn, Tier::Supporter, LATER).expect("matches") {
            if !candidate.owned {
                continue;
            }
            let input = match_bundle_input(conn, candidate.row_id, chrono_tz::UTC)
                .expect("input")
                .expect("a match");
            let hash = bundle_hash(&canonical_json(&sync_bundles::match_bundle::match_bundle(
                &input, None,
            )));
            built.push(BuiltRow {
                client_id: candidate.client_id.clone(),
                built_hash: hash.clone(),
                touch_key: candidate.touch_key.clone(),
                changed_at: 1,
            });
            let known = matches
                .get(&candidate.client_id)
                .and_then(|row| row.hash.clone());
            if known.as_ref() != Some(&hash) {
                let pulled = crate::test_support::pulled_from(conn, &candidate.client_id);
                cloud
                    .matches
                    .insert(candidate.client_id.clone(), (hash.clone(), pulled));
                stored.push((candidate.client_id, hash));
            }
        }
        record(conn, SyncType::Match, built, stored);
    }

    fn record(
        conn: &mut Connection,
        sync_type: SyncType,
        rows: Vec<BuiltRow>,
        stored: Vec<(String, String)>,
    ) {
        edit_sync(conn, &SyncEdit::Built { sync_type, rows }).expect("built");
        edit_sync(
            conn,
            &SyncEdit::Stored {
                sync_type,
                rows: stored,
                now_ms: LATER,
            },
        )
        .expect("stored");
    }

    /// The runner's pull: every row whose server hash this device does not
    /// hold, runs before matches. Returns how many rows came down.
    fn pull(conn: &mut Connection, cloud: &Cloud) -> usize {
        let mut pulled = 0;
        let decks = synced_rows(conn, SyncType::Deck).expect("rows");
        for (client_id, (hash, bundle)) in &cloud.decks {
            if decks.get(client_id).and_then(|row| row.hash.as_ref()) == Some(hash) {
                continue;
            }
            pull_deck(conn, client_id, hash, bundle);
            pulled += 1;
        }
        enable_course_deck(conn);
        let leagues = synced_rows(conn, SyncType::League).expect("rows");
        for (client_id, (hash, bundle)) in &cloud.runs {
            if leagues.get(client_id).and_then(|row| row.hash.as_ref()) == Some(hash) {
                continue;
            }
            let (run, carry) = read_arena_run_bundle(bundle).expect("reads");
            edit_sync(
                conn,
                &SyncEdit::PulledArenaRun {
                    server_updated_at_ms: None,
                    client_id: client_id.clone(),
                    run: Box::new(run),
                    hash: hash.clone(),
                    carry: carry.to_json().to_string(),
                    timezone: chrono_tz::UTC,
                    now_ms: LATER,
                },
            )
            .expect("pulled run");
            pulled += 1;
        }
        let matches = synced_rows(conn, SyncType::Match).expect("rows");
        for (token, (hash, match_bundle)) in &cloud.matches {
            if matches.get(token).and_then(|row| row.hash.as_ref()) == Some(hash) {
                continue;
            }
            let held: bool = conn
                .query_row(
                    "SELECT EXISTS (SELECT 1 FROM matches WHERE token = ?1 AND source = 'log')",
                    [token],
                    |row| row.get(0),
                )
                .expect("held");
            if held {
                continue;
            }
            edit_sync(
                conn,
                &SyncEdit::PulledMatch {
                    client_id: token.clone(),
                    pulled: Box::new(match_bundle.clone()),
                    hash: hash.clone(),
                    carry: "{}".to_string(),
                    timezone: chrono_tz::UTC,
                    now_ms: LATER,
                },
            )
            .expect("pulled match");
            pulled += 1;
        }
        after_pull(conn);
        pulled
    }

    /// A deck's bundle as the runner builds it.
    fn deck_bundle_of(
        conn: &Connection,
        candidate: &crate::sync::DeckCandidate,
    ) -> serde_json::Value {
        let input = deck_bundle_input(conn, candidate).expect("deck input");
        sync_bundles::deck_bundle(&input, None)
    }

    /// A deck bundle read back and pulled as the runner does.
    fn pull_deck(conn: &mut Connection, client_id: &str, hash: &str, bundle: &serde_json::Value) {
        let (input, keys, carry) = sync_bundles::read_deck_bundle(bundle).expect("reads");
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
        edit_sync(
            conn,
            &SyncEdit::PulledDeck {
                client_id: client_id.to_string(),
                deck: Box::new(deck),
                parts: Box::new(parts),
                hash: hash.to_string(),
                carry: carry.to_json().to_string(),
                now_ms: LATER,
            },
        )
        .expect("pulled deck");
    }

    /// The runner's last pull step: the Arena runs folded once.
    fn after_pull(conn: &mut Connection) {
        edit_sync(conn, &SyncEdit::ArenaRunsAfterPull { now_ms: LATER }).expect("fold");
    }

    fn exchange(conn: &mut Connection, cloud: &mut Cloud) -> usize {
        push(conn, cloud);
        pull(conn, cloud)
    }

    /// Owned rows still waiting to be sent (`pending_count`'s rule).
    fn pending(conn: &Connection) -> Vec<String> {
        let mut waiting = Vec::new();
        let decks = synced_rows(conn, SyncType::Deck).expect("rows");
        for candidate in deck_candidates(conn, Tier::Supporter).expect("decks") {
            if candidate.owned && is_pending(&candidate, decks.get(&candidate.client_id)) {
                waiting.push(candidate.client_id);
            }
        }
        let leagues = synced_rows(conn, SyncType::League).expect("rows");
        for candidate in league_candidates(conn, Tier::Supporter).expect("leagues") {
            if candidate.owned
                && is_league_pending(conn, &candidate, leagues.get(&candidate.client_id))
                    .expect("pending")
            {
                waiting.push(candidate.client_id);
            }
        }
        let matches = synced_rows(conn, SyncType::Match).expect("rows");
        for candidate in match_candidates(conn, Tier::Supporter, LATER).expect("matches") {
            if candidate.owned
                && is_pending_key(&candidate.touch_key, matches.get(&candidate.client_id))
            {
                waiting.push(candidate.client_id);
            }
        }
        waiting
    }

    fn run_id(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT id FROM leagues WHERE run_key = 'arena:q-course'",
            [],
            |row| row.get(0),
        )
        .expect("the run")
    }

    #[test]
    fn a_run_split_across_devices_converges() {
        let (_a, mut a, a_account) = setup("arena-run-xdevice-a");
        let (_b, mut b, b_account) = setup("arena-run-xdevice-b");
        let mut cloud = Cloud::default();

        // A plays two matches and sees 2-0.
        observe(&mut a, vec![course_at(100, 0, 0, "CreateMatch")]);
        limited_match(&a, a_account, "q-a1", 200, Some(DRAFT));
        limited_match(&a, a_account, "q-a2", 300, Some(DRAFT));
        observe(&mut a, vec![course_at(400, 2, 0, "CreateMatch")]);
        sync(&mut a, a_account, 99_000);
        enable_course_deck(&a);
        exchange(&mut a, &mut cloud);
        assert_eq!(cloud.runs.len(), 1);
        assert_eq!(cloud.matches.len(), 2);
        exchange(&mut b, &mut cloud);

        // B plays the third match and sees 3-0.
        observe(&mut b, vec![course_at(500, 2, 0, "CreateMatch")]);
        limited_match(&b, b_account, "q-b1", 600, Some(DRAFT));
        observe(&mut b, vec![course_at(700, 3, 0, "CreateMatch")]);
        sync(&mut b, b_account, 99_500);
        enable_course_deck(&b);
        exchange(&mut b, &mut cloud);
        exchange(&mut a, &mut cloud);
        // The next runs build and send nothing new.
        push(&mut b, &mut cloud);
        push(&mut a, &mut cloud);

        for (label, conn) in [("A", &a), ("B", &b)] {
            assert_eq!(
                texts(conn, "SELECT run_key || ' ' || source FROM leagues"),
                vec!["arena:q-course log".to_string()],
                "{label}: one run, never a league import"
            );
            assert_eq!(
                texts(
                    conn,
                    "SELECT course_wins || '-' || course_losses || ' ' || state FROM leagues"
                ),
                vec!["3-0 active".to_string()],
                "{label}"
            );
            assert_eq!(
                texts(
                    conn,
                    "SELECT m.token || ' ' || l.run_key FROM matches m
                     JOIN leagues l ON l.id = m.league_id ORDER BY m.token"
                ),
                vec![
                    "q-a1 arena:q-course".to_string(),
                    "q-a2 arena:q-course".to_string(),
                    "q-b1 arena:q-course".to_string(),
                ],
                "{label}: every known match in the run"
            );
            assert_eq!(
                pending(conn),
                Vec::<String>::new(),
                "{label}: nothing pending"
            );
        }
        assert_eq!(
            canonical_json(&run_bundle(&a, run_id(&a))),
            canonical_json(&run_bundle(&b, run_id(&b))),
            "byte-identical run bundles"
        );
        assert_eq!(cloud.decks.len(), 1, "the course deck");
        let run_hash = cloud.runs["arena_q-course"].0.clone();
        assert_eq!(
            run_hash,
            bundle_hash(&canonical_json(&run_bundle(&a, run_id(&a))))
        );

        // Exchanging again is a no-op: no new hash, nothing pulled.
        let before: Vec<String> = cloud.runs.values().map(|(hash, _)| hash.clone()).collect();
        assert_eq!(exchange(&mut a, &mut cloud), 0);
        assert_eq!(exchange(&mut b, &mut cloud), 0);
        let after: Vec<String> = cloud.runs.values().map(|(hash, _)| hash.clone()).collect();
        assert_eq!(before, after);
        assert_eq!(cloud.matches.len(), 3);
        for (label, conn) in [("A", &a), ("B", &b)] {
            assert_eq!(pending(conn), Vec::<String>::new(), "{label}");
        }
    }

    /// #833: a run pulled on a device that never built it reads as synced
    /// while its build here would be the server's bytes, and as waiting
    /// once something here changes it.
    #[test]
    fn a_pulled_run_never_built_here_reads_as_synced() {
        let (_a, mut a, a_account) = setup("arena-run-pulled-unbuilt-a");
        let (_b, mut b, _) = setup("arena-run-pulled-unbuilt-b");
        let mut cloud = Cloud::default();
        observe(&mut a, vec![course_at(100, 0, 0, "CreateMatch")]);
        limited_match(&a, a_account, "q-a1", 200, Some(DRAFT));
        observe(&mut a, vec![course_at(300, 1, 0, "CreateMatch")]);
        sync(&mut a, a_account, 99_000);
        enable_course_deck(&a);
        push(&mut a, &mut cloud);
        assert_eq!(pull(&mut b, &cloud), 3, "the deck, the run and its match");
        let built: Option<String> = b
            .query_row(
                "SELECT built_hash FROM synced_rows WHERE client_id = 'arena_q-course'",
                [],
                |row| row.get(0),
            )
            .expect("row");
        assert_eq!(built, None, "never built here");
        assert_eq!(pending(&b), Vec::<String>::new());

        b.execute(
            "UPDATE leagues SET notes = 'Qnew', notes_edited_at = 70",
            [],
        )
        .expect("an edit here");
        assert_eq!(pending(&b), vec!["arena_q-course".to_string()]);
    }

    /// The server's `updated_at` of a pulled run is kept as the row's time,
    /// so a merge sent later is newer than the copy it took (spec section
    /// 8). A pull of the bytes this device already builds keeps its build,
    /// so the run does not read as waiting.
    #[test]
    fn a_pulled_run_keeps_the_servers_time_and_a_same_bytes_pull_keeps_the_build() {
        let (_dir, mut conn, account_id) = setup("arena-run-server-time");
        observe(&mut conn, vec![course_at(100, 2, 1, "CreateMatch")]);
        sync(&mut conn, account_id, 99_000);
        let bundle = run_bundle(&conn, run_id(&conn));
        let hash = bundle_hash(&canonical_json(&bundle));
        let run = read_arena_run_bundle(&bundle).expect("reads").0;
        conn.execute(
            "INSERT INTO synced_rows (type, client_id, hash, synced_at, built_hash, touch_key,
                 changed_at)
             VALUES ('league', 'arena_q-course', ?1, 1, ?1, 'q-key', 5000)",
            [&hash],
        )
        .expect("a synced run");
        let pull = |hash: &str, at: i64| SyncEdit::PulledArenaRun {
            server_updated_at_ms: Some(at),
            client_id: "arena_q-course".to_string(),
            run: Box::new(run.clone()),
            hash: hash.to_string(),
            carry: "{}".to_string(),
            timezone: chrono_tz::UTC,
            now_ms: LATER,
        };
        let row = |conn: &Connection| {
            texts(
                conn,
                "SELECT hash || ' ' || COALESCE(built_hash, '-') || ' '
                     || COALESCE(touch_key, '-') || ' ' || COALESCE(changed_at, '-')
                 FROM synced_rows WHERE client_id = 'arena_q-course'",
            )
        };
        edit_sync(&mut conn, &pull(&hash, 7_000)).expect("same bytes");
        assert_eq!(row(&conn), vec![format!("{hash} {hash} q-key 7000")]);
        edit_sync(&mut conn, &pull("h-other", 8_000)).expect("other bytes");
        assert_eq!(row(&conn), vec!["h-other - - 8000".to_string()]);
    }

    #[test]
    fn phone_matches_count_in_the_record() {
        let (_dir, mut conn, account_id) = setup("arena-run-phone");
        observe(&mut conn, vec![course_at(100, 0, 0, "CreateMatch")]);
        for (token, offset) in [("q-m1", 200), ("q-m2", 300), ("q-m3", 400)] {
            limited_match(&conn, account_id, token, offset, Some(DRAFT));
        }
        observe(&mut conn, vec![course_at(500, 2, 1, "CreateMatch")]);
        sync(&mut conn, account_id, 99_000);
        assert_eq!(
            texts(
                &conn,
                "SELECT course_wins || '-' || course_losses || ' ' || state FROM leagues"
            ),
            vec!["2-1 active".to_string()]
        );

        // Another device saw the course end 5-1 after three phone matches.
        let mut phone = read_arena_run_bundle(&run_bundle(&conn, run_id(&conn)))
            .expect("reads")
            .0;
        phone.snapshot.wins = 5;
        phone.snapshot.losses = 1;
        phone.snapshot.state = "complete".to_string();
        phone.snapshot.observed_at_ms = 9_000;
        edit_sync(
            &mut conn,
            &SyncEdit::PulledArenaRun {
                server_updated_at_ms: None,
                client_id: "arena_q-course".to_string(),
                run: Box::new(phone.clone()),
                hash: "h-phone".to_string(),
                carry: "{}".to_string(),
                timezone: chrono_tz::UTC,
                now_ms: LATER,
            },
        )
        .expect("pulled run");

        assert_eq!(
            texts(
                &conn,
                "SELECT course_wins || '-' || course_losses || ' ' || state || ' '
                     || completed_at FROM leagues"
            ),
            vec!["5-1 complete 20400".to_string()],
            "completed at the later of the snapshot and the last match end"
        );
        assert_eq!(
            run_bundle(&conn, run_id(&conn))["league"]["snapshot"],
            serde_json::json!({"wins": 5, "losses": 1, "observed_at": 9_000})
        );
        assert_eq!(
            texts(
                &conn,
                "SELECT m.token FROM matches m JOIN leagues l ON l.id = m.league_id
                 ORDER BY m.token"
            ),
            vec!["q-m1".to_string(), "q-m2".to_string(), "q-m3".to_string()]
        );
        // The page read: the run shows 5-1 from three tracked matches.
        let page = page_run(&conn, account_id);
        assert_eq!(page.course_record, Some((5, 1)));
        assert_eq!(page.matches.len(), 3);
        assert_eq!(page_record(&page), (5, 1));

        // An older pulled snapshot never replaces the newer one.
        phone.snapshot.wins = 4;
        phone.snapshot.observed_at_ms = 8_000;
        edit_sync(
            &mut conn,
            &SyncEdit::PulledArenaRun {
                server_updated_at_ms: None,
                client_id: "arena_q-course".to_string(),
                run: Box::new(phone),
                hash: "h-older".to_string(),
                carry: "{}".to_string(),
                timezone: chrono_tz::UTC,
                now_ms: LATER,
            },
        )
        .expect("pulled run");
        assert_eq!(
            texts(
                &conn,
                "SELECT wins || '-' || losses || ' ' || observed_at FROM synced_arena_runs"
            ),
            vec!["5-1 9000".to_string()]
        );
        assert_eq!(
            texts(
                &conn,
                "SELECT course_wins || '-' || course_losses FROM leagues"
            ),
            vec!["5-1".to_string()]
        );
    }

    #[test]
    fn the_run_sync_makes_the_course_deck_and_links_its_matches() {
        let (_dir, mut conn, account_id) = setup("arena-run-course-deck");
        observe(&mut conn, vec![course(100, "q-course", "CreateMatch")]);
        limited_match(&conn, account_id, "q-m1", 200, Some(DRAFT));
        let mut changed = course(250, "q-course", "CreateMatch");
        changed.main = vec![(2_000_002, 2), (2_000_001, 1)];
        observe(&mut conn, vec![changed]);
        limited_match(&conn, account_id, "q-m2", 300, Some(DRAFT));
        sync(&mut conn, account_id, 99_000);

        assert_eq!(
            texts(
                &conn,
                "SELECT mtgo_id || '|' || account_id || '|' || name || '|' || format || '|'
                     || source || '|' || cloud_sync_enabled FROM decks"
            ),
            vec![format!(
                "limited:arena_q-course|{account_id}|Premier Draft \u{b7} FRA|Limited|file|0"
            )]
        );
        assert_eq!(
            texts(
                &conn,
                "SELECT m.token || ' ' || v.signature FROM matches m
                 JOIN deck_versions v ON v.id = m.deck_version_id ORDER BY m.token"
            ),
            texts(
                &conn,
                "SELECT m.token || ' ' || s.signature FROM matches m
                 JOIN limited_deck_snapshots s ON s.match_id = m.id ORDER BY m.token"
            ),
            "each match on the version of its registered deck"
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM deck_versions"), 2);
        assert_eq!(
            texts(
                &conn,
                "SELECT d.mtgo_id || ' ' || (l.deck_version_id = m.deck_version_id)
                 FROM leagues l JOIN decks d ON d.id = l.deck_id
                 JOIN matches m ON m.token = 'q-m2'"
            ),
            vec!["limited:arena_q-course 1".to_string()],
            "the run is on the deck's newest version"
        );
        let first = dump(&conn, "decks");
        sync(&mut conn, account_id, 123_000);
        assert_eq!(dump(&conn, "decks"), first, "made once");
    }

    fn course_deck_bundle(conn: &Connection) -> String {
        let candidate = deck_candidates(conn, Tier::Supporter)
            .expect("decks")
            .into_iter()
            .find(|candidate| candidate.mtgo_id == "limited:arena_q-course")
            .expect("the course deck");
        canonical_json(&deck_bundle_of(conn, &candidate))
    }

    fn records(conn: &Connection) -> Vec<String> {
        texts(
            conn,
            "SELECT course_wins || '-' || course_losses || ' ' || state FROM leagues",
        )
    }

    fn empty_snapshot(byte_offset: i64) -> CourseObservation {
        let mut marker = course(byte_offset, "", "");
        marker.event_name = String::new();
        marker.deck_id = String::new();
        marker.deck_name = String::new();
        marker.main = Vec::new();
        marker.side = Vec::new();
        marker.pool = Vec::new();
        marker
    }

    #[test]
    fn a_course_gone_from_the_set_keeps_the_newer_record_on_both_devices() {
        let (_a, mut a, a_account) = setup("arena-run-gone-a");
        let (_b, mut b, b_account) = setup("arena-run-gone-b");
        let mut cloud = Cloud::default();

        observe(&mut a, vec![course_at(100, 0, 0, "CreateMatch")]);
        limited_match(&a, a_account, "q-a1", 200, Some(DRAFT));
        observe(&mut a, vec![course_at(300, 2, 0, "CreateMatch")]);
        sync(&mut a, a_account, 99_000);
        observe(&mut b, vec![course_at(500, 4, 1, "CreateMatch")]);
        limited_match(&b, b_account, "q-b1", 600, Some(DRAFT));
        observe(&mut b, vec![course_at(700, 5, 1, "Complete")]);
        sync(&mut b, b_account, 99_000);
        // A few runs each: a device that finds the server moved pulls
        // first and sends at its next run.
        for _ in 0..3 {
            exchange(&mut a, &mut cloud);
            exchange(&mut b, &mut cloud);
        }
        assert_eq!(records(&a), vec!["5-1 complete".to_string()]);

        // A sees the course leave the set after B's record.
        observe(&mut a, vec![empty_snapshot(900)]);
        sync(&mut a, a_account, 99_500);
        for _ in 0..3 {
            exchange(&mut a, &mut cloud);
            exchange(&mut b, &mut cloud);
        }
        push(&mut a, &mut cloud);
        push(&mut b, &mut cloud);

        for (label, conn) in [("A", &a), ("B", &b)] {
            assert_eq!(records(conn), vec!["5-1 complete".to_string()], "{label}");
            assert_eq!(pending(conn), Vec::<String>::new(), "{label}");
        }
        assert_eq!(
            canonical_json(&run_bundle(&a, run_id(&a))),
            canonical_json(&run_bundle(&b, run_id(&b)))
        );
    }

    #[test]
    fn course_decks_made_on_both_devices_converge() {
        let (_a, mut a, a_account) = setup("arena-run-two-decks-a");
        let (_b, mut b, b_account) = setup("arena-run-two-decks-b");
        let mut cloud = Cloud::default();
        // Both devices play before either syncs: each makes the course deck,
        // the same list observed at different times.
        observe(&mut a, vec![course_at(100, 0, 0, "CreateMatch")]);
        limited_match(&a, a_account, "q-a1", 200, Some(DRAFT));
        observe(&mut a, vec![course_at(300, 1, 0, "CreateMatch")]);
        sync(&mut a, a_account, 99_000);
        // Seconds apart: a bundle writes a version's time to the second.
        observe(&mut b, vec![course_at(5_000, 0, 0, "CreateMatch")]);
        limited_match(&b, b_account, "q-b1", 5_100, Some(DRAFT));
        observe(&mut b, vec![course_at(5_200, 1, 1, "CreateMatch")]);
        sync(&mut b, b_account, 99_000);

        for _ in 0..3 {
            exchange(&mut a, &mut cloud);
            exchange(&mut b, &mut cloud);
        }
        push(&mut a, &mut cloud);
        push(&mut b, &mut cloud);

        for (label, conn) in [("A", &a), ("B", &b)] {
            assert_eq!(pending(conn), Vec::<String>::new(), "{label}");
            assert_eq!(records(conn), vec!["1-1 active".to_string()], "{label}");
        }
        assert_eq!(course_deck_bundle(&a), course_deck_bundle(&b));
        assert_eq!(
            canonical_json(&run_bundle(&a, run_id(&a))),
            canonical_json(&run_bundle(&b, run_id(&b)))
        );
    }

    /// A pulled copy of the run of `conn` with its notes and status edited at
    /// `at`, applied as the runner does.
    fn pull_edited_run(conn: &mut Connection, notes: &str, user_state: &str, at: i64) {
        let mut run = read_arena_run_bundle(&run_bundle(conn, run_id(conn)))
            .expect("reads")
            .0;
        run.notes = Some(notes.to_string());
        run.notes_edited_at_ms = Some(at);
        run.player.user_state = Some(user_state.to_string());
        run.player.edited_at_ms = Some(at);
        edit_sync(
            conn,
            &SyncEdit::PulledArenaRun {
                server_updated_at_ms: None,
                client_id: "arena_q-course".to_string(),
                run: Box::new(run),
                hash: format!("h-{notes}-{user_state}"),
                carry: "{}".to_string(),
                timezone: chrono_tz::UTC,
                now_ms: LATER,
            },
        )
        .expect("pulled run");
    }

    #[test]
    fn edits_tied_in_time_settle_on_one_value() {
        let (_dir, mut conn, account_id) = setup("arena-run-tied-edits");
        observe(&mut conn, vec![course_at(100, 0, 0, "CreateMatch")]);
        limited_match(&conn, account_id, "q-m1", 200, Some(DRAFT));
        sync(&mut conn, account_id, 99_000);
        conn.execute(
            "UPDATE leagues SET notes = 'Qb', notes_edited_at = 50, user_state = 'dropped',
                 edited_at = 50",
            [],
        )
        .expect("local edits");
        let shown =
            |conn: &Connection| texts(conn, "SELECT notes || ' ' || user_state FROM leagues");
        // A smaller value at the same time never replaces the local one...
        pull_edited_run(&mut conn, "Qa", "complete", 50);
        assert_eq!(shown(&conn), vec!["Qb dropped".to_string()]);
        // ...a greater one does, so both devices end on the same value.
        pull_edited_run(&mut conn, "Qc", "partial", 50);
        assert_eq!(shown(&conn), vec!["Qc partial".to_string()]);
    }

    #[test]
    fn a_pulled_match_moves_its_run_to_the_account_that_played_it() {
        let (_a, mut a, a_account) = setup("arena-run-move-a");
        observe(&mut a, vec![course_at(100, 0, 0, "CreateMatch")]);
        limited_match(&a, a_account, "q-a1", 200, Some(DRAFT));
        observe(&mut a, vec![course_at(300, 1, 0, "CreateMatch")]);
        sync(&mut a, a_account, 99_000);
        enable_course_deck(&a);

        // B holds the run under its other Arena account, the current one
        // when the run came before its match.
        let (_b, mut b, b_account) = setup("arena-run-move-b");
        let other: i64 = b
            .query_row(
                "INSERT INTO accounts (username, first_seen_at, platform, arena_user_id)
                 VALUES ('Qother#99999', 0, 'arena', 'QUSERID2') RETURNING id",
                [],
                |row| row.get(0),
            )
            .expect("other account");
        b.execute(
            "INSERT INTO synced_arena_runs (account_id, course_id, event_name, kind, wins,
                 losses, state, observed_at)
             VALUES (?1, 'q-course', ?2, 'draft', 1, 0, 'active', 1300)",
            params![other, DRAFT],
        )
        .expect("run under the other account");
        sync(&mut b, other, 99_000);
        assert_eq!(
            texts(&b, "SELECT '' || account_id FROM leagues"),
            vec![other.to_string()]
        );
        // #828: the player's notes and status on that row move with it.
        b.execute(
            "UPDATE leagues SET notes = 'Qnote', notes_edited_at = 40, user_state = 'dropped',
                 user_dropped_at = 45, edited_at = 50",
            [],
        )
        .expect("player edits");

        let mut cloud = Cloud::default();
        push(&mut a, &mut cloud);
        let (hash, pulled) = cloud.matches["q-a1"].clone();
        edit_sync(
            &mut b,
            &SyncEdit::PulledMatch {
                client_id: "q-a1".to_string(),
                pulled: Box::new(pulled),
                hash,
                carry: "{}".to_string(),
                timezone: chrono_tz::UTC,
                now_ms: LATER,
            },
        )
        .expect("pulled match");
        after_pull(&mut b);

        assert_eq!(
            texts(&b, "SELECT '' || account_id FROM synced_arena_runs"),
            vec![b_account.to_string()]
        );
        assert_eq!(
            texts(
                &b,
                "SELECT l.account_id || ' ' || m.token FROM leagues l
                 JOIN matches m ON m.league_id = l.id"
            ),
            vec![format!("{b_account} q-a1")]
        );
        assert_eq!(
            texts(
                &b,
                "SELECT notes || ' ' || notes_edited_at || ' ' || user_state || ' '
                     || user_dropped_at || ' ' || edited_at FROM leagues"
            ),
            vec!["Qnote 40 dropped 45 50".to_string()]
        );
    }

    // Review fix (#828): the run's row itself moves (id, notes, status and
    // their times) before any fold, so a target fold that fails loses
    // nothing.
    #[test]
    fn a_moved_run_keeps_its_row_when_the_targets_fold_fails() {
        let (_dir, mut conn, target) = setup("arena-run-move-row");
        let other: i64 = conn
            .query_row(
                "INSERT INTO accounts (username, first_seen_at, platform, arena_user_id)
                 VALUES ('Qother#99999', 0, 'arena', 'QUSERID2') RETURNING id",
                [],
                |row| row.get(0),
            )
            .expect("other account");
        conn.execute(
            "INSERT INTO synced_arena_runs (account_id, course_id, event_name, kind, wins,
                 losses, state, observed_at)
             VALUES (?1, 'q-course', ?2, 'draft', 1, 0, 'active', 1300)",
            params![other, DRAFT],
        )
        .expect("run under the other account");
        sync(&mut conn, other, 99_000);
        conn.execute(
            "UPDATE leagues SET notes = 'Qnote', notes_edited_at = 40, user_state = 'dropped',
                 user_dropped_at = 45, edited_at = 50",
            [],
        )
        .expect("player edits");
        let id = run_id(&conn);
        // The target's fold fails: a stored observation does not read back.
        conn.execute(
            "INSERT INTO arena_course_observations (arena_log_file_id, byte_offset,
                 observed_at, account_id, course_id, event_name, module, wins, losses,
                 deck_id, deck_name, cards)
             VALUES (1, 250, 1, ?1, 'q-broken', ?2, 'CreateMatch', 0, 0, '', '', 'not json')",
            params![target, DRAFT],
        )
        .expect("broken row");

        let tx = conn.transaction().expect("tx");
        move_pulled_run_to(&tx, target, "q-course", 99_100).expect("move");
        sync_arena_runs_guarded(&tx, target, 99_200).expect("a failed fold is skipped");
        tx.commit().expect("commit");
        assert_eq!(
            texts(
                &conn,
                "SELECT id || ' ' || account_id || ' ' || notes || ' ' || notes_edited_at || ' '
                     || user_state || ' ' || user_dropped_at || ' ' || edited_at FROM leagues"
            ),
            vec![format!("{id} {target} Qnote 40 dropped 45 50")]
        );
    }

    /// The account's one limited run as the Limited pages read it.
    fn page_run(conn: &Connection, account_id: i64) -> crate::leagues::LimitedRun {
        use crate::league_sync::LeagueOwner;
        use crate::leagues::{RunFilter, stored_limited_runs};
        let mut runs = stored_limited_runs(
            conn,
            &RunFilter {
                owners: vec![LeagueOwner::Account(account_id)],
                kinds: Vec::new(),
                deck_id: None,
                run_key: None,
            },
        )
        .expect("read")
        .runs;
        assert_eq!(runs.len(), 1);
        runs.remove(0)
    }

    /// The record a run page shows: its complete matches, raised to the
    /// course's record.
    fn page_record(run: &crate::leagues::LimitedRun) -> (u32, u32) {
        let mut tracked = (0, 0);
        for row in &run.matches {
            let played = row.run_match();
            if played.state != mtgo_match::MatchState::Complete {
                continue;
            }
            match played.outcome {
                mtgo_match::Outcome::Win => tracked.0 += 1,
                mtgo_match::Outcome::Loss => tracked.1 += 1,
                _ => {}
            }
        }
        leagues::shown_record(run.course_record, tracked)
    }

    #[test]
    fn a_wrong_match_taken_out_lets_the_course_record_show_again() {
        let (_dir, mut conn, account_id) = setup("arena-run-rectify");
        observe(&mut conn, vec![course_at(100, 0, 0, "CreateMatch")]);
        limited_match(&conn, account_id, "q-m1", 200, Some(DRAFT));
        observe(&mut conn, vec![course_at(300, 2, 0, "CreateMatch")]);
        sync(&mut conn, account_id, 99_000);
        // A loss of another entry wrongly put in the run by hand.
        limited_match(&conn, account_id, "q-wrong", 400, Some(DRAFT));
        conn.execute(
            "UPDATE matches SET outcome = 'loss', league_linked = 1,
                 league_id = (SELECT id FROM leagues) WHERE token = 'q-wrong'",
            [],
        )
        .expect("wrong link");
        sync(&mut conn, account_id, 99_100);
        assert_eq!(page_record(&page_run(&conn, account_id)), (2, 1));

        // The player takes it out of every league: the run is 2-0 again.
        conn.execute(
            "UPDATE matches SET league_id = NULL, league_linked = 0, no_league = 1
             WHERE token = 'q-wrong'",
            [],
        )
        .expect("taken out");
        sync(&mut conn, account_id, 99_200);
        let page = page_run(&conn, account_id);
        assert_eq!(page.matches.len(), 1);
        assert_eq!(page_record(&page), (2, 0));

        // The player's status still wins over the course's state.
        conn.execute("UPDATE leagues SET user_state = 'dropped'", [])
            .expect("status");
        assert_eq!(page_run(&conn, account_id).shown_state(), RunState::Dropped);
    }

    fn number(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |row| row.get(0)).expect("number")
    }
}
