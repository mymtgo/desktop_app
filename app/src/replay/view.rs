//! The replay window's data for one game (replays spec section 7; 0.x
//! `Games\ShowController`): the game's frames decorated for the app (card
//! names, types and images, the local image file for the `card` scheme,
//! `IsLocal` on every player), its log, the match's games for the picker,
//! the sideboard as the game began and the nearest earlier one, the
//! archetype names and the share state.

use std::collections::BTreeMap;

use ::replay::{CardInfo, ImageMode, ReplayFrame, decorate_frames};
use cards::tokens::TokenCard;
use chrono_tz::Tz;
use game_detail::GameLogEntry;
use serde::Serialize;
use serde_json::Value;
use store::arena_replay::{ArenaReplay, ArenaReplayRead};
use store::endstep_replay::EndstepReplayRead;
use store::recorded_replays::MtgrReplayCardRow;
use store::replay::{
    GameShare, ReplayGameInput, card_infos, game_share, previous_sideboard, replay_inputs,
};
use store::{Connection, Platform};

use crate::replay::share::ReplayShareState;

/// One frame: its `HH:MM:SS` time and its decorated content (the viewer's
/// `ReplayFrameContent`, typed in the UI).
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct ReplayFrameView {
    pub timestamp: String,
    #[specta(type = specta_typescript::Unknown)]
    pub content: Value,
}

/// One game log line, `HH:MM:SS` local time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct ReplayLogLine {
    pub timestamp: String,
    pub message: String,
}

/// One game of the match for the picker (0.x `matchGames`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct ReplayMatchGame {
    /// The `games.id`.
    pub id: u32,
    pub number: u32,
    /// From your side; `null` when unknown.
    pub won: Option<bool>,
}

/// One card of your sideboard as a game began (the viewer's
/// `ReplaySideboardEntry`), plus the stored image file for the `card`
/// scheme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct ReplaySideboardEntry {
    pub catalog_id: u32,
    pub quantity: u32,
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub type_line: Option<String>,
    pub image: Option<String>,
    pub local_image: Option<String>,
}

/// The nearest earlier game's sideboard (0.x `previousSideboard`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct ReplayPreviousSideboard {
    /// That game's number.
    pub game: u32,
    pub sideboard: Vec<ReplaySideboardEntry>,
}

/// The decks' archetype names; `null` when unknown.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, specta::Type)]
pub struct ReplayArchetypes {
    pub local: Option<String>,
    pub opponent: Option<String>,
}

/// The replay window's data for one game (spec section 7).
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ReplayView {
    pub frames: Vec<ReplayFrameView>,
    pub log: Vec<ReplayLogLine>,
    pub won: Option<bool>,
    pub game_id: u32,
    pub match_token: String,
    /// Every game of the match by game number.
    pub match_games: Vec<ReplayMatchGame>,
    /// `null` when neither the recorded deck nor a frame has it.
    pub sideboard: Option<Vec<ReplaySideboardEntry>>,
    pub previous_sideboard: Option<ReplayPreviousSideboard>,
    pub archetypes: ReplayArchetypes,
    pub share: ReplayShareState,
}

/// One card an Arena replay names, with the images of its row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MtgrReplayCard {
    pub name: String,
    /// The front's images.
    pub image: Option<String>,
    pub local_image: Option<String>,
    /// Each face has its own picture (a transforming or modal double-faced
    /// card): a back face is looked up by the card's source id.
    pub double_faced: bool,
}

/// The images of one printing of a card an Arena replay names, by Arena id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MtgrReplayPrinting {
    /// The Arena id the text gives the card (`src arena:<grp_id>`).
    pub grp_id: u32,
    pub image: Option<String>,
    pub local_image: Option<String>,
}

/// An Arena game's replay: the match as `.mtgr` text, the requested game's
/// index among the games written, and the cards the text names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MtgrReplayView {
    pub text: String,
    pub game_index: u32,
    pub cards: Vec<MtgrReplayCard>,
    /// Each printing of those cards, so a card shows the printing played.
    pub printings: Vec<MtgrReplayPrinting>,
}

/// What the replay window shows for a game: the MTGO frames, an Arena
/// `.mtgr` record, or the reason an Arena game has no replay.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ReplayWindowView {
    Snapshot(ReplayView),
    Mtgr(MtgrReplayView),
    /// The card data names a multi-faced card but not a face the record
    /// must name. A card with no card data at all is written `?`.
    MissingCardData,
    /// The game is stored but has no replay to show (an Arena or Endstep game
    /// that wrote no record, or an MTGO game with no frames). A game that is
    /// not stored at all is `None` instead.
    NoRecord,
}

/// The window's view of game row `game_id`, whichever platform it is from;
/// `None` when there is no such game. `share` is called only for an MTGO
/// game's view. A game with a recording opens it whatever its platform.
/// An Arena or Endstep game is an `.mtgr` record built on read.
/// `queue_tokens` gets an Arena replay's tokens with no art,
/// for the API's token-by-name fetch (never called with none).
pub fn replay_window_view(
    conn: &Connection,
    game_id: i64,
    tz: Tz,
    dat: &dyn Fn(&str) -> Vec<GameLogEntry>,
    share: &dyn Fn() -> Result<ReplayShareState, String>,
    queue_tokens: &dyn Fn(Vec<TokenCard>),
) -> Result<Option<ReplayWindowView>, String> {
    let Some(found) = game_share(conn, game_id).map_err(|error| error.to_string())? else {
        return Ok(None);
    };
    if let Some(recorded) =
        store::recorded_replays::recorded_game(conn, game_id).map_err(|error| error.to_string())?
        && let Some(text) = store::recorded_replays::recording_text(conn, &recorded.token)
            .map_err(|error| error.to_string())?
    {
        let cards = store::recorded_replays::recording_cards(conn, &text, Platform::Mtgo)
            .map_err(|error| error.to_string())?;
        return Ok(Some(ReplayWindowView::Mtgr(MtgrReplayView {
            text,
            game_index: recorded.position,
            cards: mtgr_cards(cards),
            printings: Vec::new(),
        })));
    }
    let platform =
        store::platform::match_platform(conn, &found.token).map_err(|error| error.to_string())?;
    match platform {
        Some(Platform::Arena) => arena_window_view(conn, game_id, queue_tokens),
        Some(Platform::Endstep) => endstep_window_view(conn, game_id),
        Some(Platform::Mtgo) | None => {
            let view = snapshot_view(conn, &found, game_id, tz, dat, share()?)?;
            Ok(Some(view.map_or(
                ReplayWindowView::NoRecord,
                ReplayWindowView::Snapshot,
            )))
        }
    }
}

/// An Arena game's `.mtgr` view, built from its stored game messages.
fn arena_window_view(
    conn: &Connection,
    game_id: i64,
    queue_tokens: &dyn Fn(Vec<TokenCard>),
) -> Result<Option<ReplayWindowView>, String> {
    let read =
        store::arena_replay::arena_replay(conn, game_id).map_err(|error| error.to_string())?;
    Ok(match read {
        ArenaReplayRead::Ready(mut replay) => {
            let tokens = std::mem::take(&mut replay.tokens_to_fetch);
            if !tokens.is_empty() {
                queue_tokens(tokens);
            }
            Some(ReplayWindowView::Mtgr(mtgr_view(replay)))
        }
        ArenaReplayRead::MissingCardData => Some(ReplayWindowView::MissingCardData),
        ArenaReplayRead::NotFound => Some(ReplayWindowView::NoRecord),
    })
}

/// An Endstep game's `.mtgr` view, built from its stored capture units.
/// Its cards carry no printing: the viewer shows each name's image.
fn endstep_window_view(
    conn: &Connection,
    game_id: i64,
) -> Result<Option<ReplayWindowView>, String> {
    let read =
        store::endstep_replay::endstep_replay(conn, game_id).map_err(|error| error.to_string())?;
    Ok(match read {
        EndstepReplayRead::Ready(replay) => Some(ReplayWindowView::Mtgr(MtgrReplayView {
            text: replay.text,
            game_index: replay.game_index,
            cards: mtgr_cards(replay.cards),
            printings: Vec::new(),
        })),
        EndstepReplayRead::NotFound => Some(ReplayWindowView::NoRecord),
    })
}

/// The viewer's cards for the rows a recording or an Endstep replay names.
fn mtgr_cards(rows: Vec<MtgrReplayCardRow>) -> Vec<MtgrReplayCard> {
    rows.into_iter()
        .map(|card| MtgrReplayCard {
            name: card.name,
            image: card.image,
            local_image: card.local_image,
            double_faced: card.double_faced,
        })
        .collect()
}

fn mtgr_view(replay: ArenaReplay) -> MtgrReplayView {
    MtgrReplayView {
        text: replay.text,
        game_index: replay.game_index,
        cards: replay
            .cards
            .into_iter()
            .map(|card| MtgrReplayCard {
                name: card.name,
                image: card.image,
                local_image: card.local_image,
                double_faced: card.double_faced,
            })
            .collect(),
        printings: replay
            .printings
            .into_iter()
            .map(|printing| MtgrReplayPrinting {
                grp_id: printing.grp_id,
                image: printing.image,
                local_image: printing.local_image,
            })
            .collect(),
    }
}

/// The view of game row `game_id`; `None` when there is no such game. Log
/// times are local to `tz`; `dat` gives a match token's `.dat` game log
/// entries (the match page's `match_dat_entries`).
pub fn replay_view(
    conn: &Connection,
    game_id: i64,
    tz: Tz,
    dat: &dyn Fn(&str) -> Vec<GameLogEntry>,
    share: ReplayShareState,
) -> Result<Option<ReplayView>, String> {
    let Some(found) = game_share(conn, game_id).map_err(|error| error.to_string())? else {
        return Ok(None);
    };
    snapshot_view(conn, &found, game_id, tz, dat, share)
}

/// `replay_view` for a game whose row `found` is already read.
fn snapshot_view(
    conn: &Connection,
    found: &GameShare,
    game_id: i64,
    tz: Tz,
    dat: &dyn Fn(&str) -> Vec<GameLogEntry>,
    share: ReplayShareState,
) -> Result<Option<ReplayView>, String> {
    let entries = dat(&found.token);
    let Some(inputs) =
        replay_inputs(conn, found.match_id, tz, &entries).map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let Some(game) = inputs.games.iter().find(|game| game.id == game_id) else {
        return Ok(None);
    };
    let previous = previous_sideboard(&inputs.games, game_id);

    let mut ids = frame_catalog_ids(&game.frames);
    for (catalog_id, _) in &game.sideboard {
        ids.push(*catalog_id);
    }
    if let Some((_, sideboard)) = &previous {
        for (catalog_id, _) in sideboard {
            ids.push(*catalog_id);
        }
    }
    ids.sort_unstable();
    ids.dedup();
    let cards = card_infos(conn, &ids).map_err(|error| error.to_string())?;

    let frames = decorate_frames(
        game.frames.clone(),
        &inputs.local_username,
        &cards,
        ImageMode::App,
    );
    let sideboard = if game.sideboard.is_empty() {
        None
    } else {
        Some(sideboard_entries(&game.sideboard, &cards))
    };
    let previous_sideboard = previous.map(|(number, sideboard)| ReplayPreviousSideboard {
        game: number,
        sideboard: sideboard_entries(&sideboard, &cards),
    });

    Ok(Some(ReplayView {
        frames: frames
            .into_iter()
            .map(|frame| ReplayFrameView {
                timestamp: viewer_time(&frame.timestamp, tz),
                content: frame.content,
            })
            .collect(),
        log: game
            .log
            .iter()
            .map(|line| ReplayLogLine {
                timestamp: line.timestamp.clone(),
                message: line.message.clone(),
            })
            .collect(),
        won: game.won,
        game_id: view_id(game_id),
        match_token: inputs.token.clone(),
        match_games: match_games(&inputs.games),
        sideboard,
        previous_sideboard,
        archetypes: ReplayArchetypes {
            local: inputs.local_archetype.clone(),
            opponent: inputs.opponent_archetype.clone(),
        },
        share,
    }))
}

/// A frame time as the viewer reads it: a time of day as stored. A match
/// pulled from another device keeps the bundle's ISO 8601 instants (spec
/// section 16); they are shown as the time of day in `tz`, `HH:MM:SS`, with
/// `.mmm` when the instant has a fraction, like every other match. Any
/// other text is passed on as written.
fn viewer_time(text: &str, tz: Tz) -> String {
    let Ok(instant) = chrono::DateTime::parse_from_rfc3339(text) else {
        return text.to_string();
    };
    let local = instant.with_timezone(&tz);
    if instant.timestamp_subsec_millis() == 0 {
        local.format("%H:%M:%S").to_string()
    } else {
        local.format("%H:%M:%S%.3f").to_string()
    }
}

/// A row id as the UI's number (row ids are small).
fn view_id(id: i64) -> u32 {
    u32::try_from(id).unwrap_or(0)
}

/// Every game of the match for the picker, by game number.
fn match_games(games: &[ReplayGameInput]) -> Vec<ReplayMatchGame> {
    games
        .iter()
        .map(|game| ReplayMatchGame {
            id: view_id(game.id),
            number: game.number,
            won: game.won,
        })
        .collect()
}

/// The catalog ids the frames' cards name.
fn frame_catalog_ids(frames: &[ReplayFrame]) -> Vec<u32> {
    let mut ids = Vec::new();
    for frame in frames {
        let Some(cards) = frame.content.get("Cards").and_then(Value::as_array) else {
            continue;
        };
        for card in cards {
            if let Some(id) = card
                .get("CatalogID")
                .and_then(Value::as_u64)
                .and_then(|id| u32::try_from(id).ok())
            {
                ids.push(id);
            }
        }
    }
    ids
}

/// A sideboard's entries with the cards' names, types and images (the
/// stored remote image and the downloaded file, as the frames in the app).
fn sideboard_entries(
    sideboard: &[(u32, u32)],
    cards: &BTreeMap<u32, CardInfo>,
) -> Vec<ReplaySideboardEntry> {
    let mut entries = Vec::new();
    for (catalog_id, quantity) in sideboard {
        let known = cards.get(catalog_id);
        entries.push(ReplaySideboardEntry {
            catalog_id: *catalog_id,
            quantity: *quantity,
            name: known.and_then(|card| card.name.clone()),
            type_line: known.and_then(|card| card.type_line.clone()),
            image: known.and_then(|card| card.image.clone()),
            local_image: known.and_then(|card| card.local_image.clone()),
        });
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matches::tests::tracked;
    use fixture_tools::TestDir;
    use serde_json::json;
    use store::params;

    const UTC: Tz = chrono_tz::UTC;

    fn no_dat(_: &str) -> Vec<GameLogEntry> {
        Vec::new()
    }

    fn share() -> ReplayShareState {
        ReplayShareState {
            linked: true,
            supporter: false,
            url: Some("https://mymtgo.test/r/q?game=1".to_string()),
            shareable: true,
        }
    }

    fn view(conn: &Connection, game_id: i64) -> ReplayView {
        replay_view(conn, game_id, UTC, &no_dat, share())
            .expect("read")
            .expect("a game")
    }

    fn catalog_ids(frame: &Value) -> Vec<u64> {
        frame["Cards"]
            .as_array()
            .map(|cards| {
                cards
                    .iter()
                    .filter_map(|card| card["CatalogID"].as_u64())
                    .filter(|id| *id > 0)
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn replay_view_for_a_fixture_game() {
        let (_dir, conn) = tracked("league_join_drop", "replay-view-fixture");
        let match_id: i64 = conn
            .query_row(
                "SELECT id FROM matches WHERE source = 'log' ORDER BY id LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("a log match");
        let with_frames = store::replay::games_with_frames(&conn, match_id).expect("read");
        let game_id = *with_frames.first().expect("a game with frames");
        let inputs = replay_inputs(&conn, match_id, UTC, &[])
            .expect("read")
            .expect("the match");
        let game = inputs
            .games
            .iter()
            .find(|game| game.id == game_id)
            .expect("the game");

        // A card of the frames with a full row and a downloaded image.
        let card_id = game
            .frames
            .iter()
            .flat_map(|frame| catalog_ids(&frame.content))
            .next()
            .expect("a known card in the frames");
        conn.execute(
            "INSERT OR REPLACE INTO cards (catalog_id, name, type, image, local_image)
             VALUES (?1, 'Qcard', 'Creature - Q', 'https://cards.scryfall.io/q.jpg', 'q.jpg')",
            params![card_id as i64],
        )
        .expect("card");

        let shown = view(&conn, game_id);
        assert_eq!(shown.game_id, game_id as u32);
        assert_eq!(shown.match_token, inputs.token);
        assert_eq!(shown.won, game.won);
        assert_eq!(shown.share, share());
        assert_eq!(shown.archetypes, ReplayArchetypes::default());
        assert_eq!(shown.frames.len(), game.frames.len());
        for (shown, stored) in shown.frames.iter().zip(&game.frames) {
            assert_eq!(shown.timestamp, stored.timestamp);
        }
        // The picker: every game by number.
        let numbers: Vec<(u32, u32, Option<bool>)> = shown
            .match_games
            .iter()
            .map(|entry| (entry.id, entry.number, entry.won))
            .collect();
        let expected: Vec<(u32, u32, Option<bool>)> = inputs
            .games
            .iter()
            .map(|entry| (entry.id as u32, entry.number, entry.won))
            .collect();
        assert_eq!(numbers, expected);
        assert_eq!(shown.log.len(), game.log.len(), "the game's own log lines");

        // Decorated for the app: one local player in every frame, cards
        // named from the catalog with the local image, MTGO's Name gone.
        let mut saw_card = false;
        for frame in &shown.frames {
            let players = frame.content["Players"].as_array().expect("players");
            let locals = players
                .iter()
                .filter(|player| player["IsLocal"] == json!(true))
                .count();
            assert_eq!(locals, 1);
            for card in frame.content["Cards"].as_array().expect("cards") {
                assert!(card.get("Name").is_none());
                assert!(card.get("name").is_some());
                if card["CatalogID"].as_u64() == Some(card_id) {
                    saw_card = true;
                    assert_eq!(card["name"], "Qcard");
                    assert_eq!(card["type"], "Creature - Q");
                    assert_eq!(card["image"], "https://cards.scryfall.io/q.jpg");
                    assert_eq!(card["local_image"], "q.jpg");
                }
            }
        }
        assert!(saw_card);
        assert_eq!(
            replay_view(&conn, 999_999, UTC, &no_dat, share()).expect("read"),
            None
        );
    }

    /// An imported match of Qplayer: game 1 with a recorded sideboard, game
    /// 2 without frames, game 3 with frames. Returns the game ids.
    fn imported(conn: &Connection) -> Vec<i64> {
        conn.execute_batch(
            "INSERT INTO accounts (username, login_id, first_seen_at) VALUES ('Qplayer', 4242, 0);
             INSERT INTO opponents (username, first_seen_at) VALUES ('Qfoe', 0);
             INSERT INTO matches (token, account_id, opponent_id, source, format, match_type,
                 state, outcome, started_at, ended_at, last_activity_at, revision,
                 derived_version)
             VALUES ('q-imported', 1, 1, 'imported', 'CModern', 'Constructed', 'complete',
                 'win', 1000, 90000, 90000, 1, 0);
             INSERT INTO cards (catalog_id, name, type, image, local_image)
             VALUES (5678, 'Qside', 'Instant', 'https://cards.scryfall.io/s.jpg', 's.jpg');",
        )
        .expect("match");
        let mut ids = Vec::new();
        for (number, result) in [(1, "loss"), (2, "win"), (3, "unknown")] {
            conn.execute(
                "INSERT INTO games (match_id, game_number, result, end_reason, started_at)
                 VALUES (1, ?1, ?2, 'unknown', ?3)",
                params![number, result, 1000 * number],
            )
            .expect("game");
            ids.push(conn.last_insert_rowid());
        }
        conn.execute(
            "INSERT INTO game_cards (game_id, side, catalog_id, main_qty, side_qty)
             VALUES (?1, 'local', 5678, 0, 2)",
            [ids[0]],
        )
        .expect("sideboard");
        let board = r#"{"Players":[{"Id":1,"Name":"Qplayer","Life":20},{"Id":2,"Name":"Qfoe","Life":20}],"Cards":[]}"#;
        for game_id in [ids[0], ids[2]] {
            let tx = conn.unchecked_transaction().expect("tx");
            store::game_sync::store_timeline(
                &tx,
                game_id,
                &[::replay::FrameText {
                    time: "18:00:00".to_string(),
                    text: board.to_string(),
                }],
            )
            .expect("timeline");
            tx.commit().expect("commit");
        }
        ids
    }

    /// Stores a recording named `token` with one game per `(position,
    /// game_number)`.
    fn record(conn: &Connection, token: &str, text: &str, games: &[(u32, u32)]) {
        conn.execute(
            "INSERT INTO recorded_replays (token, mtgo_match_id, text, file_hash, imported_at)
             VALUES (?1, NULL, ?2, 'h', 0)",
            params![token, text],
        )
        .expect("recording");
        for (position, number) in games {
            conn.execute(
                "INSERT INTO recorded_replay_games (token, position, game_number)
                 VALUES (?1, ?2, ?3)",
                params![token, position, number],
            )
            .expect("recorded game");
        }
    }

    fn window(conn: &Connection, game_id: i64) -> Option<ReplayWindowView> {
        replay_window_view(conn, game_id, UTC, &no_dat, &|| Ok(share()), &|_| {
            panic!("a recording queues no tokens")
        })
        .expect("read")
    }

    #[test]
    fn a_recorded_game_opens_in_the_mtgr_viewer() {
        let dir = TestDir::new("replay-view-recorded");
        let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        let ids = imported(&conn);
        conn.execute(
            "INSERT INTO cards (catalog_id, name, image, local_image)
             VALUES (900, 'Qbolt', 'https://c.example/b.jpg', 'b.jpg')",
            [],
        )
        .expect("card");
        record(
            &conn,
            "q-imported",
            "[Game]\nplays \"Qbolt\"\n",
            &[(0, 1), (1, 2)],
        );

        let Some(ReplayWindowView::Mtgr(shown)) = window(&conn, ids[1]) else {
            panic!("the mtgr viewer");
        };
        assert_eq!(shown.text, "[Game]\nplays \"Qbolt\"\n");
        assert_eq!(shown.game_index, 1);
        assert_eq!(
            shown.cards,
            vec![MtgrReplayCard {
                name: "Qbolt".to_string(),
                image: Some("https://c.example/b.jpg".to_string()),
                local_image: Some("b.jpg".to_string()),
                double_faced: false,
            }]
        );
        assert!(shown.printings.is_empty());
    }

    #[test]
    fn a_recording_that_starts_at_game_two_opens_game_two() {
        let dir = TestDir::new("replay-view-recorded-game-two");
        let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        let ids = imported(&conn);
        // The recorder joined at game 2: the file's only game is number 2.
        record(&conn, "q-imported", "[Game]\n", &[(0, 2)]);

        let Some(ReplayWindowView::Mtgr(shown)) = window(&conn, ids[1]) else {
            panic!("the mtgr viewer");
        };
        assert_eq!(shown.game_index, 0);
        // Game 1 was not recorded: it keeps the frames view.
        assert!(matches!(
            window(&conn, ids[0]),
            Some(ReplayWindowView::Snapshot(_))
        ));
    }

    #[test]
    fn a_stored_game_with_no_replay_is_no_record_and_a_missing_game_is_none() {
        let dir = TestDir::new("replay-view-no-record");
        let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        let ids = imported(&conn);
        assert!(window(&conn, 999_999).is_none(), "no such game");
        // An Endstep match with no stored units has no record to build.
        conn.execute("UPDATE accounts SET platform = 'endstep'", [])
            .expect("platform");
        assert_eq!(window(&conn, ids[0]), Some(ReplayWindowView::NoRecord));
        assert_eq!(
            serde_json::to_value(ReplayWindowView::NoRecord).expect("json")["kind"],
            "noRecord"
        );
    }

    #[test]
    fn a_recorded_game_without_text_falls_back() {
        let dir = TestDir::new("replay-view-recorded-no-text");
        let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        let ids = imported(&conn);
        record(&conn, "q-imported", "[Game]\n", &[(0, 1)]);
        conn.execute_batch(
            "PRAGMA foreign_keys = OFF; DELETE FROM recorded_replays; PRAGMA foreign_keys = ON;",
        )
        .expect("recording gone, game rows left");
        assert!(matches!(
            window(&conn, ids[0]),
            Some(ReplayWindowView::Snapshot(_))
        ));
    }

    #[test]
    fn the_view_carries_the_sideboard_and_the_previous_one() {
        let dir = TestDir::new("replay-view-sideboard");
        let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        let ids = imported(&conn);
        let qside = ReplaySideboardEntry {
            catalog_id: 5678,
            quantity: 2,
            name: Some("Qside".to_string()),
            type_line: Some("Instant".to_string()),
            image: Some("https://cards.scryfall.io/s.jpg".to_string()),
            local_image: Some("s.jpg".to_string()),
        };

        let first = view(&conn, ids[0]);
        assert_eq!(first.won, Some(false));
        assert_eq!(first.sideboard, Some(vec![qside.clone()]));
        assert_eq!(first.previous_sideboard, None, "game 1 has none");
        assert_eq!(first.frames.len(), 1);

        let frameless = view(&conn, ids[1]);
        assert!(frameless.frames.is_empty(), "the empty state");
        assert_eq!(frameless.sideboard, None);

        let third = view(&conn, ids[2]);
        assert_eq!(third.won, None);
        assert_eq!(
            third.previous_sideboard,
            Some(ReplayPreviousSideboard {
                game: 1,
                sideboard: vec![qside]
            })
        );
        assert_eq!(third.match_games.len(), 3);

        // The JSON the page reads: camelCase on top, the viewer's keys below.
        let value = serde_json::to_value(&third).expect("json");
        assert_eq!(value["gameId"], json!(ids[2]));
        assert_eq!(value["matchToken"], "q-imported");
        assert_eq!(
            value["matchGames"][0],
            json!({"id": ids[0], "number": 1, "won": false})
        );
        assert_eq!(
            value["previousSideboard"]["sideboard"][0],
            json!({"catalog_id": 5678, "quantity": 2, "name": "Qside", "type": "Instant",
                "image": "https://cards.scryfall.io/s.jpg", "local_image": "s.jpg"})
        );
        assert_eq!(
            value["archetypes"],
            json!({"local": null, "opponent": null})
        );
        assert_eq!(value["frames"][0]["timestamp"], "18:00:00");
        assert_eq!(value["frames"][0]["content"]["Players"][0]["IsLocal"], true);
    }

    // A match pulled from another device keeps the bundle's ISO UTC instants
    // as its frame times; the viewer shows them as the local time of day,
    // like every other match. Times of day pass through unchanged.
    #[test]
    fn a_pulled_match_shows_its_frame_times_as_the_local_time_of_day() {
        let dir = TestDir::new("replay-view-pulled-times");
        let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        let ids = imported(&conn);
        let board = r#"{"Players":[{"Id":1,"Name":"Qplayer","Life":20},{"Id":2,"Name":"Qfoe","Life":19}],"Cards":[]}"#;
        let times = [
            "2026-10-01T17:00:00Z",
            "2026-10-01T17:00:05.250Z",
            "2026-10-01T18:00:07+01:00",
            "18:00:09",
        ];
        let frames: Vec<::replay::FrameText> = times
            .iter()
            .enumerate()
            .map(|(index, time)| ::replay::FrameText {
                time: time.to_string(),
                text: board.replace("19", &(19 - index).to_string()),
            })
            .collect();
        conn.execute("DELETE FROM game_timelines WHERE game_id = ?1", [ids[2]])
            .expect("clear");
        let tx = conn.unchecked_transaction().expect("tx");
        store::game_sync::store_timeline(&tx, ids[2], &frames).expect("timeline");
        tx.commit().expect("commit");

        let london: Tz = "Europe/London".parse().expect("zone");
        let shown = replay_view(&conn, ids[2], london, &no_dat, share())
            .expect("read")
            .expect("a game");
        let shown: Vec<&str> = shown
            .frames
            .iter()
            .map(|frame| frame.timestamp.as_str())
            .collect();
        assert_eq!(shown, ["18:00:00", "18:00:05.250", "18:00:07", "18:00:09"]);
    }

    /// Every Arena card of the fixture card table, as card rows.
    fn insert_arena_cards(conn: &Connection) {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/arena/cards.json");
        let text = std::fs::read_to_string(path).expect("the fixture card table");
        let table: Value = serde_json::from_str(&text).expect("JSON");
        for (grp_id, card) in table.as_object().expect("an object") {
            let grp_id: i64 = grp_id.parse().expect("grpId keys");
            conn.execute(
                "INSERT OR REPLACE INTO cards (catalog_id, name, image, local_image)
                 VALUES (?1, ?2, 'https://cards.example/a.jpg', 'a.jpg')",
                params![
                    i64::from(cards::ARENA_ID_OFFSET) + grp_id,
                    card["name"].as_str().expect("a name")
                ],
            )
            .expect("card");
        }
    }

    #[test]
    fn replay_view_of_an_mtgo_game_is_a_snapshot() {
        let (_dir, conn) = tracked("league_join_drop", "replay-window-mtgo");
        let match_id: i64 = conn
            .query_row(
                "SELECT id FROM matches WHERE source = 'log' ORDER BY id LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("a log match");
        let game_id = *store::replay::games_with_frames(&conn, match_id)
            .expect("read")
            .first()
            .expect("a game with frames");
        let wrapped = replay_window_view(&conn, game_id, UTC, &no_dat, &|| Ok(share()), &|_| {})
            .expect("read")
            .expect("a game");
        let plain = view(&conn, game_id);
        assert_eq!(wrapped, ReplayWindowView::Snapshot(plain));
        let json = serde_json::to_value(&wrapped).expect("json");
        assert_eq!(json["kind"], "snapshot");
        assert_eq!(json["gameId"], game_id);
    }

    #[test]
    fn replay_view_of_an_arena_game_is_mtgr() {
        let (_dir, conn, token) =
            crate::matches::tests::tracked_arena("bo3-three-games", "replay-window-arena");
        let game_id: i64 = conn
            .query_row(
                "SELECT g.id FROM games g JOIN matches m ON m.id = g.match_id
                 WHERE m.token = ?1 AND g.game_number = 2",
                [&token],
                |row| row.get(0),
            )
            .expect("game 2");

        // No card data: every card is `?` with its Arena id (decision
        // 0048), nothing guessed, and the replay still opens.
        conn.execute(
            "DELETE FROM cards WHERE catalog_id >= ?1",
            [cards::ARENA_ID_OFFSET],
        )
        .expect("no cards");
        let unknown = replay_window_view(&conn, game_id, UTC, &no_dat, &|| Ok(share()), &|_| {})
            .expect("read")
            .expect("a game");
        let ReplayWindowView::Mtgr(unknown) = &unknown else {
            panic!("an Arena game with no card data is still an mtgr view, got {unknown:?}");
        };
        assert!(unknown.text.contains(" ? src arena:"));
        assert!(unknown.cards.is_empty());

        // Card data that names a double-faced card without its faces: the
        // one case still refused.
        insert_arena_cards(&conn);
        conn.execute(
            "UPDATE cards SET name = 'Fixture Piper' WHERE catalog_id = ?1",
            [cards::ARENA_ID_OFFSET + 79017],
        )
        .expect("rename");
        let missing = replay_window_view(&conn, game_id, UTC, &no_dat, &|| Ok(share()), &|_| {})
            .expect("read")
            .expect("a game");
        assert_eq!(missing, ReplayWindowView::MissingCardData);
        assert_eq!(
            serde_json::to_value(&missing).expect("json")["kind"],
            "missingCardData"
        );

        insert_arena_cards(&conn);
        let queued = std::cell::RefCell::new(Vec::new());
        let shown = replay_window_view(&conn, game_id, UTC, &no_dat, &|| Ok(share()), &|tokens| {
            queued.borrow_mut().extend(tokens)
        })
        .expect("read")
        .expect("a game");
        let ReplayWindowView::Mtgr(mtgr) = &shown else {
            panic!("an Arena game is an mtgr view, got {shown:?}");
        };
        assert!(mtgr.text.starts_with("[Mtgr \"0."));
        assert_eq!(mtgr.game_index, 1);
        assert!(!mtgr.cards.is_empty());
        let json = serde_json::to_value(&shown).expect("json");
        assert_eq!(json["kind"], "mtgr");
        assert_eq!(json["gameIndex"], 1);
        assert!(json["cards"][0].get("localImage").is_some());
        // Its Samurai tokens have no art: queued for the token-by-name fetch.
        let queued: Vec<String> = queued.borrow().iter().map(|t| t.name.clone()).collect();
        assert_eq!(queued, vec!["Samurai".to_string()]);

        // A game that is gone has no view.
        assert_eq!(
            replay_window_view(&conn, 999_999, UTC, &no_dat, &|| Ok(share()), &|_| {})
                .expect("read"),
            None
        );
    }

    #[test]
    fn an_endstep_game_opens_as_mtgr() {
        let dir = TestDir::new("replay-window-endstep");
        let path = dir.join("mymtgo.sqlite");
        crate::endstep_seed::sync_endstep_fixture(&path, &dir, 1_794_400_000_000);
        let conn = store::open_reader(&path).expect("reader");
        let game_id: i64 = conn
            .query_row(
                "SELECT g.id FROM games g JOIN matches m ON m.id = g.match_id
                 WHERE m.token = ?1 AND g.game_number = 2",
                [crate::endstep_seed::ENDSTEP_FIXTURE_TOKEN],
                |row| row.get(0),
            )
            .expect("game 2");
        let shown = replay_window_view(&conn, game_id, UTC, &no_dat, &|| Ok(share()), &|_| {
            panic!("an Endstep replay queues no tokens")
        })
        .expect("read")
        .expect("a game");
        let ReplayWindowView::Mtgr(mtgr) = &shown else {
            panic!("an Endstep game is an mtgr view, got {shown:?}");
        };
        assert!(mtgr.text.starts_with("[Mtgr \"0.3.0\"]"));
        assert!(mtgr.text.contains("[Site \"Endstep\"]"));
        assert_eq!(mtgr.game_index, 1);
        assert!(mtgr.printings.is_empty(), "no printings: images by name");
        assert!(
            mtgr.cards.iter().any(|card| card.name == "Expedition Map"),
            "the synced card rows name the cards"
        );
    }
}
