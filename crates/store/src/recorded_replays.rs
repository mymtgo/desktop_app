//! Imported `.mtgr` recordings (recorded replays spec): the files the opt-in
//! recorder wrote, kept whole as text, and how a match and its games link to
//! them. A recording is a fact about a file and may arrive before its match
//! exists, so nothing is written on the match: the link is worked out on read
//! (live equals rebuild). A match links when its lowercased token is the
//! file's token, else when its MTGO match id is the file's. A game links to
//! the recorded game with its MTGO game id, else the one with its number.

use std::collections::{HashMap, HashSet};

use mtgr::RecordingHeader;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::StoreError;
use crate::arena_replay::{quoted_strings, token_art};
use crate::cards::catalog_range;

/// What storing a file did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportOutcome {
    /// No recording had this token.
    Added,
    /// A recording with this token and the same hash was already stored.
    Unchanged,
    /// A recording with this token had another hash: its text and games were
    /// replaced.
    Replaced,
}

/// A file read from the recorder folder, ready to store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRecording {
    pub header: RecordingHeader,
    pub text: String,
    /// Lowercase hex sha256 of the file's bytes.
    pub file_hash: String,
}

/// What storing a batch of files did, file by file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub added: u32,
    pub replaced: u32,
    pub unchanged: u32,
}

/// A game's recording: the file's token and the game's position in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedGame {
    pub token: String,
    pub position: u32,
}

/// Stores a recording under its token. The same hash again changes nothing; a
/// new hash replaces the text and the games.
pub fn store_recording(
    tx: &Transaction,
    header: &RecordingHeader,
    text: &str,
    file_hash: &str,
    now_ms: i64,
) -> Result<ImportOutcome, StoreError> {
    let known: Option<String> = tx
        .query_row(
            "SELECT file_hash FROM recorded_replays WHERE token = ?1",
            [&header.match_token],
            |row| row.get(0),
        )
        .optional()?;
    let outcome = match known {
        Some(hash) if hash == file_hash => return Ok(ImportOutcome::Unchanged),
        Some(_) => ImportOutcome::Replaced,
        None => ImportOutcome::Added,
    };
    tx.execute(
        "DELETE FROM recorded_replays WHERE token = ?1",
        [&header.match_token],
    )?;
    tx.execute(
        "INSERT INTO recorded_replays
             (token, mtgo_match_id, text, file_hash, recorder, imported_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            header.match_token,
            header.mtgo_match_id,
            text,
            file_hash,
            header.recorder,
            now_ms
        ],
    )?;
    for game in &header.games {
        tx.execute(
            "INSERT INTO recorded_replay_games (token, position, game_number, mtgo_game_id)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                header.match_token,
                game.position,
                game.game_number,
                game.mtgo_game_id
            ],
        )?;
    }
    Ok(outcome)
}

/// Stores a batch of files with `store_recording` and counts the outcomes.
pub fn store_recordings(
    tx: &Transaction,
    recordings: &[NewRecording],
    now_ms: i64,
) -> Result<ImportReport, StoreError> {
    let mut report = ImportReport::default();
    for recording in recordings {
        let outcome = store_recording(
            tx,
            &recording.header,
            &recording.text,
            &recording.file_hash,
            now_ms,
        )?;
        match outcome {
            ImportOutcome::Added => report.added += 1,
            ImportOutcome::Replaced => report.replaced += 1,
            ImportOutcome::Unchanged => report.unchanged += 1,
        }
    }
    Ok(report)
}

/// Every stored recording's token and file hash, so the importer skips
/// unchanged files.
pub fn known_hashes(conn: &Connection) -> Result<HashMap<String, String>, StoreError> {
    let mut stmt = conn.prepare("SELECT token, file_hash FROM recorded_replays")?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The recording a listed match links to: by token, else by MTGO match id
/// (the most recently imported when several share one). The token wins
/// globally: a recording whose token names a listed match never links to
/// another match by id.
fn linked_token(conn: &Connection, match_id: i64) -> Result<Option<String>, StoreError> {
    let Some((token, mtgo_match_id)) = conn
        .query_row(
            "SELECT token, mtgo_match_id FROM listed_matches WHERE id = ?1",
            [match_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .optional()?
    else {
        return Ok(None);
    };
    let by_token: Option<String> = conn
        .query_row(
            "SELECT token FROM recorded_replays WHERE token = ?1",
            [token.to_lowercase()],
            |row| row.get(0),
        )
        .optional()?;
    if by_token.is_some() {
        return Ok(by_token);
    }
    let Some(mtgo_match_id) = mtgo_match_id else {
        return Ok(None);
    };
    Ok(conn
        .query_row(
            "SELECT token FROM recorded_replays WHERE mtgo_match_id = ?1
               AND NOT EXISTS (SELECT 1 FROM listed_matches lm WHERE lower(lm.token) = recorded_replays.token)
             ORDER BY imported_at DESC, token LIMIT 1",
            [mtgo_match_id],
            |row| row.get(0),
        )
        .optional()?)
}

/// One game's number and MTGO game id.
struct GameKey {
    id: i64,
    number: i64,
    mtgo_game_id: Option<i64>,
}

/// One recorded game of a recording.
struct RecordedKey {
    position: u32,
    number: i64,
    mtgo_game_id: Option<i64>,
}

/// Which recorded game each game links to (game id, position). A game links
/// to the recorded game with its MTGO game id first. Only then may a game
/// link by number, to a position no other game claimed, and only when one of
/// the two has no MTGO game id (two different ids are two different games).
fn pick_recorded(games: &[GameKey], recorded: &[RecordedKey]) -> Vec<(i64, u32)> {
    let mut links: Vec<(i64, u32)> = Vec::new();
    for game in games {
        let Some(id) = game.mtgo_game_id else {
            continue;
        };
        if let Some(r) = recorded.iter().find(|r| r.mtgo_game_id == Some(id)) {
            links.push((game.id, r.position));
        }
    }
    for game in games {
        if links.iter().any(|(id, _)| *id == game.id) {
            continue;
        }
        let free = recorded.iter().find(|r| {
            r.number == game.number
                && (game.mtgo_game_id.is_none() || r.mtgo_game_id.is_none())
                && !links.iter().any(|(_, position)| *position == r.position)
        });
        if let Some(r) = free {
            links.push((game.id, r.position));
        }
    }
    links
}

/// A match's linked recording and which of its games link to which position.
struct MatchLinks {
    token: String,
    /// (game id, position in the recording)
    games: Vec<(i64, u32)>,
}

fn links_for_match(conn: &Connection, match_id: i64) -> Result<Option<MatchLinks>, StoreError> {
    let Some(token) = linked_token(conn, match_id)? else {
        return Ok(None);
    };
    let mut stmt = conn.prepare(
        "SELECT position, game_number, mtgo_game_id FROM recorded_replay_games
         WHERE token = ?1 ORDER BY position",
    )?;
    let recorded: Vec<RecordedKey> = stmt
        .query_map([&token], |row| {
            Ok(RecordedKey {
                position: row.get(0)?,
                number: row.get(1)?,
                mtgo_game_id: row.get(2)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    let mut stmt =
        conn.prepare("SELECT id, game_number, mtgo_game_id FROM games WHERE match_id = ?1")?;
    let games: Vec<GameKey> = stmt
        .query_map([match_id], |row| {
            Ok(GameKey {
                id: row.get(0)?,
                number: row.get(1)?,
                mtgo_game_id: row.get(2)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    let games = pick_recorded(&games, &recorded);
    Ok(Some(MatchLinks { token, games }))
}

/// The recording of a game, when its match (shown in lists) links to one and
/// the game links to one of its recorded games.
pub fn recorded_game(conn: &Connection, game_id: i64) -> Result<Option<RecordedGame>, StoreError> {
    let match_id: Option<i64> = conn
        .query_row(
            "SELECT g.match_id FROM games g JOIN listed_matches m ON m.id = g.match_id
             WHERE g.id = ?1",
            [game_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(match_id) = match_id else {
        return Ok(None);
    };
    let Some(links) = links_for_match(conn, match_id)? else {
        return Ok(None);
    };
    Ok(links
        .games
        .into_iter()
        .find(|(id, _)| *id == game_id)
        .map(|(_, position)| RecordedGame {
            token: links.token,
            position,
        }))
}

/// The ids of a match's games that link to a recorded game.
pub fn recorded_game_ids(conn: &Connection, match_id: i64) -> Result<HashSet<i64>, StoreError> {
    Ok(links_for_match(conn, match_id)?
        .map(|links| links.games.into_iter().map(|(id, _)| id).collect())
        .unwrap_or_default())
}

/// A recording's `.mtgr` text.
pub fn recording_text(conn: &Connection, token: &str) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT text FROM recorded_replays WHERE token = ?1",
            [token],
            |row| row.get(0),
        )
        .optional()?)
}

/// A card a recording or an Endstep replay names, with the images of its
/// row on that platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MtgrReplayCardRow {
    pub name: String,
    pub image: Option<String>,
    pub local_image: Option<String>,
    /// MTGO rows do not say which cards have a back face picture, so this is
    /// always false for a recording. An Endstep card is double-faced when its
    /// stored name joins two faces (`Front // Back`) and its type is not an
    /// Adventure or a Room (one picture for both halves).
    pub double_faced: bool,
}

/// Names asked per query, well under SQLite's variable limit.
const NAMES_PER_QUERY: usize = 500;

/// The card rows of `platform` (its catalog range) of the cards the text
/// names, one per name: the first printing with an image, else the first.
/// A token named with a ` Token` suffix ("Eldrazi Spawn Token") with no row
/// of that name takes the images of the token row named without it
/// ("Eldrazi Spawn", either platform), as an Arena replay's tokens do. Names
/// with no row are left out.
pub fn recording_cards(
    conn: &Connection,
    text: &str,
    platform: crate::Platform,
) -> Result<Vec<MtgrReplayCardRow>, StoreError> {
    let names: Vec<String> = quoted_strings(text).into_iter().collect();
    let (low, high) = catalog_range(platform);
    let mut chosen: std::collections::BTreeMap<String, MtgrReplayCardRow> = Default::default();
    for chunk in names.chunks(NAMES_PER_QUERY) {
        let marks = vec!["?"; chunk.len()].join(",");
        let sql = format!(
            "SELECT name, image, local_image, type FROM cards
             WHERE catalog_id >= ? AND catalog_id < ? AND name IN ({marks})
             ORDER BY (image IS NULL AND local_image IS NULL), catalog_id"
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut values: Vec<rusqlite::types::Value> = vec![low.into(), high.into()];
        values.extend(chunk.iter().map(|name| name.clone().into()));
        let rows = stmt.query_map(rusqlite::params_from_iter(values), |row| {
            let name: String = row.get(0)?;
            let card_type: Option<String> = row.get(3)?;
            let double_faced =
                platform == crate::Platform::Endstep && has_back_face(&name, card_type.as_deref());
            Ok(MtgrReplayCardRow {
                name,
                image: row.get(1)?,
                local_image: row.get(2)?,
                double_faced,
            })
        })?;
        for row in rows {
            let row = row?;
            chosen.entry(row.name.clone()).or_insert(row);
        }
    }
    for name in &names {
        if chosen.contains_key(name) {
            continue;
        }
        let Some(token_name) = name.strip_suffix(" Token") else {
            continue;
        };
        if let Some((image, local_image)) = token_art(conn, token_name)? {
            chosen.insert(
                name.clone(),
                MtgrReplayCardRow {
                    name: name.clone(),
                    image,
                    local_image,
                    double_faced: false,
                },
            );
        }
    }
    Ok(chosen.into_values().collect())
}

/// Whether a card named `name` with `card_type` has a back face picture: the
/// name joins two faces, and the type is not an Adventure or a Room.
fn has_back_face(name: &str, card_type: Option<&str>) -> bool {
    if !name.contains(" // ") {
        return false;
    }
    let one_picture =
        card_type.is_some_and(|text| text.contains("Adventure") || text.contains("Room"));
    !one_picture
}

/// How many recordings are stored.
pub fn recording_count(conn: &Connection) -> Result<i64, StoreError> {
    Ok(
        conn.query_row("SELECT COUNT(*) FROM recorded_replays", [], |row| {
            row.get(0)
        })?,
    )
}

#[cfg(test)]
mod tests {
    use mtgr::HeaderGame;

    use super::*;
    use crate::Platform;
    use crate::import::import_matches;
    use crate::test_support::{db, hide, imported, match_id};

    const GUID: &str = "6f1c2d3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f";

    fn header(
        token: &str,
        mtgo_match_id: Option<i64>,
        games: &[(u32, u32, Option<i64>)],
    ) -> RecordingHeader {
        RecordingHeader {
            match_token: token.to_string(),
            mtgo_match_id,
            recorder: Some("mtgr-record 1.0".to_string()),
            games: games
                .iter()
                .map(|(position, game_number, mtgo_game_id)| HeaderGame {
                    position: *position,
                    game_number: *game_number,
                    mtgo_game_id: *mtgo_game_id,
                })
                .collect(),
        }
    }

    fn store(
        conn: &mut Connection,
        header: &RecordingHeader,
        text: &str,
        hash: &str,
    ) -> ImportOutcome {
        let tx = conn.transaction().expect("tx");
        let outcome = store_recording(&tx, header, text, hash, 99).expect("store");
        tx.commit().expect("commit");
        outcome
    }

    fn game_id(conn: &Connection, token: &str, number: i64) -> i64 {
        conn.query_row(
            "SELECT g.id FROM games g JOIN matches m ON m.id = g.match_id
             WHERE m.token = ?1 AND g.game_number = ?2",
            params![token, number],
            |row| row.get(0),
        )
        .expect("game id")
    }

    #[test]
    fn recording_cards_match_mtgo_rows_by_name() {
        let (_dir, conn) = db("recorded-cards");
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, name, image, local_image) VALUES
               (10, 'Qbolt', NULL, NULL),
               (11, 'Qbolt', 'https://c.example/b.jpg', 'b.jpg'),
               (12, 'Qbolt', 'https://c.example/b2.jpg', NULL),
               (13, 'Qbear', 'https://c.example/bear.jpg', NULL),
               (100000020, 'Qbear', 'https://c.example/arena.jpg', 'a.jpg'),
               (100000021, 'Qarena only', NULL, 'x.jpg'),
               (14, 'Qunnamed in text', NULL, NULL);",
        )
        .expect("cards");
        let text = "[Game]\nplays \"Qbolt\" and \"Qbear\" then \"Qarena only\" \"Qmissing\"\n";
        let rows = recording_cards(&conn, text, Platform::Mtgo).expect("cards");
        let expected = vec![
            MtgrReplayCardRow {
                name: "Qbear".to_string(),
                image: Some("https://c.example/bear.jpg".to_string()),
                local_image: None,
                double_faced: false,
            },
            MtgrReplayCardRow {
                name: "Qbolt".to_string(),
                image: Some("https://c.example/b.jpg".to_string()),
                local_image: Some("b.jpg".to_string()),
                double_faced: false,
            },
        ];
        assert_eq!(rows, expected);
        assert!(
            recording_cards(&conn, "", Platform::Mtgo)
                .expect("none")
                .is_empty()
        );
    }

    #[test]
    fn endstep_cards_with_two_faces_are_double_faced() {
        let (_dir, conn) = db("recorded-cards-double");
        conn.execute(
            "INSERT INTO cards (catalog_id, name, type, image) VALUES
               (?1, 'Qfront // Qback', 'Creature // Creature', 'https://c.example/f.jpg'),
               (?2, 'Qone', 'Creature', 'https://c.example/o.jpg'),
               (?3, 'Qadv // Qspell', 'Creature // Instant - Adventure', 'https://c.example/a.jpg'),
               (40, 'Qfront // Qback', 'Creature // Creature', 'https://c.example/m.jpg')",
            params![
                cards::endstep_catalog_id(1),
                cards::endstep_catalog_id(2),
                cards::endstep_catalog_id(3)
            ],
        )
        .expect("cards");
        let text = "[Game]\n\"Qfront // Qback\" \"Qone\" \"Qadv // Qspell\"\n";
        let flags = |platform: Platform| -> Vec<(String, bool)> {
            recording_cards(&conn, text, platform)
                .expect("cards")
                .into_iter()
                .map(|card| (card.name, card.double_faced))
                .collect()
        };
        assert_eq!(
            flags(Platform::Endstep),
            vec![
                ("Qadv // Qspell".to_string(), false),
                ("Qfront // Qback".to_string(), true),
                ("Qone".to_string(), false),
            ]
        );
        assert_eq!(
            flags(Platform::Mtgo),
            vec![("Qfront // Qback".to_string(), false)]
        );
    }

    #[test]
    fn recording_cards_reads_only_its_platform() {
        let (_dir, conn) = db("recorded-cards-platform");
        conn.execute(
            "INSERT INTO cards (catalog_id, name, image) VALUES
               (30, 'Qshared', 'https://c.example/mtgo.jpg'),
               (?1, 'Qshared', 'https://c.example/arena.jpg'),
               (?2, 'Qshared', 'https://c.example/endstep.jpg')",
            params![cards::ARENA_ID_OFFSET + 5, cards::endstep_catalog_id(5)],
        )
        .expect("one name on every platform");
        let text = "[Game]\nplays \"Qshared\"\n";
        let image = |platform: Platform| {
            let rows = recording_cards(&conn, text, platform).expect("cards");
            assert_eq!(rows.len(), 1, "{platform:?}");
            rows[0].image.clone()
        };
        assert_eq!(
            image(Platform::Mtgo).as_deref(),
            Some("https://c.example/mtgo.jpg")
        );
        assert_eq!(
            image(Platform::Arena).as_deref(),
            Some("https://c.example/arena.jpg")
        );
        assert_eq!(
            image(Platform::Endstep).as_deref(),
            Some("https://c.example/endstep.jpg")
        );
    }

    #[test]
    fn recording_cards_read_the_whole_arena_range_and_no_more() {
        let (_dir, conn) = db("recorded-cards-arena-range");
        conn.execute(
            "INSERT INTO cards (catalog_id, name, image) VALUES
               (?1, 'Qlow', 'https://c.example/mtgo-top.jpg'),
               (?2, 'Qlow', 'https://c.example/arena-first.jpg'),
               (?3, 'Qhigh', 'https://c.example/arena-last.jpg'),
               (?4, 'Qhigh', 'https://c.example/endstep-first.jpg'),
               (?5, 'Qmtgo only', 'https://c.example/mtgo.jpg'),
               (?6, 'Qendstep only', 'https://c.example/endstep.jpg')",
            params![
                cards::ARENA_ID_OFFSET - 1,
                cards::ARENA_ID_OFFSET,
                cards::ENDSTEP_ID_OFFSET - 1,
                cards::ENDSTEP_ID_OFFSET,
                cards::ARENA_ID_OFFSET - 2,
                cards::ENDSTEP_ID_OFFSET + 1,
            ],
        )
        .expect("one row at each edge of the Arena range");
        let text = "[Game]\nplays \"Qlow\" \"Qhigh\" \"Qmtgo only\" \"Qendstep only\"\n";
        let rows = recording_cards(&conn, text, Platform::Arena).expect("cards");
        let images: Vec<(&str, Option<&str>)> = rows
            .iter()
            .map(|row| (row.name.as_str(), row.image.as_deref()))
            .collect();
        assert_eq!(
            images,
            vec![
                ("Qhigh", Some("https://c.example/arena-last.jpg")),
                ("Qlow", Some("https://c.example/arena-first.jpg")),
            ]
        );
    }

    #[test]
    fn recording_tokens_take_the_art_of_the_token_row_without_the_suffix() {
        let (_dir, conn) = db("recorded-token-art");
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, name, type, image, local_image) VALUES
               (20, 'Qspawn', 'Token Creature', 'https://c.example/spawn.jpg', 'spawn.jpg'),
               (21, 'Qcard', 'Creature', 'https://c.example/card.jpg', NULL),
               (22, 'Qnamed Token', 'Token Creature', 'https://c.example/named.jpg', NULL);",
        )
        .expect("cards");
        let text = "[Game]\nnew token #1 \"Qspawn Token\" \"Qcard Token\" \"Qnamed Token\"\n";
        let rows = recording_cards(&conn, text, Platform::Mtgo).expect("cards");
        let expected = vec![
            MtgrReplayCardRow {
                name: "Qnamed Token".to_string(),
                image: Some("https://c.example/named.jpg".to_string()),
                local_image: None,
                double_faced: false,
            },
            MtgrReplayCardRow {
                name: "Qspawn Token".to_string(),
                image: Some("https://c.example/spawn.jpg".to_string()),
                local_image: Some("spawn.jpg".to_string()),
                double_faced: false,
            },
        ];
        assert_eq!(rows, expected);
    }

    #[test]
    fn storing_the_same_hash_twice_is_unchanged() {
        let (_dir, mut conn) = db("recorded-same-hash");
        let h = header(GUID, None, &[(0, 1, None)]);
        assert_eq!(store(&mut conn, &h, "a", "h1"), ImportOutcome::Added);
        assert_eq!(store(&mut conn, &h, "a", "h1"), ImportOutcome::Unchanged);
        assert_eq!(recording_count(&conn).unwrap(), 1);
        assert_eq!(known_hashes(&conn).unwrap()[GUID], "h1");
    }

    #[test]
    fn a_new_hash_replaces_the_recording_and_its_games() {
        let (_dir, mut conn) = db("recorded-replace");
        store(
            &mut conn,
            &header(GUID, None, &[(0, 1, None), (1, 2, None)]),
            "old",
            "h1",
        );
        let outcome = store(
            &mut conn,
            &header(GUID, None, &[(0, 2, Some(5))]),
            "new",
            "h2",
        );
        assert_eq!(outcome, ImportOutcome::Replaced);
        assert_eq!(recording_count(&conn).unwrap(), 1);
        assert_eq!(recording_text(&conn, GUID).unwrap().as_deref(), Some("new"));
        let games: i64 = conn
            .query_row("SELECT COUNT(*) FROM recorded_replay_games", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(games, 1);
        assert_eq!(recording_text(&conn, "missing").unwrap(), None);
    }

    #[test]
    fn tokens_compare_lowercased() {
        let (_dir, mut conn) = db("recorded-lowercase");
        let upper = GUID.to_uppercase();
        import_matches(&mut conn, &[imported(&upper, None)], 1).expect("import");
        store(&mut conn, &header(GUID, None, &[(0, 1, None)]), "t", "h");
        let id = game_id(&conn, &upper, 1);
        let found = recorded_game(&conn, id).unwrap().expect("linked");
        assert_eq!(
            found,
            RecordedGame {
                token: GUID.to_string(),
                position: 0
            }
        );
    }

    #[test]
    fn a_recording_links_by_mtgo_match_id_when_the_token_differs() {
        let (_dir, mut conn) = db("recorded-match-id");
        import_matches(&mut conn, &[imported("random-0x-uuid", Some(4242))], 1).expect("import");
        store(
            &mut conn,
            &header(GUID, Some(4242), &[(0, 1, None)]),
            "t",
            "h",
        );
        let id = game_id(&conn, "random-0x-uuid", 1);
        assert_eq!(
            recorded_game(&conn, id).unwrap().expect("linked").token,
            GUID
        );
    }

    #[test]
    fn a_token_match_beats_a_match_id_match() {
        let (_dir, mut conn) = db("recorded-token-wins");
        let other = "11111111-2222-4333-8444-555555555555";
        import_matches(&mut conn, &[imported(GUID, Some(4242))], 1).expect("import");
        store(
            &mut conn,
            &header(other, Some(4242), &[(0, 1, None)]),
            "x",
            "hx",
        );
        store(
            &mut conn,
            &header(GUID, Some(4242), &[(0, 1, None)]),
            "t",
            "h",
        );
        let id = game_id(&conn, GUID, 1);
        assert_eq!(
            recorded_game(&conn, id).unwrap().expect("linked").token,
            GUID
        );
    }

    #[test]
    fn a_recording_links_to_a_match_that_arrives_later() {
        let (_dir, mut conn) = db("recorded-arrives-later");
        store(
            &mut conn,
            &header(GUID, Some(77), &[(0, 1, None)]),
            "t",
            "h",
        );
        import_matches(&mut conn, &[imported(GUID, Some(77))], 1).expect("import");
        let mid = match_id(&conn, GUID);
        let id = game_id(&conn, GUID, 1);
        assert!(recorded_game(&conn, id).unwrap().is_some());
        assert_eq!(recorded_game_ids(&conn, mid).unwrap(), HashSet::from([id]));
    }

    #[test]
    fn a_game_links_by_mtgo_game_id_before_game_number() {
        let (_dir, mut conn) = db("recorded-game-id");
        // The imported games are numbers 1, 2, 3 with Game IDs 7, 8, 9. The
        // recording's first position is Game ID 8 labelled number 1.
        import_matches(&mut conn, &[imported(GUID, None)], 1).expect("import");
        store(
            &mut conn,
            &header(GUID, None, &[(0, 1, Some(8)), (1, 3, None)]),
            "t",
            "h",
        );
        let by_id = recorded_game(&conn, game_id(&conn, GUID, 2))
            .unwrap()
            .expect("by id");
        assert_eq!(by_id.position, 0);
        let by_number = recorded_game(&conn, game_id(&conn, GUID, 3))
            .unwrap()
            .expect("by number");
        assert_eq!(by_number.position, 1);
        // Game 1 (Game ID 7) clashes with position 0 (Game ID 8): no link.
        assert!(
            recorded_game(&conn, game_id(&conn, GUID, 1))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_game_with_no_recorded_game_has_no_link() {
        let (_dir, mut conn) = db("recorded-partial");
        import_matches(&mut conn, &[imported(GUID, None)], 1).expect("import");
        store(&mut conn, &header(GUID, None, &[(0, 2, None)]), "t", "h");
        assert!(
            recorded_game(&conn, game_id(&conn, GUID, 1))
                .unwrap()
                .is_none()
        );
        assert!(
            recorded_game(&conn, game_id(&conn, GUID, 2))
                .unwrap()
                .is_some()
        );
        let ids = recorded_game_ids(&conn, match_id(&conn, GUID)).unwrap();
        assert_eq!(ids, HashSet::from([game_id(&conn, GUID, 2)]));
    }

    #[test]
    fn a_hidden_match_has_no_recorded_games() {
        let (_dir, mut conn) = db("recorded-hidden");
        import_matches(&mut conn, &[imported(GUID, None)], 1).expect("import");
        store(&mut conn, &header(GUID, None, &[(0, 1, None)]), "t", "h");
        let mid = match_id(&conn, GUID);
        let id = game_id(&conn, GUID, 1);
        hide(&conn, GUID);
        assert!(recorded_game(&conn, id).unwrap().is_none());
        assert!(recorded_game_ids(&conn, mid).unwrap().is_empty());
    }

    fn set_game_id(conn: &Connection, token: &str, number: i64, id: Option<i64>) {
        conn.execute(
            "UPDATE games SET mtgo_game_id = ?3 WHERE game_number = ?2
             AND match_id = (SELECT id FROM matches WHERE token = ?1)",
            params![token, number, id],
        )
        .expect("set game id");
    }

    #[test]
    fn clashing_game_ids_never_link_by_number() {
        let (_dir, mut conn) = db("recorded-clash");
        import_matches(&mut conn, &[imported(GUID, None)], 1).expect("import");
        store(
            &mut conn,
            &header(GUID, None, &[(0, 1, Some(99))]),
            "t",
            "h",
        );
        assert!(
            recorded_game(&conn, game_id(&conn, GUID, 1))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_position_claimed_by_id_is_not_reused_by_number() {
        let (_dir, mut conn) = db("recorded-claimed");
        import_matches(&mut conn, &[imported(GUID, None)], 1).expect("import");
        // Game 2 (id 8) claims position 0 by id; game 1 loses its id and has
        // the same number as position 0, which is taken.
        set_game_id(&conn, GUID, 1, None);
        store(&mut conn, &header(GUID, None, &[(0, 1, Some(8))]), "t", "h");
        assert_eq!(
            recorded_game(&conn, game_id(&conn, GUID, 2))
                .unwrap()
                .expect("by id")
                .position,
            0
        );
        assert!(
            recorded_game(&conn, game_id(&conn, GUID, 1))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_game_without_an_id_links_by_number() {
        let (_dir, mut conn) = db("recorded-no-id");
        import_matches(&mut conn, &[imported(GUID, None)], 1).expect("import");
        set_game_id(&conn, GUID, 1, None);
        store(
            &mut conn,
            &header(GUID, None, &[(0, 1, Some(99)), (1, 2, None)]),
            "t",
            "h",
        );
        let first = recorded_game(&conn, game_id(&conn, GUID, 1))
            .unwrap()
            .expect("no id");
        assert_eq!(first.position, 0);
        // Game 2 has id 8, the recorded game has none: links by number.
        let second = recorded_game(&conn, game_id(&conn, GUID, 2))
            .unwrap()
            .expect("rec no id");
        assert_eq!(second.position, 1);
    }

    #[test]
    fn a_recording_linked_by_token_does_not_also_link_by_match_id() {
        let (_dir, mut conn) = db("recorded-token-global");
        let other = "11111111-2222-4333-8444-555555555555";
        import_matches(
            &mut conn,
            &[imported(GUID, Some(1)), imported(other, Some(4242))],
            1,
        )
        .expect("import");
        store(
            &mut conn,
            &header(GUID, Some(4242), &[(0, 1, None)]),
            "t",
            "h",
        );
        assert!(
            recorded_game(&conn, game_id(&conn, GUID, 1))
                .unwrap()
                .is_some()
        );
        assert!(
            recorded_game(&conn, game_id(&conn, other, 1))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn the_newest_recording_wins_a_shared_match_id() {
        let (_dir, mut conn) = db("recorded-shared-id");
        import_matches(&mut conn, &[imported("random-0x-uuid", Some(4242))], 1).expect("import");
        for (token, at) in [("aaaa", 10), ("bbbb", 20)] {
            let tx = conn.transaction().expect("tx");
            store_recording(
                &tx,
                &header(token, Some(4242), &[(0, 1, None)]),
                "t",
                "h",
                at,
            )
            .expect("store");
            tx.commit().expect("commit");
        }
        let found = recorded_game(&conn, game_id(&conn, "random-0x-uuid", 1)).unwrap();
        assert_eq!(found.expect("linked").token, "bbbb");
    }
}
