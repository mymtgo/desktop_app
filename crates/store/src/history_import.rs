//! MTGO's own match history (Plan M). Reads for the scan (0.x
//! `ParseAndFilterHistoryJob` and `MatchAndScoreJob`): which MTGO match ids
//! and match tokens are already stored, and the deck version a scan scores
//! against. Writes for the import (0.x `ImportMatches::runFromScan`): each
//! chosen match stored as `source = 'history'` (0.x `matches.imported`) and
//! linked to the scan's deck version, so "Delete all imported" (0.x
//! `DestroyController`) removes only those.

use std::collections::HashSet;

use game_detail::GameLogEntry;
use rusqlite::{OptionalExtension, Transaction, params};
use sha2::{Digest, Sha256};

use crate::game_sync::{GameSource, build_game_cards, store_game_log, write_game_cards};
use crate::imported_detail::{apply_rule, rule_inputs};
use crate::write::registry_id;
use crate::{Connection, StoreError};

/// The deck version a scan scores against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanVersion {
    pub id: i64,
    /// The deck's name, archived or not.
    pub deck_name: String,
    pub signature: String,
}

/// Every MTGO match id a stored match has: tracked from the log, imported
/// or entered by hand, hidden or not. A history record with one of these ids
/// is never offered again.
pub fn stored_mtgo_match_ids(conn: &Connection) -> Result<HashSet<u64>, StoreError> {
    let mut stmt =
        conn.prepare("SELECT DISTINCT mtgo_match_id FROM matches WHERE mtgo_match_id IS NOT NULL")?;
    let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
    let mut ids = HashSet::new();
    for row in rows {
        // MTGO ids are positive; a negative one cannot equal a history id.
        if let Ok(id) = u64::try_from(row?) {
            ids.insert(id);
        }
    }
    Ok(ids)
}

/// Every stored match token, of any source, hidden or not. A game log whose
/// token is one of these already belongs to a match.
pub fn stored_match_tokens(conn: &Connection) -> Result<HashSet<String>, StoreError> {
    let mut stmt = conn.prepare("SELECT token FROM matches")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut tokens = HashSet::new();
    for row in rows {
        tokens.insert(row?);
    }
    Ok(tokens)
}

/// The deck version `id` with its deck's name, `None` when there is none.
pub fn scan_version(conn: &Connection, id: i64) -> Result<Option<ScanVersion>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT v.id, d.name, v.signature FROM deck_versions v
             JOIN decks d ON d.id = v.deck_id
             WHERE v.id = ?1",
            [id],
            |row| {
                Ok(ScanVersion {
                    id: row.get(0)?,
                    deck_name: row.get(1)?,
                    signature: row.get(2)?,
                })
            },
        )
        .optional()?)
}

/// One game of a history match, read from its game log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryGame {
    /// The history's `GameIds` entry for this game.
    pub mtgo_game_id: Option<i64>,
    /// 1-based, in log order.
    pub game_number: u32,
    /// "win" | "loss" | "draw" | "unknown"
    pub result: String,
    /// "win" | "concede" | "unknown"
    pub end_reason: String,
    pub on_play: Option<bool>,
    pub local_mulligans: Option<i64>,
    pub opp_mulligans: Option<i64>,
    pub local_die_roll: Option<i64>,
    pub opp_die_roll: Option<i64>,
    pub turn_count: Option<i64>,
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
}

/// One history match to store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryMatch {
    /// The paired game log's match token (the real MTGO token); `None`
    /// without a log, and the match gets `history_token(mtgo_match_id)`.
    pub token: Option<String>,
    /// The history record's `Id`.
    pub mtgo_match_id: i64,
    /// The account the match is stored under (registered if new).
    pub local_username: String,
    pub opponent: Option<String>,
    /// The format code (`CMODERN`).
    pub format: Option<String>,
    /// "League" or "Constructed".
    pub match_type: String,
    /// "win" | "loss" | "draw"
    pub outcome: String,
    pub started_at_ms: i64,
    /// The last game's end, `None` without games.
    pub ended_at_ms: Option<i64>,
    /// The history's game counts, kept in `games_won_imported` and
    /// `games_lost_imported`.
    pub games_won: u32,
    pub games_lost: u32,
    /// Empty without a game log.
    pub games: Vec<HistoryGame>,
    /// The scan's deck version; no link when it no longer exists.
    pub deck_version_id: i64,
    /// The paired game log's decoded entries, empty without one: stored in
    /// `game_logs`, and each game's card rows are read from it.
    pub log: Vec<GameLogEntry>,
}

/// What one import batch did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryImportCounts {
    /// The tokens of the matches stored, in batch order.
    pub imported: Vec<String>,
    /// A stored match (live, 0.x import, manual or an earlier history import)
    /// already has the MTGO match id.
    pub skipped_existing: u32,
    /// A stored match already has the token.
    pub skipped_token: u32,
}

/// Store a batch of history matches in one transaction (0.x
/// `ImportMatches::runFromScan`). A match whose MTGO match id or token is
/// already stored is skipped, so a rerun never duplicates one.
pub fn import_history(
    conn: &mut Connection,
    matches: &[HistoryMatch],
    now_ms: i64,
) -> Result<HistoryImportCounts, StoreError> {
    let tx = conn.transaction()?;
    let mut counts = HistoryImportCounts::default();
    for incoming in matches {
        if exists(
            &tx,
            "SELECT EXISTS (SELECT 1 FROM matches WHERE mtgo_match_id = ?1)",
            incoming.mtgo_match_id,
        )? {
            counts.skipped_existing += 1;
            continue;
        }
        let token = match &incoming.token {
            Some(token) => {
                if exists(
                    &tx,
                    "SELECT EXISTS (SELECT 1 FROM matches WHERE token = ?1)",
                    token,
                )? {
                    counts.skipped_token += 1;
                    continue;
                }
                token.clone()
            }
            None => {
                let token = history_token(incoming.mtgo_match_id);
                if exists(
                    &tx,
                    "SELECT EXISTS (SELECT 1 FROM matches WHERE token = ?1)",
                    &token,
                )? {
                    counts.skipped_token += 1;
                    continue;
                }
                token
            }
        };
        insert_history_match(&tx, incoming, &token, now_ms)?;
        counts.imported.push(token);
    }
    tx.commit()?;
    Ok(counts)
}

/// The token of a history match without a paired game log: a version 8
/// UUID from the SHA-256 of `mymtgo:history:{mtgo_match_id}` (first 16
/// bytes, RFC 4122 variant). Both players of a match share the MTGO match
/// id, so every device that imports the same match gives it the same token,
/// and cloud sync (keyed on the token) never doubles it.
pub fn history_token(mtgo_match_id: i64) -> String {
    let digest = Sha256::digest(format!("mymtgo:history:{mtgo_match_id}").as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let mut hex = String::new();
    for byte in bytes {
        hex.push_str(&format!("{byte:02x}"));
    }
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn exists(tx: &Transaction, sql: &str, value: impl rusqlite::ToSql) -> Result<bool, StoreError> {
    Ok(tx
        .prepare_cached(sql)?
        .query_row([value], |row| row.get(0))?)
}

/// Store one history match: its row (with its deck link) and games.
fn insert_history_match(
    tx: &Transaction,
    incoming: &HistoryMatch,
    token: &str,
    now_ms: i64,
) -> Result<(), StoreError> {
    let account_id = registry_id(tx, "accounts", &incoming.local_username, now_ms)?;
    let opponent_id = match &incoming.opponent {
        Some(name) => Some(registry_id(tx, "opponents", name, now_ms)?),
        None => None,
    };
    // Only a version that still exists is linked (the foreign key).
    tx.prepare_cached(
        "INSERT INTO matches (token, mtgo_match_id, account_id, opponent_id, source, format,
             match_type, state, outcome, started_at, ended_at, last_activity_at,
             needs_attention, revision, derived_version, games_won_imported,
             games_lost_imported, deck_version_id)
         VALUES (?1, ?2, ?3, ?4, 'history', ?5, ?6, 'complete', ?7, ?8, ?9, ?10, 0, 1, 0, ?11,
             ?12, (SELECT id FROM deck_versions WHERE id = ?13))",
    )?
    .execute(params![
        token,
        incoming.mtgo_match_id,
        account_id,
        opponent_id,
        incoming.format,
        incoming.match_type,
        incoming.outcome,
        incoming.started_at_ms,
        incoming.ended_at_ms,
        incoming.ended_at_ms.unwrap_or(incoming.started_at_ms),
        incoming.games_won,
        incoming.games_lost,
        incoming.deck_version_id,
    ])?;
    let match_id = tx.last_insert_rowid();
    insert_games(tx, match_id, &incoming.games)?;
    if !incoming.log.is_empty() {
        store_game_log(tx, match_id, &incoming.log)?;
        write_log_cards(tx, match_id, incoming)?;
    }
    Ok(())
}

/// Each game's card rows from the game log alone (no frames, no deck list):
/// the log counters, the opponent's log-only reveals, the pregame actions
/// and the starting hand sizes; then the imported postboard rule's deck.
fn write_log_cards(
    tx: &Transaction,
    match_id: i64,
    incoming: &HistoryMatch,
) -> Result<(), StoreError> {
    let mut sources = Vec::new();
    let mut numbers = Vec::new();
    {
        let mut stmt = tx.prepare_cached(
            "SELECT id, on_play, game_number FROM games WHERE match_id = ?1 ORDER BY game_number",
        )?;
        let rows = stmt.query_map([match_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get::<_, u32>(2)?))
        })?;
        for row in rows {
            let (game_id, on_play, number) = row?;
            sources.push(GameSource {
                game_id,
                on_play,
                ..GameSource::default()
            });
            numbers.push(number);
        }
    }
    let built = build_game_cards(
        tx,
        &incoming.local_username,
        incoming.opponent.as_deref(),
        &sources,
        &incoming.log,
        &incoming.log,
    )?;
    // The imported postboard rule (cards spec ruling 7) when the scan's deck
    // version is linked.
    let rule = rule_inputs(tx, match_id, &incoming.local_username, &incoming.log)?;
    for (index, (source, mut cards)) in sources.iter().zip(built).enumerate() {
        if let Some(rule) = &rule {
            // A history import is a history match (cards seen only).
            apply_rule(rule, index, numbers[index], &mut cards);
        }
        write_game_cards(tx, source.game_id, &cards)?;
    }
    Ok(())
}

/// A game id used twice in one match is stored once (`games` has
/// `UNIQUE (match_id, mtgo_game_id)`); the later game keeps no id.
fn insert_games(tx: &Transaction, match_id: i64, games: &[HistoryGame]) -> Result<(), StoreError> {
    let mut stmt = tx.prepare_cached(
        "INSERT INTO games (match_id, mtgo_game_id, game_number, result, end_reason, on_play,
             local_mulligans, opp_mulligans, local_die_roll, opp_die_roll, turn_count,
             started_at, ended_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
    )?;
    let mut used: HashSet<i64> = HashSet::new();
    for game in games {
        let game_id = game.mtgo_game_id.filter(|id| used.insert(*id));
        stmt.execute(params![
            match_id,
            game_id,
            game.game_number,
            game.result,
            game.end_reason,
            game.on_play,
            game.local_mulligans,
            game.opp_mulligans,
            game.local_die_roll,
            game.opp_die_roll,
            game.turn_count,
            game.started_at_ms,
            game.ended_at_ms,
        ])?;
    }
    Ok(())
}

/// A history row a 0.x league holds (issue #180): 0.x tracked that match
/// itself (v1's import of it was skipped), so it is 0.x's, not a history
/// import, while the league holds it. A pulled league's (negative
/// `imported_from`) does not count: 0.x deleted those with the rest.
pub(crate) const HELD_BY_0X_LEAGUE: &str = "(league_linked = 1 AND league_id IN
     (SELECT id FROM leagues WHERE source IN ('imported', 'manual') AND imported_from > 0))";

/// Remove every match the history importer stored (0.x `DestroyController`,
/// narrowed to history imports): every `source = 'history'` row with its
/// games (they cascade) and league links, except one a 0.x league holds
/// (`HELD_BY_0X_LEAGUE`) or the player edited (`edited_at`: moved, removed
/// from leagues, deleted or restored; league self-management spec 4.6) or
/// corrected (`corrected_at`: "Edit match", edit a match spec section 2). A
/// history row a live match or a 0.x import took over is no longer
/// `history` and is never touched. Returns the removed tokens.
pub fn delete_history_imports(conn: &mut Connection) -> Result<Vec<String>, StoreError> {
    let tx = conn.transaction()?;
    let mut tokens: Vec<String> = Vec::new();
    {
        let mut stmt = tx.prepare(&format!(
            "SELECT token FROM matches
             WHERE source = 'history' AND NOT {HELD_BY_0X_LEAGUE} AND edited_at IS NULL
               AND corrected_at IS NULL
             ORDER BY id"
        ))?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            tokens.push(row?);
        }
    }
    for token in &tokens {
        // Its games cascade; its league link is a column of its row.
        tx.prepare_cached("DELETE FROM matches WHERE token = ?1 AND source = 'history'")?
            .execute([token])?;
    }
    tx.commit()?;
    Ok(tokens)
}

/// How many stored matches the history importer created (0.x
/// `importedCount`): the ones "Delete all imported" removes.
pub fn history_imported_count(conn: &Connection) -> Result<u32, StoreError> {
    let count: i64 = conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM matches
             WHERE source = 'history' AND NOT {HELD_BY_0X_LEAGUE} AND edited_at IS NULL
               AND corrected_at IS NULL"
        ),
        [],
        |row| row.get(0),
    )?;
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

/// A deck version the import page offers (0.x `Import\IndexController`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryDeckVersion {
    pub id: i64,
    pub deck_name: String,
    /// 0.x `deck_deleted`: v1 archives decks instead.
    pub archived: bool,
    pub modified_at: i64,
    /// The deck's format as stored.
    pub format: String,
    /// The deck's account; `None` for a deck no account has adopted yet.
    pub account_id: Option<i64>,
    /// A Limited deck (format `Limited`, or a draft pool's `limited:` deck).
    pub limited: bool,
}

/// Every deck version, archived decks included, newest first.
pub fn history_deck_versions(conn: &Connection) -> Result<Vec<HistoryDeckVersion>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT v.id, d.name, d.archived_at IS NOT NULL, v.modified_at, d.format, d.account_id,
             d.format = 'Limited' OR d.mtgo_id LIKE 'limited:%'
         FROM deck_versions v JOIN decks d ON d.id = v.deck_id
         ORDER BY v.modified_at DESC, v.id DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(HistoryDeckVersion {
            id: row.get(0)?,
            deck_name: row.get(1)?,
            archived: row.get(2)?,
            modified_at: row.get(3)?,
            format: row.get(4)?,
            account_id: row.get(5)?,
            limited: row.get(6)?,
        })
    })?;
    let mut versions = Vec::new();
    for row in rows {
        versions.push(row?);
    }
    Ok(versions)
}

/// The account a history match without a game log is stored under: the
/// scan's deck's account, else the current account (the active MTGO
/// account, else the only one). `None` when neither is known.
pub fn fallback_account(conn: &Connection, version_id: i64) -> Result<Option<String>, StoreError> {
    let deck_account: Option<String> = conn
        .query_row(
            "SELECT a.username FROM deck_versions v
             JOIN decks d ON d.id = v.deck_id
             JOIN accounts a ON a.id = d.account_id
             WHERE v.id = ?1",
            [version_id],
            |row| row.get(0),
        )
        .optional()?;
    if deck_account.is_some() {
        return Ok(deck_account);
    }
    let Some(current) = crate::accounts::current_account_id(conn, crate::Platform::Mtgo)? else {
        return Ok(None);
    };
    Ok(conn
        .query_row(
            "SELECT username FROM accounts WHERE id = ?1",
            [current],
            |row| row.get(0),
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::import_matches;
    use crate::test_support::{
        hide, imported, insert_manual, insert_manual_league, link_league_match, number, view,
    };
    use crate::write::{MatchWrite, WriteBatch, commit};
    use fixture_tools::TestDir;

    fn database(label: &str) -> (TestDir, Connection) {
        let dir = TestDir::new(label);
        let conn = crate::open_writer(&dir.join("db.sqlite")).expect("open");
        (dir, conn)
    }

    #[test]
    fn every_stored_match_id_counts_hidden_or_not() {
        let (_dir, mut conn) = database("history-ids");
        import_matches(
            &mut conn,
            &[
                imported("q-imported", Some(41)),
                imported("q-hidden", Some(42)),
                imported("q-no-id", None),
            ],
            0,
        )
        .expect("import");
        hide(&conn, "q-hidden");
        // A manual match never has an MTGO match id (a CHECK constraint).
        insert_manual(&conn, "q-manual");

        let ids = stored_mtgo_match_ids(&conn).expect("ids");
        assert_eq!(ids, HashSet::from([41, 42]));
    }

    #[test]
    fn every_stored_token_counts_whatever_its_source() {
        let (_dir, mut conn) = database("history-tokens");
        import_matches(&mut conn, &[imported("q-imported", Some(41))], 0).expect("import");
        insert_manual(&conn, "q-manual");
        hide(&conn, "q-manual");

        let tokens = stored_match_tokens(&conn).expect("tokens");
        assert_eq!(
            tokens,
            HashSet::from(["q-imported".to_string(), "q-manual".to_string()])
        );
    }

    #[test]
    fn a_scan_version_carries_its_deck_name_even_when_archived() {
        let (_dir, conn) = database("history-version");
        conn.execute_batch(
            "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source, archived_at)
                 VALUES (5, 'q-deck', 'Qdeck', 'CMODERN', 0, 0, 'file', 9);
             INSERT INTO deck_versions (id, deck_id, signature, modified_at) VALUES (7, 5, 'U0lH', 0);",
        )
        .expect("seed");

        assert_eq!(
            scan_version(&conn, 7).expect("read"),
            Some(ScanVersion {
                id: 7,
                deck_name: "Qdeck".to_string(),
                signature: "U0lH".to_string(),
            })
        );
        assert_eq!(scan_version(&conn, 8).expect("read"), None);
    }

    /// Deck 5 (`Qdeck`, account Qlocal) with version 7.
    fn deck_version(conn: &Connection) {
        conn.execute_batch(
            "INSERT INTO accounts (username, first_seen_at) VALUES ('Qlocal', 0)
                 ON CONFLICT (username) DO NOTHING;
             INSERT INTO decks (id, mtgo_id, account_id, name, format, created_at, updated_at,
                 source)
             VALUES (5, 'q-deck', (SELECT id FROM accounts WHERE username = 'Qlocal'), 'Qdeck',
                 'CMODERN', 0, 0, 'file');
             INSERT INTO deck_versions (id, deck_id, signature, modified_at) VALUES (7, 5, '', 0);",
        )
        .expect("a deck version");
    }

    fn history_game(number: u32, result: &str) -> HistoryGame {
        HistoryGame {
            mtgo_game_id: Some(900 + i64::from(number)),
            game_number: number,
            result: result.to_string(),
            end_reason: "win".to_string(),
            on_play: Some(number == 1),
            local_mulligans: Some(0),
            opp_mulligans: Some(1),
            local_die_roll: Some(4),
            opp_die_roll: Some(2),
            turn_count: Some(8),
            started_at_ms: Some(1_000 * i64::from(number)),
            ended_at_ms: Some(1_000 * i64::from(number) + 500),
        }
    }

    /// A 2-1 history win against Qopponent with three games from its log.
    fn history(token: &str, mtgo_match_id: i64) -> HistoryMatch {
        HistoryMatch {
            token: Some(token.to_string()),
            mtgo_match_id,
            local_username: "Qhistory".to_string(),
            opponent: Some("Qopponent".to_string()),
            format: Some("CMODERN".to_string()),
            match_type: "League".to_string(),
            outcome: "win".to_string(),
            started_at_ms: 500,
            ended_at_ms: Some(3_500),
            games_won: 2,
            games_lost: 1,
            games: vec![
                history_game(1, "win"),
                history_game(2, "loss"),
                history_game(3, "win"),
            ],
            deck_version_id: 7,
            log: Vec::new(),
        }
    }

    #[test]
    fn a_history_match_with_its_game_log_stores_the_log_and_card_rows() {
        let bytes = fixture_tools::load_game_log("clean_2_0_win");
        let entries = game_detail::parse_game_log(&bytes, chrono_tz::UTC)
            .expect("parses")
            .entries;
        let players = game_detail::detect_players(&entries);
        let (_dir, mut conn) = database("history-log-rows");
        let mut found = history("q-h-log", 77);
        found.local_username = players[0].clone();
        found.opponent = Some(players[1].clone());
        found.games.truncate(2);
        found.log = entries.clone();
        import_history(&mut conn, &[found], 5_000).expect("import");
        assert_eq!(
            number(&conn, "SELECT entry_count FROM game_logs"),
            entries.len() as i64
        );
        assert!(
            number(
                &conn,
                "SELECT COUNT(*) FROM game_cards WHERE side = 'local'"
            ) > 0
        );
        assert!(
            number(
                &conn,
                "SELECT COUNT(*) FROM game_cards WHERE side = 'opponent' AND seen > 0"
            ) > 0,
            "the opponent's log-only reveals"
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM games WHERE local_starting_hand_size IS NULL"
            ),
            0,
            "the log's starting hand sizes"
        );
        // Without a log: no rows.
        import_history(&mut conn, &[history("q-h-nolog", 78)], 5_000).expect("import");
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM game_logs"), 1);
    }

    #[test]
    fn a_history_import_applies_the_imported_postboard_rule() {
        let (_dir, mut conn) = database("history-rule");
        deck_version(&conn);
        let registered = vec![
            decks::DeckCard {
                catalog_id: 3001,
                quantity: 4,
                sideboard: false,
            },
            decks::DeckCard {
                catalog_id: 3002,
                quantity: 2,
                sideboard: true,
            },
        ];
        conn.execute(
            "UPDATE deck_versions SET signature = ?1 WHERE id = 7",
            [decks::signature(&registered)],
        )
        .expect("signature");
        let lines = [
            "@P@PQhistory joined the game.",
            "@P@PQopponent joined the game.",
            "@PTurn 1: Qhistory",
            "@PQhistory casts @[Qmain@:6002,100:@].",
            "@PQhistory wins the game.",
            "@P@PQhistory joined the game.",
            "@P@PQopponent joined the game.",
            "@PTurn 1: Qopponent",
            "@PQhistory casts @[Qside@:6004,200:@].",
            "@PQhistory casts @[Qside@:6004,201:@].",
            "@PQopponent wins the game.",
        ];
        let mut found = history("q-h-rule", 79);
        found.games.truncate(2);
        found.log = lines
            .iter()
            .enumerate()
            .map(|(index, message)| GameLogEntry {
                timestamp_ms: index as i64,
                message: message.to_string(),
            })
            .collect();
        import_history(&mut conn, &[found], 5_000).expect("import");
        let rows = |number: u32| -> Vec<String> {
            let mut stmt = conn
                .prepare(
                    "SELECT c.catalog_id || ':' || c.main_qty || ',' || c.side_qty || ','
                         || c.sided_in || ',' || c.`cast` || ',' || c.inferred || ','
                         || coalesce(c.played, '-') || ',' || coalesce(c.activated, '-')
                     FROM game_cards c JOIN games g ON g.id = c.game_id
                     WHERE c.side = 'local' AND g.game_number = ?1 ORDER BY c.catalog_id",
                )
                .expect("rows");
            stmt.query_map([number], |row| row.get::<_, String>(0))
                .expect("rows")
                .map(|row| row.expect("row"))
                .collect()
        };
        // Rows the rule made (3002 in game 1, 3001 in game 2) count 0
        // plays, not "no game log".
        assert_eq!(rows(1), vec!["3001:4,0,0,1,0,0,0", "3002:0,2,0,0,0,0,0"]);
        assert_eq!(rows(2), vec!["3001:4,0,0,0,1,0,0", "3002:2,0,2,2,1,0,0"]);
    }

    fn text(conn: &Connection, sql: &str) -> String {
        conn.query_row(sql, [], |row| row.get(0)).expect(sql)
    }

    #[test]
    fn a_history_match_is_stored_as_history_with_its_games_and_link() {
        let (_dir, mut conn) = database("history-import-games");
        deck_version(&conn);
        let counts =
            import_history(&mut conn, &[history("q-log-token", 501)], 9_000).expect("import");
        assert_eq!(
            counts,
            HistoryImportCounts {
                imported: vec!["q-log-token".to_string()],
                skipped_existing: 0,
                skipped_token: 0,
            }
        );
        assert_eq!(
            text(
                &conn,
                "SELECT concat_ws(',', m.token, m.mtgo_match_id, a.username, o.username, m.source,
                     m.format, m.match_type, m.state, m.outcome, m.started_at, m.ended_at,
                     m.last_activity_at, m.needs_attention, m.revision, m.derived_version,
                     m.games_won_imported, m.games_lost_imported)
                 FROM matches m JOIN accounts a ON a.id = m.account_id
                 JOIN opponents o ON o.id = m.opponent_id"
            ),
            "q-log-token,501,Qhistory,Qopponent,history,CMODERN,League,complete,win,500,3500,\
             3500,0,1,0,2,1"
        );
        assert_eq!(
            text(
                &conn,
                "SELECT group_concat(row, ';') FROM (SELECT concat_ws(',', game_number,
                     mtgo_game_id, result, end_reason, on_play, local_mulligans, opp_mulligans,
                     local_die_roll, opp_die_roll, turn_count, started_at, ended_at) AS row
                 FROM games ORDER BY game_number)"
            ),
            "1,901,win,win,1,0,1,4,2,8,1000,1500;2,902,loss,win,0,0,1,4,2,8,2000,2500;\
             3,903,win,win,0,0,1,4,2,8,3000,3500"
        );
        assert_eq!(
            text(&conn, "SELECT token || ',' || deck_version_id FROM matches"),
            "q-log-token,7"
        );
        assert_eq!(
            number(
                &conn,
                "SELECT first_seen_at FROM accounts WHERE username = 'Qhistory'"
            ),
            9_000,
            "a new account is registered"
        );
        assert_eq!(history_imported_count(&conn).expect("count"), 1);
    }

    #[test]
    fn a_gameless_history_match_keeps_its_game_counts_and_links_only_a_real_version() {
        let (_dir, mut conn) = database("history-import-gameless");
        deck_version(&conn);
        let mut gameless = history("unused", 502);
        gameless.token = None;
        gameless.games.clear();
        gameless.ended_at_ms = None;
        gameless.outcome = "loss".to_string();
        gameless.games_won = 1;
        gameless.games_lost = 2;
        let mut stale_version = history("q-other", 503);
        stale_version.games.clear();
        stale_version.deck_version_id = 99;
        let counts = import_history(&mut conn, &[gameless, stale_version], 1).expect("import");
        assert_eq!(counts.imported.len(), 2);
        let uuid = counts.imported[0].clone();
        assert_eq!(
            uuid,
            history_token(502),
            "the token comes from the match id"
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM games"), 0);
        assert_eq!(
            text(
                &conn,
                "SELECT concat_ws(',', outcome, games_won_imported, games_lost_imported,
                     last_activity_at, ended_at IS NULL)
                 FROM matches WHERE mtgo_match_id = 502"
            ),
            "loss,1,2,500,1"
        );
        assert_eq!(
            text(
                &conn,
                "SELECT group_concat(token) FROM matches WHERE deck_version_id IS NOT NULL"
            ),
            uuid,
            "a version that no longer exists is not linked"
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE source = 'history'"
            ),
            2
        );
    }

    #[test]
    fn a_history_token_is_a_version_8_uuid_from_the_match_id() {
        // SHA-256 of "mymtgo:history:502", first 16 bytes, version 8, RFC 4122 variant.
        assert_eq!(history_token(502), "0ce23c9d-c21a-870f-8a6f-3ca394a98a26");
        assert_eq!(history_token(700), "2a3faa70-f109-8a07-9865-26408bdfe245");
        let token = history_token(502);
        let groups: Vec<usize> = token.split('-').map(str::len).collect();
        assert_eq!(groups, vec![8, 4, 4, 4, 12]);
        assert!(
            token
                .chars()
                .all(|c| c == '-' || c.is_ascii_digit() || ('a'..='f').contains(&c))
        );
        assert_eq!(&token[14..15], "8", "version 8");
        assert!("89ab".contains(&token[19..20]), "RFC 4122 variant");
        assert_ne!(history_token(502), history_token(503));
    }

    #[test]
    fn two_stores_give_a_logless_history_match_the_same_token() {
        let mut logless = history("unused", 502);
        logless.token = None;
        let mut tokens = Vec::new();
        for label in ["history-token-a", "history-token-b"] {
            let (_dir, mut conn) = database(label);
            let counts =
                import_history(&mut conn, std::slice::from_ref(&logless), 1).expect("import");
            assert_eq!(counts.imported.len(), 1);
            // A rerun on the same store still skips it.
            let rerun =
                import_history(&mut conn, std::slice::from_ref(&logless), 2).expect("rerun");
            assert!(rerun.imported.is_empty());
            assert_eq!(rerun.skipped_existing, 1);
            tokens.push(counts.imported[0].clone());
        }
        assert_eq!(tokens[0], tokens[1]);
        assert_eq!(tokens[0], history_token(502));
    }

    #[test]
    fn a_derived_token_already_stored_counts_as_skipped_token() {
        let (_dir, mut conn) = database("history-token-taken");
        // The same match arrived first under its derived token without an MTGO
        // match id (a 0.x import of a synced row).
        import_matches(&mut conn, &[imported(&history_token(502), None)], 0).expect("import");
        let mut logless = history("unused", 502);
        logless.token = None;
        let counts = import_history(&mut conn, &[logless], 1).expect("history");
        assert!(counts.imported.is_empty());
        assert_eq!(counts.skipped_token, 1);
    }

    #[test]
    fn a_history_match_already_stored_by_match_id_or_token_is_skipped() {
        let (_dir, mut conn) = database("history-import-skips");
        deck_version(&conn);
        // A 0.x import with id 41, a live match with id 99 and token OTHER-ish.
        import_matches(&mut conn, &[imported("q-0x", Some(41))], 0).expect("0.x import");
        commit(
            &mut conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(view("q-live", "Qopponent", 1_000)),
                    revision: 1,
                }],
                now_ms: 1,
                ..WriteBatch::default()
            },
        )
        .expect("live match with id 99");
        insert_manual(&conn, "q-manual");
        let before = number(&conn, "SELECT COUNT(*) FROM matches");

        let batch = vec![
            history("q-a", 41),
            history("q-b", 99),
            history("q-live", 600),
            history("q-manual", 601),
            history("q-new", 602),
            // The same history id twice in one batch: the first wins.
            history("q-again", 602),
        ];
        let counts = import_history(&mut conn, &batch, 5).expect("import");
        assert_eq!(
            counts,
            HistoryImportCounts {
                imported: vec!["q-new".to_string()],
                skipped_existing: 3,
                skipped_token: 2,
            }
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), before + 1);
        assert_eq!(
            text(
                &conn,
                "SELECT group_concat(token) FROM matches WHERE source = 'history'"
            ),
            "q-new"
        );

        let rerun = import_history(&mut conn, &batch, 6).expect("rerun");
        assert!(rerun.imported.is_empty(), "a rerun stores nothing");
        assert_eq!(rerun.skipped_existing, 4);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), before + 1);
    }

    #[test]
    fn deleting_removes_only_history_imports_and_their_records() {
        let (_dir, mut conn) = database("history-delete");
        deck_version(&conn);
        import_matches(&mut conn, &[imported("q-0x", Some(41))], 0).expect("0.x import");
        insert_manual(&conn, "q-manual");
        commit(
            &mut conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(view("q-live", "Qopponent", 1_000)),
                    revision: 1,
                }],
                now_ms: 1,
                ..WriteBatch::default()
            },
        )
        .expect("live match");
        import_history(&mut conn, &[history("q-h1", 700), history("q-h2", 701)], 2)
            .expect("history");
        // Records on the history matches and on the others.
        conn.execute_batch(
            "UPDATE matches SET notes = 'Q note' WHERE token = 'q-h1';
             UPDATE matches SET notes = 'Q kept' WHERE token = 'q-0x';
             UPDATE matches SET hidden_at = 1 WHERE token = 'q-h2';
             UPDATE matches SET deck_version_id = 7 WHERE token IN ('q-0x', 'q-live');",
        )
        .expect("records");
        let local = number(&conn, "SELECT id FROM accounts WHERE username = 'Qlocal'");
        insert_manual_league(&conn, 1, Some(local), Some(5), "Q league", 1, 1);
        link_league_match(&conn, 1, "q-h1");
        let others = |conn: &Connection| {
            text(
                conn,
                "SELECT group_concat(token || source, ',') FROM
                     (SELECT token, source FROM matches WHERE token NOT LIKE 'q-h%' ORDER BY id)",
            )
        };
        let others_before = others(&conn);
        assert_eq!(history_imported_count(&conn).expect("count"), 2);

        let mut removed = delete_history_imports(&mut conn).expect("delete");
        removed.sort();
        assert_eq!(removed, vec!["q-h1".to_string(), "q-h2".to_string()]);
        assert_eq!(others(&conn), others_before, "0.x, manual and live stay");
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE token LIKE 'q-h%'"
            ),
            0
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM games g JOIN matches m ON m.id = g.match_id
                 WHERE m.source = 'history' AND m.token LIKE 'q-h%'"
            ),
            0
        );
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM games"),
            3 + 2 + 3,
            "the 0.x, manual and live games stay"
        );
        assert_eq!(
            text(
                &conn,
                "SELECT group_concat(token) FROM matches WHERE notes IS NOT NULL"
            ),
            "q-0x"
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE hidden_at IS NOT NULL"
            ),
            0
        );
        assert_eq!(
            text(
                &conn,
                "SELECT group_concat(token) FROM (SELECT token FROM matches
                     WHERE deck_version_id IS NOT NULL ORDER BY token)"
            ),
            "q-0x,q-live"
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE league_linked = 1"
            ),
            0
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE source = 'history'"
            ),
            0
        );
        assert_eq!(history_imported_count(&conn).expect("count"), 0);
        assert!(delete_history_imports(&mut conn).expect("again").is_empty());
    }

    #[test]
    fn delete_all_imported_removes_only_history_rows() {
        let (_dir, mut conn) = database("history-delete-only-history");
        deck_version(&conn);
        import_matches(&mut conn, &[imported("q-0x", Some(41))], 0).expect("0.x import");
        insert_manual(&conn, "q-manual");
        commit(
            &mut conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(view("q-live", "Qopponent", 1_000)),
                    revision: 1,
                }],
                now_ms: 1,
                ..WriteBatch::default()
            },
        )
        .expect("live match");
        import_history(&mut conn, &[history("q-h1", 700), history("q-h2", 701)], 2)
            .expect("history");
        let sources = |conn: &Connection| {
            text(
                conn,
                "SELECT group_concat(token || ':' || source, ',') FROM
                     (SELECT token, source FROM matches ORDER BY id)",
            )
        };
        assert_eq!(
            sources(&conn),
            "q-0x:imported,q-manual:manual,q-live:log,q-h1:history,q-h2:history"
        );
        let games = |conn: &Connection| number(conn, "SELECT COUNT(*) FROM games");
        assert_eq!(games(&conn), 3 + 2 + 3 + 3 + 3);

        let mut removed = delete_history_imports(&mut conn).expect("delete");
        removed.sort();
        assert_eq!(removed, vec!["q-h1".to_string(), "q-h2".to_string()]);
        assert_eq!(sources(&conn), "q-0x:imported,q-manual:manual,q-live:log");
        assert_eq!(games(&conn), 3 + 2 + 3, "the history games cascade");
        assert!(delete_history_imports(&mut conn).expect("again").is_empty());
    }

    /// A 0.x league (run 1 or manual league 2) holding `token`.
    fn league_of(
        legacy_id: i64,
        manual: bool,
        token: &str,
    ) -> crate::import_leagues::ImportedLeague {
        crate::import_leagues::ImportedLeague {
            legacy_id,
            manual,
            kind: "constructed".to_string(),
            set_code: None,
            course_id: None,
            token: if manual {
                "manual_q0000000000000000000000000".to_string()
            } else {
                "q-league".to_string()
            },
            event_id: Some(7_000),
            name: "Q League".to_string(),
            format: "CMODERN".to_string(),
            state: "complete".to_string(),
            started_at_ms: 1_000,
            joined_at_ms: None,
            completed_at_ms: None,
            dropped_at_ms: None,
            created_at_ms: Some(1_000),
            deck: None,
            notes: None,
            matches: vec![crate::import_leagues::LeagueMember {
                token: token.to_string(),
                mtgo_match_id: None,
            }],
        }
    }

    // Issue #180: a history row a 0.x league holds (0.x tracked the match,
    // v1's import of it was skipped) is 0.x's: "Delete all imported" keeps
    // it while the league holds it. A pulled league's member is deleted, as
    // 0.x deleted it.
    #[test]
    fn delete_all_imported_keeps_a_history_row_a_0x_league_holds() {
        let (_dir, mut conn) = database("history-delete-keeps-0x-league");
        deck_version(&conn);
        import_history(
            &mut conn,
            &[
                history("q-h1", 700),
                history("q-h2", 701),
                history("q-h3", 702),
                history("q-h4", 703),
            ],
            1,
        )
        .expect("history");
        let mut pulled = league_of(-5, false, "q-h4");
        pulled.token = "q-pulled".to_string();
        crate::import_leagues::import_leagues(
            &mut conn,
            &[
                league_of(1, false, "q-h1"),
                league_of(2, true, "q-h2"),
                pulled,
            ],
            2,
        )
        .expect("0.x leagues");
        let sources = |conn: &Connection| {
            text(
                conn,
                "SELECT group_concat(token || ':' || source || ':' || league_linked, ',') FROM
                     (SELECT * FROM matches ORDER BY id)",
            )
        };
        assert_eq!(
            sources(&conn),
            "q-h1:history:1,q-h2:history:1,q-h3:history:0,q-h4:history:1",
            "the rows stay history imports"
        );
        assert_eq!(history_imported_count(&conn).expect("count"), 2);

        let removed = delete_history_imports(&mut conn).expect("delete");
        assert_eq!(removed, vec!["q-h3".to_string(), "q-h4".to_string()]);
        assert_eq!(sources(&conn), "q-h1:history:1,q-h2:history:1");
        assert_eq!(history_imported_count(&conn).expect("count"), 0);

        // Unlinked, it is a history import to delete again.
        conn.execute(
            "UPDATE matches SET league_id = NULL, league_linked = 0 WHERE token = 'q-h1'",
            [],
        )
        .expect("unlink");
        assert_eq!(history_imported_count(&conn).expect("count"), 1);
        let removed = delete_history_imports(&mut conn).expect("delete again");
        assert_eq!(removed, vec!["q-h1".to_string()]);
    }

    #[test]
    fn a_history_import_a_live_match_took_over_is_neither_counted_nor_deleted() {
        let (_dir, mut conn) = database("history-live-takes-over");
        deck_version(&conn);
        import_history(
            &mut conn,
            &[history("q-same-token", 800), history("q-copy", 99)],
            1,
        )
        .expect("history");
        let mut live_same_token = view("q-same-token", "Qopponent", 1_000);
        live_same_token.mtgo_match_id = Some(800);
        commit(
            &mut conn,
            &WriteBatch {
                matches: vec![
                    MatchWrite::Upsert {
                        view: Box::new(live_same_token),
                        revision: 1,
                    },
                    // Match id 99: the history copy under another token goes.
                    MatchWrite::Upsert {
                        view: Box::new(view("q-live", "Qopponent", 1_000)),
                        revision: 1,
                    },
                ],
                now_ms: 2,
                ..WriteBatch::default()
            },
        )
        .expect("live matches");
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE source = 'history'"
            ),
            0,
            "the copy under the live token became the live match; the other copy went"
        );
        assert_eq!(history_imported_count(&conn).expect("count"), 0);
        assert!(
            delete_history_imports(&mut conn)
                .expect("delete")
                .is_empty()
        );
        assert_eq!(
            text(&conn, "SELECT group_concat(source) FROM matches"),
            "log,log",
            "both live matches stay"
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE source = 'history'"
            ),
            0
        );
    }

    #[test]
    fn a_live_takeover_clears_the_history_game_counts() {
        let (_dir, mut conn) = database("history-live-clears-counts");
        deck_version(&conn);
        import_history(&mut conn, &[history("q-same-token", 800)], 1).expect("history");
        let mut live = view("q-same-token", "Qopponent", 1_000);
        live.mtgo_match_id = Some(800);
        commit(
            &mut conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(live),
                    revision: 1,
                }],
                now_ms: 2,
                ..WriteBatch::default()
            },
        )
        .expect("live match");
        assert_eq!(
            text(
                &conn,
                "SELECT concat_ws(',', source, games_won_imported IS NULL,
                     games_lost_imported IS NULL) FROM matches"
            ),
            "log,1,1",
            "the live row folds no stale history counts"
        );
    }

    /// What a 0.x import leaves after a history import of the same match.
    fn after_0x_import(conn: &mut Connection) -> String {
        text(
            conn,
            "SELECT group_concat(row, ';') FROM (SELECT concat_ws(',', token, mtgo_match_id,
                 source, games_won_imported IS NULL, games_lost_imported IS NULL) AS row
             FROM matches ORDER BY id)",
        )
    }

    #[test]
    fn a_0x_import_with_the_history_import_s_token_replaces_it_and_becomes_a_0x_import() {
        let (_dir, mut conn) = database("history-then-0x-same-token");
        deck_version(&conn);
        import_history(&mut conn, &[history("q-real-token", 900)], 1).expect("history");

        let counts =
            import_matches(&mut conn, &[imported("q-real-token", Some(900))], 2).expect("0.x");
        assert_eq!(counts.updated, 1, "the history row takes 0.x's data");
        assert_eq!(after_0x_import(&mut conn), "q-real-token,900,imported,1,1");
        assert_eq!(
            text(
                &conn,
                "SELECT concat_ws(',', a.username, m.match_type, m.started_at)
                 FROM matches m JOIN accounts a ON a.id = m.account_id"
            ),
            "Qlocal,Constructed,10000",
            "0.x's data"
        );
        assert_eq!(
            text(
                &conn,
                "SELECT group_concat(end_reason) FROM (SELECT end_reason FROM games
                     ORDER BY game_number)"
            ),
            "unknown,unknown,unknown",
            "0.x's games"
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE source = 'history'"
            ),
            0
        );
        assert_eq!(history_imported_count(&conn).expect("count"), 0);

        assert!(
            delete_history_imports(&mut conn)
                .expect("delete")
                .is_empty()
        );
        assert_eq!(
            after_0x_import(&mut conn),
            "q-real-token,900,imported,1,1",
            "the 0.x match stays"
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM games"), 3);
    }

    #[test]
    fn a_0x_import_with_the_history_import_s_match_id_replaces_it_and_takes_its_records() {
        let (_dir, mut conn) = database("history-then-0x-same-id");
        deck_version(&conn);
        import_history(
            &mut conn,
            &[history("q-history-a", 901), history("q-history-b", 902)],
            1,
        )
        .expect("history");
        conn.execute_batch(
            "UPDATE matches SET notes = 'Q history note' WHERE token = 'q-history-a';
             UPDATE matches SET notes = 'Q history note b' WHERE token = 'q-history-b';
             UPDATE matches SET hidden_at = 1 WHERE token = 'q-history-a';",
        )
        .expect("records on the history matches");
        let local = number(&conn, "SELECT id FROM accounts WHERE username = 'Qlocal'");
        insert_manual_league(&conn, 1, Some(local), Some(5), "Q league", 1, 1);
        link_league_match(&conn, 1, "q-history-a");
        let mut with_note = imported("q-0x-b", Some(902));
        with_note.note = Some("Q 0x note".to_string());

        let counts =
            import_matches(&mut conn, &[imported("q-0x-a", Some(901)), with_note], 2).expect("0.x");
        assert_eq!(counts.added, 2);
        assert_eq!(
            after_0x_import(&mut conn),
            "q-0x-a,901,imported,1,1;q-0x-b,902,imported,1,1",
            "one row per MTGO match id, the 0.x ones"
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE source = 'history'"
            ),
            0
        );
        assert_eq!(history_imported_count(&conn).expect("count"), 0);
        assert_eq!(
            text(
                &conn,
                "SELECT group_concat(row, ';') FROM (SELECT token || ',' || notes AS row
                     FROM matches WHERE notes IS NOT NULL ORDER BY token)"
            ),
            "q-0x-a,Q history note;q-0x-b,Q 0x note",
            "the history note moves only where 0.x has none"
        );
        assert_eq!(
            text(
                &conn,
                "SELECT group_concat(token) FROM matches WHERE hidden_at IS NOT NULL"
            ),
            "q-0x-a"
        );
        assert_eq!(
            text(
                &conn,
                "SELECT group_concat(row, ';') FROM (SELECT token || ',' || deck_version_id
                     AS row FROM matches WHERE deck_version_id IS NOT NULL ORDER BY token)"
            ),
            "q-0x-a,7;q-0x-b,7"
        );
        assert_eq!(
            text(
                &conn,
                "SELECT group_concat(token) FROM matches WHERE league_linked = 1"
            ),
            "q-0x-a"
        );
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM games"),
            6,
            "0.x's games only"
        );

        assert!(
            delete_history_imports(&mut conn)
                .expect("delete")
                .is_empty()
        );
        assert_eq!(
            after_0x_import(&mut conn),
            "q-0x-a,901,imported,1,1;q-0x-b,902,imported,1,1",
            "the 0.x matches stay"
        );
    }

    #[test]
    fn a_gameless_0x_import_taking_over_a_history_import_keeps_0x_s_score() {
        let (_dir, mut conn) = database("history-then-0x-gameless");
        deck_version(&conn);
        import_history(
            &mut conn,
            &[history("q-real-token", 910), history("q-history-b", 911)],
            1,
        )
        .expect("history");
        let mut same_token = imported("q-real-token", Some(910));
        same_token.games.clear();
        same_token.outcome = "loss".to_string();
        same_token.games_won_imported = Some(0);
        same_token.games_lost_imported = Some(2);
        let mut same_id = same_token.clone();
        same_id.token = "q-0x-b".to_string();
        same_id.mtgo_match_id = Some(911);
        same_id.games_won_imported = Some(1);

        let batch = vec![same_token, same_id];
        let counts = import_matches(&mut conn, &batch, 2).expect("0.x");
        assert_eq!((counts.updated, counts.added), (1, 1));
        let scores = |conn: &Connection| {
            text(
                conn,
                "SELECT group_concat(row, ';') FROM (SELECT concat_ws(',', token, source,
                     games_won_imported, games_lost_imported) AS row FROM matches ORDER BY token)",
            )
        };
        assert_eq!(
            scores(&conn),
            "q-0x-b,imported,1,2;q-real-token,imported,0,2",
            "0.x's scores, not the history's 2-1"
        );
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM games"),
            0,
            "0.x's games"
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE source = 'history'"
            ),
            0
        );

        let rerun = import_matches(&mut conn, &batch, 3).expect("rerun");
        assert_eq!(rerun.unchanged, 2, "a rerun writes nothing");
        assert!(
            delete_history_imports(&mut conn)
                .expect("delete")
                .is_empty()
        );
        assert_eq!(
            scores(&conn),
            "q-0x-b,imported,1,2;q-real-token,imported,0,2",
            "the 0.x matches stay"
        );
    }

    #[test]
    fn deck_versions_list_every_version_newest_first_archived_decks_too() {
        let (_dir, conn) = database("history-deck-versions");
        conn.execute_batch(
            "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source,
                 archived_at)
             VALUES (5, 'q-a', 'Qalpha', 'CMODERN', 0, 0, 'file', NULL),
                    (6, 'q-b', 'Qbeta', 'CPAUPER', 0, 0, 'file', 9);
             INSERT INTO deck_versions (id, deck_id, signature, modified_at)
             VALUES (1, 5, 'a', 100), (2, 6, 'b', 300), (3, 5, 'c', 200);",
        )
        .expect("decks");
        let versions = history_deck_versions(&conn).expect("versions");
        let shown: Vec<(i64, &str, bool, i64, &str)> = versions
            .iter()
            .map(|v| {
                (
                    v.id,
                    v.deck_name.as_str(),
                    v.archived,
                    v.modified_at,
                    v.format.as_str(),
                )
            })
            .collect();
        assert_eq!(
            shown,
            vec![
                (2, "Qbeta", true, 300, "CPAUPER"),
                (3, "Qalpha", false, 200, "CMODERN"),
                (1, "Qalpha", false, 100, "CMODERN"),
            ]
        );
    }

    #[test]
    fn the_fallback_account_is_the_deck_s_then_the_current_one() {
        let (_dir, conn) = database("history-fallback-account");
        assert_eq!(fallback_account(&conn, 7).expect("none"), None);
        deck_version(&conn);
        assert_eq!(
            fallback_account(&conn, 7).expect("deck's"),
            Some("Qlocal".to_string())
        );
        conn.execute_batch(
            "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source)
                 VALUES (6, 'q-none', 'Qnone', 'CMODERN', 0, 0, 'file');
             INSERT INTO deck_versions (id, deck_id, signature, modified_at) VALUES (8, 6, '', 0);",
        )
        .expect("a deck without an account");
        assert_eq!(
            fallback_account(&conn, 8).expect("the only account"),
            Some("Qlocal".to_string())
        );
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at) VALUES ('Qsecond', 0)",
            [],
        )
        .expect("a second account");
        assert_eq!(fallback_account(&conn, 8).expect("unknown"), None);
    }

    // League self-management spec 4.6: a history row the player edited
    // (`edited_at`) is kept by "Delete all imported", as one a 0.x league
    // holds is, and is not counted.
    #[test]
    fn delete_all_imported_keeps_a_history_row_the_player_edited() {
        let (_dir, mut conn) = database("history-delete-keeps-edited");
        deck_version(&conn);
        import_history(
            &mut conn,
            &[
                history("q-h1", 700),
                history("q-h2", 701),
                history("q-h3", 702),
            ],
            1,
        )
        .expect("history");
        conn.execute_batch(
            "UPDATE matches SET no_league = 1, edited_at = 40 WHERE token = 'q-h1';
             UPDATE matches SET hidden_at = 50, edited_at = 50 WHERE token = 'q-h2';",
        )
        .expect("player edits");
        assert_eq!(history_imported_count(&conn).expect("count"), 1);

        let removed = delete_history_imports(&mut conn).expect("delete");
        assert_eq!(removed, vec!["q-h3".to_string()]);
        assert_eq!(
            text(
                &conn,
                "SELECT group_concat(token || ':' || source, ',') FROM
                     (SELECT token, source FROM matches ORDER BY id)"
            ),
            "q-h1:history,q-h2:history"
        );
        assert_eq!(history_imported_count(&conn).expect("count"), 0);
    }

    // Edit a match spec section 2: a history row the player corrected
    // (`corrected_at`) is kept by "Delete all imported" and not counted.
    #[test]
    fn delete_all_imported_keeps_a_history_row_the_player_corrected() {
        let (_dir, mut conn) = database("history-delete-keeps-corrected");
        deck_version(&conn);
        import_history(&mut conn, &[history("q-h1", 700), history("q-h2", 701)], 1)
            .expect("history");
        conn.execute(
            "UPDATE matches SET outcome = 'win', corrected_at = 40 WHERE token = 'q-h1'",
            [],
        )
        .expect("a fix");
        assert_eq!(history_imported_count(&conn).expect("count"), 1);

        let removed = delete_history_imports(&mut conn).expect("delete");
        assert_eq!(removed, vec!["q-h2".to_string()]);
        assert_eq!(
            text(&conn, "SELECT group_concat(token, ',') FROM matches"),
            "q-h1"
        );
        assert_eq!(history_imported_count(&conn).expect("count"), 0);
    }

    #[test]
    fn a_history_rerun_and_a_0x_takeover_keep_every_player_column() {
        let (_dir, mut conn) = database("history-keeps-player-columns");
        deck_version(&conn);
        import_history(&mut conn, &[history("q-h1", 700)], 1).expect("history");
        conn.execute(
            "UPDATE matches SET no_league = 1, hidden_at = 40, edited_at = 40
             WHERE token = 'q-h1'",
            [],
        )
        .expect("player edits");
        let columns = |conn: &Connection| {
            text(
                conn,
                "SELECT group_concat(row, ';') FROM (SELECT concat_ws(',', token, source,
                     no_league, league_linked, ifnull(hidden_at, '-'), ifnull(edited_at, '-'))
                     AS row FROM matches ORDER BY id)",
            )
        };

        let again = import_history(&mut conn, &[history("q-h1", 700)], 2).expect("again");
        assert_eq!(again.skipped_existing, 1);
        assert_eq!(columns(&conn), "q-h1,history,1,0,40,40");

        // 0.x has the match under another token: its row takes the history
        // copy's records, the player's league choice and edit time with them.
        import_matches(&mut conn, &[imported("q-0x", Some(700))], 3).expect("0.x");
        assert_eq!(columns(&conn), "q-0x,imported,1,0,40,40");
    }
}
