//! Raw Arena game messages: one row per message while a match is live, one
//! zstd blob per match once it is over. The game sync (Task 3 and later) reads
//! them through `game_messages`, which hides which form they are in.

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::StoreError;
use crate::write::{decode_raw, encode_raw};

/// Who sent a message: the game rules engine or the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GameMessageSource {
    Gre,
    Client,
}

impl GameMessageSource {
    fn as_str(self) -> &'static str {
        match self {
            GameMessageSource::Gre => "gre",
            GameMessageSource::Client => "client",
        }
    }

    fn from_str(text: &str) -> Result<Self, StoreError> {
        match text {
            "gre" => Ok(GameMessageSource::Gre),
            "client" => Ok(GameMessageSource::Client),
            other => Err(StoreError::Corrupt(format!("game message source {other}"))),
        }
    }
}

/// A message to store. `game_number` is `None` before the match's first
/// `gameInfo`. `json` is the message as JSON text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewGameMessage {
    pub file_id: i64,
    pub byte_offset: u64,
    pub seq: u32,
    pub match_token: String,
    pub game_number: Option<u32>,
    pub source: GameMessageSource,
    pub logged_at: i64,
    pub json: String,
}

/// A stored message (the archive holds these as a JSON array).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredGameMessage {
    pub file_id: i64,
    pub byte_offset: u64,
    pub seq: u32,
    pub game_number: Option<u32>,
    pub source: GameMessageSource,
    pub logged_at: i64,
    pub json: String,
}

impl StoredGameMessage {
    fn key(&self) -> (i64, u64, u32) {
        (self.file_id, self.byte_offset, self.seq)
    }
}

/// A replayed message (same file, offset and seq) is skipped.
pub(crate) fn insert_game_message(
    tx: &Transaction,
    message: &NewGameMessage,
) -> Result<(), StoreError> {
    let (raw, compressed) = encode_raw(&message.json)?;
    tx.prepare_cached(
        "INSERT INTO arena_game_messages (arena_log_file_id, byte_offset, seq, match_token,
             game_number, source, logged_at, raw, compressed)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (arena_log_file_id, byte_offset, seq) DO NOTHING",
    )?
    .execute(params![
        message.file_id,
        message.byte_offset as i64,
        message.seq,
        message.match_token,
        message.game_number,
        message.source.as_str(),
        message.logged_at,
        raw,
        compressed,
    ])?;
    Ok(())
}

fn live_messages(conn: &Connection, token: &str) -> Result<Vec<StoredGameMessage>, StoreError> {
    let mut statement = conn.prepare_cached(
        "SELECT arena_log_file_id, byte_offset, seq, game_number, source, logged_at, raw, compressed
         FROM arena_game_messages WHERE match_token = ?1
         ORDER BY arena_log_file_id, byte_offset, seq",
    )?;
    let rows = statement.query_map([token], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, u32>(2)?,
            row.get::<_, Option<u32>>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, Vec<u8>>(6)?,
            row.get::<_, bool>(7)?,
        ))
    })?;
    let mut messages = Vec::new();
    for row in rows {
        let (file_id, offset, seq, game_number, source, logged_at, raw, compressed) = row?;
        messages.push(StoredGameMessage {
            file_id,
            byte_offset: offset as u64,
            seq,
            game_number,
            source: GameMessageSource::from_str(&source)?,
            logged_at,
            json: decode_raw(raw, compressed)?,
        });
    }
    Ok(messages)
}

fn archived_messages(conn: &Connection, token: &str) -> Result<Vec<StoredGameMessage>, StoreError> {
    let blob: Option<Vec<u8>> = conn
        .query_row(
            "SELECT messages FROM arena_game_archives WHERE match_token = ?1",
            [token],
            |row| row.get(0),
        )
        .optional()?;
    match blob {
        None => Ok(Vec::new()),
        Some(blob) => Ok(serde_json::from_slice(&zstd::decode_all(blob.as_slice())?)?),
    }
}

/// A match's messages in log order: the archive (if any) merged with live
/// rows, a key present in both counted once. A match pulled from another
/// device has none of either: its messages are the source it came with
/// (`synced_messages`).
pub fn game_messages(
    conn: &Connection,
    match_token: &str,
) -> Result<Vec<StoredGameMessage>, StoreError> {
    if let Some(messages) = synced_messages(conn, match_token)? {
        return Ok(messages);
    }
    let mut all = archived_messages(conn, match_token)?;
    all.extend(live_messages(conn, match_token)?);
    all.sort_by_key(|message| message.key());
    all.dedup_by_key(|message| message.key());
    Ok(all)
}

/// The messages of an Arena match pulled from another device (an
/// `imported` row with a `synced_sources` row), as `source_messages` reads
/// them. `None` for any other match.
fn synced_messages(
    conn: &Connection,
    match_token: &str,
) -> Result<Option<Vec<StoredGameMessage>>, StoreError> {
    match synced_arena_sources(conn, match_token)? {
        Some(sources) => Ok(Some(source_messages(&sources)?)),
        None => Ok(None),
    }
}

/// The source an Arena match pulled from another device came with (an
/// `imported` row with an Arena `synced_sources` row): its messages and the
/// event its room named. `None` for any other match.
pub(crate) fn synced_arena_sources(
    conn: &Connection,
    match_token: &str,
) -> Result<Option<sync_bundles::ArenaSources>, StoreError> {
    let match_id: Option<i64> = conn
        .prepare_cached(
            "SELECT m.id FROM matches m JOIN synced_sources s ON s.match_id = m.id
             WHERE m.token = ?1 AND m.source = 'imported' AND s.platform = 'arena'",
        )?
        .query_row([match_token], |row| row.get(0))
        .optional()?;
    let Some(match_id) = match_id else {
        return Ok(None);
    };
    match crate::synced_sources::read(conn, match_id)? {
        Some(sync_bundles::MatchSources::Arena(sources)) => Ok(Some(sources)),
        _ => Ok(None),
    }
}

/// A pulled Arena source's messages, game by game in the order they came.
/// They have no log file: each is numbered by its place. A message whose
/// source is neither `gre` nor `client` is `Corrupt`.
pub(crate) fn source_messages(
    sources: &sync_bundles::ArenaSources,
) -> Result<Vec<StoredGameMessage>, StoreError> {
    let mut messages = Vec::new();
    for game in &sources.games {
        for message in &game.messages {
            messages.push(StoredGameMessage {
                file_id: 0,
                byte_offset: messages.len() as u64,
                seq: 0,
                game_number: Some(game.game_number),
                source: GameMessageSource::from_str(&message.source)?,
                logged_at: message.logged_at_ms,
                json: message.raw.clone(),
            });
        }
    }
    Ok(messages)
}

/// Move a match's live rows into its archive blob (merged with an existing
/// one) and delete them. `false` when there were no live rows.
pub fn archive_game_messages(
    tx: &Transaction,
    match_token: &str,
    now_ms: i64,
) -> Result<bool, StoreError> {
    let live = live_messages(tx, match_token)?;
    if live.is_empty() {
        return Ok(false);
    }
    // As `game_messages`, without reading the live rows a second time.
    let mut all = archived_messages(tx, match_token)?;
    all.extend(live);
    all.sort_by_key(|message| message.key());
    all.dedup_by_key(|message| message.key());
    let blob = zstd::encode_all(serde_json::to_vec(&all)?.as_slice(), 3)?;
    tx.execute(
        "INSERT INTO arena_game_archives (match_token, messages, message_count, archived_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (match_token) DO UPDATE SET messages = excluded.messages,
             message_count = excluded.message_count, archived_at = excluded.archived_at",
        params![match_token, blob, all.len() as i64, now_ms],
    )?;
    tx.execute(
        "DELETE FROM arena_game_messages WHERE match_token = ?1",
        [match_token],
    )?;
    Ok(true)
}

/// Arena matches from the log (not corrected, no longer in progress) with
/// no game messages, live or archived, and no Arena game sync stamp: tracked
/// before game messages were stored. The tracker looks for their messages in
/// the Arena logs still on disk once; the sync then stamps each.
pub fn arena_tokens_without_messages(conn: &Connection) -> Result<Vec<String>, StoreError> {
    let mut statement = conn.prepare_cached(
        "SELECT m.token FROM matches m
         JOIN accounts a ON a.id = m.account_id AND a.platform = 'arena'
         WHERE m.source = 'log' AND m.corrected_at IS NULL AND m.state <> 'in_progress'
           AND NOT EXISTS (SELECT 1 FROM arena_game_syncs s WHERE s.match_id = m.id)
           AND NOT EXISTS (SELECT 1 FROM arena_game_messages l WHERE l.match_token = m.token)
           AND NOT EXISTS (SELECT 1 FROM arena_game_archives r WHERE r.match_token = m.token)
         ORDER BY m.id",
    )?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut tokens = Vec::new();
    for row in rows {
        tokens.push(row?);
    }
    Ok(tokens)
}

/// The game numbers the match has messages of, live or archived, or in
/// the source a pulled match came with (the match detail's game log and
/// Replay buttons, per game).
pub fn game_numbers_with_messages(
    conn: &Connection,
    match_token: &str,
) -> Result<std::collections::BTreeSet<u32>, StoreError> {
    let mut numbers = std::collections::BTreeSet::new();
    if let Some(sources) = synced_arena_sources(conn, match_token)? {
        for game in &sources.games {
            if !game.messages.is_empty() {
                numbers.insert(game.game_number);
            }
        }
        return Ok(numbers);
    }
    for message in archived_messages(conn, match_token)? {
        numbers.extend(message.game_number);
    }
    let mut statement = conn.prepare_cached(
        "SELECT DISTINCT game_number FROM arena_game_messages
         WHERE match_token = ?1 AND game_number IS NOT NULL",
    )?;
    let rows = statement.query_map([match_token], |row| row.get::<_, u32>(0))?;
    for row in rows {
        numbers.insert(row?);
    }
    Ok(numbers)
}

/// Whether the match has any messages, live or archived.
pub fn has_game_messages(conn: &Connection, match_token: &str) -> Result<bool, StoreError> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM arena_game_messages WHERE match_token = ?1)
             OR EXISTS (SELECT 1 FROM arena_game_archives WHERE match_token = ?1)",
        [match_token],
        |row| row.get(0),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{arena_log_file, db};
    use crate::write::{COMPRESS_OVER, WriteBatch, commit};

    fn message(file_id: i64, byte_offset: u64, seq: u32, json: &str) -> NewGameMessage {
        NewGameMessage {
            file_id,
            byte_offset,
            seq,
            match_token: "q-match".to_string(),
            game_number: Some(1),
            source: GameMessageSource::Gre,
            logged_at: 100 + byte_offset as i64,
            json: json.to_string(),
        }
    }

    fn insert_all(conn: &mut Connection, messages: Vec<NewGameMessage>) {
        let batch = WriteBatch {
            arena_game_messages: messages,
            ..WriteBatch::default()
        };
        commit(conn, &batch).expect("commit");
    }

    fn live_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM arena_game_messages", [], |row| {
            row.get(0)
        })
        .expect("count")
    }

    fn jsons(messages: &[StoredGameMessage]) -> Vec<String> {
        messages.iter().map(|m| m.json.clone()).collect()
    }

    #[test]
    fn messages_read_back_in_log_order() {
        let (_dir, mut conn) = db("arena-games-order");
        let id = arena_log_file(&conn);
        let big = format!("\"{}\"", "x".repeat(COMPRESS_OVER + 10));
        insert_all(
            &mut conn,
            vec![
                message(id, 900, 0, "\"c\""),
                message(id, 100, 1, "\"b\""),
                message(id, 100, 0, &big),
            ],
        );
        let read = game_messages(&conn, "q-match").expect("read");
        assert_eq!(jsons(&read), vec![big, "\"b\"".into(), "\"c\"".into()]);
        assert!(has_game_messages(&conn, "q-match").expect("has"));
        assert!(!has_game_messages(&conn, "q-other").expect("has"));
    }

    #[test]
    fn archive_moves_rows_into_one_blob() {
        let (_dir, mut conn) = db("arena-games-archive");
        let id = arena_log_file(&conn);
        insert_all(
            &mut conn,
            vec![message(id, 100, 0, "\"a\""), message(id, 200, 0, "\"b\"")],
        );
        let before = game_messages(&conn, "q-match").expect("read");
        let tx = conn.transaction().expect("tx");
        assert!(archive_game_messages(&tx, "q-match", 5_000).expect("archive"));
        assert!(!archive_game_messages(&tx, "q-match", 5_001).expect("again"));
        tx.commit().expect("commit");
        assert_eq!(live_count(&conn), 0);
        assert_eq!(game_messages(&conn, "q-match").expect("read"), before);
        let count: i64 = conn
            .query_row(
                "SELECT message_count FROM arena_game_archives WHERE match_token = 'q-match'",
                [],
                |row| row.get(0),
            )
            .expect("archive row");
        assert_eq!(count, 2);
        assert!(has_game_messages(&conn, "q-match").expect("has"));
    }

    #[test]
    fn archive_then_late_rows_are_both_read() {
        let (_dir, mut conn) = db("arena-games-late");
        let id = arena_log_file(&conn);
        insert_all(&mut conn, vec![message(id, 100, 0, "\"a\"")]);
        let tx = conn.transaction().expect("tx");
        archive_game_messages(&tx, "q-match", 1).expect("archive");
        tx.commit().expect("commit");
        insert_all(&mut conn, vec![message(id, 300, 0, "\"late\"")]);
        let read = game_messages(&conn, "q-match").expect("read");
        assert_eq!(jsons(&read), vec!["\"a\"", "\"late\""]);
        let tx = conn.transaction().expect("tx");
        archive_game_messages(&tx, "q-match", 2).expect("archive again");
        tx.commit().expect("commit");
        assert_eq!(live_count(&conn), 0);
        assert_eq!(game_messages(&conn, "q-match").expect("read"), read);
    }

    #[test]
    fn duplicate_insert_is_ignored() {
        let (_dir, mut conn) = db("arena-games-dup");
        let id = arena_log_file(&conn);
        insert_all(&mut conn, vec![message(id, 100, 0, "\"a\"")]);
        insert_all(&mut conn, vec![message(id, 100, 0, "\"changed\"")]);
        let read = game_messages(&conn, "q-match").expect("read");
        assert_eq!(jsons(&read), vec!["\"a\""]);
    }

    #[test]
    fn tokens_without_messages_are_unsearched_arena_log_matches() {
        use crate::test_support::{arena_account, insert_manual, move_match_to_arena, view};
        use crate::write::MatchWrite;

        let (_dir, mut conn) = db("arena-games-without");
        arena_account(&conn);
        let mut matches = Vec::new();
        for token in [
            "q-bare",
            "q-live",
            "q-archived",
            "q-stamped",
            "q-corrected",
            "q-open",
            "q-mtgo",
        ] {
            let mut match_view = view(token, "arena:Qfoe-id", 1_000);
            match_view.mtgo_match_id = None;
            if token != "q-mtgo" {
                match_view.local_username = "Qarena#12345".to_string();
            }
            if token == "q-open" {
                match_view.state = mtgo_match::MatchState::InProgress;
            }
            matches.push(MatchWrite::Upsert {
                view: Box::new(match_view),
                revision: 1,
            });
        }
        let id = arena_log_file(&conn);
        let mut live = message(id, 100, 0, "\"a\"");
        live.match_token = "q-live".to_string();
        let mut archived = message(id, 200, 0, "\"b\"");
        archived.match_token = "q-archived".to_string();
        commit(
            &mut conn,
            &WriteBatch {
                matches,
                arena_game_messages: vec![live, archived],
                arena_archive_tokens: vec!["q-archived".to_string()],
                now_ms: 2_000,
                ..WriteBatch::default()
            },
        )
        .expect("commit");
        insert_manual(&conn, "q-manual");
        move_match_to_arena(&conn, "q-manual");
        conn.execute_batch(
            "INSERT INTO arena_game_syncs (match_id, status, derived_version, synced_at)
                 SELECT id, 'no_messages', 1, 1 FROM matches WHERE token = 'q-stamped';
             UPDATE matches SET corrected_at = 1 WHERE token = 'q-corrected';",
        )
        .expect("stamp and correct");
        assert_eq!(
            arena_tokens_without_messages(&conn).expect("tokens"),
            vec!["q-bare".to_string()]
        );
    }

    #[test]
    fn batch_archives_after_inserting_messages() {
        let (_dir, mut conn) = db("arena-games-batch");
        let id = arena_log_file(&conn);
        let batch = WriteBatch {
            arena_game_messages: vec![message(id, 100, 0, "\"a\""), message(id, 200, 0, "\"b\"")],
            arena_archive_tokens: vec!["q-match".to_string()],
            now_ms: 9_000,
            ..WriteBatch::default()
        };
        commit(&mut conn, &batch).expect("commit");
        assert_eq!(live_count(&conn), 0);
        let read = game_messages(&conn, "q-match").expect("read");
        assert_eq!(jsons(&read), vec!["\"a\"", "\"b\""]);
    }
}
