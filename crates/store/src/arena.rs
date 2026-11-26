//! Arena store writes (Arena spec section 7): the Arena log files and their
//! decoded events (facts, never edited or deleted), Arena accounts and
//! opponents in the shared registries (told apart by `platform`), and rank
//! observations.
//!
//! Hashes are `u64`; `arena_log_files` keeps them in `INTEGER` columns, so
//! they are stored bit for bit as `i64` and read back the same way.

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::write::{decode_raw, encode_raw};
use crate::{Platform, StoreError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewArenaLogFile {
    pub path: String,
    pub identity_hash: u64,
    pub head_hash: u64,
    pub head_len: u64,
    pub created_at_fs: Option<i64>,
    pub seen_at_ms: i64,
}

/// An unsealed Arena log file and how far it has been read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArenaLogFileRow {
    pub id: i64,
    pub path: String,
    pub head_hash: u64,
    pub head_len: u64,
    pub created_at_fs: Option<i64>,
    pub last_observed_size: u64,
    pub read_offset: u64,
}

/// Where an Arena file's reading has got to. Committed with the events it
/// covers. `head_hash` and `head_len` follow the file's head while it grows
/// to its full length, so the file is known again (as `Player-prev.log`, or
/// after a restart) by its whole head. `sealed` is `(sealed_at ms, reason)`;
/// a file already sealed keeps its first seal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArenaFileProgress {
    pub id: i64,
    pub last_observed_size: u64,
    pub read_offset: u64,
    pub head_hash: u64,
    pub head_len: u64,
    pub sealed: Option<(i64, String)>,
}

/// One decoded Arena event. One log entry can decode into several events:
/// `seq` is the event's index within its entry. `raw` is the decoded event
/// serialised as JSON by the tracker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewArenaEvent {
    pub file_id: i64,
    pub byte_offset: u64,
    pub seq: u32,
    pub kind: &'static str,
    pub match_token: Option<String>,
    pub logged_at_ms: i64,
    pub raw: String,
}

/// A stored Arena event, for rebuild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredArenaEvent {
    pub file_id: i64,
    pub byte_offset: u64,
    pub seq: u32,
    pub kind: String,
    pub logged_at_ms: i64,
    pub raw: String,
}

/// A local account of a platform with accounts of its own (Arena,
/// Endstep). An Arena `username` is the login's `Name#12345`, or
/// `arena:<user id>` (untagged); an Endstep one is `endstep:<user id>`.
/// `user_id` is `None` until the log names it (an Arena match can name the
/// account by its login before that); an untagged name carries its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformAccount {
    pub platform: Platform,
    pub username: String,
    pub user_id: Option<String>,
    /// The untagged screen name of an account known only by its user id.
    pub display_name: Option<String>,
    pub seen_at_ms: i64,
}

/// An opponent on a platform with accounts of its own: `username` is
/// already the `arena:<user id>` (or `endstep:<user id>`) key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformOpponent {
    pub platform: Platform,
    pub username: String,
    pub display_name: String,
}

/// One rank reading of one ladder (`"constructed"` or `"limited"`).
#[derive(Debug, Clone, PartialEq)]
pub struct RankObservation {
    pub account_username: String,
    /// The account's user id when the login named it: the reading is stored
    /// against the row holding it, so a rename later in the same batch
    /// cannot drop it (#862).
    pub account_user_id: Option<String>,
    pub ladder: &'static str,
    pub season: u32,
    pub observed_at_ms: i64,
    pub class: String,
    pub level: u32,
    pub step: u32,
    pub won: u32,
    pub lost: u32,
    pub mythic_percentile: Option<f64>,
    pub mythic_placement: Option<u32>,
    pub file_id: i64,
    pub byte_offset: u64,
}

/// A new unsealed `arena_log_files` row, read from the start.
pub fn insert_arena_log_file(conn: &Connection, file: &NewArenaLogFile) -> Result<i64, StoreError> {
    conn.execute(
        "INSERT INTO arena_log_files (path, identity_hash, head_hash, head_len, created_at_fs,
             first_seen_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            file.path,
            file.identity_hash as i64,
            file.head_hash as i64,
            file.head_len as i64,
            file.created_at_fs,
            file.seen_at_ms,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// The unsealed Arena log files, oldest first.
pub fn active_arena_log_files(conn: &Connection) -> Result<Vec<ArenaLogFileRow>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, path, head_hash, head_len, created_at_fs, last_observed_size,
             read_offset
         FROM arena_log_files WHERE sealed_at IS NULL ORDER BY id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ArenaLogFileRow {
            id: row.get(0)?,
            path: row.get(1)?,
            head_hash: row.get::<_, i64>(2)? as u64,
            head_len: row.get::<_, i64>(3)? as u64,
            created_at_fs: row.get(4)?,
            last_observed_size: row.get::<_, i64>(5)? as u64,
            read_offset: row.get::<_, i64>(6)? as u64,
        })
    })?;
    let mut files = Vec::new();
    for row in rows {
        files.push(row?);
    }
    Ok(files)
}

/// The furthest any Arena file starting with this head (sealed or not) was
/// read, or `None` when no file has it. `Player-prev.log` is the previous
/// `Player.log` kept aside: this says where its unread part starts.
pub fn arena_read_offset_for_head(
    conn: &Connection,
    head_hash: u64,
    head_len: u64,
) -> Result<Option<u64>, StoreError> {
    let offset: Option<i64> = conn.query_row(
        "SELECT MAX(read_offset) FROM arena_log_files WHERE head_hash = ?1 AND head_len = ?2",
        params![head_hash as i64, head_len as i64],
        |row| row.get(0),
    )?;
    Ok(offset.map(|offset| offset as u64))
}

/// The `arena_log_files` row of a file starting with this head: the unsealed
/// one if any (its messages then share keys with what the tailer reads),
/// else the first. `None` when no file had it. Game messages found again in
/// a log at start are stored under this id.
pub fn arena_file_for_head(
    conn: &Connection,
    head_hash: u64,
    head_len: u64,
) -> Result<Option<i64>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT id FROM arena_log_files WHERE head_hash = ?1 AND head_len = ?2
             ORDER BY sealed_at IS NOT NULL, id LIMIT 1",
            params![head_hash as i64, head_len as i64],
            |row| row.get(0),
        )
        .optional()?)
}

/// Every stored Arena event in log order (file id, offset, seq), for rebuild.
pub fn arena_events_in_order(conn: &Connection) -> Result<Vec<StoredArenaEvent>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT arena_log_file_id, byte_offset, seq, kind, logged_at, raw, compressed
         FROM arena_log_events ORDER BY arena_log_file_id, byte_offset, seq",
    )?;
    let mut rows = stmt.query([])?;
    let mut events = Vec::new();
    while let Some(row) = rows.next()? {
        let raw: Vec<u8> = row.get(5)?;
        let compressed: bool = row.get(6)?;
        let file_id: i64 = row.get(0)?;
        let byte_offset = row.get::<_, i64>(1)? as u64;
        // One unreadable row must not stop the tracker from opening.
        let raw = match decode_raw(raw, compressed) {
            Ok(raw) => raw,
            Err(error) => {
                tracing::warn!(file_id, byte_offset, %error, "a stored Arena event did not decode; skipped");
                continue;
            }
        };
        events.push(StoredArenaEvent {
            file_id,
            byte_offset,
            seq: row.get(2)?,
            kind: row.get(3)?,
            logged_at_ms: row.get(4)?,
            raw,
        });
    }
    Ok(events)
}

pub(crate) fn update_arena_file(
    tx: &Transaction,
    file: &ArenaFileProgress,
) -> Result<(), StoreError> {
    let (sealed_at, reason) = match &file.sealed {
        Some((at_ms, reason)) => (Some(*at_ms), Some(reason.as_str())),
        None => (None, None),
    };
    tx.prepare_cached(
        "UPDATE arena_log_files SET last_observed_size = ?2, read_offset = ?3,
             head_hash = ?6, head_len = ?7,
             seal_reason = CASE WHEN sealed_at IS NULL THEN ?5 ELSE seal_reason END,
             sealed_at = COALESCE(sealed_at, ?4)
         WHERE id = ?1",
    )?
    .execute(params![
        file.id,
        file.last_observed_size as i64,
        file.read_offset as i64,
        sealed_at,
        reason,
        file.head_hash as i64,
        file.head_len as i64,
    ])?;
    Ok(())
}

/// A replayed event (same file, offset and seq) is skipped; a CHECK or
/// foreign key violation still fails the batch.
pub(crate) fn insert_arena_event(
    tx: &Transaction,
    event: &NewArenaEvent,
) -> Result<(), StoreError> {
    let (raw, compressed) = encode_raw(&event.raw)?;
    tx.prepare_cached(
        "INSERT INTO arena_log_events (arena_log_file_id, byte_offset, seq, kind, match_token,
             logged_at, raw, compressed)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (arena_log_file_id, byte_offset, seq) DO NOTHING",
    )?
    .execute(params![
        event.file_id,
        event.byte_offset as i64,
        event.seq,
        event.kind,
        event.match_token,
        event.logged_at_ms,
        raw,
        compressed,
    ])?;
    Ok(())
}

/// The platform and user id of an untagged account name: `arena:<user id>`
/// (an Arena account seen only through fast logins with no tagged name,
/// trap 29) or `endstep:<user id>` (every Endstep account). `None` for any
/// other name.
fn untagged_user_id(username: &str) -> Option<(Platform, &str)> {
    if let Some(user_id) = username.strip_prefix("arena:") {
        return Some((Platform::Arena, user_id));
    }
    if let Some(user_id) = username.strip_prefix("endstep:") {
        return Some((Platform::Endstep, user_id));
    }
    None
}

/// The username a platform's rows are stored against for `username`: an
/// untagged name is the account of its platform holding its user id,
/// whatever a login named it; any other name, or an untagged one no account
/// holds, is itself.
pub fn platform_account_username(conn: &Connection, username: &str) -> Result<String, StoreError> {
    let Some((platform, user_id)) = untagged_user_id(username) else {
        return Ok(username.to_string());
    };
    let held: Option<String> = conn
        .prepare_cached("SELECT username FROM accounts WHERE arena_user_id = ?1 AND platform = ?2")?
        .query_row(params![user_id, platform], |row| row.get(0))
        .optional()?;
    Ok(held.unwrap_or_else(|| username.to_string()))
}

/// Upserts on `username`, rows of the account's platform only: a name an
/// account of another platform holds is never turned into this one's (the
/// row is not written, with a warning). An account written with no user id
/// keeps any id it has; an untagged name's user id is in the name. A rename
/// (the same user id under a new name) renames the row that holds the id,
/// when no other row holds the new name. An untagged name for an account
/// stored under a tagged one writes nothing: a tagged name is never
/// replaced by an untagged one. The user id (`arena_user_id`) is unique: an
/// id another account still holds is left out (the log is hostile input),
/// so the batch never fails on it. Never sets `last_login_at` (the MTGO
/// current account rule). A display name is kept when an account is written
/// without one, and a rename takes the new name's (none for a tagged name).
pub(crate) fn upsert_platform_account(
    tx: &Transaction,
    account: &PlatformAccount,
) -> Result<(), StoreError> {
    if platform_account_username(tx, &account.username)? != account.username {
        return Ok(());
    }
    let user_id = account.user_id.as_deref().or_else(|| {
        untagged_user_id(&account.username)
            .filter(|(platform, _)| *platform == account.platform)
            .map(|(_, user_id)| user_id)
    });
    if let Some(user_id) = user_id {
        merge_renamed_account(tx, account, user_id)?;
    }
    tx.prepare_cached(
        "UPDATE accounts SET username = ?1, display_name = ?3
         WHERE arena_user_id = ?2 AND platform = ?4 AND username <> ?1
           AND NOT EXISTS (SELECT 1 FROM accounts WHERE username = ?1)",
    )?
    .execute(params![
        account.username,
        user_id,
        account.display_name,
        account.platform
    ])?;
    let written = tx
        .prepare_cached(
            "INSERT INTO accounts (username, first_seen_at, platform, arena_user_id, display_name)
             VALUES (?1, ?3, ?5, (SELECT ?2 WHERE NOT EXISTS
                         (SELECT 1 FROM accounts WHERE arena_user_id = ?2 AND username <> ?1)), ?4)
             ON CONFLICT (username) DO UPDATE SET
                 arena_user_id = COALESCE(excluded.arena_user_id, accounts.arena_user_id),
                 display_name = COALESCE(excluded.display_name, accounts.display_name)
             WHERE accounts.platform = ?5",
        )?
        .execute(params![
            account.username,
            user_id,
            account.seen_at_ms,
            account.display_name,
            account.platform
        ])?;
    if written == 0 {
        tracing::warn!(
            platform = account.platform.as_str(),
            "account not stored: an account of another platform holds its username"
        );
    }
    Ok(())
}

/// The id of the local account `username` of `platform`, made when it is
/// not stored: the account a match or deck write names. MTGO's is the
/// registry row (made with the default platform, as always); an Arena or
/// Endstep name no account holds is made on its own platform, never as an
/// MTGO account (#862), the way the source writes an account it knows only
/// by name (no user id: a later login naming it with its user id merges it,
/// #538). A name an account of another platform holds stays that row.
pub(crate) fn local_account_id(
    tx: &Transaction,
    username: &str,
    platform: Platform,
    now_ms: i64,
) -> Result<i64, StoreError> {
    let username = platform_account_username(tx, username)?;
    if platform == Platform::Mtgo {
        return crate::write::registry_id(tx, "accounts", &username, now_ms);
    }
    let held: Option<i64> = tx
        .prepare_cached("SELECT id FROM accounts WHERE username = ?1")?
        .query_row([&username], |row| row.get(0))
        .optional()?;
    if let Some(id) = held {
        return Ok(id);
    }
    tracing::warn!(
        platform = platform.as_str(),
        "a match or deck names an account that is not stored: it is made"
    );
    upsert_platform_account(
        tx,
        &PlatformAccount {
            platform,
            username: username.clone(),
            user_id: None,
            display_name: None,
            seen_at_ms: now_ms,
        },
    )?;
    crate::write::registry_id(tx, "accounts", &username, now_ms)
}

/// A rename seen before authentication (#538): the new name arrived with no
/// user id and got a row of its own, and now `account` names it with the
/// user id an older row of the platform holds. The new name's row is merged
/// into the row holding the id: a pulled run record of a course both hold
/// is joined first (`merge_pulled_runs`), then every row that points at an
/// account moves over (each foreign key to `accounts`, found from the
/// schema), as does the platform's account switch, and the new name's row is
/// deleted, so the rename that follows finds its name free. The older row
/// keeps its id and the earlier first sighting, takes the player's tracking
/// choice from either row, and its runs are folded again (at the login's
/// own time, so live equals rebuild). A new name's row that holds a user id
/// of its own is another account and is left alone. A move that would leave
/// rows under the new name's row (a unique key the merge does not join) is
/// undone and logged: the two rows stay, never failing the batch.
fn merge_renamed_account(
    tx: &Transaction,
    account: &PlatformAccount,
    user_id: &str,
) -> Result<(), StoreError> {
    let holder: Option<i64> = tx
        .prepare_cached(
            "SELECT id FROM accounts
             WHERE arena_user_id = ?1 AND platform = ?2 AND username <> ?3",
        )?
        .query_row(
            params![user_id, account.platform, account.username],
            |row| row.get(0),
        )
        .optional()?;
    let duplicate: Option<i64> = tx
        .prepare_cached(
            "SELECT id FROM accounts
             WHERE username = ?1 AND platform = ?2 AND arena_user_id IS NULL",
        )?
        .query_row(params![account.username, account.platform], |row| {
            row.get(0)
        })
        .optional()?;
    let (Some(holder), Some(duplicate)) = (holder, duplicate) else {
        return Ok(());
    };
    tx.execute_batch("SAVEPOINT account_merge")?;
    let left = move_account_rows(tx, account.platform, duplicate, holder)?;
    if left > 0 {
        tx.execute_batch("ROLLBACK TO account_merge; RELEASE account_merge")?;
        tracing::warn!(
            platform = account.platform.as_str(),
            left,
            "a renamed account was not merged: rows would be left under its new name"
        );
        return Ok(());
    }
    tx.prepare_cached(
        "UPDATE accounts SET
             first_seen_at = (SELECT MIN(first_seen_at) FROM accounts WHERE id IN (?1, ?2)),
             last_login_at = (SELECT MAX(last_login_at) FROM accounts WHERE id IN (?1, ?2)),
             tracked = CASE
                 WHEN EXISTS (SELECT 1 FROM accounts WHERE id IN (?1, ?2)
                              AND tracked_chosen = 1 AND tracked = 0) THEN 0
                 WHEN EXISTS (SELECT 1 FROM accounts WHERE id IN (?1, ?2)
                              AND tracked_chosen = 1) THEN 1
                 ELSE tracked END,
             tracked_chosen = (SELECT MAX(tracked_chosen) FROM accounts WHERE id IN (?1, ?2))
         WHERE id = ?1",
    )?
    .execute(params![holder, duplicate])?;
    tx.prepare_cached("DELETE FROM accounts WHERE id = ?1")?
        .execute([duplicate])?;
    tx.execute_batch("RELEASE account_merge")?;
    if account.platform == Platform::Arena {
        crate::arena_run_sync::sync_arena_runs_guarded(tx, holder, account.seen_at_ms)?;
    }
    Ok(())
}

/// Move every row that points at account `from` onto `to` (each foreign
/// key to `accounts`, found from the schema), joining pulled run records of
/// one course first, and the platform's switch. Returns how many rows are
/// still under `from`.
fn move_account_rows(
    tx: &Transaction,
    platform: Platform,
    from: i64,
    to: i64,
) -> Result<i64, StoreError> {
    crate::arena_run_sync::merge_pulled_runs(tx, from, to)?;
    let references: Vec<(String, String)> = tx
        .prepare_cached(
            "SELECT m.name, f.\"from\" FROM sqlite_master m
             JOIN pragma_foreign_key_list(m.name) f
             WHERE m.type = 'table' AND f.\"table\" = 'accounts'
             ORDER BY m.name, f.\"from\"",
        )?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let mut left = 0;
    for (table, column) in references {
        // OR IGNORE: a row a unique key keeps under `from` is counted below.
        tx.execute(
            &format!("UPDATE OR IGNORE \"{table}\" SET \"{column}\" = ?1 WHERE \"{column}\" = ?2"),
            params![to, from],
        )?;
        let count: i64 = tx.query_row(
            &format!("SELECT COUNT(*) FROM \"{table}\" WHERE \"{column}\" = ?1"),
            [from],
            |row| row.get(0),
        )?;
        left += count;
    }
    crate::accounts::move_switch(tx, platform, from, to)?;
    Ok(left)
}

/// Upserts on `username`, rows of the opponent's platform only: an opponent
/// of another platform holding the name is left alone (the row is not
/// written, with a warning).
pub(crate) fn upsert_platform_opponent(
    tx: &Transaction,
    opponent: &PlatformOpponent,
    now_ms: i64,
) -> Result<(), StoreError> {
    let written = tx
        .prepare_cached(
            "INSERT INTO opponents (username, first_seen_at, platform, display_name)
             VALUES (?1, ?2, ?4, ?3)
             ON CONFLICT (username) DO UPDATE SET display_name = excluded.display_name
             WHERE opponents.platform = ?4",
        )?
        .execute(params![
            opponent.username,
            now_ms,
            opponent.display_name,
            opponent.platform
        ])?;
    if written == 0 {
        tracing::warn!(
            platform = opponent.platform.as_str(),
            "opponent not stored: an opponent of another platform holds its username"
        );
    }
    Ok(())
}

/// The opponent a manual Arena match names (the player knows only the
/// shown name, not the user id): `arena:name:<name>`, an Arena row shown as
/// `name`. Upserted like a tracked Arena opponent; the row's id.
pub fn named_arena_opponent(tx: &Transaction, name: &str, now_ms: i64) -> Result<i64, StoreError> {
    let username = format!("arena:name:{name}");
    upsert_platform_opponent(
        tx,
        &PlatformOpponent {
            platform: Platform::Arena,
            username: username.clone(),
            display_name: name.to_string(),
        },
        now_ms,
    )?;
    Ok(tx.query_row(
        "SELECT id FROM opponents WHERE username = ?1",
        [&username],
        |row| row.get(0),
    )?)
}

/// Stored against the Arena account holding `account_user_id`, else the
/// one named `account_username`: the batch writes every account before any
/// rank, so a rename later in the batch has already moved the name (#862),
/// while the user id stays on the row. A replayed reading (same file,
/// offset and ladder) is skipped. With no such Arena account the reading is
/// left out with a warning.
pub(crate) fn insert_rank_observation(
    tx: &Transaction,
    rank: &RankObservation,
) -> Result<(), StoreError> {
    let Some(account_id) = rank_account_id(tx, rank)? else {
        tracing::warn!(
            file_id = rank.file_id,
            byte_offset = rank.byte_offset,
            "rank observation skipped: its Arena account is not stored"
        );
        return Ok(());
    };
    tx.prepare_cached(
        "INSERT INTO arena_rank_observations (account_id, ladder, season, observed_at, class,
             level, step, matches_won, matches_lost, mythic_percentile, mythic_placement,
             arena_log_file_id, byte_offset)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT (arena_log_file_id, byte_offset, ladder) DO NOTHING",
    )?
    .execute(params![
        account_id,
        rank.ladder,
        rank.season,
        rank.observed_at_ms,
        rank.class,
        rank.level,
        rank.step,
        rank.won,
        rank.lost,
        rank.mythic_percentile,
        rank.mythic_placement,
        rank.file_id,
        rank.byte_offset as i64,
    ])?;
    Ok(())
}

/// The Arena account a rank reading belongs to: the row holding its user
/// id, else the row under its name.
fn rank_account_id(tx: &Transaction, rank: &RankObservation) -> Result<Option<i64>, StoreError> {
    if let Some(user_id) = &rank.account_user_id {
        let held: Option<i64> = tx
            .prepare_cached(
                "SELECT id FROM accounts WHERE arena_user_id = ?1 AND platform = 'arena'",
            )?
            .query_row([user_id], |row| row.get(0))
            .optional()?;
        if held.is_some() {
            return Ok(held);
        }
    }
    let username = platform_account_username(tx, &rank.account_username)?;
    Ok(tx
        .prepare_cached("SELECT id FROM accounts WHERE username = ?1 AND platform = 'arena'")?
        .query_row([&username], |row| row.get(0))
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::db;
    use crate::write::{COMPRESS_OVER, WriteBatch, commit};

    fn new_file(path: &str) -> NewArenaLogFile {
        NewArenaLogFile {
            path: path.to_string(),
            identity_hash: u64::MAX,
            head_hash: 7,
            head_len: 100,
            created_at_fs: Some(5),
            seen_at_ms: 10,
        }
    }

    fn event(file_id: i64, byte_offset: u64, seq: u32, raw: &str) -> NewArenaEvent {
        NewArenaEvent {
            file_id,
            byte_offset,
            seq,
            kind: "game_info",
            match_token: Some("q-match".to_string()),
            logged_at_ms: 1_000 + byte_offset as i64,
            raw: raw.to_string(),
        }
    }

    fn rank(account_username: &str, byte_offset: u64) -> RankObservation {
        RankObservation {
            account_username: account_username.to_string(),
            account_user_id: None,
            ladder: "constructed",
            season: 42,
            observed_at_ms: 5_000,
            class: "Mythic".to_string(),
            level: 1,
            step: 0,
            won: 12,
            lost: 7,
            mythic_percentile: Some(87.5),
            mythic_placement: Some(1234),
            file_id: 1,
            byte_offset,
        }
    }

    fn commit_batch(conn: &mut Connection, batch: WriteBatch) {
        commit(conn, &batch).expect("commit");
    }

    #[test]
    fn an_arena_log_file_round_trips_and_a_seal_retires_it() {
        let (_dir, mut conn) = db("arena-file");
        let id = insert_arena_log_file(&conn, &new_file("/arena/Player.log")).expect("file");
        let files = active_arena_log_files(&conn).expect("read");
        assert_eq!(
            files,
            vec![ArenaLogFileRow {
                id,
                path: "/arena/Player.log".to_string(),
                head_hash: 7,
                head_len: 100,
                created_at_fs: Some(5),
                last_observed_size: 0,
                read_offset: 0,
            }]
        );

        let progress = |sealed| ArenaFileProgress {
            id,
            last_observed_size: 900,
            read_offset: 800,
            head_hash: 9,
            head_len: 4096,
            sealed,
        };
        commit_batch(
            &mut conn,
            WriteBatch {
                arena_files: vec![progress(None)],
                ..WriteBatch::default()
            },
        );
        let files = active_arena_log_files(&conn).expect("read");
        assert_eq!(
            (files[0].last_observed_size, files[0].read_offset),
            (900, 800)
        );
        assert_eq!(
            (files[0].head_hash, files[0].head_len),
            (9, 4096),
            "the grown head is saved"
        );

        commit_batch(
            &mut conn,
            WriteBatch {
                arena_files: vec![progress(Some((50, "truncated".to_string())))],
                ..WriteBatch::default()
            },
        );
        commit_batch(
            &mut conn,
            WriteBatch {
                arena_files: vec![progress(Some((60, "head_changed".to_string())))],
                ..WriteBatch::default()
            },
        );
        assert!(active_arena_log_files(&conn).expect("read").is_empty());
        let seal: (i64, String) = conn
            .query_row(
                "SELECT sealed_at, seal_reason FROM arena_log_files WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("seal");
        assert_eq!(seal, (50, "truncated".to_string()), "the first seal holds");
    }

    #[test]
    fn arena_event_replay_is_ignored() {
        let (_dir, mut conn) = db("arena-events");
        let file = insert_arena_log_file(&conn, &new_file("/arena/Player.log")).expect("file");
        let long = "x".repeat(COMPRESS_OVER + 1);
        let batch = WriteBatch {
            arena_events: vec![
                event(file, 40, 1, "{\"second\":1}"),
                event(file, 40, 0, "{\"first\":1}"),
                event(file, 10, 0, &long),
            ],
            ..WriteBatch::default()
        };
        commit_batch(&mut conn, batch.clone());
        // The same (file, offset, seq) again: one row each, the first kept.
        let mut replay = batch;
        replay.arena_events[0].raw = "{\"changed\":1}".to_string();
        commit_batch(&mut conn, replay);

        let events = arena_events_in_order(&conn).expect("read");
        let keys: Vec<(i64, u64, u32, &str)> = events
            .iter()
            .map(|e| (e.file_id, e.byte_offset, e.seq, e.raw.as_str()))
            .collect();
        assert_eq!(
            keys,
            vec![
                (file, 10, 0, long.as_str()),
                (file, 40, 0, "{\"first\":1}"),
                (file, 40, 1, "{\"second\":1}"),
            ]
        );
        assert_eq!(events[0].kind, "game_info");
        assert_eq!(events[0].logged_at_ms, 1_010);
        let compressed: i64 = conn
            .query_row(
                "SELECT compressed FROM arena_log_events WHERE byte_offset = 10",
                [],
                |row| row.get(0),
            )
            .expect("row");
        assert_eq!(compressed, 1, "a long raw is compressed");
    }

    #[test]
    fn a_corrupt_stored_event_is_skipped_not_fatal() {
        let (_dir, mut conn) = db("arena-event-corrupt");
        let file = insert_arena_log_file(&conn, &new_file("/arena/Player.log")).expect("file");
        let batch = WriteBatch {
            arena_events: vec![
                event(file, 10, 0, "{\"good\":1}"),
                event(file, 20, 0, "{\"bad\":1}"),
            ],
            ..WriteBatch::default()
        };
        commit_batch(&mut conn, batch);
        // Marked compressed but not zstd: the row cannot be decoded.
        conn.execute(
            "UPDATE arena_log_events SET raw = X'0102', compressed = 1 WHERE byte_offset = 20",
            [],
        )
        .expect("corrupt");

        let events = arena_events_in_order(&conn).expect("a bad row does not fail the read");
        let offsets: Vec<u64> = events.iter().map(|e| e.byte_offset).collect();
        assert_eq!(offsets, vec![10]);
    }

    #[test]
    fn an_unknown_arena_event_kind_fails_the_batch() {
        let (_dir, mut conn) = db("arena-event-kind");
        let file = insert_arena_log_file(&conn, &new_file("/arena/Player.log")).expect("file");
        let mut bad = event(file, 0, 0, "{}");
        bad.kind = "nonsense";
        let result = commit(
            &mut conn,
            &WriteBatch {
                arena_events: vec![bad],
                ..WriteBatch::default()
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn arena_account_upsert_sets_platform_and_user_id() {
        let (_dir, mut conn) = db("arena-account");
        let account = |username: &str, user_id: &str| PlatformAccount {
            platform: Platform::Arena,
            username: username.to_string(),
            user_id: Some(user_id.to_string()),
            display_name: None,
            seen_at_ms: 20,
        };
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![account("Qplayer#12345", "QUSERID1")],
                ..WriteBatch::default()
            },
        );
        // Seen again; a second account; then that second name claiming the
        // first one's user id (no rename: both names are taken).
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![
                    account("Qplayer#12345", "QUSERID1"),
                    account("Qother#99999", "QUSERID2"),
                    account("Qother#99999", "QUSERID1"),
                ],
                ..WriteBatch::default()
            },
        );
        // username, platform, arena_user_id, first_seen_at, last_login_at
        type AccountRow = (String, String, Option<String>, i64, Option<i64>);
        let rows: Vec<AccountRow> = conn
            .prepare("SELECT username, platform, arena_user_id, first_seen_at, last_login_at FROM accounts ORDER BY id")
            .expect("prepare")
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
            })
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        assert_eq!(
            rows,
            vec![
                (
                    "Qplayer#12345".to_string(),
                    "arena".to_string(),
                    Some("QUSERID1".to_string()),
                    20,
                    None
                ),
                (
                    "Qother#99999".to_string(),
                    "arena".to_string(),
                    Some("QUSERID2".to_string()),
                    20,
                    None
                ),
            ],
            "a user id another account holds is left out; each keeps its own"
        );
    }

    fn arena_account(username: &str, user_id: &str) -> PlatformAccount {
        PlatformAccount {
            platform: Platform::Arena,
            username: username.to_string(),
            user_id: Some(user_id.to_string()),
            display_name: None,
            seen_at_ms: 20,
        }
    }

    /// `(username, platform, arena_user_id)` of every account, by id.
    fn account_rows(conn: &Connection) -> Vec<(String, String, Option<String>)> {
        conn.prepare("SELECT username, platform, arena_user_id FROM accounts ORDER BY id")
            .expect("prepare")
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows")
    }

    #[test]
    fn an_arena_login_never_converts_an_mtgo_account() {
        let (_dir, mut conn) = db("arena-account-mtgo-name");
        crate::test_support::account(&conn, "Qplayer");
        insert_arena_log_file(&conn, &new_file("/arena/Player.log")).expect("file");
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![arena_account("Qplayer", "QUSERID1")],
                rank_observations: vec![rank("Qplayer", 70)],
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            account_rows(&conn),
            vec![("Qplayer".to_string(), "mtgo".to_string(), None)],
            "the MTGO account stays an MTGO one; the Arena row is not written"
        );
        let ranks: i64 = conn
            .query_row("SELECT COUNT(*) FROM arena_rank_observations", [], |row| {
                row.get(0)
            })
            .expect("count");
        assert_eq!(ranks, 0, "a rank is never stored against an MTGO account");
    }

    #[test]
    fn an_arena_opponent_never_converts_an_mtgo_opponent() {
        let (_dir, mut conn) = db("arena-opponent-mtgo-name");
        conn.execute(
            "INSERT INTO opponents (username, first_seen_at) VALUES ('arena:QOPPID', 0)",
            [],
        )
        .expect("mtgo opponent");
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_opponents: vec![PlatformOpponent {
                    platform: Platform::Arena,
                    username: "arena:QOPPID".to_string(),
                    display_name: "Qrival".to_string(),
                }],
                now_ms: 30,
                ..WriteBatch::default()
            },
        );
        let row: (String, Option<String>) = conn
            .query_row(
                "SELECT platform, display_name FROM opponents WHERE username = 'arena:QOPPID'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("row");
        assert_eq!(row, ("mtgo".to_string(), None));
    }

    #[test]
    fn an_untagged_arena_account_keeps_its_display_name_until_named() {
        let (_dir, mut conn) = db("arena-account-untagged");
        let untagged = |display_name: Option<&str>| PlatformAccount {
            platform: Platform::Arena,
            username: "arena:QUSERID1".to_string(),
            user_id: Some("QUSERID1".to_string()),
            display_name: display_name.map(str::to_string),
            seen_at_ms: 20,
        };
        let display = |conn: &Connection| -> (String, Option<String>) {
            conn.query_row("SELECT username, display_name FROM accounts", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .expect("one account")
        };
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![untagged(Some("Qplayer"))],
                ..WriteBatch::default()
            },
        );
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![untagged(None)],
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            display(&conn),
            ("arena:QUSERID1".to_string(), Some("Qplayer".to_string())),
            "a write without a display name keeps it"
        );
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![arena_account("Qplayer#12345", "QUSERID1")],
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            display(&conn),
            ("Qplayer#12345".to_string(), None),
            "the tagged name renames the same row"
        );
    }

    #[test]
    fn an_arena_rename_keeps_one_account() {
        let (_dir, mut conn) = db("arena-account-rename");
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![arena_account("Qold#12345", "QUSERID1")],
                ..WriteBatch::default()
            },
        );
        let id: i64 = conn
            .query_row("SELECT id FROM accounts", [], |row| row.get(0))
            .expect("id");
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![arena_account("Qnew#67890", "QUSERID1")],
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            account_rows(&conn),
            vec![(
                "Qnew#67890".to_string(),
                "arena".to_string(),
                Some("QUSERID1".to_string())
            )]
        );
        let kept: i64 = conn
            .query_row("SELECT id FROM accounts", [], |row| row.get(0))
            .expect("id");
        assert_eq!(kept, id, "the same row, renamed");
    }

    /// Rows of every table that points at an account, under account `id`
    /// (a match, a deck, a league, a tournament, a draft, an untracked match,
    /// a course observation and a pulled run), and the Arena switch on it.
    fn rows_under(conn: &Connection, id: i64, tag: &str) {
        crate::test_support::insert_manual(conn, &format!("q-match-{tag}"));
        conn.execute_batch(&format!(
            "UPDATE matches SET account_id = {id} WHERE token = 'q-match-{tag}';
             INSERT INTO decks (mtgo_id, account_id, name, format, created_at, updated_at, source)
             VALUES ('q-deck-{tag}', {id}, 'Qdeck', 'CSTANDARD', 0, 0, 'file');
             INSERT INTO leagues (run_key, account_id, source, kind, state, token, name,
                 started_at, created_at)
             VALUES ('arena:q-course-{tag}', {id}, 'log', 'draft', 'active',
                 'arena:q-course-{tag}', 'Qrun', 0, 0);
             INSERT INTO tournaments (mtgo_event_id, account_id, name, started_at)
             VALUES ({id}00, {id}, 'Qcup', 0);
             INSERT INTO drafts (draft_token, account_id, source, state)
             VALUES ('q-draft-{tag}', {id}, 'log', 'finished');
             INSERT INTO untracked_matches (match_token, account_id, skipped_at)
             VALUES ('q-untracked-{tag}', {id}, 0);
             INSERT INTO arena_course_observations (arena_log_file_id, byte_offset, observed_at,
                 account_id, course_id, event_name, module, wins, losses, deck_id, deck_name,
                 cards)
             VALUES (1, {id}0, 0, {id}, 'q-course-{tag}', 'QuickDraft', 'Draft', 0, 0, '', '',
                 '{{}}');
             INSERT INTO synced_arena_runs (account_id, course_id, event_name, kind, wins,
                 losses, state, observed_at)
             VALUES ({id}, 'q-course-{tag}', 'QuickDraft', 'draft', 1, 0, 'active', 5);
             INSERT INTO settings (key, value)
             VALUES ('{key}', '{{\"account_id\":{id},\"switched_at_ms\":7}}')
             ON CONFLICT (key) DO UPDATE SET value = excluded.value;",
            key = crate::accounts::ACTIVE_ARENA_ACCOUNT,
        ))
        .expect("rows under the account");
    }

    /// How many rows of each table that points at an account name `id`.
    fn rows_of(conn: &Connection, id: i64) -> Vec<String> {
        let mut found = Vec::new();
        for (table, column) in [
            ("arena_course_observations", "account_id"),
            ("arena_rank_observations", "account_id"),
            ("decks", "account_id"),
            ("drafts", "account_id"),
            ("leagues", "account_id"),
            ("matches", "account_id"),
            ("synced_arena_runs", "account_id"),
            ("tournaments", "account_id"),
            ("untracked_matches", "account_id"),
        ] {
            let count: i64 = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?1"),
                    [id],
                    |row| row.get(0),
                )
                .expect("count");
            found.push(format!("{table} {count}"));
        }
        found
    }

    /// #538: a rename seen before authentication (the new name with no user
    /// id) makes a second row; authentication then names the new name with
    /// the user id the old row holds. The two rows merge into the old one,
    /// renamed, holding everything both had, and the switch follows it.
    #[test]
    fn a_rename_before_authentication_merges_into_one_account() {
        let (_dir, mut conn) = db("arena-account-rename-early");
        insert_arena_log_file(&conn, &new_file("/arena/Player.log")).expect("file");
        let mut early = arena_account("Qnew#67890", "QUSERID1");
        early.user_id = None;
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![arena_account("Qold#12345", "QUSERID1")],
                rank_observations: vec![rank("Qold#12345", 70)],
                ..WriteBatch::default()
            },
        );
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![early],
                rank_observations: vec![rank("Qnew#67890", 80)],
                ..WriteBatch::default()
            },
        );
        let id_of = |conn: &Connection, name: &str| -> i64 {
            conn.query_row(
                "SELECT id FROM accounts WHERE username = ?1",
                [name],
                |row| row.get(0),
            )
            .expect("id")
        };
        let old = id_of(&conn, "Qold#12345");
        let new = id_of(&conn, "Qnew#67890");
        assert_ne!(old, new, "the early name makes a second row");
        rows_under(&conn, old, "old");
        rows_under(&conn, new, "new");

        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![arena_account("Qnew#67890", "QUSERID1")],
                ..WriteBatch::default()
            },
        );
        let arena_rows: Vec<_> = account_rows(&conn)
            .into_iter()
            .filter(|(_, platform, _)| platform == "arena")
            .collect();
        assert_eq!(
            arena_rows,
            vec![(
                "Qnew#67890".to_string(),
                "arena".to_string(),
                Some("QUSERID1".to_string())
            )],
            "one account, renamed"
        );
        assert_eq!(
            id_of(&conn, "Qnew#67890"),
            old,
            "the row holding the user id"
        );
        let expected: Vec<String> = [
            "arena_course_observations 2",
            "arena_rank_observations 2",
            "decks 2",
            "drafts 2",
            "leagues 2",
            "matches 2",
            "synced_arena_runs 2",
            "tournaments 2",
            "untracked_matches 2",
        ]
        .map(str::to_string)
        .to_vec();
        assert_eq!(rows_of(&conn, old), expected);
        let switched: i64 = conn
            .query_row(
                "SELECT json_extract(value, '$.account_id') FROM settings WHERE key = ?1",
                [crate::accounts::ACTIVE_ARENA_ACCOUNT],
                |row| row.get(0),
            )
            .expect("a switch");
        assert_eq!(switched, old, "the switch follows the merge");
        let problems: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .expect("check");
        assert_eq!(problems, 0);
    }

    /// The old name's row (holding the user id) and the new name's row seen
    /// before authentication, by id.
    fn renamed_early(conn: &mut Connection) -> (i64, i64) {
        let mut early = arena_account("Qnew#67890", "QUSERID1");
        early.user_id = None;
        commit_batch(
            conn,
            WriteBatch {
                platform_accounts: vec![arena_account("Qold#12345", "QUSERID1"), early],
                ..WriteBatch::default()
            },
        );
        let id_of = |name: &str| -> i64 {
            conn.query_row(
                "SELECT id FROM accounts WHERE username = ?1",
                [name],
                |row| row.get(0),
            )
            .expect("id")
        };
        (id_of("Qold#12345"), id_of("Qnew#67890"))
    }

    fn authenticate(conn: &mut Connection) {
        commit_batch(
            conn,
            WriteBatch {
                platform_accounts: vec![arena_account("Qnew#67890", "QUSERID1")],
                ..WriteBatch::default()
            },
        );
    }

    fn pulled_record(conn: &Connection, account_id: i64, course_id: &str) {
        conn.execute(
            "INSERT INTO synced_arena_runs (account_id, course_id, event_name, kind, wins,
                 losses, state, observed_at)
             VALUES (?1, ?2, 'QuickDraft_FDN_20261101', 'draft', ?3, 0, 'active', ?4)",
            params![account_id, course_id, account_id, 1_000 * account_id],
        )
        .expect("a pulled record");
    }

    /// #538 review: a course both rows hold a pulled record of keeps the
    /// joined record (the newer), never only the older account's.
    #[test]
    fn a_merge_joins_pulled_records_of_one_course() {
        let (_dir, mut conn) = db("arena-account-merge-runs");
        let (old, new) = renamed_early(&mut conn);
        assert!(new > old);
        pulled_record(&conn, old, "q-course");
        pulled_record(&conn, new, "q-course");
        authenticate(&mut conn);
        let records: Vec<String> = conn
            .prepare(
                "SELECT account_id || ' ' || wins || ' ' || observed_at FROM synced_arena_runs",
            )
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        assert_eq!(records, vec![format!("{old} {new} {}", 1_000 * new)]);
    }

    /// #538 review: the merged account keeps the player's untracked choice
    /// from either row, and its runs are folded again, so a run pulled under
    /// the new name shows under the merged account.
    #[test]
    fn a_merge_keeps_an_untracked_choice_and_folds_the_runs() {
        let (_dir, mut conn) = db("arena-account-merge-tracked");
        let (old, new) = renamed_early(&mut conn);
        conn.execute(
            "UPDATE accounts SET tracked = 0, tracked_chosen = 1 WHERE id = ?1",
            [new],
        )
        .expect("untracked by the player");
        pulled_record(&conn, new, "q-course");
        authenticate(&mut conn);
        let account: (i64, i64, i64) = conn
            .query_row(
                "SELECT id, tracked, tracked_chosen FROM accounts WHERE platform = 'arena'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("one Arena account");
        assert_eq!(account, (old, 0, 1));
        let runs: Vec<String> = conn
            .prepare("SELECT run_key || ' ' || account_id FROM leagues")
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        assert_eq!(runs, vec![format!("arena:q-course {old}")]);
    }

    /// #538 review: the tables that point at an account with a unique key
    /// holding that column. The merge joins each of them before the move; a
    /// new one needs the same, or the merge would leave rows behind.
    #[test]
    fn account_tables_with_a_unique_account_key_are_known() {
        let (_dir, conn) = db("arena-account-unique-keys");
        let found: Vec<String> = conn
            .prepare(
                "SELECT DISTINCT m.name FROM sqlite_master m
                 JOIN pragma_foreign_key_list(m.name) f ON f.\"table\" = 'accounts'
                 JOIN pragma_index_list(m.name) i ON i.\"unique\" = 1
                 JOIN pragma_index_info(i.name) c ON c.name = f.\"from\"
                 WHERE m.type = 'table' ORDER BY m.name",
            )
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        // The tournament tables hold MTGO accounts only (MTGO log facts),
        // and so does a draft's tournament (`drafts_tournament_id`, unique
        // per account only while a tournament is set); the merge is for
        // Arena and Endstep renames, so it never meets their rows. If it
        // did, the move would be undone and logged.
        assert_eq!(
            found,
            vec![
                "drafts".to_string(),
                "synced_arena_runs".to_string(),
                "tournament_rounds".to_string(),
                "tournament_standings".to_string(),
            ]
        );
    }

    /// #538 review: rows the move would leave under the new name's row (a
    /// unique key the merge does not join) skip the merge, logged, and never
    /// fail the batch: both rows stay as they were.
    #[test]
    fn a_merge_that_would_leave_rows_behind_is_skipped() {
        let (_dir, mut conn) = db("arena-account-merge-skip");
        let (old, new) = renamed_early(&mut conn);
        conn.execute_batch(&format!(
            "CREATE TABLE q_notes (
                 account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE RESTRICT,
                 slot INTEGER NOT NULL,
                 PRIMARY KEY (account_id, slot)
             ) STRICT;
             INSERT INTO q_notes VALUES ({old}, 1), ({new}, 1);"
        ))
        .expect("a table the merge does not know");
        crate::test_support::insert_manual(&conn, "q-match");
        conn.execute(
            "UPDATE matches SET account_id = ?1 WHERE token = 'q-match'",
            [new],
        )
        .expect("a match under the new name");
        authenticate(&mut conn);
        let arena: Vec<(String, String, Option<String>)> = account_rows(&conn)
            .into_iter()
            .filter(|(_, platform, _)| platform == "arena")
            .collect();
        assert_eq!(
            arena,
            vec![
                (
                    "Qold#12345".to_string(),
                    "arena".to_string(),
                    Some("QUSERID1".to_string())
                ),
                ("Qnew#67890".to_string(), "arena".to_string(), None),
            ]
        );
        let under_new: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM matches WHERE account_id = ?1",
                [new],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(under_new, 1, "nothing moved");
    }

    /// Live equals rebuild: the same logins in one batch end as they do one
    /// batch at a time.
    #[test]
    fn a_rename_before_authentication_ends_the_same_in_one_batch() {
        let logins = || {
            let mut early = arena_account("Qnew#67890", "QUSERID1");
            early.user_id = None;
            vec![
                arena_account("Qold#12345", "QUSERID1"),
                early,
                arena_account("Qnew#67890", "QUSERID1"),
            ]
        };
        // Only the new name's: a rank the old name read is written after
        // every login of its batch, so in one batch it would find no account.
        let ranks = || vec![rank("Qnew#67890", 80)];
        let (_a, mut live) = db("arena-account-rename-live");
        insert_arena_log_file(&live, &new_file("/arena/Player.log")).expect("file");
        for (index, account) in logins().into_iter().enumerate() {
            let mut batch = WriteBatch {
                platform_accounts: vec![account],
                ..WriteBatch::default()
            };
            if index == 1 {
                batch.rank_observations = ranks();
            }
            commit_batch(&mut live, batch);
        }
        let (_b, mut rebuilt) = db("arena-account-rename-rebuild");
        insert_arena_log_file(&rebuilt, &new_file("/arena/Player.log")).expect("file");
        commit_batch(
            &mut rebuilt,
            WriteBatch {
                platform_accounts: logins(),
                rank_observations: ranks(),
                ..WriteBatch::default()
            },
        );
        let ranks_of = |conn: &Connection| -> Vec<String> {
            conn.prepare(
                "SELECT a.username || ' ' || o.byte_offset FROM arena_rank_observations o
                 JOIN accounts a ON a.id = o.account_id ORDER BY o.byte_offset",
            )
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows")
        };
        assert_eq!(account_rows(&live), account_rows(&rebuilt));
        assert_eq!(ranks_of(&live), ranks_of(&rebuilt));
        assert_eq!(ranks_of(&live), vec!["Qnew#67890 80".to_string()]);
        assert_eq!(
            account_rows(&live),
            vec![(
                "Qnew#67890".to_string(),
                "arena".to_string(),
                Some("QUSERID1".to_string())
            )]
        );
    }

    #[test]
    fn a_rename_to_a_name_an_mtgo_account_holds_changes_nothing() {
        let (_dir, mut conn) = db("arena-rename-blocked-by-mtgo");
        crate::test_support::account(&conn, "Qnew#67890");
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![arena_account("Qold#12345", "QUSERID1")],
                ..WriteBatch::default()
            },
        );
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![arena_account("Qnew#67890", "QUSERID1")],
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            account_rows(&conn),
            vec![
                ("Qnew#67890".to_string(), "mtgo".to_string(), None),
                (
                    "Qold#12345".to_string(),
                    "arena".to_string(),
                    Some("QUSERID1".to_string())
                ),
            ],
            "the MTGO row is untouched and the Arena row keeps its name and id"
        );
    }

    /// #862: one batch holds a rank read under the old name and the rename
    /// that follows (a fresh log read in one go). The batch applies every
    /// account before any rank, so the rank is found by its user id.
    #[test]
    fn a_rank_read_in_the_same_batch_as_a_rename_is_kept() {
        let (_dir, mut conn) = db("arena-rank-rename-one-batch");
        insert_arena_log_file(&conn, &new_file("/arena/Player.log")).expect("file");
        let mut before = rank("Qold#12345", 70);
        before.account_user_id = Some("QUSERID1".to_string());
        let mut after = rank("Qnew#67890", 80);
        after.account_user_id = Some("QUSERID1".to_string());
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![
                    arena_account("Qold#12345", "QUSERID1"),
                    arena_account("Qnew#67890", "QUSERID1"),
                ],
                rank_observations: vec![before, after],
                ..WriteBatch::default()
            },
        );
        let names: Vec<(String, i64)> = conn
            .prepare(
                "SELECT a.username, r.byte_offset
                 FROM arena_rank_observations r JOIN accounts a ON a.id = r.account_id
                 ORDER BY r.byte_offset",
            )
            .expect("prepare")
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        assert_eq!(
            names,
            vec![
                ("Qnew#67890".to_string(), 70),
                ("Qnew#67890".to_string(), 80)
            ],
            "the reading under the old name follows the renamed account"
        );
    }

    #[test]
    fn rank_history_follows_the_account_through_a_rename() {
        let (_dir, mut conn) = db("arena-rank-after-rename");
        insert_arena_log_file(&conn, &new_file("/arena/Player.log")).expect("file");
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![arena_account("Qold#12345", "QUSERID1")],
                rank_observations: vec![rank("Qold#12345", 70)],
                ..WriteBatch::default()
            },
        );
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![arena_account("Qnew#67890", "QUSERID1")],
                rank_observations: vec![rank("Qnew#67890", 80)],
                ..WriteBatch::default()
            },
        );
        let names: Vec<(String, i64)> = conn
            .prepare(
                "SELECT a.username, r.byte_offset
                 FROM arena_rank_observations r JOIN accounts a ON a.id = r.account_id
                 ORDER BY r.byte_offset",
            )
            .expect("prepare")
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        assert_eq!(
            names,
            vec![
                ("Qnew#67890".to_string(), 70),
                ("Qnew#67890".to_string(), 80)
            ],
            "both readings belong to the one renamed account"
        );
    }

    #[test]
    fn an_untagged_login_never_renames_a_tagged_account() {
        let (_dir, mut conn) = db("arena-account-untagged-after-tagged");
        insert_arena_log_file(&conn, &new_file("/arena/Player.log")).expect("file");
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![
                    arena_account("Qother#00002", "QUSERID2"),
                    arena_account("Qplayer#00001", "QUSERID1"),
                ],
                ..WriteBatch::default()
            },
        );
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![PlatformAccount {
                    platform: Platform::Arena,
                    username: "arena:QUSERID2".to_string(),
                    user_id: Some("QUSERID2".to_string()),
                    display_name: Some("Qother".to_string()),
                    seen_at_ms: 30,
                }],
                rank_observations: vec![rank("arena:QUSERID2", 70)],
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            account_rows(&conn),
            vec![
                (
                    "Qother#00002".to_string(),
                    "arena".to_string(),
                    Some("QUSERID2".to_string())
                ),
                (
                    "Qplayer#00001".to_string(),
                    "arena".to_string(),
                    Some("QUSERID1".to_string())
                ),
            ],
            "the tagged name stays and no second row holds the user id"
        );
        let shown: String = conn
            .query_row(
                "SELECT COALESCE(display_name, username) FROM accounts WHERE username = 'Qother#00002'",
                [],
                |row| row.get(0),
            )
            .expect("shown");
        assert_eq!(shown, "Qother#00002");
        let rank_account: String = conn
            .query_row(
                "SELECT a.username FROM arena_rank_observations r JOIN accounts a ON a.id = r.account_id",
                [],
                |row| row.get(0),
            )
            .expect("one rank");
        assert_eq!(
            rank_account, "Qother#00002",
            "the untagged name is its account"
        );
    }

    #[test]
    fn an_untagged_name_without_a_user_id_joins_the_account_holding_its_id() {
        let (_dir, mut conn) = db("arena-account-untagged-no-id");
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![arena_account("Qplayer#00001", "QUSERID1")],
                ..WriteBatch::default()
            },
        );
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![PlatformAccount {
                    platform: Platform::Arena,
                    username: "arena:QUSERID1".to_string(),
                    user_id: None,
                    display_name: None,
                    seen_at_ms: 30,
                }],
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            account_rows(&conn),
            vec![(
                "Qplayer#00001".to_string(),
                "arena".to_string(),
                Some("QUSERID1".to_string())
            )]
        );
        let id: i64 = conn
            .query_row("SELECT id FROM accounts", [], |row| row.get(0))
            .expect("id");
        assert_eq!(
            crate::read::account_id(&conn, "arena:QUSERID1").expect("read"),
            Some(id),
            "the untagged name reads as the same account"
        );
    }

    #[test]
    fn an_untagged_name_no_account_holds_keeps_its_user_id() {
        let (_dir, mut conn) = db("arena-account-untagged-new");
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![PlatformAccount {
                    platform: Platform::Arena,
                    username: "arena:QUSERID1".to_string(),
                    user_id: None,
                    display_name: None,
                    seen_at_ms: 30,
                }],
                ..WriteBatch::default()
            },
        );
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![arena_account("Qplayer#00001", "QUSERID1")],
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            account_rows(&conn),
            vec![(
                "Qplayer#00001".to_string(),
                "arena".to_string(),
                Some("QUSERID1".to_string())
            )],
            "the login renames the row its name's user id is on"
        );
    }

    #[test]
    fn an_endstep_account_never_takes_an_arena_row() {
        let (_dir, mut conn) = db("endstep-account-arena-name");
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![arena_account("Qplayer#00001", "QUSERID1")],
                platform_opponents: vec![PlatformOpponent {
                    platform: Platform::Arena,
                    username: "Qrival#00002".to_string(),
                    display_name: "Qrival".to_string(),
                }],
                now_ms: 30,
                ..WriteBatch::default()
            },
        );
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![PlatformAccount {
                    platform: Platform::Endstep,
                    username: "Qplayer#00001".to_string(),
                    user_id: Some("QESID1".to_string()),
                    display_name: Some("Qplayer".to_string()),
                    seen_at_ms: 40,
                }],
                platform_opponents: vec![PlatformOpponent {
                    platform: Platform::Endstep,
                    username: "Qrival#00002".to_string(),
                    display_name: "Qother".to_string(),
                }],
                now_ms: 40,
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            account_rows(&conn),
            vec![(
                "Qplayer#00001".to_string(),
                "arena".to_string(),
                Some("QUSERID1".to_string())
            )],
            "the Arena account keeps its row; the Endstep one is not written"
        );
        let opponent: (String, String) = conn
            .query_row("SELECT platform, display_name FROM opponents", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .expect("one opponent");
        assert_eq!(opponent, ("arena".to_string(), "Qrival".to_string()));
    }

    #[test]
    fn an_endstep_account_is_known_by_its_user_id() {
        let (_dir, mut conn) = db("endstep-account");
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![PlatformAccount {
                    platform: Platform::Endstep,
                    username: "endstep:QESID1".to_string(),
                    user_id: None,
                    display_name: Some("Qplayer".to_string()),
                    seen_at_ms: 40,
                }],
                platform_opponents: vec![PlatformOpponent {
                    platform: Platform::Endstep,
                    username: "endstep:QESID2".to_string(),
                    display_name: "Qrival".to_string(),
                }],
                now_ms: 40,
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            account_rows(&conn),
            vec![(
                "endstep:QESID1".to_string(),
                "endstep".to_string(),
                Some("QESID1".to_string())
            )]
        );
        assert_eq!(
            platform_account_username(&conn, "endstep:QESID1").expect("read"),
            "endstep:QESID1"
        );
        let opponent: (String, String, String) = conn
            .query_row(
                "SELECT username, platform, display_name FROM opponents",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("one opponent");
        assert_eq!(
            opponent,
            (
                "endstep:QESID2".to_string(),
                "endstep".to_string(),
                "Qrival".to_string()
            )
        );
    }

    #[test]
    fn arena_opponent_gets_platform_and_display_name() {
        let (_dir, mut conn) = db("arena-opponent");
        let opponent = |display_name: &str| PlatformOpponent {
            platform: Platform::Arena,
            username: "arena:QOPPID".to_string(),
            display_name: display_name.to_string(),
        };
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_opponents: vec![opponent("Qrival")],
                now_ms: 30,
                ..WriteBatch::default()
            },
        );
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_opponents: vec![opponent("Qrenamed")],
                now_ms: 40,
                ..WriteBatch::default()
            },
        );
        let row: (String, String, String, i64, i64) = conn
            .query_row(
                "SELECT username, platform, display_name, first_seen_at, COUNT(*) FROM opponents",
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
            .expect("row");
        assert_eq!(
            row,
            (
                "arena:QOPPID".to_string(),
                "arena".to_string(),
                "Qrenamed".to_string(),
                30,
                1
            )
        );
    }

    #[test]
    fn rank_observation_round_trips() {
        let (_dir, mut conn) = db("arena-rank");
        let file = insert_arena_log_file(&conn, &new_file("/arena/Player.log")).expect("file");
        assert_eq!(file, 1);
        let mut limited = rank("Qplayer#12345", 70);
        limited.ladder = "limited";
        limited.mythic_percentile = None;
        limited.mythic_placement = None;
        let batch = WriteBatch {
            platform_accounts: vec![PlatformAccount {
                platform: Platform::Arena,
                username: "Qplayer#12345".to_string(),
                user_id: Some("QUSERID1".to_string()),
                display_name: None,
                seen_at_ms: 1,
            }],
            rank_observations: vec![rank("Qplayer#12345", 70), limited],
            ..WriteBatch::default()
        };
        commit_batch(&mut conn, batch.clone());
        commit_batch(&mut conn, batch);

        let rows: Vec<String> = conn
            .prepare(
                "SELECT a.username, r.ladder, r.season, r.observed_at, r.class, r.level, r.step,
                     r.matches_won, r.matches_lost, r.mythic_percentile, r.mythic_placement,
                     r.arena_log_file_id, r.byte_offset
                 FROM arena_rank_observations r JOIN accounts a ON a.id = r.account_id
                 ORDER BY r.ladder",
            )
            .expect("prepare")
            .query_map([], |row| {
                let mut values = Vec::new();
                for index in 0..13 {
                    let value: rusqlite::types::Value = row.get(index)?;
                    values.push(match value {
                        rusqlite::types::Value::Null => "NULL".to_string(),
                        rusqlite::types::Value::Integer(number) => number.to_string(),
                        rusqlite::types::Value::Real(number) => number.to_string(),
                        rusqlite::types::Value::Text(text) => text,
                        rusqlite::types::Value::Blob(_) => "BLOB".to_string(),
                    });
                }
                Ok(values.join(" "))
            })
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        assert_eq!(
            rows,
            vec![
                "Qplayer#12345 constructed 42 5000 Mythic 1 0 12 7 87.5 1234 1 70",
                "Qplayer#12345 limited 42 5000 Mythic 1 0 12 7 NULL NULL 1 70",
            ],
            "both ladders stored once, a replay skipped"
        );
    }

    #[test]
    fn a_rank_observation_without_its_account_is_skipped() {
        let (_dir, mut conn) = db("arena-rank-no-account");
        insert_arena_log_file(&conn, &new_file("/arena/Player.log")).expect("file");
        commit_batch(
            &mut conn,
            WriteBatch {
                rank_observations: vec![rank("Qnobody#00000", 70)],
                ..WriteBatch::default()
            },
        );
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM arena_rank_observations", [], |row| {
                row.get(0)
            })
            .expect("count");
        assert_eq!(rows, 0);
    }

    #[test]
    fn an_arena_account_seen_before_its_user_id_gets_it_later() {
        let (_dir, mut conn) = db("arena-account-no-id");
        let mut early = arena_account("Qplayer#12345", "QUSERID1");
        early.user_id = None;
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![early.clone()],
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            account_rows(&conn),
            vec![("Qplayer#12345".to_string(), "arena".to_string(), None)]
        );
        commit_batch(
            &mut conn,
            WriteBatch {
                platform_accounts: vec![arena_account("Qplayer#12345", "QUSERID1"), early],
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            account_rows(&conn),
            vec![(
                "Qplayer#12345".to_string(),
                "arena".to_string(),
                Some("QUSERID1".to_string())
            )],
            "the id is kept once known"
        );
    }

    #[test]
    fn the_read_offset_for_a_head_is_the_furthest_read_of_any_file_with_it() {
        let (_dir, mut conn) = db("arena-head");
        assert_eq!(
            arena_read_offset_for_head(&conn, 7, 100).expect("read"),
            None
        );
        let first = insert_arena_log_file(&conn, &new_file("/arena/Player.log")).expect("file");
        let second = insert_arena_log_file(&conn, &new_file("/arena/Player.log")).expect("file");
        let mut other = new_file("/arena/Player.log");
        other.head_hash = 8;
        let third = insert_arena_log_file(&conn, &other).expect("file");
        let progress = |id: i64, read_offset: u64| ArenaFileProgress {
            id,
            last_observed_size: 5_000,
            read_offset,
            head_hash: if id == third { 8 } else { 7 },
            head_len: 100,
            sealed: Some((9, "truncated".to_string())),
        };
        commit_batch(
            &mut conn,
            WriteBatch {
                arena_files: vec![
                    progress(first, 300),
                    progress(second, 200),
                    progress(third, 900),
                ],
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            arena_read_offset_for_head(&conn, 7, 100).expect("read"),
            Some(300)
        );
        assert_eq!(
            arena_read_offset_for_head(&conn, 7, 99).expect("read"),
            None
        );
    }

    #[test]
    fn the_file_for_a_head_is_the_unsealed_one_else_the_first() {
        let (_dir, mut conn) = db("arena-file-for-head");
        assert_eq!(arena_file_for_head(&conn, 7, 100).expect("read"), None);
        let first = insert_arena_log_file(&conn, &new_file("/arena/Player.log")).expect("file");
        let second = insert_arena_log_file(&conn, &new_file("/arena/Player.log")).expect("file");
        assert_eq!(
            arena_file_for_head(&conn, 7, 100).expect("read"),
            Some(first)
        );
        commit_batch(
            &mut conn,
            WriteBatch {
                arena_files: vec![ArenaFileProgress {
                    id: first,
                    last_observed_size: 5_000,
                    read_offset: 300,
                    head_hash: 7,
                    head_len: 100,
                    sealed: Some((9, "truncated".to_string())),
                }],
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            arena_file_for_head(&conn, 7, 100).expect("read"),
            Some(second)
        );
        assert_eq!(arena_file_for_head(&conn, 7, 99).expect("read"), None);
    }
}
