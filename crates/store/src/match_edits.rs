//! A player's edits of one match (dashboard spec section 4): its notes,
//! "Delete match" (formerly "Remove from stats") and its Restore (league
//! self-management spec 4.5), its shared replay link (replays spec section 8)
//! and its deck (issue #332). Notes, the hidden mark and the share link are
//! input columns on the match row (`matches.notes`, `matches.hidden_at`,
//! `matches.replay_share_*`) that no derivation writes, so a rebuild keeps
//! them.
//!
//! Delete and Restore set `matches.edited_at`, the time of the player's last
//! edit, which sync compares (league self-management spec 6).
//!
//! The deck link (`matches.deck_version_id`) set here survives a rebuild and
//! a relink of a log or manual match: the deck linker and the 0.x deck import
//! only fill a match without a link, and a live match taking over a copy
//! keeps its own. Two writers can replace it:
//! - a sync pull rewrites the link of a pulled, imported or history match it
//!   adds or updates (the server's copy wins; such a match is not pushed, so
//!   a deck set here on it stays local until then);
//! - a 0.x import that carries a deck link clears the link of a history copy
//!   of the same match (ruling I1: 0.x's link wins), so the 0.x link replaces
//!   it.
//!
//! A log or manual match's link is part of its sync touch key, so a new deck
//! is pushed.

use rusqlite::{OptionalExtension, Transaction, params};

use crate::{Connection, StoreError};

/// The longest match note, in characters (0.x `max:5000`).
pub const MAX_NOTES_CHARS: usize = 5000;

/// A user edit of one match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchEdit {
    /// 0.x `UpdateNotesController` (`nullable|string|max:5000`): trimmed, as
    /// Laravel trims every string; empty clears.
    SetNotes { token: String, notes: String },
    /// "Delete match" (0.x `DeleteController`, as a hide, spec ruling 2):
    /// every read that lists or counts matches leaves the match out from
    /// now on, and `edited_at` records the edit. Hiding a hidden match is
    /// done and keeps its first time.
    Hide { token: String, now_ms: i64 },
    /// "Restore" in Settings, Archives, Deleted, or the Undo after "Delete
    /// match" (league self-management spec 4.5): the match is listed again
    /// and `edited_at` records the edit. Restoring a listed match is done
    /// and writes nothing.
    Restore { token: String, now_ms: i64 },
    /// Store the match's shared replay link after an upload (replays spec
    /// section 8): every share uploads again, so a new one replaces the
    /// stored link and its time. A hidden match takes it too (hiding never
    /// revokes, ruling 11).
    SetReplayShare {
        token: String,
        uuid: String,
        url: String,
        shared_at_ms: i64,
    },
    /// Forget the match's shared replay link once it is switched off:
    /// all three columns cleared.
    ClearReplayShare { token: String },
    /// "Set deck" on the Matches page (issue #332): the matches were played
    /// with this deck version, replacing any link (see the module doc for the
    /// writers that can replace it in turn). A missing, detached or hidden
    /// match is skipped (`NotFound` when that leaves none). Refused, writing
    /// nothing, when the version is gone, its deck is a Limited deck, a match
    /// is a Limited match, the deck's format is not a match's, or the deck
    /// belongs to another account than a match's. A match whose link changes
    /// is asked for detection again unless its player seat is hand-set (its
    /// seat follows the deck, as a deck archetype change does), and a
    /// history match has the imported postboard rule applied again for the
    /// new list. When the new list cannot be read (an empty or undecodable
    /// signature) the rule is not applied and the game rows keep the deck
    /// columns the previous list gave them: rows from a list the rule could
    /// read beat rows with no list at all.
    SetDeckVersion {
        tokens: Vec<String>,
        deck_version_id: i64,
    },
}

/// What happened to a `MatchEdit`. Only `Done` changed anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchEditOutcome {
    Done,
    /// There is no such match (a live one not stored yet, or one gone), or it
    /// was removed from stats.
    NotFound,
    NotesTooLong,
    /// The deck version chosen no longer exists.
    DeckVersionNotFound,
    /// The deck chosen belongs to another account than the match.
    OtherAccountsDeck,
    /// The deck chosen is a Limited (draft or sealed) deck.
    LimitedDeck,
    /// The match is a Limited (draft or sealed) match.
    LimitedMatch,
    /// The deck's format is not the match's.
    FormatMismatch,
    /// Restore of a match deleted together with its league while the
    /// league stays deleted (issue #418): only the league's Restore brings
    /// it back.
    DeletedWithLeague,
}

impl MatchEditOutcome {
    /// The message to show for a refusal (0.x's where 0.x has one), `None`
    /// for `Done`.
    pub fn refusal(self) -> Option<&'static str> {
        match self {
            MatchEditOutcome::Done => None,
            MatchEditOutcome::NotFound => {
                Some("This match is not stored yet, or it no longer exists.")
            }
            MatchEditOutcome::NotesTooLong => {
                Some("The notes field must not be greater than 5000 characters.")
            }
            MatchEditOutcome::DeckVersionNotFound => {
                Some("That deck version no longer exists. Choose another one.")
            }
            MatchEditOutcome::OtherAccountsDeck => {
                Some("That deck belongs to another account than the match.")
            }
            MatchEditOutcome::LimitedDeck => Some("A Limited deck cannot be set on a match."),
            MatchEditOutcome::LimitedMatch => {
                Some("A Limited match is played with its draft or sealed pool, not a deck.")
            }
            MatchEditOutcome::FormatMismatch => {
                Some("That deck is for another format than the match.")
            }
            MatchEditOutcome::DeletedWithLeague => Some("Restore its league to bring it back."),
        }
    }
}

/// Apply a user's match edit in its own transaction.
pub fn edit_match(conn: &mut Connection, edit: &MatchEdit) -> Result<MatchEditOutcome, StoreError> {
    let tx = conn.transaction()?;
    let outcome = match edit {
        MatchEdit::SetNotes { token, notes } => set_notes(&tx, token, notes)?,
        MatchEdit::Hide { token, now_ms } => hide(&tx, token, *now_ms)?,
        MatchEdit::Restore { token, now_ms } => restore(&tx, token, *now_ms)?,
        MatchEdit::SetReplayShare {
            token,
            uuid,
            url,
            shared_at_ms,
        } => set_replay_share(&tx, token, uuid, url, *shared_at_ms)?,
        MatchEdit::ClearReplayShare { token } => clear_replay_share(&tx, token)?,
        MatchEdit::SetDeckVersion {
            tokens,
            deck_version_id,
        } => set_deck_version(&tx, tokens, *deck_version_id)?,
    };
    // A refusal wrote nothing, so committing it changes nothing either.
    tx.commit()?;
    Ok(outcome)
}

/// Whether a match row exists for `token`, hidden or not. A detached match
/// (no longer derived from the log) has none to edit.
fn has_row(tx: &Transaction, token: &str) -> Result<bool, StoreError> {
    let found: Option<i64> = tx
        .query_row(
            "SELECT id FROM matches WHERE token = ?1 AND detached_at IS NULL",
            [token],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

fn set_notes(tx: &Transaction, token: &str, notes: &str) -> Result<MatchEditOutcome, StoreError> {
    if !has_row(tx, token)? || is_hidden(tx, token)? {
        return Ok(MatchEditOutcome::NotFound);
    }
    let notes = notes.trim();
    if notes.chars().count() > MAX_NOTES_CHARS {
        return Ok(MatchEditOutcome::NotesTooLong);
    }
    let notes = if notes.is_empty() { None } else { Some(notes) };
    tx.execute(
        "UPDATE matches SET notes = ?2 WHERE token = ?1",
        params![token, notes],
    )?;
    Ok(MatchEditOutcome::Done)
}

/// "Delete match". Setting `hidden_at` fires migration 0003's trigger,
/// which records the league the match is in (`deleted_from_league_id`).
fn hide(tx: &Transaction, token: &str, now_ms: i64) -> Result<MatchEditOutcome, StoreError> {
    if !has_row(tx, token)? {
        return Ok(MatchEditOutcome::NotFound);
    }
    tx.execute(
        "UPDATE matches SET hidden_at = ?2, edited_at = ?2 WHERE token = ?1 AND hidden_at IS NULL",
        params![token, now_ms],
    )?;
    Ok(MatchEditOutcome::Done)
}

/// "Restore" of a deleted match (league self-management spec 4.5).
///
/// Its league is the one it was in when it was deleted
/// (`deleted_from_league_id`, recorded by the schema's triggers; spec
/// section 13).
///
/// A match deleted together with that league ("Delete them too", see
/// `leagues::DELETED_WITH_LEAGUE`) cannot be restored on its own while the
/// league stays deleted (owner ruling, issue #418): it is refused and
/// nothing changes. The league's Restore brings it back with the rest of
/// the league, so the league comes back whole (same run key, notes and
/// status).
///
/// Otherwise it comes back by the restore rule (`unhide`).
fn restore(tx: &Transaction, token: &str, now_ms: i64) -> Result<MatchEditOutcome, StoreError> {
    if !has_row(tx, token)? {
        return Ok(MatchEditOutcome::NotFound);
    }
    let hidden: Option<i64> = tx
        .query_row(
            "SELECT id FROM matches
             WHERE token = ?1 AND hidden_at IS NOT NULL AND detached_at IS NULL",
            [token],
            |row| row.get(0),
        )
        .optional()?;
    let Some(match_id) = hidden else {
        return Ok(MatchEditOutcome::Done);
    };
    if crate::leagues::deleted_with_league(tx, match_id)? {
        return Ok(MatchEditOutcome::DeletedWithLeague);
    }
    tx.execute(
        "UPDATE matches SET edited_at = ?2 WHERE id = ?1",
        params![match_id, now_ms],
    )?;
    unhide(tx, match_id)?;
    Ok(MatchEditOutcome::Done)
}

/// The restore rule (spec 4.5 and section 13), one function for a Restore
/// here and a pulled Restore (`sync_player::apply_match_fields`): hidden
/// match `match_id` is listed again as it was just before its delete. When
/// the league it was deleted from (`deleted_from_league_id`) is deleted:
///
/// - Deleted before that league (`hidden_at < removed_at`), it was in a
///   listed league then, which is gone: it comes back as a non-league match,
///   as "Move to league..., No league" leaves it (`no_league = 1`, no link,
///   no `league_id`). A match is never linked to a deleted league (owner
///   rulings, issues #416 and #419): a link to it would be refused on the
///   other devices (`move_refusal`), and a log match would fold into a new
///   league. With these fields every device agrees.
/// - Deleted at or after that league's delete, it was a match the league
///   kept ("Keep as non-league matches"): it comes back as one, its
///   `league_id` the deleted league again (the league sync cleared it
///   while hidden), read as non-league. The league's Restore then shows it
///   in the league again. (At the same time is "Delete them too", which a
///   Restore here refuses; a pulled Restore cannot be refused.)
///
/// Clearing `hidden_at` fires migration 0003's trigger, which clears
/// `deleted_from_league_id`, so it is read first. The caller writes
/// `edited_at`.
pub(crate) fn unhide(tx: &Transaction, match_id: i64) -> Result<(), StoreError> {
    let recorded: Option<(i64, i64, i64)> = tx
        .query_row(
            "SELECT l.id, l.removed_at, m.hidden_at
             FROM matches m JOIN leagues l ON l.id = m.deleted_from_league_id
             WHERE m.id = ?1 AND l.removed_at IS NOT NULL AND m.hidden_at IS NOT NULL",
            [match_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    tx.execute(
        "UPDATE matches SET hidden_at = NULL WHERE id = ?1",
        [match_id],
    )?;
    let Some((league_id, removed_at, hidden_at)) = recorded else {
        return Ok(());
    };
    if hidden_at < removed_at {
        tx.execute(
            "UPDATE matches SET no_league = 1, league_linked = 0, league_id = NULL WHERE id = ?1",
            [match_id],
        )?;
    } else {
        tx.execute(
            "UPDATE matches SET league_id = ?2 WHERE id = ?1 AND no_league = 0",
            params![match_id, league_id],
        )?;
    }
    Ok(())
}

fn set_replay_share(
    tx: &Transaction,
    token: &str,
    uuid: &str,
    url: &str,
    shared_at_ms: i64,
) -> Result<MatchEditOutcome, StoreError> {
    if !has_row(tx, token)? {
        return Ok(MatchEditOutcome::NotFound);
    }
    tx.execute(
        "UPDATE matches SET replay_share_uuid = ?2, replay_share_url = ?3, replay_shared_at = ?4
         WHERE token = ?1",
        params![token, uuid, url, shared_at_ms],
    )?;
    Ok(MatchEditOutcome::Done)
}

fn clear_replay_share(tx: &Transaction, token: &str) -> Result<MatchEditOutcome, StoreError> {
    if !has_row(tx, token)? {
        return Ok(MatchEditOutcome::NotFound);
    }
    tx.execute(
        "UPDATE matches SET replay_share_uuid = NULL, replay_share_url = NULL,
             replay_shared_at = NULL
         WHERE token = ?1",
        [token],
    )?;
    Ok(MatchEditOutcome::Done)
}

fn set_deck_version(
    tx: &Transaction,
    tokens: &[String],
    deck_version_id: i64,
) -> Result<MatchEditOutcome, StoreError> {
    let deck: Option<(Option<i64>, String, String)> = tx
        .query_row(
            "SELECT d.account_id, d.format, d.mtgo_id
             FROM deck_versions v JOIN decks d ON d.id = v.deck_id
             WHERE v.id = ?1",
            [deck_version_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((deck_account, deck_format, deck_mtgo_id)) = deck else {
        return Ok(MatchEditOutcome::DeckVersionNotFound);
    };
    if decks::is_limited(&deck_format) || deck_mtgo_id.starts_with("limited:") {
        return Ok(MatchEditOutcome::LimitedDeck);
    }
    // Every check comes before the first write, so a refusal writes nothing.
    let mut match_ids: Vec<i64> = Vec::new();
    for token in tokens {
        let found: Option<(i64, i64, Option<String>)> = tx
            .query_row(
                "SELECT id, account_id, format FROM matches
                 WHERE token = ?1 AND detached_at IS NULL AND hidden_at IS NULL",
                [token],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((match_id, account_id, match_format)) = found else {
            continue;
        };
        if let Some(outcome) = format_refusal(match_format.as_deref(), &deck_format) {
            return Ok(outcome);
        }
        if deck_account.is_some_and(|deck_account| deck_account != account_id) {
            return Ok(MatchEditOutcome::OtherAccountsDeck);
        }
        if !match_ids.contains(&match_id) {
            match_ids.push(match_id);
        }
    }
    if match_ids.is_empty() {
        return Ok(MatchEditOutcome::NotFound);
    }
    for match_id in match_ids {
        let changed = tx.execute(
            "UPDATE matches SET deck_version_id = ?2
             WHERE id = ?1 AND deck_version_id IS NOT ?2",
            params![match_id, deck_version_id],
        )?;
        if changed == 0 {
            continue;
        }
        tx.execute(
            "UPDATE matches SET archetypes_detected_at = NULL
             WHERE id = ?1 AND archetypes_detected_at IS NOT NULL
               AND NOT EXISTS (SELECT 1 FROM match_archetypes x
                               WHERE x.match_id = matches.id AND x.is_opponent = 0
                                 AND x.manual = 1)",
            [match_id],
        )?;
        reapply_rule(tx, match_id)?;
    }
    Ok(MatchEditOutcome::Done)
}

/// Why a match of format `match_format` cannot take a deck of
/// `deck_format`, if it cannot: a Limited match takes no deck, and a known
/// format must be the deck's (compared without case). A match or deck with
/// no format is not checked.
fn format_refusal(match_format: Option<&str>, deck_format: &str) -> Option<MatchEditOutcome> {
    let match_format = match_format.map(str::trim).unwrap_or("");
    if decks::is_limited_match_format(match_format) {
        return Some(MatchEditOutcome::LimitedMatch);
    }
    let deck_format = deck_format.trim();
    if match_format.is_empty() || deck_format.is_empty() {
        return None;
    }
    if match_format.eq_ignore_ascii_case(deck_format) {
        None
    } else {
        Some(MatchEditOutcome::FormatMismatch)
    }
}

/// Apply the imported postboard rule again for a match's new deck list (it
/// leaves every match but a history one alone). A stored game log that
/// cannot be read is logged and its partial writes undone (a savepoint), as
/// the 0.x deck import does; a SQLite failure still fails the edit.
fn reapply_rule(tx: &Transaction, match_id: i64) -> Result<(), StoreError> {
    tx.execute_batch("SAVEPOINT set_deck_rule")?;
    match crate::imported_detail::reapply_imported_rule(tx, match_id) {
        Ok(_) => tx.execute_batch("RELEASE set_deck_rule")?,
        Err(StoreError::Sqlite(error)) => {
            tx.execute_batch("ROLLBACK TO set_deck_rule; RELEASE set_deck_rule")?;
            return Err(StoreError::Sqlite(error));
        }
        Err(error) => {
            tx.execute_batch("ROLLBACK TO set_deck_rule; RELEASE set_deck_rule")?;
            tracing::warn!(%error, match_id, "the imported postboard rule was not applied");
        }
    }
    Ok(())
}

/// Whether the player removed this match from stats.
pub fn is_hidden(conn: &Connection, token: &str) -> Result<bool, StoreError> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM matches WHERE token = ?1 AND hidden_at IS NOT NULL",
            [token],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

/// Whether a 0.x or history copy of Match ID `mtgo_match_id` under another
/// token than `token` is hidden: a live match taking it over is hidden too.
pub fn hidden_copy_exists(
    conn: &Connection,
    token: &str,
    mtgo_match_id: i64,
) -> Result<bool, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM matches
                 WHERE mtgo_match_id = ?2 AND source IN ('imported', 'history')
                   AND token <> ?1 AND hidden_at IS NOT NULL)",
        )?
        .query_row(params![token, mtgo_match_id], |row| row.get(0))?)
}

/// The tokens of the 0.x and history copies of Match ID `mtgo_match_id` under
/// another token than `token`: a live match with that ID replaces them.
pub fn copy_tokens_for_match_id(
    conn: &Connection,
    token: &str,
    mtgo_match_id: i64,
) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT token FROM matches
         WHERE mtgo_match_id = ?2 AND source IN ('imported', 'history') AND token <> ?1
         ORDER BY id",
    )?;
    let rows = stmt.query_map(params![token, mtgo_match_id], |row| row.get::<_, String>(0))?;
    let mut tokens = Vec::new();
    for row in rows {
        tokens.push(row?);
    }
    Ok(tokens)
}

/// Every hidden match token, sorted.
pub fn hidden_match_tokens(conn: &Connection) -> Result<Vec<String>, StoreError> {
    let mut stmt =
        conn.prepare("SELECT token FROM matches WHERE hidden_at IS NOT NULL ORDER BY token")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut tokens = Vec::new();
    for row in rows {
        tokens.push(row?);
    }
    Ok(tokens)
}

/// The match's note, `None` when it has none.
pub fn match_note(conn: &Connection, token: &str) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT notes FROM matches WHERE token = ?1",
            [token],
            |row| row.get(0),
        )
        .optional()?
        .flatten())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open_writer;
    use crate::read::list_matches;
    use crate::test_support::*;
    use crate::write::{MatchWrite, WriteBatch, commit};
    use fixture_tools::TestDir;

    fn db(label: &str) -> (TestDir, Connection) {
        let dir = TestDir::new(label);
        let mut conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
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
        .expect("a match");
        (dir, conn)
    }

    fn edit(conn: &mut Connection, edit: MatchEdit) -> MatchEditOutcome {
        edit_match(conn, &edit).expect("edit")
    }

    fn notes(token: &str, notes: &str) -> MatchEdit {
        MatchEdit::SetNotes {
            token: token.to_string(),
            notes: notes.to_string(),
        }
    }

    fn hide_edit(token: &str, now_ms: i64) -> MatchEdit {
        MatchEdit::Hide {
            token: token.to_string(),
            now_ms,
        }
    }

    fn restore_edit(token: &str, now_ms: i64) -> MatchEdit {
        MatchEdit::Restore {
            token: token.to_string(),
            now_ms,
        }
    }

    #[test]
    fn hiding_stamps_the_edit_and_restoring_lists_the_match_again() {
        let (_dir, mut conn) = db("match-edits-restore");
        let columns = |conn: &Connection| -> (Option<i64>, Option<i64>) {
            conn.query_row(
                "SELECT hidden_at, edited_at FROM matches WHERE token = ?1",
                [TOKEN],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("columns")
        };
        assert_eq!(edit(&mut conn, hide_edit(TOKEN, 5)), MatchEditOutcome::Done);
        assert_eq!(columns(&conn), (Some(5), Some(5)));
        assert_eq!(edit(&mut conn, hide_edit(TOKEN, 6)), MatchEditOutcome::Done);
        assert_eq!(
            columns(&conn),
            (Some(5), Some(5)),
            "a second hide writes nothing"
        );

        assert_eq!(
            edit(&mut conn, restore_edit(TOKEN, 7)),
            MatchEditOutcome::Done
        );
        assert_eq!(columns(&conn), (None, Some(7)));
        assert!(!is_hidden(&conn, TOKEN).expect("hidden"));
        assert_eq!(list_matches(&conn, 10, 0).expect("list").len(), 1);
        assert_eq!(
            edit(&mut conn, restore_edit(TOKEN, 8)),
            MatchEditOutcome::Done
        );
        assert_eq!(
            columns(&conn),
            (None, Some(7)),
            "restoring a listed match writes nothing"
        );
        assert_eq!(
            edit(&mut conn, restore_edit(OTHER, 8)),
            MatchEditOutcome::NotFound
        );
    }

    /// Issue #418 (owner ruling): a match deleted with its league ("Delete
    /// them too": hidden at the league's `removed_at` while in it; the
    /// league sync has cleared its league since) cannot be restored on its
    /// own while the league stays deleted: Restore is refused and writes
    /// nothing (the league's Restore brings it back). Issue #416 still holds
    /// for a match deleted on its own: it is never linked to a deleted
    /// league.
    #[test]
    fn restoring_a_match_deleted_with_its_league_is_refused() {
        let (_dir, mut conn) = db("match-edits-restore-with-league");
        let account = number(
            &conn,
            &format!("SELECT account_id FROM matches WHERE token = '{TOKEN}'"),
        );
        insert_manual_league(&conn, 4, Some(account), None, "Q deleted", 0, 0);
        conn.execute_batch(
            "UPDATE leagues SET removed_at = 5 WHERE id = 4;
             UPDATE matches SET league_id = 4;
             UPDATE matches SET hidden_at = 5, edited_at = 5;
             UPDATE matches SET league_id = NULL;",
        )
        .expect("deleted with its league");
        let columns = |conn: &Connection| -> (Option<i64>, Option<i64>, bool, bool, Option<i64>) {
            conn.query_row(
                "SELECT hidden_at, league_id, league_linked, no_league, edited_at FROM matches
                 WHERE token = ?1",
                [TOKEN],
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
            .expect("columns")
        };
        assert_eq!(
            edit(&mut conn, restore_edit(TOKEN, 7)),
            MatchEditOutcome::DeletedWithLeague
        );
        assert_eq!(
            MatchEditOutcome::DeletedWithLeague.refusal(),
            Some("Restore its league to bring it back.")
        );
        assert_eq!(
            columns(&conn),
            (Some(5), None, false, false, Some(5)),
            "a refusal writes nothing"
        );

        // Deleted at the league's time but in no league: deleted alone, so
        // restored.
        conn.execute_batch(
            "UPDATE matches SET hidden_at = NULL;
             UPDATE matches SET hidden_at = 5, edited_at = 5;",
        )
        .expect("deleted at the same time, in no league");
        assert_eq!(
            edit(&mut conn, restore_edit(TOKEN, 7)),
            MatchEditOutcome::Done
        );
        assert_eq!(columns(&conn), (None, None, false, false, Some(7)));

        // Linked by hand, deleted on its own, then its league deleted: the
        // link to the deleted league goes too.
        conn.execute(
            "UPDATE matches SET hidden_at = 4, league_id = 4, league_linked = 1, no_league = 0",
            [],
        )
        .expect("linked, then deleted alone before its league");
        assert_eq!(
            edit(&mut conn, restore_edit(TOKEN, 8)),
            MatchEditOutcome::Done
        );
        assert_eq!(columns(&conn), (None, None, false, true, Some(8)));

        // Kept by its league's delete, then deleted on its own: it comes
        // back as it was, a kept match of the deleted league.
        conn.execute(
            "UPDATE matches SET hidden_at = 6, league_id = 4, league_linked = 1, no_league = 0",
            [],
        )
        .expect("kept, then deleted alone after its league");
        assert_eq!(
            edit(&mut conn, restore_edit(TOKEN, 8)),
            MatchEditOutcome::Done
        );
        assert_eq!(columns(&conn), (None, Some(4), true, false, Some(8)));

        // Deleted on its own, its league listed: restored with no league
        // change.
        conn.execute_batch(
            "UPDATE leagues SET removed_at = NULL WHERE id = 4;
             UPDATE matches SET hidden_at = 9, league_id = 4, league_linked = 1, no_league = 0;",
        )
        .expect("deleted alone");
        assert_eq!(
            edit(&mut conn, restore_edit(TOKEN, 10)),
            MatchEditOutcome::Done
        );
        assert_eq!(columns(&conn), (None, Some(4), true, false, Some(10)));
        conn.execute(
            "UPDATE matches SET hidden_at = 11, league_id = NULL, league_linked = 0",
            [],
        )
        .expect("deleted alone, in no league");
        assert_eq!(
            edit(&mut conn, restore_edit(TOKEN, 12)),
            MatchEditOutcome::Done
        );
        assert_eq!(columns(&conn), (None, None, false, false, Some(12)));
    }

    #[test]
    fn notes_are_trimmed_limited_and_cleared_when_empty() {
        let (_dir, mut conn) = db("match-edits-notes");
        assert_eq!(
            edit(&mut conn, notes(TOKEN, "  Q misplayed turn 3  ")),
            MatchEditOutcome::Done
        );
        assert_eq!(
            match_note(&conn, TOKEN).expect("note").as_deref(),
            Some("Q misplayed turn 3")
        );

        let too_long = "é".repeat(MAX_NOTES_CHARS + 1);
        assert_eq!(
            edit(&mut conn, notes(TOKEN, &too_long)),
            MatchEditOutcome::NotesTooLong
        );
        assert_eq!(
            match_note(&conn, TOKEN).expect("note").as_deref(),
            Some("Q misplayed turn 3"),
            "a refusal changes nothing"
        );
        let longest = "é".repeat(MAX_NOTES_CHARS);
        assert_eq!(
            edit(&mut conn, notes(TOKEN, &longest)),
            MatchEditOutcome::Done
        );
        assert_eq!(match_note(&conn, TOKEN).expect("note"), Some(longest));

        assert_eq!(edit(&mut conn, notes(TOKEN, "   ")), MatchEditOutcome::Done);
        assert_eq!(
            match_note(&conn, TOKEN).expect("note"),
            None,
            "empty clears"
        );
        assert_eq!(edit(&mut conn, notes(TOKEN, "")), MatchEditOutcome::Done);
    }

    #[test]
    fn notes_need_a_match_that_is_not_hidden() {
        let (_dir, mut conn) = db("match-edits-notes-missing");
        assert_eq!(
            edit(&mut conn, notes(OTHER, "Q note")),
            MatchEditOutcome::NotFound
        );
        assert_eq!(edit(&mut conn, hide_edit(TOKEN, 5)), MatchEditOutcome::Done);
        assert_eq!(
            edit(&mut conn, notes(TOKEN, "Q note")),
            MatchEditOutcome::NotFound
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE notes IS NOT NULL"
            ),
            0
        );
    }

    #[test]
    fn hiding_is_idempotent_and_keeps_the_first_time() {
        let (_dir, mut conn) = db("match-edits-hide");
        assert!(!is_hidden(&conn, TOKEN).expect("hidden"));
        assert_eq!(edit(&mut conn, hide_edit(TOKEN, 5)), MatchEditOutcome::Done);
        assert_eq!(edit(&mut conn, hide_edit(TOKEN, 6)), MatchEditOutcome::Done);
        assert!(is_hidden(&conn, TOKEN).expect("hidden"));
        assert_eq!(
            hidden_match_tokens(&conn).expect("tokens"),
            vec![TOKEN.to_string()]
        );
        assert_eq!(number(&conn, "SELECT hidden_at FROM matches"), 5);
        assert!(list_matches(&conn, 10, 0).expect("list").is_empty());
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM matches"),
            1,
            "the derived row stays"
        );
        assert_eq!(
            edit(&mut conn, hide_edit(OTHER, 5)),
            MatchEditOutcome::NotFound
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE hidden_at IS NOT NULL"
            ),
            1
        );
    }

    #[test]
    fn hiding_keeps_the_note() {
        let (_dir, mut conn) = db("match-edits-hide-note");
        edit(&mut conn, notes(TOKEN, "Q note"));
        edit(&mut conn, hide_edit(TOKEN, 5));
        assert_eq!(
            match_note(&conn, TOKEN).expect("note").as_deref(),
            Some("Q note")
        );
    }

    #[test]
    fn every_refusal_has_a_message_and_done_has_none() {
        assert_eq!(MatchEditOutcome::Done.refusal(), None);
        for outcome in [
            MatchEditOutcome::NotFound,
            MatchEditOutcome::NotesTooLong,
            MatchEditOutcome::DeckVersionNotFound,
            MatchEditOutcome::OtherAccountsDeck,
            MatchEditOutcome::LimitedDeck,
            MatchEditOutcome::LimitedMatch,
            MatchEditOutcome::FormatMismatch,
        ] {
            assert!(outcome.refusal().is_some(), "{outcome:?}");
        }
        assert_eq!(
            MatchEditOutcome::NotesTooLong.refusal(),
            Some("The notes field must not be greater than 5000 characters.")
        );
    }

    /// The match's three share columns as one text.
    fn share_columns(conn: &Connection, token: &str) -> String {
        conn.query_row(
            "SELECT concat_ws('|', COALESCE(replay_share_uuid, '-'),
                 COALESCE(replay_share_url, '-'), COALESCE(replay_shared_at, '-'))
             FROM matches WHERE token = ?1",
            [token],
            |row| row.get(0),
        )
        .expect("the match")
    }

    fn share_edit(token: &str) -> MatchEdit {
        MatchEdit::SetReplayShare {
            token: token.to_string(),
            uuid: "9b2f6f4e-0000-4000-8000-000000000001".to_string(),
            url: "https://mymtgo.test/players/Qplayer/replays/9b2f6f4e".to_string(),
            shared_at_ms: 4_000,
        }
    }

    // Replays spec section 8: the link is stored on the match through the
    // tracker; switching it off clears all three columns. An unknown match
    // is refused, and a match removed from stats keeps and still takes its
    // link (hiding never revokes, ruling 11).
    #[test]
    fn set_and_clear_replay_share() {
        let (_dir, mut conn) = db("match-edits-share");
        assert_eq!(share_columns(&conn, TOKEN), "-|-|-");
        assert_eq!(
            edit(&mut conn, share_edit("q-unknown")),
            MatchEditOutcome::NotFound
        );
        assert_eq!(
            edit(
                &mut conn,
                MatchEdit::ClearReplayShare {
                    token: "q-unknown".to_string()
                }
            ),
            MatchEditOutcome::NotFound
        );
        assert_eq!(edit(&mut conn, share_edit(TOKEN)), MatchEditOutcome::Done);
        assert_eq!(
            share_columns(&conn, TOKEN),
            "9b2f6f4e-0000-4000-8000-000000000001|https://mymtgo.test/players/Qplayer/replays/9b2f6f4e|4000"
        );
        // Sharing again refreshes the link and its time.
        let mut again = share_edit(TOKEN);
        if let MatchEdit::SetReplayShare { shared_at_ms, .. } = &mut again {
            *shared_at_ms = 6_000;
        }
        assert_eq!(edit(&mut conn, again), MatchEditOutcome::Done);
        assert!(share_columns(&conn, TOKEN).ends_with("|6000"));

        edit(&mut conn, hide_edit(TOKEN, 5));
        assert!(
            share_columns(&conn, TOKEN).ends_with("|6000"),
            "hiding keeps it"
        );
        let clear = MatchEdit::ClearReplayShare {
            token: TOKEN.to_string(),
        };
        assert_eq!(edit(&mut conn, clear.clone()), MatchEditOutcome::Done);
        assert_eq!(share_columns(&conn, TOKEN), "-|-|-");
        assert_eq!(
            edit(&mut conn, clear),
            MatchEditOutcome::Done,
            "already off"
        );
        assert_eq!(edit(&mut conn, share_edit(TOKEN)), MatchEditOutcome::Done);
        assert!(share_columns(&conn, TOKEN).ends_with("|4000"));
    }

    // Replays spec section 9: a link is all three columns or none.
    #[test]
    fn the_share_columns_are_all_set_or_all_null() {
        let (_dir, conn) = db("match-edits-share-check");
        for partial in [
            "replay_share_uuid = 'Q-uuid'",
            "replay_share_url = 'https://mymtgo.test/r/Q'",
            "replay_shared_at = 5",
            "replay_share_uuid = 'Q-uuid', replay_share_url = 'https://mymtgo.test/r/Q'",
            "replay_share_uuid = 'Q-uuid', replay_shared_at = 5",
            "replay_share_url = 'https://mymtgo.test/r/Q', replay_shared_at = 5",
        ] {
            assert!(
                conn.execute(&format!("UPDATE matches SET {partial}"), [])
                    .is_err(),
                "{partial}"
            );
        }
        conn.execute(
            "UPDATE matches SET replay_share_uuid = 'Q-uuid',
                 replay_share_url = 'https://mymtgo.test/r/Q', replay_shared_at = 5",
            [],
        )
        .expect("all three");
        conn.execute(
            "UPDATE matches SET replay_share_uuid = NULL, replay_share_url = NULL,
                 replay_shared_at = NULL",
            [],
        )
        .expect("none");
    }

    // ---- Set deck (issue #332) ----

    mod set_deck {
        use super::super::*;
        use crate::archetypes::tests::{db, insert_archetype, insert_deck, insert_match, seat};
        use crate::test_support::number;

        fn set_deck(conn: &mut Connection, tokens: &[&str], version: i64) -> MatchEditOutcome {
            let edit = MatchEdit::SetDeckVersion {
                tokens: tokens.iter().map(|token| token.to_string()).collect(),
                deck_version_id: version,
            };
            edit_match(conn, &edit).expect("edit")
        }

        fn link(conn: &Connection, match_id: i64) -> Option<i64> {
            conn.query_row(
                "SELECT deck_version_id FROM matches WHERE id = ?1",
                [match_id],
                |row| row.get(0),
            )
            .expect("match")
        }

        fn detected_at(conn: &Connection, match_id: i64) -> Option<i64> {
            conn.query_row(
                "SELECT archetypes_detected_at FROM matches WHERE id = ?1",
                [match_id],
                |row| row.get(0),
            )
            .expect("match")
        }

        #[test]
        fn sets_and_replaces_the_link_of_each_match() {
            let (_dir, mut conn) = db("match-edits-set-deck");
            insert_deck(&conn, 1, "CMODERN");
            insert_deck(&conn, 2, "CMODERN");
            insert_match(&conn, 1, "log", 100);
            insert_match(&conn, 2, "manual", 200);
            conn.execute("UPDATE matches SET deck_version_id = 10 WHERE id = 1", [])
                .expect("linked");

            assert_eq!(
                set_deck(&mut conn, &["Q-match-1", "Q-match-2"], 20),
                MatchEditOutcome::Done
            );
            assert_eq!(link(&conn, 1), Some(20), "the linker's link is replaced");
            assert_eq!(link(&conn, 2), Some(20));
            assert_eq!(
                set_deck(&mut conn, &["Q-match-1"], 20),
                MatchEditOutcome::Done,
                "the same deck again is done"
            );
        }

        #[test]
        fn skips_missing_and_hidden_matches_and_needs_one() {
            let (_dir, mut conn) = db("match-edits-set-deck-skip");
            insert_deck(&conn, 1, "CMODERN");
            insert_match(&conn, 1, "log", 100);
            insert_match(&conn, 2, "log", 200);
            conn.execute("UPDATE matches SET hidden_at = 5 WHERE id = 2", [])
                .expect("hidden");

            assert_eq!(
                set_deck(&mut conn, &["Q-missing", "Q-match-2"], 10),
                MatchEditOutcome::NotFound
            );
            assert_eq!(
                set_deck(&mut conn, &["Q-missing", "Q-match-1", "Q-match-2"], 10),
                MatchEditOutcome::Done
            );
            assert_eq!(link(&conn, 1), Some(10));
            assert_eq!(link(&conn, 2), None, "a hidden match is left alone");
        }

        #[test]
        fn refuses_a_gone_version_or_another_accounts_deck_and_writes_nothing() {
            let (_dir, mut conn) = db("match-edits-set-deck-refused");
            insert_deck(&conn, 1, "CMODERN");
            insert_deck(&conn, 2, "CMODERN");
            insert_match(&conn, 1, "log", 100);
            conn.execute_batch(
                "INSERT INTO accounts (id, username, first_seen_at) VALUES (2, 'Qother', 0);
                 UPDATE decks SET account_id = 1 WHERE id = 1;
                 UPDATE decks SET account_id = 2 WHERE id = 2;",
            )
            .expect("accounts");

            assert_eq!(
                set_deck(&mut conn, &["Q-match-1"], 999),
                MatchEditOutcome::DeckVersionNotFound
            );
            assert_eq!(
                set_deck(&mut conn, &["Q-match-1"], 20),
                MatchEditOutcome::OtherAccountsDeck
            );
            assert_eq!(link(&conn, 1), None);
            assert_eq!(
                set_deck(&mut conn, &["Q-match-1"], 10),
                MatchEditOutcome::Done,
                "the account's own deck"
            );
        }

        #[test]
        fn refuses_a_limited_deck_or_match_and_another_format() {
            let (_dir, mut conn) = db("match-edits-set-deck-formats");
            insert_deck(&conn, 1, "CMODERN");
            insert_deck(&conn, 2, "Limited");
            insert_deck(&conn, 3, "CMODERN");
            insert_deck(&conn, 4, "CPAUPER");
            insert_deck(&conn, 5, "");
            conn.execute(
                "UPDATE decks SET mtgo_id = 'limited:q-pool' WHERE id = 3",
                [],
            )
            .expect("pool deck");
            insert_match(&conn, 1, "log", 100);
            insert_match(&conn, 2, "log", 200);
            insert_match(&conn, 3, "imported", 300);
            conn.execute_batch(
                "UPDATE matches SET format = 'DHOBHOBHOB' WHERE id = 2;
                 UPDATE matches SET format = NULL WHERE id = 3;",
            )
            .expect("formats");

            assert_eq!(
                set_deck(&mut conn, &["Q-match-1"], 20),
                MatchEditOutcome::LimitedDeck
            );
            assert_eq!(
                set_deck(&mut conn, &["Q-match-1"], 30),
                MatchEditOutcome::LimitedDeck,
                "a draft pool's deck"
            );
            assert_eq!(
                set_deck(&mut conn, &["Q-match-1", "Q-match-2"], 10),
                MatchEditOutcome::LimitedMatch
            );
            assert_eq!(
                set_deck(&mut conn, &["Q-match-1"], 40),
                MatchEditOutcome::FormatMismatch
            );
            assert_eq!(link(&conn, 1), None, "a refusal writes nothing");
            assert_eq!(link(&conn, 2), None);

            conn.execute("UPDATE decks SET format = 'cmodern' WHERE id = 1", [])
                .expect("case");
            assert_eq!(
                set_deck(&mut conn, &["Q-match-1"], 10),
                MatchEditOutcome::Done,
                "the format compared without case"
            );
            assert_eq!(
                set_deck(&mut conn, &["Q-match-3"], 40),
                MatchEditOutcome::Done,
                "a match without a format is not checked"
            );
            assert_eq!(
                set_deck(&mut conn, &["Q-match-1"], 50),
                MatchEditOutcome::Done,
                "nor a deck without one"
            );
        }

        #[test]
        fn a_new_deck_asks_for_detection_unless_the_player_seat_is_hand_set() {
            let (_dir, mut conn) = db("match-edits-set-deck-detect");
            insert_deck(&conn, 1, "CMODERN");
            insert_deck(&conn, 2, "CMODERN");
            insert_match(&conn, 1, "log", 100);
            insert_match(&conn, 2, "log", 200);
            insert_match(&conn, 3, "log", 300);
            let archetype = insert_archetype(&conn, "q-uuid", "Qarch", Some("modern"), "manual");
            seat(&conn, 1, false, archetype);
            seat(&conn, 2, false, archetype);
            conn.execute_batch(
                "UPDATE match_archetypes SET manual = 1 WHERE match_id = 2;
                 UPDATE matches SET archetypes_detected_at = 7;
                 UPDATE matches SET deck_version_id = 10 WHERE id = 3;",
            )
            .expect("detected");

            assert_eq!(
                set_deck(&mut conn, &["Q-match-1", "Q-match-2", "Q-match-3"], 10),
                MatchEditOutcome::Done
            );
            assert_eq!(
                detected_at(&conn, 1),
                None,
                "the auto seat follows the deck"
            );
            assert_eq!(detected_at(&conn, 2), Some(7), "a hand-set seat stays");
            assert_eq!(
                detected_at(&conn, 3),
                Some(7),
                "an unchanged link asks nothing"
            );
            assert_eq!(
                number(&conn, "SELECT COUNT(*) FROM match_archetypes"),
                2,
                "the rows stay until detection runs"
            );
        }
    }
}
