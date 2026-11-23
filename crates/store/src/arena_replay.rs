//! An Arena game's replay, built on read (arena replays spec, section 4):
//! the match's stored game messages (live and archived; for a match pulled
//! from another device, the source it came with) and its match row go
//! through `arena_game::mtgr::match_record`, and the `.mtgr` text comes
//! back with the requested game's index and the images of the cards it
//! names. Nothing is stored: a replay is derived each time it is opened.
//!
//! - Names: the account's username and the opponent's display name, as the
//!   match page shows them. The local name carries the Arena `#tag`
//!   (`Qarena#12345`), as stored.
//! - Event: the match's league name, else the Arena event the match room
//!   named (`Traditional_Explorer_Ladder`; for a match pulled from another
//!   device, the event its source carries), else unknown.
//! - Format: the stored format as the app shows it (`CPIONEER` is
//!   `Pioneer`).
//! - Archetypes: the match's archetype for each seat, when assigned.
//! - Card data: every named Arena card row (`catalog_id = ARENA_ID_OFFSET +
//!   grpId`) that is not a token, its faces split from its `A // B` name.
//!   A card with none is written `?` with its source id (decision 0048).
//! - Token art: a token named by its subtypes ("Map Token", CR 111.4) takes
//!   the images of a token row named by them ("Map", Scryfall's name for
//!   it), from either platform. With no such row, the token is queued for
//!   the API's token-by-name fetch as its own Arena row (`tokens_to_fetch`),
//!   so its art comes on a later open. Token rows never name a token in the
//!   record.

use std::collections::{BTreeMap, BTreeSet};

use arena_game::mtgr::{
    CardData, CardTable, GameMessages, MatchInput, MtgrError, SubtypeToken, write_match,
};
use cards::tokens::{TOKEN_RARITY, TokenCard};
use cards::{ARENA_ID_OFFSET, ENDSTEP_ID_OFFSET};
use rusqlite::{Connection, OptionalExtension};

use crate::StoreError;
use crate::arena_game_sync::group_by_game;
use crate::arena_games::{game_messages, source_messages, synced_arena_sources};
use crate::replay_shared::{seat_archetype, written_game_numbers};
use crate::write::decode_raw;

/// One Arena game's replay: the whole match as `.mtgr` text, the index of
/// the requested game among the games written, and the cards it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArenaReplay {
    pub text: String,
    pub game_index: u32,
    pub cards: Vec<ArenaReplayCard>,
    /// The printings of the cards it names, one per Arena id.
    pub printings: Vec<ArenaReplayPrinting>,
    /// Tokens with no art to queue for the API's token-by-name fetch.
    pub tokens_to_fetch: Vec<TokenCard>,
}

/// The images of one printing of a card the text names, by its Arena id
/// (`grp_id`, the `arena:` source id the text gives each card). The viewer
/// shows the exact printing played (a basic land has dozens, each with its
/// own art), where `ArenaReplayCard` holds the one row chosen for the name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArenaReplayPrinting {
    pub grp_id: u32,
    pub image: Option<String>,
    pub local_image: Option<String>,
}

/// A card the text names. `image` is the stored remote URL, `local_image`
/// the stored file name, as MTGO replays read them: both show the front.
/// `double_faced` is set for a card whose faces each have their own picture
/// (Arena showed it with another side), so its back face is looked up apart;
/// a split card, Room or Adventure shows its one picture for every face.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArenaReplayCard {
    pub name: String,
    pub image: Option<String>,
    pub local_image: Option<String>,
    pub double_faced: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArenaReplayRead {
    Ready(ArenaReplay),
    /// The card data names a multi-faced card but not a face the record must
    /// name: no replay (the writer never guesses a name). A card with no
    /// card data at all is written `?`, not refused.
    MissingCardData,
    /// No such game, not an Arena game, no stored messages, or the game
    /// never started.
    NotFound,
}

/// A whole Arena match as `.mtgr` text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArenaMatchText {
    pub text: String,
    /// The game number of each game written, in order.
    pub games: Vec<u32>,
    pub cards: Vec<ArenaReplayCard>,
    /// The printings of the cards it names, one per Arena id.
    pub printings: Vec<ArenaReplayPrinting>,
    /// Tokens with no art to queue for the API's token-by-name fetch: each
    /// as its Arena row, named by its subtypes (Scryfall's token name).
    pub tokens_to_fetch: Vec<TokenCard>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArenaMatchRead {
    Ready(ArenaMatchText),
    /// The grpIds of the cards whose card data lacks a face name.
    MissingCardData(Vec<u32>),
    /// No Arena match with that token, or it has no stored messages.
    NotFound,
}

/// The replay of Arena game row `game_id`.
pub fn arena_replay(conn: &Connection, game_id: i64) -> Result<ArenaReplayRead, StoreError> {
    let found: Option<(String, u32)> = conn
        .prepare_cached(
            "SELECT m.token, g.game_number FROM games g
             JOIN matches m ON m.id = g.match_id
             JOIN accounts a ON a.id = m.account_id AND a.platform = 'arena'
             WHERE g.id = ?1",
        )?
        .query_row([game_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let Some((token, game_number)) = found else {
        return Ok(ArenaReplayRead::NotFound);
    };
    let written = match arena_match_text(conn, &token)? {
        ArenaMatchRead::Ready(written) => written,
        ArenaMatchRead::MissingCardData(_) => return Ok(ArenaReplayRead::MissingCardData),
        ArenaMatchRead::NotFound => return Ok(ArenaReplayRead::NotFound),
    };
    let Some(index) = written
        .games
        .iter()
        .position(|number| *number == game_number)
    else {
        return Ok(ArenaReplayRead::NotFound);
    };
    Ok(ArenaReplayRead::Ready(ArenaReplay {
        text: written.text,
        game_index: index as u32,
        cards: written.cards,
        printings: written.printings,
        tokens_to_fetch: written.tokens_to_fetch,
    }))
}

/// The Arena match `match_token` as `.mtgr` text.
pub fn arena_match_text(
    conn: &Connection,
    match_token: &str,
) -> Result<ArenaMatchRead, StoreError> {
    let Some(head) = read_head(conn, match_token)? else {
        return Ok(ArenaMatchRead::NotFound);
    };
    // A match pulled from another device reads its messages and event
    // from the source it came with (one decode); a recorded one from the
    // local fact tables.
    let (messages, pulled_event) = match synced_arena_sources(conn, match_token)? {
        Some(sources) => (source_messages(&sources)?, Some(sources.event_id)),
        None => (game_messages(conn, match_token)?, None),
    };
    let mut games = Vec::new();
    for (number, messages) in group_by_game(messages) {
        games.push(GameMessages { number, messages });
    }
    if games.is_empty() {
        return Ok(ArenaMatchRead::NotFound);
    }
    let event = match (head.league, pulled_event) {
        (Some(league), _) => league,
        (None, Some(event)) => event.unwrap_or_default(),
        (None, None) => room_event(conn, match_token)?.unwrap_or_default(),
    };
    let input = MatchInput {
        match_token: match_token.to_string(),
        event,
        format: head
            .format
            .map(|code| decks::display_format(&code))
            .unwrap_or_default(),
        local_name: head.local_name,
        opponent_name: head.opponent_name.unwrap_or_default(),
        local_archetype: seat_archetype(conn, head.match_id, false)?,
        opponent_archetype: seat_archetype(conn, head.match_id, true)?,
        started_at: head.started_at,
        games,
    };
    let rows = arena_card_rows(conn)?;
    let written = match write_match(&input, &card_table(&rows)) {
        Ok(written) => written,
        Err(MtgrError::MissingCardData(grp_ids)) => {
            return Ok(ArenaMatchRead::MissingCardData(grp_ids));
        }
    };
    let record = written.record;
    let text = ::mtgr::write(&record);
    let numbers = written_game_numbers(&record);
    let mut cards = written_cards(
        &text,
        &rows,
        &match_catalog_ids(conn, head.match_id)?,
        &written.double_faced,
    );
    let printings = written_printings(&text, &rows);
    let mut tokens_to_fetch = Vec::new();
    for token in &written.tokens {
        let Some(scryfall_name) = scryfall_token_name(token) else {
            continue;
        };
        match token_art(conn, &scryfall_name)? {
            Some((image, local_image)) => {
                if !cards.iter().any(|card| card.name == token.name) {
                    cards.push(ArenaReplayCard {
                        name: token.name.clone(),
                        image,
                        local_image,
                        double_faced: false,
                    });
                }
            }
            None => {
                let catalog_id = ARENA_ID_OFFSET + token.grp_id;
                if !card_row_exists(conn, catalog_id)? {
                    tokens_to_fetch.push(token_to_fetch(catalog_id, &scryfall_name));
                }
            }
        }
    }
    Ok(ArenaMatchRead::Ready(ArenaMatchText {
        text,
        games: numbers,
        cards,
        printings,
        tokens_to_fetch,
    }))
}

/// The tokens of every Arena match with stored messages, live or archived,
/// oldest first (the `arena-mtgr` dev command).
pub fn arena_tokens_with_messages(conn: &Connection) -> Result<Vec<String>, StoreError> {
    let mut statement = conn.prepare_cached(
        "SELECT m.token FROM matches m
         JOIN accounts a ON a.id = m.account_id AND a.platform = 'arena'
         WHERE EXISTS (SELECT 1 FROM arena_game_messages l WHERE l.match_token = m.token)
            OR EXISTS (SELECT 1 FROM arena_game_archives r WHERE r.match_token = m.token)
         ORDER BY m.started_at, m.id",
    )?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut tokens = Vec::new();
    for row in rows {
        tokens.push(row?);
    }
    Ok(tokens)
}

/// The match columns the record reads.
struct Head {
    match_id: i64,
    format: Option<String>,
    started_at: i64,
    local_name: String,
    opponent_name: Option<String>,
    league: Option<String>,
}

/// An Arena match's row, with the names the match page shows and its
/// constructed league run's name (as the match page's header reads it).
fn read_head(conn: &Connection, match_token: &str) -> Result<Option<Head>, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT m.id, m.format, m.started_at, a.username,
                 COALESCE(o.display_name, o.username), l.name
             FROM matches m
             JOIN accounts a ON a.id = m.account_id AND a.platform = 'arena'
             LEFT JOIN opponents o ON o.id = m.opponent_id
             LEFT JOIN listed_leagues l ON l.id = m.league_id AND l.detached_at IS NULL
                 AND l.kind = 'constructed'
             WHERE m.token = ?1",
        )?
        .query_row([match_token], |row| {
            Ok(Head {
                match_id: row.get(0)?,
                format: row.get(1)?,
                started_at: row.get(2)?,
                local_name: row.get(3)?,
                opponent_name: row.get(4)?,
                league: row.get(5)?,
            })
        })
        .optional()?)
}

/// The Arena event the match's first room state named (its seats'
/// `event_id`), when the log gave one.
pub(crate) fn room_event(
    conn: &Connection,
    match_token: &str,
) -> Result<Option<String>, StoreError> {
    let mut statement = conn.prepare_cached(
        "SELECT raw, compressed FROM arena_log_events
         WHERE match_token = ?1 AND kind = 'room_state'
         ORDER BY arena_log_file_id, byte_offset, seq",
    )?;
    let rows = statement.query_map([match_token], |row| {
        Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, bool>(1)?))
    })?;
    for row in rows {
        let (raw, compressed) = row?;
        // One undecodable row must not fail the replay over its Event tag.
        let Ok(text) = decode_raw(raw, compressed) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let Some(players) = value.get("players").and_then(|p| p.as_array()) else {
            continue;
        };
        for player in players {
            let event = player.get("event_id").and_then(|e| e.as_str());
            if let Some(event) = event.filter(|e| !e.is_empty()) {
                return Ok(Some(event.to_string()));
            }
        }
    }
    Ok(None)
}

/// One Arena card row.
struct CardRow {
    catalog_id: u32,
    name: String,
    image: Option<String>,
    local_image: Option<String>,
}

/// Every named Arena card row that is not a token, by catalog id. A token
/// row (MTGO's `rarity = 'token'`, Scryfall's `Token ...` type line) holds
/// Scryfall's token name ("Treasure"), which is not the token's name (CR
/// 111.4: "Treasure Token"), so it never names one.
fn arena_card_rows(conn: &Connection) -> Result<Vec<CardRow>, StoreError> {
    let mut statement = conn.prepare_cached(
        "SELECT catalog_id, name, image, local_image FROM cards
         WHERE catalog_id >= ?1 AND catalog_id < ?3 AND name IS NOT NULL
           AND COALESCE(rarity, '') <> ?2 AND COALESCE(type, '') NOT LIKE 'Token%'
         ORDER BY catalog_id",
    )?;
    let rows = statement.query_map(
        rusqlite::params![ARENA_ID_OFFSET, TOKEN_RARITY, ENDSTEP_ID_OFFSET],
        |row| {
            Ok(CardRow {
                catalog_id: row.get(0)?,
                name: row.get(1)?,
                image: row.get(2)?,
                local_image: row.get(3)?,
            })
        },
    )?;
    let mut cards = Vec::new();
    for row in rows {
        cards.push(row?);
    }
    Ok(cards)
}

/// The writer's card table: each row by grpId, a multi-faced card's faces
/// split from its `A // B` name.
fn card_table(rows: &[CardRow]) -> CardTable {
    let mut table = CardTable::new();
    for row in rows {
        table.insert(
            row.catalog_id - ARENA_ID_OFFSET,
            CardData {
                name: row.name.clone(),
                faces: faces_of(&row.name),
            },
        );
    }
    table
}

/// Scryfall's name for a token the record names by its subtypes: the
/// subtypes ("Map" for "Map Token"). `None` for a token with none.
fn scryfall_token_name(token: &SubtypeToken) -> Option<String> {
    let name = token.subtypes.join(" ");
    (!name.is_empty()).then_some(name)
}

/// A row's remote image and local image file.
type Images = (Option<String>, Option<String>);

/// The images of a token row named `name` (either platform), the one with a
/// local file first, else the lowest catalog id; `None` when no token row
/// by that name has any.
pub(crate) fn token_art(conn: &Connection, name: &str) -> Result<Option<Images>, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT image, local_image FROM cards
             WHERE name = ?1 AND (rarity = ?2 OR type LIKE 'Token%')
               AND (image IS NOT NULL OR local_image IS NOT NULL)
             ORDER BY local_image IS NULL, catalog_id LIMIT 1",
        )?
        .query_row(rusqlite::params![name, TOKEN_RARITY], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()?)
}

fn card_row_exists(conn: &Connection, catalog_id: u32) -> Result<bool, StoreError> {
    Ok(conn
        .prepare_cached("SELECT 1 FROM cards WHERE catalog_id = ?1")?
        .exists([catalog_id])?)
}

/// A token's Arena row as the token-by-name fetch asks for it, as MTGO's
/// token XML gives one (`cards::tokens`): its Scryfall name, a `Token`
/// type and colourless identity until the API answers.
fn token_to_fetch(catalog_id: u32, scryfall_name: &str) -> TokenCard {
    TokenCard {
        catalog_id,
        name: scryfall_name.to_string(),
        card_type: "Token".to_string(),
        sub_type: Some(scryfall_name.to_string()),
        color_identity: "C".to_string(),
    }
}

fn faces_of(name: &str) -> Vec<String> {
    if !name.contains(" // ") {
        return Vec::new();
    }
    name.split(" // ").map(str::to_string).collect()
}

/// The catalog ids the match's stored game rows name (its cards' printings).
fn match_catalog_ids(conn: &Connection, match_id: i64) -> Result<BTreeSet<u32>, StoreError> {
    let mut statement = conn.prepare_cached(
        "SELECT DISTINCT gc.catalog_id FROM game_cards gc
         JOIN games g ON g.id = gc.game_id WHERE g.match_id = ?1",
    )?;
    let rows = statement.query_map([match_id], |row| row.get::<_, u32>(0))?;
    let mut ids = BTreeSet::new();
    for row in rows {
        ids.insert(row?);
    }
    Ok(ids)
}

/// The cards the text names, one per distinct card name. When printings
/// share a name, the match's own printing (`preferred`) gives the images,
/// else the lowest catalog id. A name is double-faced when any of its rows
/// is one of `double_faced` (front grpIds). Names with no card row (tokens
/// named by their types) have no images and are left out.
fn written_cards(
    text: &str,
    rows: &[CardRow],
    preferred: &BTreeSet<u32>,
    double_faced: &BTreeSet<u32>,
) -> Vec<ArenaReplayCard> {
    let names = quoted_strings(text);
    let mut chosen: BTreeMap<String, &CardRow> = BTreeMap::new();
    let mut double_faced_names: BTreeSet<&str> = BTreeSet::new();
    for row in rows {
        if !names.contains(&row.name) {
            continue;
        }
        if double_faced.contains(&(row.catalog_id - ARENA_ID_OFFSET)) {
            double_faced_names.insert(&row.name);
        }
        let replace = match chosen.get(&row.name) {
            None => true,
            Some(known) => {
                !preferred.contains(&known.catalog_id) && preferred.contains(&row.catalog_id)
            }
        };
        if replace {
            chosen.insert(row.name.clone(), row);
        }
    }
    let mut cards = Vec::new();
    for (name, row) in chosen {
        cards.push(ArenaReplayCard {
            double_faced: double_faced_names.contains(name.as_str()),
            name,
            image: row.image.clone(),
            local_image: row.local_image.clone(),
        });
    }
    cards
}

/// The images of every printing (Arena card row) of a card the text names,
/// by Arena id, so each card shows the printing it was played as. A row with
/// neither image is left out.
fn written_printings(text: &str, rows: &[CardRow]) -> Vec<ArenaReplayPrinting> {
    let names = quoted_strings(text);
    rows.iter()
        .filter(|row| names.contains(&row.name))
        .filter(|row| row.image.is_some() || row.local_image.is_some())
        .map(|row| ArenaReplayPrinting {
            grp_id: row.catalog_id - ARENA_ID_OFFSET,
            image: row.image.clone(),
            local_image: row.local_image.clone(),
        })
        .collect()
}

/// The quoted strings of the text's body lines (tags left out).
pub(crate) fn quoted_strings(text: &str) -> BTreeSet<String> {
    let mut all = BTreeSet::new();
    for line in text.lines() {
        if line.starts_with('[') {
            continue;
        }
        let mut rest = line;
        while let Some(start) = rest.find('"') {
            let (value, after) = read_quoted(&rest[start + 1..]);
            all.insert(value);
            rest = after;
        }
    }
    all
}

/// The string up to the closing quote (escapes undone) and the text after
/// it.
fn read_quoted(text: &str) -> (String, &str) {
    let mut value = String::new();
    let mut chars = text.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '"' => return (value, &text[at + 1..]),
            '\\' => match chars.next() {
                Some((_, 'n')) => value.push('\n'),
                Some((_, escaped)) => value.push(escaped),
                None => {}
            },
            other => value.push(other),
        }
    }
    (value, "")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use arena_protocol::{ArenaEvent, MessageSource, decode_all, split_complete};
    use fixture_tools::arena_fixture;
    use mtgo_match::GameResult;
    use rusqlite::params;

    use super::*;
    use crate::arena::NewArenaEvent;
    use crate::arena_games::{GameMessageSource, NewGameMessage};
    use crate::test_support::*;
    use crate::write::{MatchWrite, WriteBatch, commit};

    const FIXTURE: &str = "bo3-three-games";
    const ARENA: &str = "q-arena-replay";

    /// The fixture's game messages with the game numbers the tracker gives
    /// them: game 1 until a message names its game, then the named game,
    /// the next one after an intermission. The fixture holds one match;
    /// messages after its room completes belong to none.
    fn fixture_messages(fixture: &str, file_id: i64) -> (Vec<NewGameMessage>, Vec<NewArenaEvent>) {
        let fixture = arena_fixture(fixture);
        let mut messages = Vec::new();
        let mut rooms = Vec::new();
        let mut current = 1;
        let mut named: Option<u32> = None;
        let mut over = false;
        for entry in &split_complete(&fixture.bytes) {
            if over {
                break;
            }
            for (seq, message) in arena_protocol::game_messages(entry).into_iter().enumerate() {
                if let Some(number) = message.game_number {
                    current = number;
                    named = Some(number);
                }
                let intermission = message.json.contains("\"GREMessageType_IntermissionReq\"");
                messages.push(NewGameMessage {
                    file_id,
                    byte_offset: entry.offset,
                    seq: seq as u32,
                    match_token: ARENA.to_string(),
                    game_number: Some(current),
                    source: match message.source {
                        MessageSource::Gre => GameMessageSource::Gre,
                        MessageSource::Client => GameMessageSource::Client,
                    },
                    logged_at: message.at_ms.unwrap_or(0),
                    json: message.json,
                });
                if intermission && message.source == MessageSource::Gre {
                    current = named.unwrap_or(current) + 1;
                }
            }
            for (seq, event) in decode_all(entry).into_iter().enumerate() {
                let ArenaEvent::RoomState { completed, .. } = &event else {
                    continue;
                };
                over |= *completed;
                rooms.push(NewArenaEvent {
                    file_id,
                    byte_offset: entry.offset,
                    seq: 100 + seq as u32,
                    kind: "room_state",
                    match_token: Some(ARENA.to_string()),
                    logged_at_ms: 0,
                    raw: serde_json::to_string(&event).expect("event JSON"),
                });
            }
        }
        (messages, rooms)
    }

    /// The fixture card table, `fixtures/arena/cards.json`, as Arena card
    /// rows.
    fn insert_cards(conn: &Connection) {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/arena/cards.json");
        let text = std::fs::read_to_string(path).expect("the fixture card table exists");
        let value: serde_json::Value = serde_json::from_str(&text).expect("JSON");
        for (grp_id, card) in value.as_object().expect("an object") {
            let grp_id: u32 = grp_id.parse().expect("grpId keys");
            conn.execute(
                "INSERT INTO cards (catalog_id, name, image, local_image) VALUES (?1, ?2, ?3, ?4)",
                params![
                    ARENA_ID_OFFSET + grp_id,
                    card["name"].as_str().expect("a name"),
                    format!("https://cards.example/{grp_id}.jpg"),
                    format!("{grp_id}.jpg"),
                ],
            )
            .expect("card");
        }
    }

    fn commit_messages(conn: &mut Connection, messages: Vec<NewGameMessage>, archive: bool) {
        let archive_tokens = if archive {
            vec![ARENA.to_string()]
        } else {
            Vec::new()
        };
        commit(
            conn,
            &WriteBatch {
                arena_game_messages: messages,
                arena_archive_tokens: archive_tokens,
                now_ms: 9_000,
                ..WriteBatch::default()
            },
        )
        .expect("messages");
    }

    /// The tracked fixture match: its row, games, opponent name, card rows
    /// and messages. With `archive_game_one`, game 1's messages are archived
    /// before the later games' are stored (an app restarted mid-match).
    /// Returns the game ids, game 1 first.
    fn tracked(conn: &mut Connection, archive_game_one: bool) -> Vec<i64> {
        tracked_fixture(conn, FIXTURE, archive_game_one)
    }

    /// `tracked` with the first match of another fixture.
    fn tracked_fixture(conn: &mut Connection, fixture: &str, archive_game_one: bool) -> Vec<i64> {
        arena_account(conn);
        let mut match_view = view(ARENA, "arena:Qfoe-id", 1_000);
        match_view.mtgo_match_id = None;
        match_view.local_username = "Qarena#12345".to_string();
        match_view.format = Some("CPIONEER".to_string());
        match_view.match_type = Some("ranked".to_string());
        match_view.games = vec![
            game(1, None, GameResult::Win),
            game(2, None, GameResult::Loss),
            game(3, None, GameResult::Win),
        ];
        commit(
            conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(match_view),
                    revision: 1,
                }],
                now_ms: 2_000,
                ..WriteBatch::default()
            },
        )
        .expect("match");
        name_arena_opponent(conn, "arena:Qfoe-id", "Opponent");
        insert_cards(conn);
        let file_id = arena_log_file(conn);
        let (messages, rooms) = fixture_messages(fixture, file_id);
        commit(
            conn,
            &WriteBatch {
                arena_events: rooms,
                ..WriteBatch::default()
            },
        )
        .expect("room events");
        if archive_game_one {
            let (first, later): (Vec<_>, Vec<_>) = messages
                .into_iter()
                .partition(|message| message.game_number == Some(1));
            commit_messages(conn, first, true);
            commit_messages(conn, later, false);
        } else {
            commit_messages(conn, messages, false);
        }
        game_ids(conn, ARENA)
    }

    fn game_ids(conn: &Connection, token: &str) -> Vec<i64> {
        let mut statement = conn
            .prepare(
                "SELECT g.id FROM games g JOIN matches m ON m.id = g.match_id
                 WHERE m.token = ?1 ORDER BY g.game_number",
            )
            .expect("games");
        statement
            .query_map([token], |row| row.get(0))
            .expect("rows")
            .map(|row| row.expect("id"))
            .collect()
    }

    fn ready(read: ArenaReplayRead) -> ArenaReplay {
        match read {
            ArenaReplayRead::Ready(replay) => replay,
            other => panic!("expected a replay, got {other:?}"),
        }
    }

    #[test]
    fn arena_replay_reads_a_tracked_match() {
        let (_dir, mut conn) = db("arena-replay-tracked");
        let games = tracked(&mut conn, false);
        let replay = ready(arena_replay(&conn, games[1]).expect("read"));
        assert!(
            replay.text.starts_with("[Mtgr \"0.3.0\"]"),
            "{}",
            &replay.text[..40]
        );
        assert_eq!(replay.game_index, 1);
        assert!(replay.text.contains("[Format \"Pioneer\"]"));
        assert!(
            replay
                .text
                .contains("[Event \"Traditional_Explorer_Ladder\"]")
        );
        assert!(replay.text.contains("\"Qarena#12345\""));
        assert!(replay.text.contains("\"Opponent\""));
        assert_eq!(replay.text.matches("[Game \"").count(), 3);
        let first = ready(arena_replay(&conn, games[0]).expect("read"));
        assert_eq!(first.game_index, 0);
        assert_eq!(first.text, replay.text);

        assert!(!replay.cards.is_empty());
        let mut names: Vec<&str> = replay.cards.iter().map(|card| card.name.as_str()).collect();
        let count = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), count, "one entry per name");
        let double_faced: Vec<&str> = replay
            .cards
            .iter()
            .filter(|card| card.double_faced)
            .map(|card| card.name.as_str())
            .collect();
        assert!(
            double_faced.contains(&"Fixture Pathway 73476 // Fixture Pathway 73477")
                && double_faced.contains(&"Reckless Stormseeker // Storm-Charged Slasher"),
            "the cards Arena showed with another side: {double_faced:?}"
        );
        assert!(
            double_faced.iter().all(|name| name.contains(" // ")),
            "a one-faced card is never double-faced: {double_faced:?}"
        );
        for card in &replay.cards {
            assert!(replay.text.contains(&format!("\"{}\"", card.name)));
            assert!(
                card.local_image
                    .as_deref()
                    .is_some_and(|i| i.ends_with(".jpg"))
            );
            assert!(
                card.image
                    .as_deref()
                    .is_some_and(|i| i.starts_with("https://"))
            );
        }
    }

    /// Printings that share a name each keep their own images, by Arena id:
    /// the name's one chosen row is not the printing every copy was.
    #[test]
    fn arena_replay_sends_each_printings_own_images() {
        let (_dir, mut conn) = db("arena-replay-printings");
        let games = tracked(&mut conn, false);
        let replay = ready(arena_replay(&conn, games[0]).expect("read"));
        let name: String = conn
            .query_row(
                "SELECT name FROM cards WHERE catalog_id = ?1",
                [ARENA_ID_OFFSET + 75350],
                |row| row.get(0),
            )
            .expect("a card the match names");
        conn.execute(
            "INSERT INTO cards (catalog_id, name, image, local_image)
             VALUES (?1, ?2, 'https://cards.example/other.jpg', 'other.jpg')",
            rusqlite::params![ARENA_ID_OFFSET + 1, name],
        )
        .expect("another printing of the same name");
        let again = ready(arena_replay(&conn, games[0]).expect("read"));
        let own = again
            .printings
            .iter()
            .find(|printing| printing.grp_id == 75350)
            .expect("the played printing");
        let other = again
            .printings
            .iter()
            .find(|printing| printing.grp_id == 1)
            .expect("the other printing");
        assert_eq!(other.local_image.as_deref(), Some("other.jpg"));
        assert_ne!(own.local_image.as_deref(), Some("other.jpg"));
        assert!(
            own.image
                .as_deref()
                .is_some_and(|image| image.starts_with("https://")),
            "{own:?}"
        );
        assert_eq!(
            again.cards.iter().filter(|card| card.name == name).count(),
            1,
            "the cards stay one per name"
        );
        for printing in &replay.printings {
            assert!(printing.image.is_some() || printing.local_image.is_some());
        }
    }

    #[test]
    fn arena_replay_archived_and_live_read_the_same() {
        let (_dir, mut live_conn) = db("arena-replay-live");
        let live_games = tracked(&mut live_conn, false);
        let (_dir2, mut split_conn) = db("arena-replay-split");
        let split_games = tracked(&mut split_conn, true);
        let archived: i64 = split_conn
            .query_row("SELECT COUNT(*) FROM arena_game_archives", [], |row| {
                row.get(0)
            })
            .expect("archives");
        let live_rows: i64 = split_conn
            .query_row(
                "SELECT COUNT(DISTINCT game_number) FROM arena_game_messages",
                [],
                |row| row.get(0),
            )
            .expect("live");
        assert_eq!(
            (archived, live_rows),
            (1, 2),
            "game 1 archived, 2 and 3 live"
        );
        for index in 0..3 {
            let live = ready(arena_replay(&live_conn, live_games[index]).expect("live"));
            let split = ready(arena_replay(&split_conn, split_games[index]).expect("split"));
            assert_eq!(split, live);
        }
    }

    /// Decision 0048: a card with no card data is written `?` with its
    /// source id; the replay is not refused.
    #[test]
    fn arena_replay_writes_unknown_cards() {
        let (_dir, mut conn) = db("arena-replay-unknown");
        let games = tracked(&mut conn, false);
        conn.execute(
            "DELETE FROM cards WHERE catalog_id = ?1",
            [ARENA_ID_OFFSET + 75350],
        )
        .expect("delete");
        let replay = ready(arena_replay(&conn, games[0]).expect("read"));
        assert!(replay.text.contains("main 4 ? src arena:75350"));
        assert!(!replay.text.contains("Youthful Valkyrie"));
        assert!(
            !replay
                .cards
                .iter()
                .any(|card| card.name == "Youthful Valkyrie")
        );
    }

    /// The one refusal left: card data that names a multi-faced card
    /// without its faces (`face` has no `?` form).
    #[test]
    fn arena_replay_refuses_a_face_with_no_name() {
        let (_dir, mut conn) = db("arena-replay-face");
        let games = tracked(&mut conn, false);
        conn.execute(
            "UPDATE cards SET name = 'Fixture Piper' WHERE catalog_id = ?1",
            [ARENA_ID_OFFSET + 79017],
        )
        .expect("rename");
        assert_eq!(
            arena_replay(&conn, games[0]).expect("read"),
            ArenaReplayRead::MissingCardData
        );
        match arena_match_text(&conn, ARENA).expect("read") {
            ArenaMatchRead::MissingCardData(grp_ids) => assert_eq!(grp_ids, vec![79017]),
            other => panic!("expected missing card data, got {other:?}"),
        }
    }

    const LOSS: &str = "bo3-loss-and-match-concede";

    /// The written match of the loss fixture (its Map tokens).
    fn loss_text(conn: &Connection) -> ArenaMatchText {
        match arena_match_text(conn, ARENA).expect("read") {
            ArenaMatchRead::Ready(text) => text,
            other => panic!("expected a record, got {other:?}"),
        }
    }

    /// A token with no card data by its grpId takes the art of a token row
    /// with its Scryfall name (its subtypes: "Map"); the record keeps "Map
    /// Token" (CR 111.4).
    #[test]
    fn arena_replay_tokens_take_token_art_by_name() {
        let (_dir, mut conn) = db("arena-replay-token-art");
        tracked_fixture(&mut conn, LOSS, false);
        conn.execute(
            "INSERT INTO cards (catalog_id, name, type, rarity, image, local_image)
             VALUES (90001, 'Map', 'Token Artifact', 'common', 'https://cards.example/map.jpg', 'map.jpg'),
                    (90002, 'Map', 'Instant', 'common', 'https://cards.example/not-a-token.jpg', NULL)",
            [],
        )
        .expect("token rows");
        let written = loss_text(&conn);
        assert!(written.text.contains("\"Map Token\""));
        let map = written
            .cards
            .iter()
            .find(|card| card.name == "Map Token")
            .expect("the token has art");
        assert_eq!(map.image.as_deref(), Some("https://cards.example/map.jpg"));
        assert_eq!(map.local_image.as_deref(), Some("map.jpg"));
        assert!(
            !written
                .tokens_to_fetch
                .iter()
                .any(|token| token.name == "Map"),
            "found, so not fetched"
        );
    }

    #[test]
    fn arena_card_rows_leave_endstep_cards_out() {
        let (_dir, conn) = db("arena-replay-endstep-rows");
        conn.execute(
            "INSERT INTO cards (catalog_id, name) VALUES (?1, 'Qarena'), (?2, 'Qendstep')",
            params![ARENA_ID_OFFSET + 7, cards::endstep_catalog_id(1)],
        )
        .expect("cards");
        let rows = arena_card_rows(&conn).expect("rows");
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, vec!["Qarena"]);
    }

    /// With no token row by that name, the token is queued for the API's
    /// token-by-name fetch as its Arena row (`rarity = 'token'`), once: a
    /// row for its grpId already queued is not queued again. That row never
    /// names the token in the record.
    #[test]
    fn arena_replay_queues_tokens_without_art() {
        let (_dir, mut conn) = db("arena-replay-token-queue");
        tracked_fixture(&mut conn, LOSS, false);
        let written = loss_text(&conn);
        assert!(!written.cards.iter().any(|card| card.name == "Map Token"));
        let map = written
            .tokens_to_fetch
            .iter()
            .find(|token| token.name == "Map")
            .expect("queued by its Scryfall name")
            .clone();
        assert!(map.catalog_id > ARENA_ID_OFFSET);
        assert_eq!(map.sub_type.as_deref(), Some("Map"));

        conn.execute(
            "INSERT INTO cards (catalog_id, name, type, rarity) VALUES (?1, 'Map', 'Token', 'token')",
            [map.catalog_id],
        )
        .expect("the queued row");
        let again = loss_text(&conn);
        assert!(
            !again
                .tokens_to_fetch
                .iter()
                .any(|token| token.name == "Map")
        );
        assert_eq!(again.text, written.text, "the token row names nothing");
    }

    #[test]
    fn arena_replay_of_an_mtgo_game_is_not_found() {
        let (_dir, mut conn) = db("arena-replay-mtgo");
        tracked(&mut conn, false);
        commit(
            &mut conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(view(TOKEN, "Qfoe", 1_000)),
                    revision: 1,
                }],
                now_ms: 2_000,
                ..WriteBatch::default()
            },
        )
        .expect("mtgo match");
        let mtgo_games = game_ids(&conn, TOKEN);
        assert!(!mtgo_games.is_empty());
        assert_eq!(
            arena_replay(&conn, mtgo_games[0]).expect("read"),
            ArenaReplayRead::NotFound
        );
        assert_eq!(
            arena_replay(&conn, 999_999).expect("read"),
            ArenaReplayRead::NotFound
        );
    }

    #[test]
    fn room_event_skips_a_corrupt_row() {
        let (_dir, conn) = db("arena-room-event-corrupt");
        let file = arena_log_file(&conn);
        let insert = |offset: i64, raw: &[u8], compressed: bool| {
            conn.execute(
                "INSERT INTO arena_log_events (arena_log_file_id, byte_offset, seq, kind,
                     match_token, logged_at, raw, compressed)
                 VALUES (?1, ?2, 0, 'room_state', 'tok', 1, ?3, ?4)",
                params![file, offset, raw, compressed],
            )
            .expect("event");
        };
        insert(1, b"not json", false);
        insert(2, b"not zstd", true);
        insert(
            3,
            br#"{"players":[{"event_id":"Traditional_Ladder"}]}"#,
            false,
        );
        assert_eq!(
            room_event(&conn, "tok").expect("event"),
            Some("Traditional_Ladder".to_string())
        );
    }

    // ---- Pulled from another device (cross-device sync spec section 7) ----

    /// The tracked fixture match recorded on device A, pulled into device
    /// B (which holds the same card table). Returns B's game ids.
    fn pulled_fixture(here: &Connection, there: &mut Connection) -> Vec<i64> {
        insert_cards(there);
        pull_into(there, pulled_from(here, ARENA), 9_000);
        game_ids(there, ARENA)
    }

    fn match_text(conn: &Connection) -> ArenaMatchText {
        match arena_match_text(conn, ARENA).expect("read") {
            ArenaMatchRead::Ready(text) => text,
            other => panic!("expected a record, got {other:?}"),
        }
    }

    #[test]
    fn pulled_arena_match_replay_equals_recorded() {
        let (_a, mut here) = db("arena-replay-pulled-here");
        let recorded_games = tracked(&mut here, false);
        let (_b, mut there) = db("arena-replay-pulled-there");
        let pulled_games = pulled_fixture(&here, &mut there);
        assert_eq!(
            number(
                &there,
                "SELECT COUNT(*) FROM matches WHERE source = 'imported'"
            ),
            1
        );
        let recorded = match_text(&here);
        let pulled = match_text(&there);
        assert!(
            pulled
                .text
                .contains("[Event \"Traditional_Explorer_Ladder\"]"),
            "the event comes with the source"
        );
        assert_eq!(pulled.text, recorded.text);
        assert_eq!(pulled.games, recorded.games);
        for index in 0..recorded_games.len() {
            let recorded = ready(arena_replay(&here, recorded_games[index]).expect("here"));
            let pulled = ready(arena_replay(&there, pulled_games[index]).expect("there"));
            assert_eq!(pulled.text, recorded.text);
            assert_eq!(pulled.game_index, recorded.game_index);
        }
        // The match page offers the replay and the game log of each game.
        assert_eq!(
            crate::arena_games::game_numbers_with_messages(&there, ARENA).expect("there"),
            crate::arena_games::game_numbers_with_messages(&here, ARENA).expect("here"),
        );
    }

    #[test]
    fn pulled_arena_game_log_equals_recorded() {
        let (_a, mut here) = db("arena-log-pulled-here");
        tracked(&mut here, false);
        let (_b, mut there) = db("arena-log-pulled-there");
        pulled_fixture(&here, &mut there);
        for game_number in 1..=3 {
            let recorded =
                crate::arena_game_sync::game_log_lines(&here, ARENA, game_number).expect("here");
            assert!(!recorded.is_empty(), "game {game_number}");
            assert_eq!(
                crate::arena_game_sync::game_log_lines(&there, ARENA, game_number).expect("there"),
                recorded,
                "game {game_number}"
            );
        }
    }

    #[test]
    fn quoted_strings_undo_escapes() {
        let text = "[Player1 \"Tag\"]\n0:01 #1 \"A \\\"B\\\"\" cast from hand face \"C\"\n";
        assert_eq!(
            quoted_strings(text),
            BTreeSet::from(["A \"B\"".to_string(), "C".to_string()])
        );
    }
}
