//! Accounts: which one the pages show, and which ones record matches
//! (release spec 5; 0.x `Account`, `SwitchAccountController`,
//! `UpdateAccountTrackingController`).
//!
//! 0.x kept an `accounts.active` flag (exactly one, set by every Login line
//! and every switch). v1 keeps no flag: the current account is the newer of
//! the user's last switch (the `active_account` setting) and the newest
//! stored Login line, counting tracked accounts only (`current_account`).

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::platform::PLATFORM_CONTEXT;
use crate::{Connection, Platform, StoreError};

/// The settings key of the user's last switch (release spec 14).
pub const ACTIVE_ACCOUNT: &str = "active_account";

/// The settings key of the user's last Arena switch (same JSON as
/// `ACTIVE_ACCOUNT`).
pub const ACTIVE_ARENA_ACCOUNT: &str = "active_arena_account";

/// The settings key of the user's last Endstep switch (same JSON as
/// `ACTIVE_ACCOUNT`).
pub const ACTIVE_ENDSTEP_ACCOUNT: &str = "active_endstep_account";

/// The switch key of a platform.
fn switch_key(platform: Platform) -> &'static str {
    match platform {
        Platform::Mtgo => ACTIVE_ACCOUNT,
        Platform::Arena => ACTIVE_ARENA_ACCOUNT,
        Platform::Endstep => ACTIVE_ENDSTEP_ACCOUNT,
    }
}

/// The user's last switch from the header: which account, and when (UTC
/// unix ms, wall clock). Stored as JSON under `ACTIVE_ACCOUNT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveAccount {
    pub account_id: i64,
    pub switched_at_ms: i64,
}

/// The newest stored Login line of a tracked account: its account and its
/// log time (UTC unix ms).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LatestLogin {
    pub account_id: i64,
    pub logged_at_ms: i64,
}

/// One account, for the header and the Accounts card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRow {
    pub id: i64,
    pub username: String,
    pub tracked: bool,
    /// What the pages show: the display name of an Arena account known only
    /// by its user id, else the username.
    pub label: String,
}

/// The current account (0.x "a Login activates, a switch activates"): the
/// newer of the last switch and the newest Login, each counted only for a
/// tracked account (a switch wins a tie: the user chose it). With neither,
/// the only tracked account; else `None`, which the pages read as every
/// account.
pub fn current_account(
    switch: Option<ActiveAccount>,
    latest_login: Option<LatestLogin>,
    tracked_ids: &[i64],
) -> Option<i64> {
    let switch = switch.filter(|switch| tracked_ids.contains(&switch.account_id));
    let login = latest_login.filter(|login| tracked_ids.contains(&login.account_id));
    match (switch, login) {
        (Some(switch), Some(login)) => {
            if login.logged_at_ms > switch.switched_at_ms {
                Some(login.account_id)
            } else {
                Some(switch.account_id)
            }
        }
        (Some(switch), None) => Some(switch.account_id),
        (None, Some(login)) => Some(login.account_id),
        (None, None) => {
            if tracked_ids.len() == 1 {
                Some(tracked_ids[0])
            } else {
                None
            }
        }
    }
}

/// The account the pages show (0.x `Account::currentId`) on `platform`.
/// MTGO: the switch setting and the accounts' newest Login (stored as
/// `accounts.last_login_at` when the tracker stores the line, so the pages
/// read no log events). Arena and Endstep: the same rule with the newest
/// match of that platform in the place of a Login.
pub fn current_account_id(
    conn: &Connection,
    platform: Platform,
) -> Result<Option<i64>, StoreError> {
    let tracked_ids = tracked_account_ids(conn, platform)?;
    if tracked_ids.is_empty() {
        return Ok(None);
    }
    let switch = stored_switch(conn, switch_key(platform))?;
    let login = match platform {
        Platform::Mtgo => latest_tracked_login(conn)?,
        Platform::Arena | Platform::Endstep => newest_platform_match(conn, platform)?,
    };
    Ok(current_account(switch, login, &tracked_ids))
}

/// Point a platform's account switch at `to` when it names `from` (an
/// account merged into another, #538).
pub(crate) fn move_switch(
    conn: &Connection,
    platform: Platform,
    from: i64,
    to: i64,
) -> Result<(), StoreError> {
    conn.prepare_cached(
        "UPDATE settings SET value = json_set(value, '$.account_id', ?3)
         WHERE key = ?1 AND json_valid(value) AND json_extract(value, '$.account_id') = ?2",
    )?
    .execute(params![switch_key(platform), from, to])?;
    Ok(())
}

/// The stored MTGO switch, if any. A row that does not parse is no switch.
pub fn active_account(conn: &Connection) -> Result<Option<ActiveAccount>, StoreError> {
    stored_switch(conn, ACTIVE_ACCOUNT)
}

fn stored_switch(conn: &Connection, key: &str) -> Result<Option<ActiveAccount>, StoreError> {
    let value: Option<String> = conn
        .prepare_cached("SELECT value FROM settings WHERE key = ?1")?
        .query_row([key], |row| row.get(0))
        .optional()?;
    Ok(value.and_then(|value| {
        serde_json::from_str::<Option<ActiveAccount>>(&value)
            .ok()
            .flatten()
    }))
}

/// The newest listed match of a tracked account of `platform`, as a
/// `LatestLogin` (its account and start time).
fn newest_platform_match(
    conn: &Connection,
    platform: Platform,
) -> Result<Option<LatestLogin>, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT m.account_id, m.started_at FROM listed_matches m
             JOIN accounts a ON a.id = m.account_id
             WHERE a.tracked = 1 AND a.platform = ?1
             ORDER BY m.started_at DESC, m.id DESC LIMIT 1",
        )?
        .query_row([platform], |row| {
            Ok(LatestLogin {
                account_id: row.get(0)?,
                logged_at_ms: row.get(1)?,
            })
        })
        .optional()?)
}

/// The newest Login of a tracked MTGO account, by log time (two clients write
/// two files); a tie goes to the newer account.
pub fn latest_tracked_login(conn: &Connection) -> Result<Option<LatestLogin>, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT id, last_login_at FROM accounts
             WHERE tracked = 1 AND platform = 'mtgo' AND last_login_at IS NOT NULL
             ORDER BY last_login_at DESC, id DESC LIMIT 1",
        )?
        .query_row([], |row| {
            Ok(LatestLogin {
                account_id: row.get(0)?,
                logged_at_ms: row.get(1)?,
            })
        })
        .optional()?)
}

/// The tracked accounts' ids of `platform`, oldest first.
fn tracked_account_ids(conn: &Connection, platform: Platform) -> Result<Vec<i64>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id FROM accounts WHERE tracked = 1 AND platform = ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map([platform], |row| row.get::<_, i64>(0))?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row?);
    }
    Ok(ids)
}

/// Every account of `platform`, by the label it shows, ignoring case: an
/// untagged Arena account (`arena:<user id>`) sorts by its display name,
/// not before every letter (#633).
pub fn accounts(conn: &Connection, platform: Platform) -> Result<Vec<AccountRow>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, username, tracked, COALESCE(display_name, username) FROM accounts
         WHERE platform = ?1 ORDER BY COALESCE(display_name, username) COLLATE NOCASE, id",
    )?;
    let rows = stmt.query_map([platform], |row| {
        Ok(AccountRow {
            id: row.get(0)?,
            username: row.get(1)?,
            tracked: row.get(2)?,
            label: row.get(3)?,
        })
    })?;
    let mut list = Vec::new();
    for row in rows {
        list.push(row?);
    }
    Ok(list)
}

/// The usernames of the untracked accounts: their new log matches are not
/// built (0.x `ProcessMatchEvents`).
pub fn untracked_usernames(conn: &Connection) -> Result<Vec<String>, StoreError> {
    let mut stmt =
        conn.prepare_cached("SELECT username FROM accounts WHERE tracked = 0 ORDER BY username")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut list = Vec::new();
    for row in rows {
        list.push(row?);
    }
    Ok(list)
}

/// The skipped matches of tracked accounts, sorted: an account tracked
/// again whose rebuild has not written them yet (the tracker stopped first).
/// The tracker queues them for rebuild when it opens.
pub fn retrack_queue(conn: &Connection) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT u.match_token FROM untracked_matches u
         JOIN accounts a ON a.id = u.account_id
         WHERE a.tracked = 1 ORDER BY u.match_token",
    )?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut tokens = Vec::new();
    for row in rows {
        tokens.push(row?);
    }
    Ok(tokens)
}

/// A user's account edit, by username as 0.x's controllers take it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountEdit {
    /// The header's switch (0.x `SwitchAccountController`): stores the
    /// `active_account` setting (`active_arena_account` for an Arena
    /// account) at `at_ms` (wall clock).
    Switch { username: String, at_ms: i64 },
    /// The Accounts card's switch (0.x `UpdateAccountTrackingController`).
    /// Turning tracking back on hands back the account's skipped matches,
    /// so the tracker rebuilds them (ruling 7); their rows stay until each
    /// rebuilt match is written.
    SetTracked { username: String, tracked: bool },
    /// The header's platform switch: stores the `platform_context` setting.
    SwitchPlatform { platform: Platform },
}

/// What an `AccountEdit` did. Only `Done` changed anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountEditOutcome {
    /// `retrack`: the skipped match tokens to rebuild, sorted (empty unless
    /// tracking was turned back on).
    Done { retrack: Vec<String> },
    /// No account has that username.
    NotFound,
}

impl AccountEditOutcome {
    /// The message to show for a refusal (Laravel's `exists` rule message),
    /// `None` for `Done`.
    pub fn refusal(&self) -> Option<&'static str> {
        match self {
            AccountEditOutcome::Done { .. } => None,
            AccountEditOutcome::NotFound => Some("The selected username is invalid."),
        }
    }
}

/// Apply a user's account edit in its own transaction.
pub fn edit_account(
    conn: &mut Connection,
    edit: &AccountEdit,
) -> Result<AccountEditOutcome, StoreError> {
    let tx = conn.transaction()?;
    let outcome = match edit {
        AccountEdit::Switch { username, at_ms } => match account_of(&tx, username)? {
            None => AccountEditOutcome::NotFound,
            Some((account_id, platform)) => {
                let value = serde_json::to_string(&ActiveAccount {
                    account_id,
                    switched_at_ms: *at_ms,
                })
                .map_err(|error| StoreError::Corrupt(error.to_string()))?;
                tx.execute(
                    "INSERT INTO settings (key, value) VALUES (?1, ?2)
                         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                    params![switch_key(platform), value],
                )?;
                AccountEditOutcome::Done {
                    retrack: Vec::new(),
                }
            }
        },
        AccountEdit::SwitchPlatform { platform } => {
            tx.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                     ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                params![PLATFORM_CONTEXT, platform.as_str()],
            )?;
            AccountEditOutcome::Done {
                retrack: Vec::new(),
            }
        }
        AccountEdit::SetTracked { username, tracked } => match account_id(&tx, username)? {
            None => AccountEditOutcome::NotFound,
            Some(account_id) => {
                tx.execute(
                    "UPDATE accounts SET tracked = ?2, tracked_chosen = 1 WHERE id = ?1",
                    params![account_id, tracked],
                )?;
                let mut retrack = Vec::new();
                if *tracked {
                    let mut stmt = tx.prepare(
                        "SELECT match_token FROM untracked_matches WHERE account_id = ?1
                         ORDER BY match_token",
                    )?;
                    let rows = stmt.query_map([account_id], |row| row.get::<_, String>(0))?;
                    for row in rows {
                        retrack.push(row?);
                    }
                    // The rows stay: each one goes when its rebuilt match is
                    // written (`MatchWrite::Upsert` or `Remove`), so a stop
                    // before the rebuild finishes loses nothing
                    // (`retrack_queue` finds them again).
                }
                AccountEditOutcome::Done { retrack }
            }
        },
    };
    // A refusal wrote nothing, so committing it changes nothing either.
    tx.commit()?;
    Ok(outcome)
}

/// An account's id and platform, by username.
fn account_of(conn: &Connection, username: &str) -> Result<Option<(i64, Platform)>, StoreError> {
    let row: Option<(i64, Platform)> = conn
        .prepare_cached("SELECT id, platform FROM accounts WHERE username = ?1")?
        .query_row([username], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    Ok(row)
}

fn account_id(conn: &Connection, username: &str) -> Result<Option<i64>, StoreError> {
    Ok(conn
        .prepare_cached("SELECT id FROM accounts WHERE username = ?1")?
        .query_row([username], |row| row.get(0))
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::import_matches;
    use crate::test_support::*;
    use crate::write::{MatchWrite, NewAccount, NewEvent, WriteBatch, commit, insert_log_file};
    use mtgo_protocol::{EventKind, Subjects};

    fn switch(account_id: i64, switched_at_ms: i64) -> Option<ActiveAccount> {
        Some(ActiveAccount {
            account_id,
            switched_at_ms,
        })
    }

    fn login(account_id: i64, logged_at_ms: i64) -> Option<LatestLogin> {
        Some(LatestLogin {
            account_id,
            logged_at_ms,
        })
    }

    // ---- The pure rule ----------------------------------------------------

    #[test]
    fn a_switch_holds_until_the_next_login() {
        // Logged in as 1, switched to 2: 2 holds.
        assert_eq!(
            current_account(switch(2, 200), login(1, 100), &[1, 2]),
            Some(2)
        );
        // Then 1 logs in again: a Login activates.
        assert_eq!(
            current_account(switch(2, 200), login(1, 300), &[1, 2]),
            Some(1)
        );
    }

    #[test]
    fn a_newer_login_wins_over_an_older_switch() {
        assert_eq!(
            current_account(switch(1, 100), login(2, 200), &[1, 2]),
            Some(2)
        );
        assert_eq!(
            current_account(switch(1, 300), login(2, 200), &[1, 2]),
            Some(1)
        );
    }

    #[test]
    fn untracked_accounts_are_never_current() {
        // A switch to an untracked account does not count.
        assert_eq!(
            current_account(switch(2, 300), login(1, 100), &[1]),
            Some(1)
        );
        // Nor does its Login.
        assert_eq!(
            current_account(switch(1, 100), login(2, 300), &[1]),
            Some(1)
        );
        // Nothing tracked: no account (every account's data).
        assert_eq!(current_account(switch(1, 100), login(1, 200), &[]), None);
    }

    #[test]
    fn with_neither_the_only_tracked_account_is_current() {
        assert_eq!(current_account(None, None, &[4]), Some(4));
        assert_eq!(current_account(None, None, &[4, 5]), None);
        assert_eq!(current_account(None, None, &[]), None);
        // A switch alone, or a Login alone.
        assert_eq!(current_account(switch(5, 1), None, &[4, 5]), Some(5));
        assert_eq!(current_account(None, login(4, 1), &[4, 5]), Some(4));
    }

    // ---- The reads and edits (0.x `Account` activation rules) -------------

    fn add_account(conn: &Connection, username: &str) -> i64 {
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at) VALUES (?1, 1)",
            [username],
        )
        .expect("account");
        conn.last_insert_rowid()
    }

    /// A Login line for `username` at `at_ms`, in its own log file.
    fn add_login(conn: &mut Connection, username: &str, at_ms: i64) {
        let file = insert_log_file(
            conn,
            &new_log_file(&format!("/logs/{username}-{at_ms}.log")),
        )
        .expect("file");
        let mut event: NewEvent = event_with(
            file,
            0,
            EventKind::Login,
            Subjects::default(),
            &format!("12:00:00 [INF] (Login|MtGO Login Success) Username: {username} (7)"),
        );
        event.logged_at_ms = at_ms;
        // As the tracker stores a Login line: the event and its account.
        commit(
            conn,
            &WriteBatch {
                events: vec![event],
                accounts: vec![NewAccount {
                    username: username.to_string(),
                    login_id: None,
                    seen_at_ms: 1,
                    logged_at_ms: Some(at_ms),
                }],
                ..WriteBatch::default()
            },
        )
        .expect("login");
    }

    fn edit(conn: &mut Connection, edit: AccountEdit) -> AccountEditOutcome {
        edit_account(conn, &edit).expect("edit")
    }

    fn switch_to(username: &str, at_ms: i64) -> AccountEdit {
        AccountEdit::Switch {
            username: username.to_string(),
            at_ms,
        }
    }

    fn set_tracked(username: &str, tracked: bool) -> AccountEdit {
        AccountEdit::SetTracked {
            username: username.to_string(),
            tracked,
        }
    }

    fn add_arena_account(conn: &Connection, username: &str, first_seen_at: i64) -> i64 {
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at, platform, arena_user_id, last_login_at)
             VALUES (?1, ?2, 'arena', ?1, ?2)",
            params![username, first_seen_at],
        )
        .expect("arena account");
        conn.last_insert_rowid()
    }

    #[test]
    fn account_list_skips_arena_accounts() {
        let (_dir, conn) = db("accounts-list-arena");
        add_arena_account(&conn, "Qarena#12345", 900);
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at) VALUES ('Qmtgo', 0)",
            [],
        )
        .expect("mtgo account");
        let names: Vec<String> = accounts(&conn, Platform::Mtgo)
            .expect("list")
            .into_iter()
            .map(|row| row.username)
            .collect();
        assert!(names.contains(&"Qmtgo".to_string()));
        assert!(!names.iter().any(|name| name.starts_with("Qarena")));
    }

    #[test]
    fn current_account_ignores_arena_accounts() {
        let (_dir, conn) = db("accounts-arena-only");
        add_arena_account(&conn, "Qarena#12345", 900);
        assert_eq!(
            current_account_id(&conn, Platform::Mtgo).expect("read"),
            None,
            "an Arena account alone is never current"
        );

        let (_dir, mut conn) = db("accounts-arena-and-mtgo");
        add_login(&mut conn, "Qplayer", 100);
        let mtgo = conn
            .query_row(
                "SELECT id FROM accounts WHERE username = 'Qplayer'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("id");
        add_arena_account(&conn, "Qarena#12345", 900);
        assert_eq!(
            current_account_id(&conn, Platform::Mtgo).expect("read"),
            Some(mtgo)
        );
    }

    #[test]
    fn one_current_account_and_a_login_activates() {
        let (_dir, mut conn) = db("accounts-login");
        assert_eq!(
            current_account_id(&conn, Platform::Mtgo).expect("read"),
            None,
            "no account"
        );
        let a = add_account(&conn, "Qplayer");
        assert_eq!(
            current_account_id(&conn, Platform::Mtgo).expect("read"),
            Some(a),
            "the only one"
        );
        let b = add_account(&conn, "Qsecond");
        assert_eq!(
            current_account_id(&conn, Platform::Mtgo).expect("read"),
            None,
            "two, no login"
        );
        add_login(&mut conn, "Qplayer", 1_000);
        assert_eq!(
            current_account_id(&conn, Platform::Mtgo).expect("read"),
            Some(a)
        );
        add_login(&mut conn, "Qsecond", 2_000);
        assert_eq!(
            current_account_id(&conn, Platform::Mtgo).expect("read"),
            Some(b)
        );
    }

    #[test]
    fn switching_replaces_the_current_account_until_the_next_login() {
        let (_dir, mut conn) = db("accounts-switch");
        let a = add_account(&conn, "Qplayer");
        let b = add_account(&conn, "Qsecond");
        add_login(&mut conn, "Qplayer", 1_000);
        assert_eq!(
            edit(&mut conn, switch_to("Qsecond", 2_000)),
            AccountEditOutcome::Done {
                retrack: Vec::new()
            }
        );
        assert_eq!(
            current_account_id(&conn, Platform::Mtgo).expect("read"),
            Some(b)
        );
        assert_eq!(
            active_account(&conn).expect("read"),
            switch(b, 2_000),
            "the setting holds the switch"
        );
        add_login(&mut conn, "Qplayer", 3_000);
        assert_eq!(
            current_account_id(&conn, Platform::Mtgo).expect("read"),
            Some(a)
        );
    }

    // 0.x `SwitchAccountController`: `exists:accounts,username`.
    #[test]
    fn switching_to_an_unknown_username_is_refused() {
        let (_dir, mut conn) = db("accounts-switch-unknown");
        add_account(&conn, "Qplayer");
        assert_eq!(
            edit(&mut conn, switch_to("Qnobody", 2_000)),
            AccountEditOutcome::NotFound
        );
        assert_eq!(active_account(&conn).expect("read"), None, "nothing stored");
        assert_eq!(
            AccountEditOutcome::NotFound.refusal(),
            Some("The selected username is invalid.")
        );
    }

    // 0.x `UpdateAccountTrackingController`.
    #[test]
    fn tracking_is_stored_per_account_and_unknown_names_are_refused() {
        let (_dir, mut conn) = db("accounts-tracking");
        let a = add_account(&conn, "Qplayer");
        let b = add_account(&conn, "Qsecond");
        assert_eq!(
            edit(&mut conn, set_tracked("Qsecond", false)),
            AccountEditOutcome::Done {
                retrack: Vec::new()
            }
        );
        assert_eq!(
            accounts(&conn, Platform::Mtgo).expect("read"),
            vec![
                AccountRow {
                    id: a,
                    username: "Qplayer".to_string(),
                    tracked: true,
                    label: "Qplayer".to_string(),
                },
                AccountRow {
                    id: b,
                    username: "Qsecond".to_string(),
                    tracked: false,
                    label: "Qsecond".to_string(),
                },
            ]
        );
        assert_eq!(untracked_usernames(&conn).expect("read"), ["Qsecond"]);
        assert_eq!(
            edit(&mut conn, set_tracked("Qnobody", false)),
            AccountEditOutcome::NotFound
        );
        // Untracked is never current: the only tracked account is.
        add_login(&mut conn, "Qsecond", 5_000);
        assert_eq!(
            current_account_id(&conn, Platform::Mtgo).expect("read"),
            Some(a)
        );
    }

    #[test]
    fn the_newest_login_of_a_tracked_account_counts() {
        let (_dir, mut conn) = db("accounts-tracked-login");
        let a = add_account(&conn, "Qplayer");
        add_account(&conn, "Qsecond");
        add_account(&conn, "Qthird");
        add_login(&mut conn, "Qplayer", 1_000);
        add_login(&mut conn, "Qsecond", 2_000);
        edit(&mut conn, set_tracked("Qsecond", false));
        assert_eq!(
            latest_tracked_login(&conn).expect("read"),
            login(a, 1_000),
            "an older login of a tracked account"
        );
        assert_eq!(
            current_account_id(&conn, Platform::Mtgo).expect("read"),
            Some(a)
        );
    }

    /// A file caught up later can store an older Login after a newer one:
    /// the newest log time is kept.
    #[test]
    fn an_older_login_read_later_never_moves_the_newest_back() {
        let (_dir, mut conn) = db("accounts-older-login");
        let a = add_account(&conn, "Qplayer");
        let b = add_account(&conn, "Qsecond");
        add_login(&mut conn, "Qplayer", 3_000);
        add_login(&mut conn, "Qsecond", 2_000);
        add_login(&mut conn, "Qplayer", 1_000);
        assert_eq!(latest_tracked_login(&conn).expect("read"), login(a, 3_000));
        add_login(&mut conn, "Qsecond", 4_000);
        assert_eq!(latest_tracked_login(&conn).expect("read"), login(b, 4_000));
    }

    /// The skip rows stay until each rebuilt match is written (its `Upsert`
    /// or `Remove` clears it), so a restart before the rebuild finishes
    /// finds them again (`retrack_queue`).
    #[test]
    fn retracking_hands_back_the_skipped_tokens_and_keeps_them_until_rebuilt() {
        let (_dir, mut conn) = db("accounts-retrack");
        let a = add_account(&conn, "Qplayer");
        let b = add_account(&conn, "Qsecond");
        edit(&mut conn, set_tracked("Qplayer", false));
        edit(&mut conn, set_tracked("Qsecond", false));
        for (token, account) in [("Q-2", a), ("Q-1", a), ("Q-3", b)] {
            conn.execute(
                "INSERT INTO untracked_matches (match_token, account_id, skipped_at)
                 VALUES (?1, ?2, 1)",
                rusqlite::params![token, account],
            )
            .expect("skipped");
        }
        assert!(
            retrack_queue(&conn).expect("read").is_empty(),
            "both untracked"
        );
        assert_eq!(
            edit(&mut conn, set_tracked("Qplayer", true)),
            AccountEditOutcome::Done {
                retrack: vec!["Q-1".to_string(), "Q-2".to_string()]
            }
        );
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM untracked_matches"),
            3,
            "kept until each match is rebuilt"
        );
        assert_eq!(retrack_queue(&conn).expect("read"), ["Q-1", "Q-2"]);
        assert_eq!(untracked_usernames(&conn).expect("read"), ["Qsecond"]);
    }

    #[test]
    fn a_skipped_match_is_recorded_once_and_cleared_by_its_row_or_its_removal() {
        let (_dir, mut conn) = db("accounts-skip-writes");
        let a = add_account(&conn, "Qlocal");
        let skip = |token: &str, now_ms: i64| WriteBatch {
            matches: vec![MatchWrite::Skip {
                token: token.to_string(),
                username: "Qlocal".to_string(),
            }],
            now_ms,
            ..WriteBatch::default()
        };
        commit(&mut conn, &skip(TOKEN, 10)).expect("skip");
        commit(&mut conn, &skip(TOKEN, 20)).expect("skip again");
        commit(&mut conn, &skip(OTHER, 30)).expect("skip another");
        assert_eq!(
            dump(&conn, "untracked_matches"),
            vec![
                vec![
                    format!("Text({OTHER:?})"),
                    format!("Integer({a})"),
                    "Integer(30)".to_string()
                ],
                vec![
                    format!("Text({TOKEN:?})"),
                    format!("Integer({a})"),
                    "Integer(10)".to_string()
                ],
            ],
            "the first skip's time is kept"
        );
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM matches"),
            0,
            "no row built"
        );
        // Built after all (tracking came back): never both.
        commit(
            &mut conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(view(TOKEN, "Qopponent", 1_000)),
                    revision: 1,
                }],
                ..WriteBatch::default()
            },
        )
        .expect("built");
        // No longer a match at all (a phantom): its skip goes too.
        commit(
            &mut conn,
            &WriteBatch {
                matches: vec![MatchWrite::Remove {
                    token: OTHER.to_string(),
                }],
                ..WriteBatch::default()
            },
        )
        .expect("removed");
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM untracked_matches"), 0);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), 1);
    }

    /// Tracked again, a skipped match that fails is a failure, not a skip:
    /// its skip row would queue it again at every start.
    #[test]
    fn a_skipped_match_that_fails_loses_its_skip_row() {
        let (_dir, mut conn) = db("accounts-skip-then-fail");
        add_account(&conn, "Qlocal");
        commit(
            &mut conn,
            &WriteBatch {
                matches: vec![MatchWrite::Skip {
                    token: TOKEN.to_string(),
                    username: "Qlocal".to_string(),
                }],
                now_ms: 10,
                ..WriteBatch::default()
            },
        )
        .expect("skip");
        commit(
            &mut conn,
            &WriteBatch {
                matches: vec![MatchWrite::Attention {
                    token: TOKEN.to_string(),
                    reason: "projection panicked: Q boom".to_string(),
                }],
                now_ms: 20,
                ..WriteBatch::default()
            },
        )
        .expect("failed");
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM untracked_matches"), 0);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM match_failures"), 1);
    }

    #[test]
    fn imports_ignore_the_tracked_flag() {
        let (_dir, mut conn) = db("accounts-imports");
        add_account(&conn, "Qlocal");
        edit(&mut conn, set_tracked("Qlocal", false));
        let counts = import_matches(&mut conn, &[imported(TOKEN, Some(77))], 5).expect("import");
        assert_eq!(counts.added, 1);
        assert_eq!(
            number(
                &conn,
                &format!("SELECT COUNT(*) FROM matches WHERE token = '{TOKEN}'")
            ),
            1,
            "a 0.x import is a user action: stored for an untracked account"
        );
        insert_manual(&conn, OTHER);
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM matches"),
            2,
            "so is a manual match"
        );
        let history = crate::history_import::HistoryMatch {
            token: None,
            mtgo_match_id: 88,
            local_username: "Qlocal".to_string(),
            opponent: Some("Qopponent".to_string()),
            format: Some("CMODERN".to_string()),
            match_type: "Constructed".to_string(),
            outcome: "win".to_string(),
            started_at_ms: 50_000,
            ended_at_ms: None,
            games_won: 2,
            games_lost: 0,
            games: Vec::new(),
            deck_version_id: 999,
            log: Vec::new(),
        };
        crate::history_import::import_history(&mut conn, &[history], 6).expect("history");
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE source = 'history'"
            ),
            1,
            "and so is a history import"
        );
    }
    fn add_arena_match(conn: &Connection, token: &str, account_id: i64, started_at: i64) {
        conn.execute(
            "INSERT INTO matches (token, account_id, source, format, match_type, state,
                 outcome, started_at, last_activity_at, revision, derived_version)
             VALUES (?1, ?2, 'log', 'ARENA_STANDARD', 'Ranked', 'complete', 'win', ?3, ?3, 1, 4)",
            params![token, account_id, started_at],
        )
        .expect("arena match");
    }

    #[test]
    fn arena_current_account_is_the_switch_then_the_newest_match() {
        let (_dir, mut conn) = db("accounts-arena-current");
        let a = add_arena_account(&conn, "Player#00001", 1);
        let b = add_arena_account(&conn, "Player#00002", 1);
        assert_eq!(
            current_account_id(&conn, Platform::Arena).expect("read"),
            None,
            "two accounts, no match"
        );
        add_arena_match(&conn, "m1", a, 1_000);
        add_arena_match(&conn, "m2", b, 2_000);
        assert_eq!(
            current_account_id(&conn, Platform::Arena).expect("read"),
            Some(b),
            "the newest match's account"
        );
        let outcome = edit(
            &mut conn,
            AccountEdit::Switch {
                username: "Player#00001".to_string(),
                at_ms: 3_000,
            },
        );
        assert_eq!(
            outcome,
            AccountEditOutcome::Done {
                retrack: Vec::new()
            }
        );
        assert_eq!(
            current_account_id(&conn, Platform::Arena).expect("read"),
            Some(a),
            "the switch"
        );
        add_arena_match(&conn, "m3", b, 4_000);
        assert_eq!(
            current_account_id(&conn, Platform::Arena).expect("read"),
            Some(b),
            "a match newer than the switch"
        );
    }

    #[test]
    fn the_current_endstep_account_is_its_newest_match() {
        let (_dir, mut conn) = db("accounts-endstep-current");
        let arena = add_arena_account(&conn, "Player#00001", 1);
        let mut endstep_ids = Vec::new();
        for username in ["endstep:1", "endstep:2"] {
            conn.execute(
                "INSERT INTO accounts (username, first_seen_at, platform, display_name)
                 VALUES (?1, 1, 'endstep', 'Player')",
                [username],
            )
            .expect("endstep account");
            endstep_ids.push(conn.last_insert_rowid());
        }
        let (first, second) = (endstep_ids[0], endstep_ids[1]);
        assert_eq!(
            current_account_id(&conn, Platform::Endstep).expect("read"),
            None,
            "two accounts, no match"
        );
        add_arena_match(&conn, "a1", arena, 9_000);
        add_arena_match(&conn, "e1", first, 1_000);
        add_arena_match(&conn, "e2", second, 2_000);
        assert_eq!(
            current_account_id(&conn, Platform::Endstep).expect("read"),
            Some(second),
            "the newest Endstep match's account, never an Arena one"
        );
        assert_eq!(
            current_account_id(&conn, Platform::Arena).expect("read"),
            Some(arena),
            "Arena reads only Arena matches"
        );
        edit(&mut conn, switch_to("endstep:1", 3_000));
        assert_eq!(
            current_account_id(&conn, Platform::Endstep).expect("read"),
            Some(first),
            "the switch"
        );
        assert_eq!(
            current_account_id(&conn, Platform::Arena).expect("read"),
            Some(arena),
            "an Endstep switch is not an Arena switch"
        );
    }

    #[test]
    fn switching_an_arena_account_keeps_the_mtgo_switch() {
        let (_dir, mut conn) = db("accounts-arena-switch-keeps-mtgo");
        let mtgo = add_account(&conn, "Qplayer");
        add_account(&conn, "Qsecond");
        let arena = add_arena_account(&conn, "Player#00001", 1);
        add_arena_account(&conn, "Player#00002", 1);
        edit(&mut conn, switch_to("Qplayer", 2_000));
        edit(&mut conn, switch_to("Player#00001", 2_001));
        assert_eq!(
            current_account_id(&conn, Platform::Mtgo).expect("read"),
            Some(mtgo)
        );
        assert_eq!(
            current_account_id(&conn, Platform::Arena).expect("read"),
            Some(arena)
        );
    }

    #[test]
    fn switch_platform_is_stored() {
        let (_dir, mut conn) = db("accounts-switch-platform");
        let outcome = edit(
            &mut conn,
            AccountEdit::SwitchPlatform {
                platform: Platform::Arena,
            },
        );
        assert_eq!(
            outcome,
            AccountEditOutcome::Done {
                retrack: Vec::new()
            }
        );
        assert_eq!(
            crate::platform::stored_platform(&conn).expect("read"),
            Some(Platform::Arena)
        );
    }

    #[test]
    fn accounts_lists_one_platform() {
        let (_dir, conn) = db("accounts-lists-one-platform");
        add_account(&conn, "Qplayer");
        add_arena_account(&conn, "Player#00001", 1);
        let names = |platform| -> Vec<String> {
            accounts(&conn, platform)
                .expect("list")
                .into_iter()
                .map(|row| row.username)
                .collect()
        };
        assert_eq!(names(Platform::Mtgo), vec!["Qplayer".to_string()]);
        assert_eq!(names(Platform::Arena), vec!["Player#00001".to_string()]);
    }

    /// An untagged Arena account (`arena:<user id>`) sorts by its shown
    /// name, not before every letter, and case does not split the list
    /// (#633).
    #[test]
    fn accounts_sort_by_their_shown_label() {
        let (_dir, conn) = db("accounts-sort-by-label");
        add_arena_account(&conn, "Qcharlie#00003", 1);
        add_arena_account(&conn, "arena:00000000-0000-0000-0000-000000000002", 2);
        conn.execute(
            "UPDATE accounts SET display_name = 'Qbravo' WHERE username LIKE 'arena:%'",
            [],
        )
        .expect("display name");
        add_arena_account(&conn, "qalpha#00001", 3);
        let labels: Vec<String> = accounts(&conn, Platform::Arena)
            .expect("list")
            .into_iter()
            .map(|row| row.label)
            .collect();
        assert_eq!(
            labels,
            vec![
                "qalpha#00001".to_string(),
                "Qbravo".to_string(),
                "Qcharlie#00003".to_string(),
            ]
        );
    }
}
