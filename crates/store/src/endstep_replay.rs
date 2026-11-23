//! An Endstep game's replay, built on read: the match's stored capture
//! units (for a match pulled from another device, the units it came with)
//! go through `endstep_match::match_record`, and the `.mtgr` text
//! comes back with the requested game's index and the images of the cards
//! it names. Nothing is stored: a replay is derived each time it is opened.
//!
//! - Names, event, format and the rest of the tags come from the units.
//! - Archetypes: the match's archetype for each seat, when assigned.
//! - Card images: the Endstep card rows of the names the text holds
//!   (`recording_cards`). Endstep cards carry no printing in the record, so
//!   the viewer shows each name's image.

use endstep_match::{ReplayNames, match_record};
use rusqlite::{Connection, OptionalExtension};

use crate::endstep_sync::match_units;
use crate::recorded_replays::{MtgrReplayCardRow, recording_cards};
use crate::replay_shared::{seat_archetype, written_game_numbers};
use crate::{Platform, StoreError};

/// One Endstep game's replay: the whole match as `.mtgr` text, the index of
/// the requested game among the games written, and the cards it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndstepReplay {
    pub text: String,
    pub game_index: u32,
    pub cards: Vec<MtgrReplayCardRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndstepReplayRead {
    Ready(EndstepReplay),
    /// No such game, not an Endstep log or pulled match, no stored units,
    /// or the game wrote no record.
    NotFound,
}

/// The Endstep matches (`m`) with units to write: a log match (its stored
/// capture units), or a match pulled from another device with the source
/// it came with (`synced_sources`).
const REPLAYABLE: &str = "(m.source = 'log' OR (m.source = 'imported' AND EXISTS (
    SELECT 1 FROM synced_sources s WHERE s.match_id = m.id AND s.platform = 'endstep')))";

/// The replay of Endstep game row `game_id`.
pub fn endstep_replay(conn: &Connection, game_id: i64) -> Result<EndstepReplayRead, StoreError> {
    let found: Option<(i64, String, u32)> = conn
        .prepare_cached(&format!(
            "SELECT m.id, m.token, g.game_number FROM games g
                 JOIN matches m ON m.id = g.match_id
                 JOIN accounts a ON a.id = m.account_id AND a.platform = 'endstep'
                 WHERE g.id = ?1 AND {REPLAYABLE}"
        ))?
        .query_row([game_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .optional()?;
    let Some((match_id, token, game_number)) = found else {
        return Ok(EndstepReplayRead::NotFound);
    };
    let Some(written) = written_match(conn, match_id, &token)? else {
        return Ok(EndstepReplayRead::NotFound);
    };
    let Some(index) = written
        .games
        .iter()
        .position(|number| *number == game_number)
    else {
        return Ok(EndstepReplayRead::NotFound);
    };
    let cards = recording_cards(conn, &written.text, Platform::Endstep)?;
    Ok(EndstepReplayRead::Ready(EndstepReplay {
        text: written.text,
        game_index: index as u32,
        cards,
    }))
}

/// The Endstep log or pulled match `token` as `.mtgr` text; `None` when
/// there is no such match or it writes no record.
pub fn endstep_match_text(conn: &Connection, token: &str) -> Result<Option<String>, StoreError> {
    let match_id: Option<i64> = conn
        .prepare_cached(&format!(
            "SELECT m.id FROM matches m
                 JOIN accounts a ON a.id = m.account_id AND a.platform = 'endstep'
                 WHERE m.token = ?1 AND {REPLAYABLE}"
        ))?
        .query_row([token], |row| row.get(0))
        .optional()?;
    let Some(match_id) = match_id else {
        return Ok(None);
    };
    Ok(written_match(conn, match_id, token)?.map(|written| written.text))
}

/// The tokens of every Endstep log match, oldest first (the `endstep-mtgr`
/// dev command).
pub fn endstep_tokens(conn: &Connection) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT m.token FROM matches m
         JOIN accounts a ON a.id = m.account_id AND a.platform = 'endstep'
         WHERE m.source = 'log'
         ORDER BY m.started_at, m.id",
    )?;
    let rows = stmt.query_map([], |row| row.get(0))?;
    Ok(rows.collect::<Result<Vec<String>, _>>()?)
}

/// A match's written text and the game number of each game in it, in order.
struct WrittenMatch {
    text: String,
    games: Vec<u32>,
}

/// The Endstep match `match_id` (token `token`) written from its stored
/// units; `None` when it has none or they write no record.
fn written_match(
    conn: &Connection,
    match_id: i64,
    token: &str,
) -> Result<Option<WrittenMatch>, StoreError> {
    let Some((forge_game_id, units)) = match_units(conn, token)? else {
        return Ok(None);
    };
    let names = ReplayNames {
        local_archetype: seat_archetype(conn, match_id, false)?,
        opponent_archetype: seat_archetype(conn, match_id, true)?,
    };
    let Some(record) = match_record(&forge_game_id, &units, &names) else {
        return Ok(None);
    };
    Ok(Some(WrittenMatch {
        text: ::mtgr::write(&record),
        games: written_game_numbers(&record),
    }))
}

#[cfg(test)]
mod tests {
    use endstep_protocol::{FrameKind, decode_unit, unit_refs};
    use mtgo_match::GameResult;

    use super::*;
    use crate::endstep_captures::{NewCapture, store_captures};
    use crate::test_support::{TOKEN, db, seed_endstep, view};
    use crate::write::{MatchWrite, WriteBatch, commit};

    const UNIT_01: &str =
        include_str!("../../../fixtures/endstep/ranked-modern-bo3/01-session-a7a2056a.jsonl");
    const UNIT_02: &str =
        include_str!("../../../fixtures/endstep/ranked-modern-bo3/02-match-329f3d80.jsonl");
    const UNIT_03: &str =
        include_str!("../../../fixtures/endstep/ranked-modern-bo3/03-session-e3f486f1.jsonl");
    const UNIT_04: &str =
        include_str!("../../../fixtures/endstep/ranked-modern-bo3/04-match-946164b1.jsonl");
    const UNIT_05: &str =
        include_str!("../../../fixtures/endstep/ranked-modern-bo3/05-session-1a930f54.jsonl");
    const ALL: [&str; 5] = [UNIT_01, UNIT_02, UNIT_03, UNIT_04, UNIT_05];
    /// The fixture's Endstep match id (the stored token).
    const MATCH_ID: &str = "00000000-0000-4000-8000-e0000000003a";

    fn capture(text: &str) -> NewCapture {
        let parsed = endstep_capture::parse_capture(text).expect("a capture");
        NewCapture {
            header: parsed.header,
            frame_count: parsed.frame_count,
            text: text.to_string(),
            file_hash: "h".to_string(),
        }
    }

    /// Stores each unit, one at a time in the order given, with its refs
    /// (as the tracker's sync stores them).
    fn store_units(conn: &mut Connection, texts: &[&str]) {
        for text in texts {
            let tx = conn.transaction().expect("tx");
            store_captures(&tx, &[capture(text)], 1).expect("store");
            tx.commit().expect("commit");
            let unit = decode_unit(text).expect("the unit decodes");
            commit(
                conn,
                &WriteBatch {
                    endstep_refs: vec![(unit.unit_id.clone(), unit_refs(&unit))],
                    ..WriteBatch::default()
                },
            )
            .expect("refs");
        }
    }

    /// The fixture match's row (three games) and its units stored in
    /// `texts` order. Returns its game ids, game 1 first.
    fn fixture_match(conn: &mut Connection, texts: &[&str]) -> Vec<i64> {
        seed_endstep(conn, MATCH_ID);
        store_units(conn, texts);
        game_ids(conn, MATCH_ID)
    }

    fn game_ids(conn: &Connection, token: &str) -> Vec<i64> {
        let mut stmt = conn
            .prepare(
                "SELECT g.id FROM games g JOIN matches m ON m.id = g.match_id
                 WHERE m.token = ?1 ORDER BY g.game_number",
            )
            .expect("games");
        stmt.query_map([token], |row| row.get(0))
            .expect("rows")
            .map(|row| row.expect("id"))
            .collect()
    }

    fn ready(read: EndstepReplayRead) -> EndstepReplay {
        match read {
            EndstepReplayRead::Ready(replay) => replay,
            other => panic!("expected a replay, got {other:?}"),
        }
    }

    #[test]
    fn the_fixture_replay_reads_back() {
        let (_dir, mut conn) = db("endstep-replay-fixture");
        let games = fixture_match(&mut conn, &ALL);
        conn.execute(
            "INSERT INTO cards (catalog_id, name, image, local_image) VALUES
               (?1, 'Expedition Map', 'https://cards.example/map.jpg', 'map.jpg'),
               (12, 'Expedition Map', 'https://cards.example/mtgo-map.jpg', NULL)",
            [cards::endstep_catalog_id(1)],
        )
        .expect("one resolved Endstep card, and an MTGO row of its name");

        let replay = ready(endstep_replay(&conn, games[1]).expect("read"));
        assert!(replay.text.starts_with("[Mtgr \"0.3.0\"]"));
        assert_eq!(replay.game_index, 1);
        assert_eq!(replay.text.matches("\n[Game \"").count(), 3);
        assert!(replay.text.contains("[Site \"Endstep\"]"));
        let map = replay
            .cards
            .iter()
            .find(|card| card.name == "Expedition Map")
            .expect("the resolved card");
        assert_eq!(map.image.as_deref(), Some("https://cards.example/map.jpg"));
        assert_eq!(map.local_image.as_deref(), Some("map.jpg"));

        let first = ready(endstep_replay(&conn, games[0]).expect("read"));
        assert_eq!(first.game_index, 0);
        assert_eq!(first.text, replay.text);
        assert_eq!(
            endstep_match_text(&conn, MATCH_ID).expect("text"),
            Some(replay.text)
        );
        assert_eq!(
            endstep_tokens(&conn).expect("tokens"),
            vec![MATCH_ID.to_string()]
        );
    }

    #[test]
    fn the_replay_names_the_assigned_archetypes() {
        let (_dir, mut conn) = db("endstep-replay-archetypes");
        let games = fixture_match(&mut conn, &ALL);
        conn.execute_batch(&format!(
            "INSERT INTO archetypes (id, uuid, name, format, source, created_at, updated_at)
             VALUES (9001, 'q-a1', 'Qtron', 'CMODERN', 'manual', 0, 0),
                    (9002, 'q-a2', 'Qburn', 'CMODERN', 'manual', 0, 0);
             INSERT OR REPLACE INTO match_archetypes (match_id, archetype_id, is_opponent,
                 confidence)
             SELECT id, 9001, 0, 1 FROM matches WHERE token = '{MATCH_ID}';
             INSERT OR REPLACE INTO match_archetypes (match_id, archetype_id, is_opponent,
                 confidence)
             SELECT id, 9002, 1, 1 FROM matches WHERE token = '{MATCH_ID}';"
        ))
        .expect("archetypes");
        let replay = ready(endstep_replay(&conn, games[0]).expect("read"));
        // The user is seat 0 in the fixture.
        assert!(replay.text.contains("[Player1Archetype \"Qtron\"]"));
        assert!(replay.text.contains("[Player2Archetype \"Qburn\"]"));
    }

    /// Unit 04 without game 2's game socket frames: everything after game
    /// 1's `GAME_OUTCOME` up to and including game 2's (the fold's
    /// `unit_04_without_game_2`). One frame per non-empty line.
    fn unit_04_without_game_2() -> String {
        let unit = decode_unit(UNIT_04).expect("unit 04 decodes");
        let mut lines = UNIT_04.lines();
        let mut kept = vec![lines.next().expect("the header").to_string()];
        let body: Vec<&str> = lines.filter(|line| !line.trim().is_empty()).collect();
        assert_eq!(body.len(), unit.frames.len());
        let mut outcomes = 0;
        for (line, frame) in body.iter().zip(&unit.frames) {
            let is_game_frame = matches!(
                frame.kind,
                FrameKind::GameState(_) | FrameKind::GameDelta(_) | FrameKind::GameEvent(_)
            );
            let is_outcome =
                matches!(&frame.kind, FrameKind::GameEvent(event) if event.kind == "GAME_OUTCOME");
            let in_game_2 = outcomes == 1;
            if is_outcome {
                outcomes += 1;
            }
            if !(is_game_frame && in_game_2) {
                kept.push(line.to_string());
            }
        }
        kept.join("\n") + "\n"
    }

    #[test]
    fn a_game_without_frames_is_not_found() {
        let (_dir, mut conn) = db("endstep-replay-placeholder");
        let short_04 = unit_04_without_game_2();
        let games = fixture_match(&mut conn, &[UNIT_01, UNIT_02, UNIT_03, &short_04, UNIT_05]);
        assert_eq!(
            endstep_replay(&conn, games[1]).expect("read"),
            EndstepReplayRead::NotFound
        );
        let third = ready(endstep_replay(&conn, games[2]).expect("read"));
        assert_eq!(third.game_index, 1, "game 3 is the second game written");
        assert_eq!(third.text.matches("\n[Game \"").count(), 2);
    }

    #[test]
    fn an_mtgo_game_is_not_found() {
        let (_dir, mut conn) = db("endstep-replay-mtgo");
        fixture_match(&mut conn, &ALL);
        let mut mtgo = view(TOKEN, "Qfoe", 1_000);
        mtgo.games = vec![crate::test_support::game(1, Some(7), GameResult::Win)];
        commit(
            &mut conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(mtgo),
                    revision: 1,
                }],
                now_ms: 2_000,
                ..WriteBatch::default()
            },
        )
        .expect("an MTGO match");
        let mtgo_games = game_ids(&conn, TOKEN);
        assert_eq!(
            endstep_replay(&conn, mtgo_games[0]).expect("read"),
            EndstepReplayRead::NotFound
        );
        assert_eq!(
            endstep_replay(&conn, 999_999).expect("read"),
            EndstepReplayRead::NotFound
        );
        assert_eq!(endstep_match_text(&conn, TOKEN).expect("read"), None);
        assert_eq!(
            endstep_tokens(&conn).expect("tokens"),
            vec![MATCH_ID.to_string()]
        );
    }

    #[test]
    fn an_arena_game_is_not_found() {
        // The fixture match moved to the Arena account: its units are still
        // stored, so only the platform keeps it out.
        let (_dir, mut conn) = db("endstep-replay-arena");
        let games = fixture_match(&mut conn, &ALL);
        assert!(matches!(
            endstep_replay(&conn, games[0]).expect("read"),
            EndstepReplayRead::Ready(_)
        ));
        crate::test_support::move_match_to_arena(&conn, MATCH_ID);
        for game in games {
            assert_eq!(
                endstep_replay(&conn, game).expect("read"),
                EndstepReplayRead::NotFound
            );
        }
        assert_eq!(endstep_match_text(&conn, MATCH_ID).expect("read"), None);
        assert!(endstep_tokens(&conn).expect("tokens").is_empty());
    }

    #[test]
    fn match_units_reads_each_unit_once_in_fold_order() {
        let (_dir, mut conn) = db("endstep-replay-match-units");
        let mut reversed = ALL;
        reversed.reverse();
        fixture_match(&mut conn, &reversed);
        let (forge_game_id, units) = match_units(&conn, MATCH_ID)
            .expect("read")
            .expect("the match has units");
        let expected = crate::endstep_sync::units_of_game(&conn, &forge_game_id).expect("ids");
        // Units 01 to 04: unit 05 carries no ref.
        assert_eq!(expected.len(), 4);
        let ids: Vec<String> = units.iter().map(|unit| unit.unit_id.clone()).collect();
        assert_eq!(ids, expected);
        for unit in &units {
            let stored = crate::endstep_captures::decoded_unit(&conn, &unit.unit_id)
                .expect("read")
                .expect("decodes");
            assert_eq!(&stored, unit);
        }
    }

    #[test]
    fn the_replay_does_not_depend_on_import_order() {
        let (_dir, mut forward) = db("endstep-replay-order-forward");
        fixture_match(&mut forward, &ALL);
        let (_dir2, mut reverse) = db("endstep-replay-order-reverse");
        let mut reversed = ALL;
        reversed.reverse();
        fixture_match(&mut reverse, &reversed);
        let text = endstep_match_text(&forward, MATCH_ID).expect("forward");
        assert!(text.is_some());
        assert_eq!(
            endstep_match_text(&reverse, MATCH_ID).expect("reverse"),
            text
        );
    }
}
