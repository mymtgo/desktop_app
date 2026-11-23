//! The raw source of an Arena or Endstep match bundle (cross-device sync
//! spec section 5.4): what another device folds again for the match's game
//! rows, game log and replay. MTGO bundles carry frames instead.
//!
//! A match recorded here sends its stored facts: an Arena match its game
//! messages by game and its room state's event; an Endstep match its
//! capture units trimmed to it (`endstep_match::trim_units`). A match
//! pulled from another device (`imported`) sends the source it came with,
//! unchanged (`synced_sources`), so editing it changes only the player's
//! fields of the bundle.

use arena_game::Source;
use rusqlite::Connection;
use sync_bundles::{
    ArenaGameSource, ArenaSourceMessage, ArenaSources, BundlePlatform, EndstepSourceUnit,
    EndstepSources, MatchSources,
};

use crate::StoreError;
use crate::arena_game_sync::group_by_game;
use crate::arena_games::game_messages;
use crate::endstep_captures::capture_text;
use crate::endstep_sync::{forge_game_of_match, units_of_game};

/// The source of the match `match_id` (token `token`) on `platform`, for
/// its bundle. `None` for MTGO, for a match the player entered or imported
/// from history (it has no source), for a pulled match that came without
/// one, and for an Endstep match whose units are gone.
pub fn match_sources(
    conn: &Connection,
    match_id: i64,
    token: &str,
    platform: BundlePlatform,
) -> Result<Option<MatchSources>, StoreError> {
    if platform == BundlePlatform::Mtgo {
        return Ok(None);
    }
    let source: String = conn
        .prepare_cached("SELECT source FROM matches WHERE id = ?1")?
        .query_row([match_id], |row| row.get(0))?;
    match (source.as_str(), platform) {
        ("imported", _) => crate::synced_sources::read(conn, match_id),
        ("log", BundlePlatform::Arena) => {
            Ok(Some(MatchSources::Arena(arena_sources(conn, token)?)))
        }
        ("log", BundlePlatform::Endstep) => {
            Ok(endstep_sources(conn, token)?.map(MatchSources::Endstep))
        }
        _ => Ok(None),
    }
}

/// An Arena match's messages, game by game in log order (the messages
/// before the first game, which no game reads, are left out), and the
/// event its room state named. A match tracked before messages were
/// stored sends no games.
fn arena_sources(conn: &Connection, token: &str) -> Result<ArenaSources, StoreError> {
    let mut games = Vec::new();
    for (game_number, inputs) in group_by_game(game_messages(conn, token)?) {
        let messages = inputs
            .into_iter()
            .map(|input| ArenaSourceMessage {
                source: match input.source {
                    Source::Gre => "gre".to_string(),
                    Source::Client => "client".to_string(),
                },
                logged_at_ms: input.logged_at,
                raw: input.json,
            })
            .collect();
        games.push(ArenaGameSource {
            game_number,
            messages,
        });
    }
    Ok(ArenaSources {
        event_id: crate::arena_replay::room_event(conn, token)?,
        games,
    })
}

/// An Endstep match's units, in fold order, each trimmed to the match.
/// `None` when no stored unit has the match's body.
fn endstep_sources(conn: &Connection, token: &str) -> Result<Option<EndstepSources>, StoreError> {
    let Some(forge_game_id) = forge_game_of_match(conn, token)? else {
        return Ok(None);
    };
    let mut texts = Vec::new();
    for unit_id in units_of_game(conn, &forge_game_id)? {
        // A unit that does not read is left out, as the fold leaves it out.
        match capture_text(conn, &unit_id) {
            Ok(Some(text)) => texts.push(text),
            Ok(None) => {}
            Err(StoreError::Sqlite(error)) => return Err(StoreError::Sqlite(error)),
            Err(error) => {
                tracing::warn!(unit_id, %error, "an Endstep capture unit could not be read; not sent");
            }
        }
    }
    let units = endstep_match::trim_units(&forge_game_id, &texts)
        .into_iter()
        .map(|unit| EndstepSourceUnit {
            unit_id: unit.unit_id,
            lines: unit.lines,
        })
        .collect();
    Ok(Some(EndstepSources {
        forge_game_id,
        units,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena_games::{GameMessageSource, NewGameMessage};
    use crate::endstep_captures::{NewCapture, store_captures};
    use crate::endstep_sync::EndstepRanked;
    use crate::sync::match_bundle_input;
    use crate::test_support::{
        arena_account, arena_log_file, db, game, match_id, name_arena_opponent, seed_endstep, view,
    };
    use crate::write::{MatchWrite, WriteBatch, commit};
    use endstep_match::{EndstepMatch, Folded, fold_match};
    use endstep_protocol::{Unit, decode_unit, unit_refs};
    use mtgo_match::{GameResult, MatchView};
    use rusqlite::params;
    use serde_json::Value;
    use sync_bundles::match_bundle::match_bundle;
    use sync_bundles::{
        ArenaGameSource, ArenaSourceMessage, ArenaSources, BundlePlatform, EndstepSourceUnit,
        EndstepSources, MatchSources, canonical_json,
    };

    const ARENA: &str = "q-arena-sources";

    /// A complete Arena Bo3 log match, as the Arena projector writes it.
    fn arena_view(token: &str) -> MatchView {
        let mut played = view(token, "arena:Qfoe-id", 1_000);
        played.mtgo_match_id = None;
        played.local_username = "Qarena#12345".to_string();
        played.games = vec![
            game(1, None, GameResult::Win),
            game(2, None, GameResult::Win),
        ];
        played
    }

    fn message(file_id: i64, offset: u64, game_number: Option<u32>, json: &str) -> NewGameMessage {
        NewGameMessage {
            file_id,
            byte_offset: offset,
            seq: 0,
            match_token: ARENA.to_string(),
            game_number,
            source: if offset.is_multiple_of(2) {
                GameMessageSource::Gre
            } else {
                GameMessageSource::Client
            },
            logged_at: 10_000 + offset as i64,
            json: json.to_string(),
        }
    }

    /// The Arena log match `ARENA` with messages of games 1 and 2 (stored
    /// out of order, and one before the first `gameInfo`) and its room
    /// state's event. Returns its id.
    fn arena_log_match(conn: &mut Connection) -> i64 {
        arena_account(conn);
        commit(
            conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(arena_view(ARENA)),
                    revision: 1,
                }],
                now_ms: 2_000,
                ..WriteBatch::default()
            },
        )
        .expect("arena match");
        name_arena_opponent(conn, "arena:Qfoe-id", "Qfoe");
        let file_id = arena_log_file(conn);
        commit(
            conn,
            &WriteBatch {
                arena_game_messages: vec![
                    message(file_id, 4, Some(2), "{\"g\":2,\"n\":1}"),
                    message(file_id, 1, None, "{\"before\":true}"),
                    message(file_id, 2, Some(1), "{\"g\":1,\"n\":1}"),
                    message(file_id, 3, Some(1), "{\"g\":1,\"n\":2}"),
                ],
                ..WriteBatch::default()
            },
        )
        .expect("messages");
        conn.execute(
            "INSERT INTO arena_log_events (arena_log_file_id, byte_offset, seq, kind,
                 match_token, logged_at, raw, compressed)
             VALUES (?1, 0, 0, 'room_state', ?2, 1, ?3, 0)",
            params![
                file_id,
                ARENA,
                br#"{"players":[{"event_id":""},{"event_id":"Traditional_Ladder"}]}"#.to_vec()
            ],
        )
        .expect("room state");
        match_id(conn, ARENA)
    }

    fn arena_message(offset: i64, raw: &str) -> ArenaSourceMessage {
        ArenaSourceMessage {
            source: if offset % 2 == 0 { "gre" } else { "client" }.to_string(),
            logged_at_ms: 10_000 + offset,
            raw: raw.to_string(),
        }
    }

    #[test]
    fn arena_match_bundle_has_messages_per_game() {
        let (_dir, mut conn) = db("sync-sources-arena");
        let id = arena_log_match(&mut conn);
        let expected = MatchSources::Arena(ArenaSources {
            event_id: Some("Traditional_Ladder".to_string()),
            games: vec![
                ArenaGameSource {
                    game_number: 1,
                    messages: vec![
                        arena_message(2, "{\"g\":1,\"n\":1}"),
                        arena_message(3, "{\"g\":1,\"n\":2}"),
                    ],
                },
                ArenaGameSource {
                    game_number: 2,
                    messages: vec![arena_message(4, "{\"g\":2,\"n\":1}")],
                },
            ],
        });
        assert_eq!(
            match_sources(&conn, id, ARENA, BundlePlatform::Arena).expect("sources"),
            Some(expected.clone())
        );

        let input = match_bundle_input(&conn, id, chrono_tz::UTC)
            .expect("input")
            .expect("a match");
        assert_eq!(input.platform, BundlePlatform::Arena);
        assert_eq!(input.sources, Some(expected.clone()));
        assert_eq!(
            input.local_player_fields.arena_user_id.as_deref(),
            Some("Qarena-id")
        );
        assert_eq!(input.local_player_fields.display_name, None);
        assert_eq!(
            input.opponent_player_fields.display_name.as_deref(),
            Some("Qfoe")
        );
        assert_eq!(
            input.opponent_player_fields.arena_user_id.as_deref(),
            Some("Qfoe-id")
        );
        let bundle = match_bundle(&input, None);
        assert_eq!(bundle["platform"], Value::from("arena"));
        assert!(bundle.get("timelines").is_none());
        assert_eq!(
            sync_bundles::read_sources(&bundle, BundlePlatform::Arena),
            Ok(Some(expected))
        );
        assert!(bundle["match"].get("endstep_ranked").is_none());
    }

    #[test]
    fn an_archived_arena_match_sends_the_same_messages() {
        let (_dir, mut conn) = db("sync-sources-arena-archived");
        let id = arena_log_match(&mut conn);
        let live = match_sources(&conn, id, ARENA, BundlePlatform::Arena).expect("live");
        commit(
            &mut conn,
            &WriteBatch {
                arena_archive_tokens: vec![ARENA.to_string()],
                now_ms: 5_000,
                ..WriteBatch::default()
            },
        )
        .expect("archive");
        assert_eq!(
            match_sources(&conn, id, ARENA, BundlePlatform::Arena).expect("archived"),
            live
        );
    }

    #[test]
    fn edited_pulled_arena_match_keeps_sources_bytes() {
        let (_dir, mut conn) = db("sync-sources-pulled");
        let id = arena_log_match(&mut conn);
        conn.execute(
            "UPDATE matches SET source = 'imported', hidden_at = 7, edited_at = 7 WHERE id = ?1",
            [id],
        )
        .expect("a pulled, hidden match");
        let pulled = MatchSources::Arena(ArenaSources {
            event_id: Some("Play_Brawl_Historic".to_string()),
            games: vec![ArenaGameSource {
                game_number: 1,
                messages: vec![ArenaSourceMessage {
                    source: "gre".to_string(),
                    logged_at_ms: 1_700_000_000_123,
                    raw: "{\"pulled\":\"\\u00e9\"}".to_string(),
                }],
            }],
        });
        let tx = conn.transaction().expect("tx");
        crate::synced_sources::write(&tx, id, &pulled).expect("write");
        tx.commit().expect("commit");

        let input = match_bundle_input(&conn, id, chrono_tz::UTC)
            .expect("input")
            .expect("a match");
        assert_eq!(input.sources, Some(pulled), "not the local messages");
        let bundle = match_bundle(&input, None);
        let payload: Vec<u8> = conn
            .query_row(
                "SELECT payload FROM synced_sources WHERE match_id = ?1",
                [id],
                |row| row.get(0),
            )
            .expect("payload");
        let stored =
            String::from_utf8(zstd::decode_all(payload.as_slice()).expect("zstd")).expect("text");
        assert_eq!(canonical_json(&bundle["sources"]), stored);
    }

    #[test]
    fn a_pulled_match_without_sources_sends_none() {
        let (_dir, mut conn) = db("sync-sources-pulled-none");
        let id = arena_log_match(&mut conn);
        conn.execute("UPDATE matches SET source = 'imported' WHERE id = ?1", [id])
            .expect("pulled");
        assert_eq!(
            match_sources(&conn, id, ARENA, BundlePlatform::Arena).expect("sources"),
            None
        );
    }

    #[test]
    fn mtgo_and_manual_matches_have_no_sources() {
        let (_dir, mut conn) = db("sync-sources-mtgo");
        let id = arena_log_match(&mut conn);
        assert_eq!(
            match_sources(&conn, id, ARENA, BundlePlatform::Mtgo).expect("mtgo"),
            None
        );
        conn.execute("UPDATE matches SET source = 'manual' WHERE id = ?1", [id])
            .expect("manual");
        assert_eq!(
            match_sources(&conn, id, ARENA, BundlePlatform::Arena).expect("manual"),
            None
        );
    }

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
    /// The fixture's Endstep match id (the stored token) and Forge game.
    const MATCH_ID: &str = "00000000-0000-4000-8000-e0000000003a";
    const FORGE_GAME: &str = "00000000-0000-4000-8000-e0000000003e";
    const NOW_MS: i64 = 1_900_000_000_000;

    /// A second match in the same units: its match body and a game state
    /// with a delta after it.
    const OTHER_MATCH: [&str; 3] = [
        r#"{"at":1794310900001,"src":"rest","dir":"in","url":"/api/matches/q-other-match","body":{"id":"q-other-match","forgeGameId":"q-other-game","lobbyId":"q-other-lobby","myPlayerIndex":0,"participants":[{"userId":"00000000-0000-4000-8000-00000000000a","username":"Player"},{"userId":"00000000-0000-4000-8000-00000000000d","username":"Person9"}]}}"#,
        r#"{"at":1794310900002,"src":"game_ws","dir":"in","url":"wss://gw/q-other-game","body":{"type":"GAME_STATE","seq":990001,"payload":{"gameId":"q-other-game","turnNumber":1}}}"#,
        r#"{"at":1794310900003,"src":"game_ws","dir":"in","url":"wss://gw/q-other-game","body":{"type":"GAME_DELTA","seq":990002,"payload":{"patch":{"note":"q-other-game delta"}}}}"#,
    ];

    /// `text` with the other match's lines right after the header.
    fn with_other_match(text: &str) -> String {
        let mut lines: Vec<&str> = text.lines().collect();
        for (offset, line) in OTHER_MATCH.iter().enumerate() {
            lines.insert(1 + offset, line);
        }
        lines.join("\n") + "\n"
    }

    /// Stores each unit with its refs, as the tracker's sync stores them.
    fn store_units(conn: &mut Connection, texts: &[String]) {
        for text in texts {
            let parsed = endstep_capture::parse_capture(text).expect("a capture");
            let capture = NewCapture {
                header: parsed.header,
                frame_count: parsed.frame_count,
                text: text.clone(),
                file_hash: "h".to_string(),
            };
            let tx = conn.transaction().expect("tx");
            store_captures(&tx, &[capture], 1).expect("store");
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

    /// The fixture with a second match in sessions unit 01 and match unit
    /// 04, stored under the seeded Endstep match. Returns the units' texts.
    fn shared_endstep_match(conn: &mut Connection) -> Vec<String> {
        seed_endstep(conn, MATCH_ID);
        let texts = vec![
            with_other_match(UNIT_01),
            UNIT_02.to_string(),
            UNIT_03.to_string(),
            with_other_match(UNIT_04),
            UNIT_05.to_string(),
        ];
        store_units(conn, &texts);
        texts
    }

    fn folded(units: &[Unit]) -> EndstepMatch {
        match fold_match(FORGE_GAME, units, NOW_MS) {
            Folded::Match(found) => *found,
            Folded::Dropped(reason) => panic!("dropped: {reason:?}"),
        }
    }

    fn endstep_sources(conn: &Connection) -> EndstepSources {
        let id = match_id(conn, MATCH_ID);
        match match_sources(conn, id, MATCH_ID, BundlePlatform::Endstep).expect("sources") {
            Some(MatchSources::Endstep(sources)) => sources,
            other => panic!("expected Endstep sources, got {other:?}"),
        }
    }

    fn source_units(sources: &EndstepSources) -> Vec<Unit> {
        sources
            .units
            .iter()
            .map(|unit: &EndstepSourceUnit| {
                decode_unit(&unit.lines.join("\n")).expect("a sent unit decodes")
            })
            .collect()
    }

    #[test]
    fn endstep_sources_hold_only_this_match() {
        let (_dir, mut conn) = db("sync-sources-endstep");
        let texts = shared_endstep_match(&mut conn);
        let sources = endstep_sources(&conn);
        assert_eq!(sources.forge_game_id, FORGE_GAME);
        assert_eq!(sources.units.len(), 4, "unit 05 holds nothing of the match");
        for unit in &sources.units {
            for line in &unit.lines {
                assert!(
                    !line.contains("q-other"),
                    "kept in {}: {line}",
                    unit.unit_id
                );
            }
        }
        // Folding what is sent gives the match the whole units give.
        let whole: Vec<Unit> = texts
            .iter()
            .map(|text| decode_unit(text).expect("decodes"))
            .collect();
        assert_eq!(folded(&source_units(&sources)), folded(&whole));
    }

    /// The shared fixture with its game rows and ranked result written as
    /// the Endstep sync writes them. Returns the match id.
    fn synced_endstep_match(conn: &mut Connection) -> i64 {
        let texts = shared_endstep_match(conn);
        let units: Vec<Unit> = texts
            .iter()
            .map(|text| decode_unit(text).expect("decodes"))
            .collect();
        let found = folded(&units);
        let ranked = found.ranked.clone().expect("the fixture is ranked");
        commit(
            conn,
            &WriteBatch {
                endstep_ranked: vec![EndstepRanked::from_result(MATCH_ID, &ranked)],
                endstep_game_cards: vec![(MATCH_ID.to_string(), found.game_cards)],
                now_ms: 3_000,
                ..WriteBatch::default()
            },
        )
        .expect("game rows");
        // The deck version the match was played with lists the game 1
        // deck's Endstep cards, as the deck sync stores them.
        let id = match_id(conn, MATCH_ID);
        let mut stmt = conn
            .prepare(
                "SELECT c.catalog_id, c.main_qty, c.side_qty FROM game_cards c
                 JOIN games g ON g.id = c.game_id
                 WHERE g.match_id = ?1 AND g.game_number = 1 AND c.side = 'local'",
            )
            .expect("rows");
        let mut cards = Vec::new();
        let rows = stmt
            .query_map([id], |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, u32>(2)?,
                ))
            })
            .expect("query");
        for row in rows {
            let (catalog_id, main, side) = row.expect("row");
            if main > 0 {
                cards.push(decks::DeckCard {
                    catalog_id,
                    quantity: main,
                    sideboard: false,
                });
            }
            if side > 0 {
                cards.push(decks::DeckCard {
                    catalog_id,
                    quantity: side,
                    sideboard: true,
                });
            }
        }
        drop(stmt);
        conn.execute(
            "UPDATE deck_versions SET signature = ?2
             WHERE id = (SELECT deck_version_id FROM matches WHERE id = ?1)",
            params![id, decks::signature(&cards)],
        )
        .expect("signature");
        id
    }

    /// Every number anywhere in `value`.
    fn numbers(value: &Value, found: &mut Vec<u64>) {
        match value {
            Value::Number(number) => found.extend(number.as_u64()),
            Value::Array(items) => items.iter().for_each(|item| numbers(item, found)),
            Value::Object(fields) => fields.values().for_each(|item| numbers(item, found)),
            _ => {}
        }
    }

    #[test]
    fn endstep_bundle_names_cards_portably() {
        let (_dir, mut conn) = db("sync-sources-endstep-cards");
        let id = synced_endstep_match(&mut conn);
        let endstep_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM game_cards c JOIN games g ON g.id = c.game_id
                 WHERE g.match_id = ?1 AND c.catalog_id >= ?2",
                params![id, cards::ENDSTEP_ID_OFFSET],
                |row| row.get(0),
            )
            .expect("rows");
        assert!(endstep_rows > 0, "the match has Endstep card rows");

        let input = match_bundle_input(&conn, id, chrono_tz::UTC)
            .expect("input")
            .expect("a match");
        assert_eq!(input.platform, BundlePlatform::Endstep);
        let bundle = match_bundle(&input, None);
        assert_eq!(bundle["platform"], Value::from("endstep"));
        let mut found = Vec::new();
        numbers(&bundle, &mut found);
        let offset = u64::from(cards::ENDSTEP_ID_OFFSET);
        assert!(
            found.iter().all(|number| *number < offset),
            "an Endstep catalog id in the bundle"
        );
        // Each deck card is named by its printing.
        let local = bundle["players"]
            .as_array()
            .expect("players")
            .iter()
            .find(|seat| seat["is_local"] == Value::Bool(true))
            .expect("the player");
        let first = &local["deck_json"][0]["mtgo_id"];
        assert!(
            sync_bundles::EndstepCardRef::from_value(first).is_some(),
            "{first}"
        );
        // The deck version's signature names the same cards by ref, with
        // no local Endstep id in it.
        let signature = bundle["match"]["deck_version_signature"]
            .as_str()
            .expect("a signature");
        for digits in signature.split(|c: char| !c.is_ascii_digit()) {
            if let Ok(number) = digits.parse::<u64>() {
                assert!(number < offset, "a local id in the signature: {signature}");
            }
        }
        let portable =
            sync_bundles::endstep_cards::read_endstep_signature(signature).expect("portable");
        let local_signature: String = conn
            .query_row(
                "SELECT v.signature FROM matches m JOIN deck_versions v ON v.id = m.deck_version_id
                 WHERE m.id = ?1",
                [id],
                |row| row.get(0),
            )
            .expect("local signature");
        let local_cards = decks::decode_signature(&local_signature);
        assert_eq!(portable.len(), local_cards.len());
        assert!(!portable.is_empty());
        let ids: Vec<u32> = local_cards.iter().map(|card| card.catalog_id).collect();
        let refs = crate::endstep_cards::refs_of(&conn, &ids).expect("refs");
        for card in &local_cards {
            let expected = sync_bundles::endstep_cards::EndstepDeckCard {
                card: refs[&card.catalog_id].clone(),
                quantity: card.quantity,
                sideboard: card.sideboard,
            };
            assert!(portable.contains(&expected), "{expected:?}");
        }
        // The ranked result travels in the match section.
        let ranked =
            sync_bundles::match_bundle::match_endstep_ranked(&bundle).expect("the ranked result");
        let stored = crate::endstep_sync::endstep_rating(&conn, id)
            .expect("rating")
            .expect("stored");
        assert_eq!(ranked.rating_before, stored.rating_before);
        assert_eq!(ranked.rank, stored.rank);
        assert_eq!(ranked.status, stored.status);
    }

    // ---- Pulled on another device (cross-device sync spec section 7) ----

    /// Device B, holding the deck the match names (pulled with the decks).
    fn pull_db(label: &str) -> (fixture_tools::TestDir, Connection) {
        let (dir, conn) = db(label);
        conn.execute(
            "INSERT INTO decks (mtgo_id, name, format, created_at, updated_at, source)
             VALUES ('endstep:d1', 'Qendstep deck', 'CMODERN', 1, 1, 'file')",
            [],
        )
        .expect("deck");
        (dir, conn)
    }

    #[test]
    fn pulled_endstep_match_equals_local_fold() {
        let (_a, mut here) = db("sync-pull-endstep-here");
        synced_endstep_match(&mut here);
        let local = crate::test_support::pulled_rows(&here, MATCH_ID);
        assert!(local.iter().any(|row| row.starts_with("card ")));
        let (_b, mut there) = pull_db("sync-pull-endstep-there");
        crate::test_support::pull_into(
            &mut there,
            crate::test_support::pulled_from(&here, MATCH_ID),
            NOW_MS,
        );
        assert_eq!(crate::test_support::pulled_rows(&there, MATCH_ID), local);

        let (source, account, opponent): (String, String, String) = there
            .query_row(
                "SELECT m.source, a.platform, o.platform FROM matches m
                 JOIN accounts a ON a.id = m.account_id
                 JOIN opponents o ON o.id = m.opponent_id WHERE m.token = ?1",
                [MATCH_ID],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("match");
        assert_eq!(
            (source.as_str(), account.as_str(), opponent.as_str()),
            ("imported", "endstep", "endstep")
        );
        assert_eq!(
            crate::test_support::number(
                &there,
                "SELECT COUNT(*) FROM accounts a JOIN opponents o
                 WHERE a.display_name IS NOT NULL AND o.display_name IS NOT NULL"
            ),
            1,
            "the seats are named from the fold"
        );
        // Pulled sources never enter the local fact tables.
        for table in ["endstep_captures", "endstep_unit_refs"] {
            assert_eq!(
                crate::test_support::number(&there, &format!("SELECT COUNT(*) FROM {table}")),
                0
            );
        }
        assert_eq!(
            crate::test_support::number(&there, "SELECT COUNT(*) FROM synced_sources"),
            1
        );
        // The units read back through the seam the readers use.
        for game_number in 1..=3 {
            assert_eq!(
                crate::endstep_sync::game_log_lines(&there, MATCH_ID, game_number).expect("there"),
                crate::endstep_sync::game_log_lines(&here, MATCH_ID, game_number).expect("here"),
            );
        }
    }

    #[test]
    fn pulled_endstep_match_keeps_ranked_result() {
        let (_a, mut here) = db("sync-pull-endstep-ranked-here");
        let id = synced_endstep_match(&mut here);
        let (_b, mut there) = pull_db("sync-pull-endstep-ranked-there");
        crate::test_support::pull_into(
            &mut there,
            crate::test_support::pulled_from(&here, MATCH_ID),
            NOW_MS,
        );
        let stored = crate::endstep_sync::endstep_rating(&here, id).expect("here");
        assert!(stored.is_some());
        assert_eq!(
            crate::endstep_sync::endstep_rating(&there, match_id(&there, MATCH_ID)).expect("there"),
            stored
        );
    }

    /// Device A (`here`) with the synced fixture match, and device B
    /// (`there`) that pulled it. The connections close before their folders
    /// go.
    struct Pulled {
        here: Connection,
        there: Connection,
        _here_dir: fixture_tools::TestDir,
        _there_dir: fixture_tools::TestDir,
    }

    /// Device A's synced fixture match pulled into device B.
    fn pulled_endstep(label: &str) -> Pulled {
        let (there_dir, mut there) = pull_db(label);
        let (here_dir, mut here) = db(&format!("{label}-here"));
        synced_endstep_match(&mut here);
        crate::test_support::pull_into(
            &mut there,
            crate::test_support::pulled_from(&here, MATCH_ID),
            NOW_MS,
        );
        Pulled {
            here,
            there,
            _here_dir: here_dir,
            _there_dir: there_dir,
        }
    }

    /// The game row ids of the match `MATCH_ID`, game 1 first.
    fn game_ids(conn: &Connection) -> Vec<i64> {
        let mut stmt = conn
            .prepare(
                "SELECT g.id FROM games g JOIN matches m ON m.id = g.match_id
                 WHERE m.token = ?1 ORDER BY g.game_number",
            )
            .expect("games");
        stmt.query_map([MATCH_ID], |row| row.get(0))
            .expect("rows")
            .map(|row| row.expect("id"))
            .collect()
    }

    #[test]
    fn pulled_endstep_match_replay_equals_recorded() {
        let Pulled { here, there, .. } = pulled_endstep("sync-pull-endstep-replay");
        let recorded = crate::endstep_replay::endstep_match_text(&here, MATCH_ID)
            .expect("here")
            .expect("a record");
        assert_eq!(
            crate::endstep_replay::endstep_match_text(&there, MATCH_ID).expect("there"),
            Some(recorded)
        );
        let recorded_games = game_ids(&here);
        let pulled_games = game_ids(&there);
        assert_eq!(pulled_games.len(), 3);
        for (recorded, pulled) in recorded_games.iter().zip(&pulled_games) {
            let recorded = crate::endstep_replay::endstep_replay(&here, *recorded).expect("here");
            let pulled = crate::endstep_replay::endstep_replay(&there, *pulled).expect("there");
            match (recorded, pulled) {
                (
                    crate::endstep_replay::EndstepReplayRead::Ready(recorded),
                    crate::endstep_replay::EndstepReplayRead::Ready(pulled),
                ) => {
                    assert_eq!(pulled.text, recorded.text);
                    assert_eq!(pulled.game_index, recorded.game_index);
                }
                other => panic!("expected two replays, got {other:?}"),
            }
        }
    }

    #[test]
    fn pulled_endstep_game_log_equals_recorded() {
        let Pulled { here, there, .. } = pulled_endstep("sync-pull-endstep-log");
        for game_number in 1..=3 {
            let recorded =
                crate::endstep_sync::game_log_lines(&here, MATCH_ID, game_number).expect("here");
            assert!(!recorded.is_empty(), "game {game_number}");
            assert_eq!(
                crate::endstep_sync::game_log_lines(&there, MATCH_ID, game_number).expect("there"),
                recorded,
                "game {game_number}"
            );
        }
    }

    /// A raise of `ENDSTEP_SYNC_VERSION` (the resync batch the tracker sends
    /// at start) derives a pulled match again from the source it came
    /// with; its sync row (hash, touch key) is not changed. Twice is the
    /// same as once.
    #[test]
    fn endstep_version_raise_rederives_pulled_matches() {
        let Pulled { mut there, .. } = pulled_endstep("sync-pull-endstep-raise");
        let id = match_id(&there, MATCH_ID);
        let rows = crate::test_support::pulled_rows(&there, MATCH_ID);
        assert!(rows.iter().any(|row| row.starts_with("card ")));
        let synced = crate::test_support::dump(&there, "synced_rows");
        let key = crate::sync::match_touch_key(&there, id).expect("key");

        // Derived under older rules: the rows differ from today's.
        there
            .execute_batch(
                "DELETE FROM game_cards; DELETE FROM game_mulligan_cards;
                 UPDATE derivations SET version = 2 WHERE name = 'endstep_sync';",
            )
            .expect("an older derive");
        let resync = WriteBatch {
            endstep_resync: true,
            now_ms: NOW_MS + 1,
            ..WriteBatch::default()
        };
        commit(&mut there, &resync).expect("resync");
        assert_eq!(crate::test_support::pulled_rows(&there, MATCH_ID), rows);
        commit(&mut there, &resync).expect("again");
        assert_eq!(crate::test_support::pulled_rows(&there, MATCH_ID), rows);

        assert_eq!(
            crate::endstep_sync::sync_version(&there).expect("version"),
            Some(crate::endstep_sync::ENDSTEP_SYNC_VERSION)
        );
        assert_eq!(crate::test_support::dump(&there, "synced_rows"), synced);
        assert_eq!(
            crate::sync::match_touch_key(&there, id).expect("key"),
            key,
            "no push"
        );
    }

    /// A local capture unit, stored unsynced, as the tracker's sync takes
    /// it after a resync.
    fn local_unit(conn: &mut Connection) -> String {
        let parsed = endstep_capture::parse_capture(UNIT_05).expect("a capture");
        let unit_id = decode_unit(UNIT_05).expect("decodes").unit_id;
        let tx = conn.transaction().expect("tx");
        store_captures(
            &tx,
            &[NewCapture {
                header: parsed.header,
                frame_count: parsed.frame_count,
                text: UNIT_05.to_string(),
                file_hash: "h".to_string(),
            }],
            1,
        )
        .expect("store");
        tx.commit().expect("commit");
        unit_id
    }

    /// The resync batch at start with a local unit's synced mark in it:
    /// whatever a pulled match does, the batch commits, the version is
    /// stored and the unit is marked.
    fn assert_resync_runs(there: &mut Connection, unit_id: &str) {
        commit(
            there,
            &WriteBatch {
                endstep_resync: true,
                endstep_synced: vec![(unit_id.to_string(), "h".to_string())],
                now_ms: NOW_MS + 1,
                ..WriteBatch::default()
            },
        )
        .expect("the resync commits");
        assert_eq!(
            crate::endstep_sync::sync_version(there).expect("version"),
            Some(crate::endstep_sync::ENDSTEP_SYNC_VERSION)
        );
        let synced: i64 = there
            .query_row(
                "SELECT synced_version FROM endstep_captures WHERE unit_id = ?1",
                [unit_id],
                |row| row.get(0),
            )
            .expect("the unit");
        assert_eq!(synced, crate::endstep_sync::ENDSTEP_SYNC_VERSION);
    }

    #[test]
    fn a_corrupt_pulled_endstep_source_leaves_the_resync_to_run() {
        let Pulled { mut there, .. } = pulled_endstep("sync-pull-endstep-corrupt");
        let unit_id = local_unit(&mut there);
        there
            .execute("UPDATE synced_sources SET payload = x'00'", [])
            .expect("a corrupt source");
        assert_resync_runs(&mut there, &unit_id);
    }

    /// A pulled match whose derive fails (its card rows are past the
    /// Endstep id range) is rolled back and skipped; the resync still runs.
    #[test]
    fn a_pulled_endstep_match_that_fails_to_derive_leaves_the_resync_to_run() {
        let Pulled { mut there, .. } = pulled_endstep("sync-pull-endstep-derive-fails");
        let unit_id = local_unit(&mut there);
        let rows = crate::test_support::pulled_rows(&there, MATCH_ID);
        there
            .execute(
                "UPDATE endstep_cards SET id = id + ?1",
                [i64::from(cards::ENDSTEP_ID_OFFSET)],
            )
            .expect("rows past the range");
        assert_resync_runs(&mut there, &unit_id);
        let after = crate::test_support::pulled_rows(&there, MATCH_ID);
        assert_eq!(
            after.iter().filter(|row| row.starts_with("game ")).count(),
            rows.iter().filter(|row| row.starts_with("game ")).count(),
            "the match keeps its rows"
        );
        assert_eq!(
            crate::test_support::number(&there, "SELECT COUNT(*) FROM game_cards"),
            rows.iter().filter(|row| row.starts_with("card ")).count() as i64,
            "nothing half written"
        );
    }

    /// The cards of the match's deck version, each named by its ref.
    fn version_cards(conn: &Connection) -> Vec<(sync_bundles::EndstepCardRef, u32, bool)> {
        let signature: String = conn
            .query_row(
                "SELECT v.signature FROM matches m JOIN deck_versions v ON v.id = m.deck_version_id
                 WHERE m.token = ?1",
                [MATCH_ID],
                |row| row.get(0),
            )
            .expect("a linked version");
        let cards = decks::decode_signature(&signature);
        let ids: Vec<u32> = cards.iter().map(|card| card.catalog_id).collect();
        let refs = crate::endstep_cards::refs_of(conn, &ids).expect("refs");
        let mut named: Vec<(sync_bundles::EndstepCardRef, u32, bool)> = cards
            .iter()
            .map(|card| {
                (
                    refs[&card.catalog_id].clone(),
                    card.quantity,
                    card.sideboard,
                )
            })
            .collect();
        named.sort_by(|a, b| {
            (&a.0.name, &a.0.set, &a.0.number, a.2).cmp(&(&b.0.name, &b.0.set, &b.0.number, b.2))
        });
        named
    }

    #[test]
    fn a_pulled_endstep_matchs_deck_version_names_the_right_cards() {
        let (_a, mut here) = db("sync-pull-endstep-deck-here");
        synced_endstep_match(&mut here);
        let (_b, mut there) = pull_db("sync-pull-endstep-deck-there");
        // An unrelated card first, so this device's ids differ from A's.
        let tx = there.transaction().expect("tx");
        crate::endstep_cards::catalog_for_ref(
            &tx,
            &sync_bundles::EndstepCardRef {
                name: "Qother card".to_string(),
                set: "QQQ".to_string(),
                number: "1".to_string(),
            },
            1,
        )
        .expect("a card");
        tx.commit().expect("commit");
        crate::test_support::pull_into(
            &mut there,
            crate::test_support::pulled_from(&here, MATCH_ID),
            NOW_MS,
        );
        let cards = version_cards(&there);
        assert!(!cards.is_empty());
        assert_eq!(cards, version_cards(&here));
    }

    #[test]
    fn a_pulled_endstep_signature_that_does_not_read_links_no_version() {
        let (_a, mut here) = db("sync-pull-endstep-junk-here");
        synced_endstep_match(&mut here);
        let (_b, mut there) = pull_db("sync-pull-endstep-junk-there");
        let mut pulled = crate::test_support::pulled_from(&here, MATCH_ID);
        let deck = pulled.deck.clone().expect("a deck").0;
        pulled.deck = Some((deck, "endstep:not a card list".to_string()));
        crate::test_support::pull_into(&mut there, pulled, NOW_MS);
        assert_eq!(
            crate::test_support::number(
                &there,
                "SELECT COUNT(*) FROM matches WHERE deck_version_id IS NULL"
            ),
            1
        );
        assert_eq!(
            crate::test_support::number(&there, "SELECT COUNT(*) FROM deck_versions"),
            0,
            "the junk signature is never stored"
        );
    }
}
