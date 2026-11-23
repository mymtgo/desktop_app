//! A pulled row's player fields (league self-management spec 6.3): applied
//! by client id to the local row of any source, the newest `edited_at`
//! winning for the fields as a whole. A pulled edit runs the owners' league
//! sync and becomes the row's pulled key, so it is not sent back; a newer
//! local edit marks the row changed, so it is sent again and the server
//! converges on it.
//!
//! Reads every row by token or client id, hidden and removed ones too
//! (read audit allowlist: sync).

use chrono_tz::Tz;
use mymtgo_api::SyncType;
use rusqlite::{OptionalExtension, Transaction, params};
use sync_bundles::PulledLeagueFields;

use crate::StoreError;
use crate::import::PulledMatch;
use crate::league_sync::{LeagueOwner, sync_owner};
use crate::sync::{league_own_key, match_touch_key};

/// Which copy's player fields win.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Newer {
    Pulled,
    Local,
    Same,
}

/// The newer `edited_at` wins; none is the oldest; a tie keeps the local
/// row (spec 6.3).
fn newer(local: Option<i64>, pulled: Option<i64>) -> Newer {
    if pulled > local {
        Newer::Pulled
    } else if local > pulled {
        Newer::Local
    } else {
        Newer::Same
    }
}

/// The local match a pulled one is: the live row with its token or MTGO
/// match id, else the row with its token.
struct LocalMatch {
    id: i64,
    token: String,
    account_id: i64,
    league_id: Option<i64>,
    /// Linked by hand or by the 0.x import (`league_linked = 1`).
    league_linked: bool,
    hidden_at: Option<i64>,
    edited_at: Option<i64>,
}

fn local_match(
    tx: &Transaction,
    token: &str,
    mtgo_match_id: Option<i64>,
) -> Result<Option<LocalMatch>, StoreError> {
    let token = crate::import::live_row_token(tx, token, mtgo_match_id)?
        .unwrap_or_else(|| token.to_string());
    Ok(tx
        .prepare_cached(
            "SELECT id, token, account_id, league_id, league_linked, hidden_at, edited_at
             FROM matches WHERE token = ?1",
        )?
        .query_row([&token], |row| {
            Ok(LocalMatch {
                id: row.get(0)?,
                token: row.get(1)?,
                account_id: row.get(2)?,
                league_id: row.get(3)?,
                league_linked: row.get(4)?,
                hidden_at: row.get(5)?,
                edited_at: row.get(6)?,
            })
        })
        .optional()?)
}

/// The owner whose league sync a league row is in.
fn league_owner(tx: &Transaction, league_id: i64) -> Result<LeagueOwner, StoreError> {
    let account: Option<i64> = tx
        .prepare_cached("SELECT account_id FROM leagues WHERE id = ?1")?
        .query_row([league_id], |row| row.get(0))
        .optional()?
        .flatten();
    Ok(account.map_or(LeagueOwner::Nobody, LeagueOwner::Account))
}

/// Sync each owner once.
fn sync_owners(
    tx: &Transaction,
    owners: &[LeagueOwner],
    timezone: Tz,
    now_ms: i64,
) -> Result<(), StoreError> {
    let mut done: Vec<LeagueOwner> = Vec::new();
    for owner in owners {
        if !done.contains(owner) {
            sync_owner(tx, *owner, timezone, now_ms)?;
            done.push(*owner);
        }
    }
    Ok(())
}

/// The row is the server's copy (with the pulled edit) until it is edited
/// here: its key is its pulled key.
fn set_pulled_key(
    tx: &Transaction,
    sync_type: SyncType,
    client_id: &str,
    key: &str,
) -> Result<(), StoreError> {
    tx.prepare_cached(
        "INSERT INTO synced_rows (type, client_id, pulled_key) VALUES (?1, ?2, ?3)
         ON CONFLICT (type, client_id) DO UPDATE SET pulled_key = excluded.pulled_key",
    )?
    .execute(params![sync_type.as_str(), client_id, key])?;
    Ok(())
}

/// A local edit newer than the pulled copy: built again at the next run and
/// sent with that run's time (`first_build_time`), so the server keeps it.
fn mark_changed(
    tx: &Transaction,
    sync_type: SyncType,
    client_id: &str,
    now_ms: i64,
) -> Result<(), StoreError> {
    tx.prepare_cached(
        "INSERT INTO synced_rows (type, client_id, changed_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (type, client_id) DO UPDATE SET changed_at = excluded.changed_at,
             built_hash = NULL, pulled_key = NULL",
    )?
    .execute(params![sync_type.as_str(), client_id, now_ms])?;
    Ok(())
}

/// The other league a pulled player link moves the match into, when the
/// move rules (4.1) allow it; else `None` (a refusal, or a league unknown
/// here, is logged). The rules see the match listed, whatever hidden mark
/// it has now (`move_refusal_judged_listed`); the caller writes the pulled
/// one.
fn link_target(
    tx: &Transaction,
    local: &LocalMatch,
    league_client_id: Option<&str>,
) -> Result<Option<i64>, StoreError> {
    let Some(league_id) = crate::import::pulled_league_id(tx, league_client_id)? else {
        tracing::warn!(
            token = %local.token,
            "a pulled league link names no league here; the match keeps its league"
        );
        return Ok(None);
    };
    if Some(league_id) == local.league_id {
        return Ok(None);
    }
    match crate::leagues::move_refusal_judged_listed(tx, local.id, league_id)? {
        None => Ok(Some(league_id)),
        Some(refusal) => {
            tracing::warn!(
                token = %local.token,
                ?refusal,
                "a pulled league link was refused; the match keeps its league"
            );
            Ok(None)
        }
    }
}

/// Apply a pulled match's player fields to its local row (spec 6.3).
/// `no_league` takes the match out of every league; a player link moves it
/// into the league its `league_client_id` names, when that is another
/// league and the move rules (4.1) allow it (else it is dropped and
/// logged); neither clears `no_league` and a player link here (one with an
/// edit time), so the fold places the match, and leaves a 0.x import link
/// to the import and the league sync.
///
/// The league fields are written before the hide (spec section 13, write
/// order), and a pulled Restore follows the local restore rule
/// (`match_edits::unhide`).
///
/// Neither copy edited (both `edited_at` NULL): a hide made before this
/// release has no edit time, so a pulled `hidden_at` on a listed local row
/// is written once, without one (`hide_without_edit`).
pub(crate) fn apply_match_fields(
    tx: &Transaction,
    pulled: &PulledMatch,
    timezone: Tz,
    now_ms: i64,
) -> Result<(), StoreError> {
    let Some(local) = local_match(tx, &pulled.imported.token, pulled.imported.mtgo_match_id)?
    else {
        return Ok(());
    };
    let fields = &pulled.player;
    match newer(local.edited_at, fields.edited_at_ms) {
        Newer::Same => return hide_without_edit(tx, &local, fields.hidden_at_ms, timezone, now_ms),
        Newer::Local => return mark_changed(tx, SyncType::Match, &local.token, now_ms),
        Newer::Pulled => {}
    }
    let mut owners = vec![LeagueOwner::Account(local.account_id)];
    if let Some(league_id) = local.league_id {
        owners.push(league_owner(tx, league_id)?);
    }
    // The move rules judge the match as a listed one, before the hide or
    // restore this edit carries: a match moved and then deleted on another
    // device (or restored and moved) moves here too.
    let link_to = if !fields.no_league && fields.league_linked {
        link_target(tx, &local, pulled.league_client_id.as_deref())?
    } else {
        None
    };
    tx.prepare_cached("UPDATE matches SET edited_at = ?2 WHERE id = ?1")?
        .execute(params![local.id, fields.edited_at_ms])?;
    if fields.no_league {
        tx.prepare_cached(
            "UPDATE matches SET no_league = 1, league_linked = 0, league_id = NULL
             WHERE id = ?1",
        )?
        .execute([local.id])?;
    } else {
        tx.prepare_cached("UPDATE matches SET no_league = 0 WHERE id = ?1")?
            .execute([local.id])?;
        if let Some(league_id) = link_to {
            tx.prepare_cached(
                "UPDATE matches SET league_linked = 1, league_id = ?2 WHERE id = ?1",
            )?
            .execute(params![local.id, league_id])?;
            owners.push(league_owner(tx, league_id)?);
        } else if !fields.league_linked && local.league_linked && local.edited_at.is_some() {
            // The other device's newer edit has no link: the player's link
            // here goes, and the fold places the match.
            tx.prepare_cached(
                "UPDATE matches SET league_linked = 0, league_id = NULL WHERE id = ?1",
            )?
            .execute([local.id])?;
        }
    }
    // The hide or restore last (spec section 13, write order): a hide
    // records the league just written as the one the match was deleted
    // from; a restore follows the restore rule, so a match whose recorded
    // league is deleted here comes back non-league, as a Restore here does.
    if local.hidden_at.is_some() && fields.hidden_at_ms.is_none() {
        crate::match_edits::unhide(tx, local.id)?;
    } else {
        tx.prepare_cached("UPDATE matches SET hidden_at = ?2 WHERE id = ?1")?
            .execute(params![local.id, fields.hidden_at_ms])?;
    }
    // Pushes run before pulls for each type, so this run's own edits are
    // already sent: from here the row is the server's copy (its key is its
    // pulled key) until it is edited again.
    sync_owners(tx, &owners, timezone, now_ms)?;
    let key = match_touch_key(tx, local.id)?;
    set_pulled_key(tx, SyncType::Match, &local.token, &key)
}

/// A pulled `hidden_at` without an edit time on either copy (a hide from
/// before this release) lands on a listed local row once, without
/// `edited_at`, then the owners' league sync. Its key becomes its pulled
/// key, so it is not sent back. A local hide is never undone here.
fn hide_without_edit(
    tx: &Transaction,
    local: &LocalMatch,
    pulled_hidden_at: Option<i64>,
    timezone: Tz,
    now_ms: i64,
) -> Result<(), StoreError> {
    let (Some(hidden_at), None, None) = (pulled_hidden_at, local.hidden_at, local.edited_at) else {
        return Ok(());
    };
    tx.prepare_cached("UPDATE matches SET hidden_at = ?2 WHERE id = ?1")?
        .execute(params![local.id, hidden_at])?;
    let mut owners = vec![LeagueOwner::Account(local.account_id)];
    if let Some(league_id) = local.league_id {
        owners.push(league_owner(tx, league_id)?);
    }
    sync_owners(tx, &owners, timezone, now_ms)?;
    let key = match_touch_key(tx, local.id)?;
    set_pulled_key(tx, SyncType::Match, &local.token, &key)
}

/// The row `apply_league_fields` writes: the one that stores `client_id`
/// (`sync_client_id`) first, else (an id from a device that has not stored
/// its ids yet) the one with its token and start.
fn local_league(tx: &Transaction, client_id: &str) -> Result<Option<LocalLeague>, StoreError> {
    let stored: Option<LocalLeague> = tx
        .prepare_cached(
            "SELECT id, account_id, edited_at FROM leagues WHERE sync_client_id = ?1
             ORDER BY detached_at IS NOT NULL, id LIMIT 1",
        )?
        .query_row([client_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()?;
    if stored.is_some() {
        return Ok(stored);
    }
    let Some((token, started_at)) = crate::import::split_league_client_id(client_id) else {
        return Ok(None);
    };
    Ok(tx
        .prepare_cached(
            "SELECT id, account_id, edited_at FROM leagues
             WHERE token = ?1 AND started_at / 1000 = ?2 / 1000
             ORDER BY detached_at IS NOT NULL, id LIMIT 1",
        )?
        .query_row(params![token, started_at], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()?)
}

/// A local league row for a pulled league's player fields: its id, account
/// and `edited_at`.
type LocalLeague = (i64, Option<i64>, Option<i64>);

/// Apply a pulled league's player fields to the local row that stores its
/// client id, else the row with its token and start (any source, removed
/// and detached rows too), then its owner's league sync (a drop feeds the
/// fold).
pub(crate) fn apply_league_fields(
    tx: &Transaction,
    client_id: &str,
    fields: &PulledLeagueFields,
    timezone: Tz,
    now_ms: i64,
) -> Result<(), StoreError> {
    let Some((league_id, account_id, edited_at)) = local_league(tx, client_id)? else {
        return Ok(());
    };
    match newer(edited_at, fields.edited_at_ms) {
        Newer::Same => return Ok(()),
        Newer::Local => return mark_changed(tx, SyncType::League, client_id, now_ms),
        Newer::Pulled => {}
    }
    // As for a match: pushes run before pulls for each type, so the pulled
    // key below marks the row as the server's copy until it is edited again.
    tx.prepare_cached(
        "UPDATE leagues SET removed_at = ?2, user_state = ?3, user_dropped_at = ?4,
             edited_at = ?5
         WHERE id = ?1",
    )?
    .execute(params![
        league_id,
        fields.removed_at_ms,
        fields.user_state,
        fields.user_dropped_at_ms,
        fields.edited_at_ms
    ])?;
    let owner = account_id.map_or(LeagueOwner::Nobody, LeagueOwner::Account);
    sync_owner(tx, owner, timezone, now_ms)?;
    let key = league_own_key(tx, league_id)?;
    set_pulled_key(tx, SyncType::League, client_id, &key)
}

/// Whether a website delete beats the local row (spec 6.4): a row never
/// edited here, or edited before the last run (`since_ms`, after which the
/// server deleted it). On a full run (`None`) an edited row keeps its edit:
/// it may be a restore made after the delete.
fn tombstone_applies(edited_at: Option<i64>, since_ms: Option<i64>) -> bool {
    match (edited_at, since_ms) {
        (None, _) => true,
        (Some(edited_at), Some(since_ms)) => edited_at <= since_ms,
        (Some(_), None) => false,
    }
}

/// Hide each listed match this device has (spec 6.4), without `edited_at`:
/// the server deleted it, so there is nothing to push. Its key becomes its
/// pulled key, so it leaves the manifest's dirty list. Returns how many
/// were hidden.
pub(crate) fn apply_tombstones(
    tx: &Transaction,
    client_ids: &[String],
    since_ms: Option<i64>,
    now_ms: i64,
) -> Result<u32, StoreError> {
    let mut hidden = 0;
    for token in client_ids {
        let row: Option<(i64, Option<i64>, Option<i64>)> = tx
            .prepare_cached("SELECT id, hidden_at, edited_at FROM matches WHERE token = ?1")?
            .query_row([token], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .optional()?;
        let Some((match_id, hidden_at, edited_at)) = row else {
            continue;
        };
        if !tombstone_applies(edited_at, since_ms) {
            continue;
        }
        if hidden_at.is_none() {
            tx.prepare_cached("UPDATE matches SET hidden_at = ?2 WHERE id = ?1")?
                .execute(params![match_id, now_ms])?;
            hidden += 1;
        }
        let key = match_touch_key(tx, match_id)?;
        set_pulled_key(tx, SyncType::Match, token, &key)?;
    }
    Ok(hidden)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tombstone_beats_only_an_edit_older_than_the_last_run() {
        assert!(tombstone_applies(None, None));
        assert!(tombstone_applies(None, Some(5)));
        assert!(tombstone_applies(Some(5), Some(5)));
        assert!(!tombstone_applies(Some(6), Some(5)));
        assert!(!tombstone_applies(Some(1), None));
    }

    #[test]
    fn the_newer_edit_wins_none_is_oldest_and_a_tie_keeps_the_local_row() {
        assert_eq!(newer(None, Some(1)), Newer::Pulled);
        assert_eq!(newer(Some(1), Some(2)), Newer::Pulled);
        assert_eq!(newer(Some(2), Some(1)), Newer::Local);
        assert_eq!(newer(Some(1), None), Newer::Local);
        assert_eq!(newer(Some(1), Some(1)), Newer::Same);
        assert_eq!(newer(None, None), Newer::Same);
    }
}
