//! Reading the 0.x accounts (Plan O, audit sections 4.1 and 4.2): every
//! account not deleted, with its login id and creation time, and the local
//! player of a match with no local seat. Values are read loosely, as in
//! `read`.

use std::collections::HashMap;

use rusqlite::Connection;
use store::import::ImportedAccount;

use crate::read::{ImportError, read_error};
use crate::sql::{
    ACCOUNT_DELETED_AT, as_bool, as_integer, as_text, has_all, parse_utc_ms, select_list,
    table_columns, value, where_account_not_deleted,
};

/// `accounts` columns every 0.x with accounts has, read before
/// `OPTIONAL_ACCOUNT_COLUMNS`.
const ACCOUNT_KEYS: [&str; 2] = ["id", "username"];

/// `accounts` columns read when present, `NULL` otherwise, in the order
/// `read_accounts` reads them.
const OPTIONAL_ACCOUNT_COLUMNS: [&str; 2] = ["login_id", "created_at"];

/// `accounts` columns of tracking and the active account (issue #310), read
/// when present after `OPTIONAL_ACCOUNT_COLUMNS`: without `tracked` every
/// account is tracked, without `active` none is active.
const STATE_ACCOUNT_COLUMNS: [&str; 3] = ["tracked", "active", "updated_at"];

/// `decks` columns joined on to find a deck's account.
const DECK_COLUMNS: [&str; 2] = ["id", "account_id"];

/// `deck_versions` columns joined on to find a version's deck.
const VERSION_COLUMNS: [&str; 2] = ["id", "deck_id"];

/// Every `(table, columns)` this reader selects or joins on, for the
/// coverage ledger.
pub const READ_COLUMNS: [(&str, &[&str]); 6] = [
    ("accounts", &ACCOUNT_KEYS),
    ("accounts", &OPTIONAL_ACCOUNT_COLUMNS),
    ("accounts", &[ACCOUNT_DELETED_AT]),
    ("accounts", &STATE_ACCOUNT_COLUMNS),
    ("decks", &DECK_COLUMNS),
    ("deck_versions", &VERSION_COLUMNS),
];

/// One `accounts` row, loosely typed.
struct AccountRow {
    username: Option<String>,
    login_id: Option<i64>,
    created_at: Option<String>,
    tracked: Option<bool>,
    active: Option<bool>,
    updated_at: Option<String>,
}

/// Where a match without a local seat finds its account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AccountFallback {
    /// The account name of each 0.x deck version's deck, by version id.
    pub by_version: HashMap<i64, String>,
    /// The only account 0.x has, when it has exactly one and that one is not
    /// deleted. With a deleted account beside an active one, a match could be
    /// either's, so there is no only account (issue #190).
    pub only: Option<String>,
    /// The only account 0.x has not deleted, deleted ones left out: the
    /// player of a manual match whose seat is 0.x's "You" placeholder (that
    /// match was entered on this install, so not on a deleted account).
    pub only_active: Option<String>,
}

/// Every account not deleted (`where_account_not_deleted`), in id order. Empty when the table is missing
/// (an old 0.x); a missing optional column reads as `NULL`.
pub(crate) fn read_accounts(conn: &Connection) -> Result<Vec<ImportedAccount>, ImportError> {
    let present = table_columns(conn, "accounts")?;
    if !has_all(&present, &ACCOUNT_KEYS) {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT \"username\", {}, {} FROM accounts{} ORDER BY \"id\"",
        select_list(&present, &OPTIONAL_ACCOUNT_COLUMNS),
        select_list(&present, &STATE_ACCOUNT_COLUMNS),
        where_account_not_deleted(&present, "")
    );
    let mut stmt = conn.prepare(&sql).map_err(read_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(AccountRow {
                username: as_text(value(row, 0)?),
                login_id: as_integer(value(row, 1)?),
                created_at: as_text(value(row, 2)?),
                tracked: as_bool(value(row, 3)?),
                active: as_bool(value(row, 4)?),
                updated_at: as_text(value(row, 5)?),
            })
        })
        .map_err(read_error)?;
    let mut accounts = Vec::new();
    for row in rows {
        let AccountRow {
            username,
            login_id,
            created_at,
            tracked,
            active,
            updated_at,
        } = row.map_err(read_error)?;
        let Some(username) = username else {
            continue;
        };
        if username.trim().is_empty() {
            continue;
        }
        let created_at_ms = created_at.as_deref().and_then(parse_utc_ms);
        // When 0.x last made it active: `activate()` updates the row.
        let active_since_ms = if active == Some(true) {
            Some(
                updated_at
                    .as_deref()
                    .and_then(parse_utc_ms)
                    .or(created_at_ms)
                    .unwrap_or(0),
            )
        } else {
            None
        };
        accounts.push(ImportedAccount {
            username,
            login_id,
            created_at_ms,
            tracked: tracked != Some(false),
            active_since_ms,
        });
    }
    Ok(accounts)
}

/// The account fallbacks: each deck version's account (deleted accounts
/// left out), and the only account of `accounts` (the accounts not deleted)
/// when 0.x has exactly one account, deleted ones counted.
pub(crate) fn read_account_fallback(
    conn: &Connection,
    accounts: &[ImportedAccount],
) -> Result<AccountFallback, ImportError> {
    let only_active = if accounts.len() == 1 {
        Some(accounts[0].username.clone())
    } else {
        None
    };
    let only = if account_rows(conn)? == 1 {
        only_active.clone()
    } else {
        None
    };
    Ok(AccountFallback {
        by_version: read_version_accounts(conn)?,
        only,
        only_active,
    })
}

/// How many named rows `accounts` holds, deleted ones too (0 without the
/// table).
fn account_rows(conn: &Connection) -> Result<i64, ImportError> {
    if table_columns(conn, "accounts")?.is_empty() {
        return Ok(0);
    }
    conn.query_row(
        "SELECT COUNT(*) FROM accounts WHERE trim(COALESCE(\"username\", '')) <> ''",
        [],
        |row| row.get(0),
    )
    .map_err(read_error)
}

/// Each deck version's account name, when the deck has an account that is
/// not deleted (0.x's soft delete leaves a deleted account out of a deck's
/// `account`). Empty when a table or column is missing (an old 0.x).
fn read_version_accounts(conn: &Connection) -> Result<HashMap<i64, String>, ImportError> {
    let mut found = HashMap::new();
    let accounts = table_columns(conn, "accounts")?;
    let decks = table_columns(conn, "decks")?;
    let versions = table_columns(conn, "deck_versions")?;
    let needed: [(&Vec<String>, &[&str]); 3] = [
        (&accounts, &ACCOUNT_KEYS),
        (&decks, &DECK_COLUMNS),
        (&versions, &VERSION_COLUMNS),
    ];
    for (present, columns) in needed {
        if !has_all(present, columns) {
            return Ok(found);
        }
    }
    let not_deleted = where_account_not_deleted(&accounts, "a.");
    let sql = format!(
        "SELECT v.\"id\", a.\"username\" FROM deck_versions v
         JOIN decks d ON d.\"id\" = v.\"deck_id\"
         JOIN accounts a ON a.\"id\" = d.\"account_id\"{not_deleted}"
    );
    let mut stmt = conn.prepare(&sql).map_err(read_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((as_integer(value(row, 0)?), as_text(value(row, 1)?)))
        })
        .map_err(read_error)?;
    for row in rows {
        if let (Some(version_id), Some(username)) = row.map_err(read_error)?
            && !username.trim().is_empty()
        {
            found.insert(version_id, username);
        }
    }
    Ok(found)
}
