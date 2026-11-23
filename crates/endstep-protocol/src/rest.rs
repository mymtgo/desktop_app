//! REST bodies: the match, its lobby, a deck and a ranked result.
//!
//! Only object bodies at the four known paths decode; anything else (an
//! array body, a gateway note like `{"owner":"gw-4"}`) is `Other`.

use serde_json::Value;

use crate::frame::FrameKind;
use crate::json;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Participant {
    pub user_id: String,
    pub username: String,
    /// `isBot` or `isAi` is true. A missing key means a human.
    pub bot: bool,
}

/// `GET /api/matches/<id>`: only a body with `forgeGameId` is a match body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchBody {
    pub id: String,
    pub db_id: Option<String>,
    pub lobby_id: Option<String>,
    pub forge_game_id: String,
    pub format_id: Option<String>,
    pub ranked: bool,
    pub games_per_match: Option<u32>,
    pub started_at_ms: Option<i64>,
    pub my_player_index: Option<u32>,
    pub my_deck: Vec<String>,
    pub my_sideboard: Vec<String>,
    /// (lower-case card name, set code, collector number), sorted by name.
    pub my_deck_art: Vec<(String, String, String)>,
    pub participants: Vec<Participant>,
}

/// `GET /api/lobbies/<id>`: `config.seatDecks` as (user id, deck id), sorted
/// by user id. A hidden seat keeps its `hidden-<id>` key and `hidden-deck` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LobbyBody {
    pub id: String,
    pub seat_decks: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckLine {
    pub name: String,
    pub set_code: Option<String>,
    pub collector_number: Option<String>,
    pub quantity: u32,
}

/// `GET /api/decks/<id>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckBody {
    pub id: String,
    pub name: String,
    pub format_id: Option<String>,
    pub main: Vec<DeckLine>,
    pub side: Vec<DeckLine>,
}

/// `GET /api/ranked/results/<db id>`: the id comes from the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankedResult {
    pub db_id: String,
    pub status: Option<String>,
    pub format_id: Option<String>,
    pub rating_before: Option<i64>,
    pub rating_after: Option<i64>,
    pub score_before: Option<i64>,
    pub score_after: Option<i64>,
    pub delta: Option<i64>,
    pub rd_before: Option<i64>,
    pub rd_after: Option<i64>,
    pub rank: Option<String>,
}

pub(crate) fn decode_rest(url: &str, body: &Value) -> FrameKind {
    if !body.is_object() {
        return FrameKind::Other;
    }
    let path = rest_path(url);
    if id_after(&path, "/api/matches/").is_some() {
        return match match_body(body) {
            Some(found) => FrameKind::MatchBody(found),
            None => FrameKind::Other,
        };
    }
    if id_after(&path, "/api/lobbies/").is_some() {
        return match lobby_body(body) {
            Some(found) => FrameKind::LobbyBody(found),
            None => FrameKind::Other,
        };
    }
    if id_after(&path, "/api/decks/").is_some() {
        return match deck_body(body) {
            Some(found) => FrameKind::DeckBody(found),
            None => FrameKind::Other,
        };
    }
    if let Some(db_id) = id_after(&path, "/api/ranked/results/") {
        return match ranked_result(db_id, body) {
            Some(found) => FrameKind::RankedResult(found),
            None => FrameKind::Other,
        };
    }
    FrameKind::Other
}

/// The path of a captured URL: the extension stores paths, but a full URL
/// keeps only its path. The query string is dropped.
fn rest_path(url: &str) -> String {
    let mut path = url;
    if let Some(scheme_end) = path.find("://") {
        let rest = &path[scheme_end + 3..];
        path = match rest.find('/') {
            Some(slash) => &rest[slash..],
            None => "",
        };
    }
    if let Some(query) = path.find(['?', '#']) {
        path = &path[..query];
    }
    path.to_string()
}

/// The one path segment after `prefix`, when the path is exactly
/// `<prefix><id>` with a non-empty id.
fn id_after(path: &str, prefix: &str) -> Option<String> {
    let id = path.strip_prefix(prefix)?;
    if id.is_empty() || id.contains('/') {
        return None;
    }
    Some(id.to_string())
}

/// A match body needs `id` and `forgeGameId`.
fn match_body(body: &Value) -> Option<MatchBody> {
    let id = json::string(body, "id")?;
    let forge_game_id = json::string(body, "forgeGameId")?;
    Some(MatchBody {
        id,
        db_id: json::string(body, "dbId"),
        lobby_id: json::string(body, "lobbyId"),
        forge_game_id,
        format_id: json::string(body, "formatId"),
        ranked: json::flag(body, "ranked"),
        games_per_match: json::u32_of(body.get("gamesPerMatch")),
        started_at_ms: json::string(body, "startedAt").and_then(|text| iso_ms(&text)),
        my_player_index: json::u32_of(body.get("myPlayerIndex")),
        my_deck: json::strings(body.get("myDeck")),
        my_sideboard: json::strings(body.get("mySideboard")),
        my_deck_art: deck_art(body.get("myDeckArt")),
        participants: participants(body.get("participants")),
    })
}

/// An ISO 8601 time with an offset (`2026-11-10T11:41:49.543Z`) as UTC ms.
fn iso_ms(text: &str) -> Option<i64> {
    let time = chrono::DateTime::parse_from_rfc3339(text.trim()).ok()?;
    Some(time.timestamp_millis())
}

/// `myDeckArt` `{lower-case name: {setCode, collectorNumber}}`; entries
/// missing either field are skipped.
fn deck_art(value: Option<&Value>) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    if let Some(Value::Object(entries)) = value {
        for (name, art) in entries {
            let set = json::string(art, "setCode");
            let number = json::string(art, "collectorNumber");
            if let (Some(set), Some(number)) = (set, number) {
                out.push((name.clone(), set, number));
            }
        }
    }
    out.sort();
    out
}

fn participants(value: Option<&Value>) -> Vec<Participant> {
    let mut out = Vec::new();
    if let Some(Value::Array(items)) = value {
        for item in items {
            let user_id = json::string(item, "userId");
            let username = json::string(item, "username");
            if let (Some(user_id), Some(username)) = (user_id, username) {
                let bot = json::flag(item, "isBot") || json::flag(item, "isAi");
                out.push(Participant {
                    user_id,
                    username,
                    bot,
                });
            }
        }
    }
    out
}

/// A lobby body needs its `id`; seat decks are the string entries of
/// `config.seatDecks`.
fn lobby_body(body: &Value) -> Option<LobbyBody> {
    let id = json::string(body, "id")?;
    let mut seat_decks = Vec::new();
    let seats = body
        .get("config")
        .and_then(|config| config.get("seatDecks"));
    if let Some(Value::Object(entries)) = seats {
        for (user_id, deck_id) in entries {
            if let Some(deck_id) = deck_id.as_str() {
                seat_decks.push((user_id.clone(), deck_id.to_string()));
            }
        }
    }
    seat_decks.sort();
    Some(LobbyBody { id, seat_decks })
}

/// A deck body needs its `id`. A missing name is empty.
fn deck_body(body: &Value) -> Option<DeckBody> {
    let id = json::string(body, "id")?;
    Some(DeckBody {
        id,
        name: json::string(body, "name").unwrap_or_default(),
        format_id: json::string(body, "formatId"),
        main: deck_lines(body.get("cards")),
        side: deck_lines(body.get("sideboard")),
    })
}

/// Lines need a name; a missing quantity counts as one copy.
fn deck_lines(value: Option<&Value>) -> Vec<DeckLine> {
    let mut out = Vec::new();
    if let Some(Value::Array(items)) = value {
        for item in items {
            let Some(name) = json::string(item, "name") else {
                continue;
            };
            out.push(DeckLine {
                name,
                set_code: json::string(item, "setCode"),
                collector_number: json::string(item, "collectorNumber"),
                quantity: json::u32_of(item.get("quantity")).unwrap_or(1),
            });
        }
    }
    out
}

/// A ranked result needs a `you` object (an error body has none); the numbers
/// come from it. `rank` keeps a string as it is and any other non-null value
/// as its JSON text.
fn ranked_result(db_id: String, body: &Value) -> Option<RankedResult> {
    let you = body.get("you").filter(|you| you.is_object())?;
    let number = |key: &str| json::i64_of(you.get(key));
    let rank = match you.get("rank") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text.clone()),
        Some(other) => Some(other.to_string()),
    };
    Some(RankedResult {
        db_id,
        status: json::string(body, "status"),
        format_id: json::string(body, "formatId"),
        rating_before: number("ratingBefore"),
        rating_after: number("ratingAfter"),
        score_before: number("scoreBefore"),
        score_after: number("scoreAfter"),
        delta: number("delta"),
        rd_before: number("rdBefore"),
        rd_after: number("rdAfter"),
        rank,
    })
}
