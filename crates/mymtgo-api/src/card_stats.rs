//! Card stats (survey section 10, 07 spec section 8, ruling 3), shipped
//! while `CARD_STATS_LIVE` is on (on since #295 was fixed). One payload per
//! game from the local side's `game_cards` and `game_card_casts` (0.x
//! `BuildCardStatsGamePayload`), shipped in gzip chunks of 100 to
//! `POST /api/card-stats/report`.

use serde_json::{Map, Value, json};

use crate::gate::RequestClass;
use crate::outbox::{ClaimedRow, NewOutboxRow, OutboxKind, pack_payload, readable_rows};
use crate::request::{ApiRequest, AuthMode, Body, Method, gzip};

pub const CARD_STATS_PATH: &str = "/api/card-stats/report";

/// Games per request (0.x `HTTP_CHUNK`).
pub const CARD_STATS_CHUNK: usize = 100;

/// The alternative cast methods, in 0.x's payload order (`game_card_casts.method`).
/// A test keeps it equal to `sync_bundles::match_bundle::CAST_METHODS`.
pub const CAST_METHODS: [&str; 23] = [
    "kicked",
    "flashback",
    "madness",
    "evoked",
    "warp",
    "free_cast",
    "bargained",
    "dashed",
    "bestowed",
    "replicated",
    "spectacle",
    "rebound",
    "escaped",
    "ninjutsu",
    "suspended",
    "buyback",
    "disturb",
    "foretold",
    "retraced",
    "mayhem",
    "miracle",
    "gifted",
    "casualty",
];

/// One local card of one game: its `game_cards` row, its casts and its
/// card's oracle id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardStatsCard {
    /// `cards.oracle_id`; a card without one is left out.
    pub oracle_id: Option<String>,
    /// `main_qty`.
    pub quantity: i64,
    /// `starting_hand`.
    pub kept: i64,
    pub seen: i64,
    pub hand_seen: i64,
    pub graveyard_seen: i64,
    pub exile_seen: i64,
    pub battlefield_seen: i64,
    pub discarded: i64,
    pub cast_turn: Option<i64>,
    pub has_zone_data: bool,
    pub cast: i64,
    pub played: Option<i64>,
    pub activated: Option<i64>,
    /// `(method, count)` from `game_card_casts`.
    pub casts: Vec<(String, i64)>,
    pub sided_in: i64,
    pub sided_out: i64,
    pub pregame_revealed: i64,
    pub pregame_played: i64,
}

/// One game, as v1 stores it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardStatsGame {
    pub match_token: String,
    pub mtgo_game_id: i64,
    /// The match's local player (its account).
    pub local_player: Option<String>,
    pub player_archetype_uuid: Option<String>,
    pub opponent_archetype_uuid: Option<String>,
    pub format: Option<String>,
    /// `games.result = 'win'`.
    pub won: bool,
    pub on_play: Option<bool>,
    pub game_number: i64,
    pub game_started_at_ms: Option<i64>,
    pub match_started_at_ms: Option<i64>,
    pub cards: Vec<CardStatsCard>,
}

/// The payload of one game (survey section 10), or `None` when there is no
/// local player, player archetype uuid, format, date or card (the caller
/// logs `card_stats_skip_reason`).
pub fn card_stats_payload(game: &CardStatsGame) -> Option<Value> {
    build_payload(game).ok()
}

/// Why a game has no payload (0.x's warning reasons), or `None` when it has
/// one.
pub fn card_stats_skip_reason(game: &CardStatsGame) -> Option<&'static str> {
    build_payload(game).err()
}

fn build_payload(game: &CardStatsGame) -> Result<Value, &'static str> {
    if game.local_player.is_none() {
        return Err("no local player");
    }
    let Some(player_archetype_uuid) = &game.player_archetype_uuid else {
        return Err("no player archetype uuid");
    };
    let Some(format) = game.format.as_deref().filter(|format| !format.is_empty()) else {
        return Err("no match format");
    };
    let started_at = game.game_started_at_ms.or(game.match_started_at_ms);
    let Some(played_on) = started_at.and_then(chrono::DateTime::from_timestamp_millis) else {
        return Err("no played_on date");
    };
    let cards: Vec<Value> = by_oracle(&game.cards).iter().map(card_payload).collect();
    if cards.is_empty() {
        return Err("no player-side card stats");
    }
    Ok(json!({
        "player_archetype_uuid": player_archetype_uuid,
        "opponent_archetype_uuid": game.opponent_archetype_uuid,
        "format": format,
        "won": game.won,
        "on_play": game.on_play.unwrap_or(false),
        "is_postboard": game.game_number > 1,
        "played_on": played_on.format("%Y-%m-%d").to_string(),
        "cards": cards,
    }))
}

/// The cards with an oracle id, one per oracle id in first-seen order (0.x
/// `card_game_stats` keyed rows by oracle id, so two printings of one card
/// were one row): counts added, the earliest cast turn, a flag when either
/// printing has it.
fn by_oracle(cards: &[CardStatsCard]) -> Vec<CardStatsCard> {
    let mut merged: Vec<CardStatsCard> = Vec::new();
    for card in cards {
        let Some(oracle_id) = card.oracle_id.as_deref().filter(|id| !id.is_empty()) else {
            continue;
        };
        let found = merged
            .iter_mut()
            .find(|seen| seen.oracle_id.as_deref() == Some(oracle_id));
        match found {
            Some(seen) => add_printing(seen, card),
            None => merged.push(card.clone()),
        }
    }
    merged
}

/// Add another printing of the same oracle into `total`, as the payload
/// does (`by_oracle`): counts added, the earliest cast turn, a flag when
/// either has it.
pub fn add_printing(total: &mut CardStatsCard, card: &CardStatsCard) {
    total.quantity += card.quantity;
    total.kept += card.kept;
    total.seen += card.seen;
    total.hand_seen += card.hand_seen;
    total.graveyard_seen += card.graveyard_seen;
    total.exile_seen += card.exile_seen;
    total.battlefield_seen += card.battlefield_seen;
    total.discarded += card.discarded;
    total.cast_turn = match (total.cast_turn, card.cast_turn) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    total.has_zone_data = total.has_zone_data || card.has_zone_data;
    total.cast += card.cast;
    total.played = add_optional(total.played, card.played);
    total.activated = add_optional(total.activated, card.activated);
    for (method, count) in &card.casts {
        match total.casts.iter_mut().find(|(seen, _)| seen == method) {
            Some((_, total_count)) => *total_count += count,
            None => total.casts.push((method.clone(), *count)),
        }
    }
    total.sided_in += card.sided_in;
    total.sided_out += card.sided_out;
    total.pregame_revealed += card.pregame_revealed;
    total.pregame_played += card.pregame_played;
}

fn add_optional(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
    }
}

fn card_payload(card: &CardStatsCard) -> Value {
    let oracle_id = card.oracle_id.clone().unwrap_or_default();
    let mut fields = Map::new();
    let mut put = |key: &str, value: Value| {
        fields.insert(key.to_string(), value);
    };
    put("oracle_id", json!(oracle_id));
    put("quantity", json!(card.quantity));
    put("kept", json!(card.kept));
    put("seen", json!(card.seen));
    put("hand_seen", json!(card.hand_seen));
    put("graveyard_seen", json!(card.graveyard_seen));
    put("exile_seen", json!(card.exile_seen));
    put("battlefield_seen", json!(card.battlefield_seen));
    put("discarded", json!(card.discarded));
    put("cast_turn", json!(card.cast_turn));
    // 0.x measured zones only for the cards in the game's deck: its
    // zero-copy rows (sided out, left in the sideboard) carried no zone data.
    put(
        "has_zone_data",
        json!(card.has_zone_data && card.quantity > 0),
    );
    put("cast", json!(card.cast));
    // 0.x `(int) $stat->played`: a missing count is 0.
    put("played", json!(card.played.unwrap_or(0)));
    for method in CAST_METHODS {
        let count = card
            .casts
            .iter()
            .find(|(cast, _)| cast == method)
            .map_or(0, |(_, count)| *count);
        put(method, json!(count));
    }
    put("activated", json!(card.activated.unwrap_or(0)));
    put("sided_in", json!(card.sided_in > 0));
    put("sided_out", json!(card.sided_out > 0));
    put("pregame_revealed", json!(card.pregame_revealed > 0));
    put("pregame_played", json!(card.pregame_played > 0));
    Value::Object(fields)
}

/// The outbox row of a game: subject `"{match token}:{mtgo game id}"`,
/// payload the zstd JSON. `None` when the game is skipped.
pub fn card_stats_row(game: &CardStatsGame, now_ms: i64) -> Option<NewOutboxRow> {
    let payload = card_stats_payload(game)?;
    Some(NewOutboxRow {
        kind: OutboxKind::CardStats,
        subject: format!("{}:{}", game.match_token, game.mtgo_game_id),
        payload: Some(pack_payload(&payload)?),
        now_ms,
    })
}

/// The requests for a claim: `{"games": [...]}` gzipped, 100 games each
/// (0.x `ShipCardStats`), class `Community`, 30 s total and 10 s connect.
/// A row whose stored payload cannot be read is left out (the worker fails
/// it as unreadable).
pub fn card_stats_requests(rows: &[ClaimedRow]) -> Vec<(Vec<String>, ApiRequest)> {
    let readable = readable_rows(rows);
    let mut requests = Vec::new();
    for chunk in readable.chunks(CARD_STATS_CHUNK) {
        let games: Vec<&Value> = chunk.iter().map(|(_, game)| game).collect();
        let json = serde_json::to_vec(&json!({ "games": games })).unwrap_or_default();
        let mut request = ApiRequest::new(
            Method::Post,
            CARD_STATS_PATH,
            RequestClass::Community,
            AuthMode::DeviceOrBearer,
        );
        request.body = Body::GzipJson(gzip(&json));
        let subjects = chunk.iter().map(|(subject, _)| subject.clone()).collect();
        requests.push((subjects, request));
    }
    requests
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::RequestClass;
    use crate::outbox::ClaimedRow;
    use crate::request::{AuthMode, Body, Method};
    use crate::test_support::gunzip;
    use serde_json::{Value, json};

    #[test]
    fn cast_methods_match_the_sync_bundles_list() {
        assert_eq!(CAST_METHODS, sync_bundles::match_bundle::CAST_METHODS);
    }

    /// 2026-05-19T23:30:00Z: late evening in the Americas.
    const LATE: i64 = 1_779_233_400_000;

    fn card(oracle_id: Option<&str>) -> CardStatsCard {
        CardStatsCard {
            oracle_id: oracle_id.map(str::to_string),
            quantity: 4,
            kept: 1,
            seen: 2,
            hand_seen: 0,
            graveyard_seen: 0,
            exile_seen: 0,
            battlefield_seen: 0,
            discarded: 0,
            cast_turn: Some(3),
            has_zone_data: false,
            cast: 1,
            played: Some(0),
            activated: None,
            casts: vec![("kicked".to_string(), 1)],
            sided_in: 0,
            sided_out: 2,
            pregame_revealed: 0,
            pregame_played: 1,
        }
    }

    /// 0.x `CardStatsTelemetryFactory::make()`: one eligible game.
    fn game() -> CardStatsGame {
        CardStatsGame {
            match_token: "q-match-0001".to_string(),
            mtgo_game_id: 959_764_088,
            local_player: Some("Qplayer".to_string()),
            player_archetype_uuid: Some("aaaaaaaa-1111-4111-8111-111111111111".to_string()),
            opponent_archetype_uuid: Some("bbbbbbbb-2222-4222-8222-222222222222".to_string()),
            format: Some("CStandard".to_string()),
            won: true,
            on_play: Some(true),
            game_number: 1,
            game_started_at_ms: Some(LATE),
            match_started_at_ms: Some(LATE - 60_000),
            cards: vec![card(Some("q-oracle-1"))],
        }
    }

    fn payload(game: &CardStatsGame) -> Value {
        card_stats_payload(game).expect("a payload")
    }

    // 0.x EnqueueCardStatsTest "stores serialized payload at enqueue time".
    #[test]
    fn stores_the_payload_with_format_and_player_archetype() {
        let sent = payload(&game());
        assert_eq!(sent["format"], json!("CStandard"));
        assert_eq!(
            sent["player_archetype_uuid"],
            json!("aaaaaaaa-1111-4111-8111-111111111111")
        );
        assert_eq!(
            sent["opponent_archetype_uuid"],
            json!("bbbbbbbb-2222-4222-8222-222222222222")
        );
        assert_eq!(sent["won"], json!(true));
        assert_eq!(sent["on_play"], json!(true));
        assert_eq!(sent["is_postboard"], json!(false));
    }

    // 0.x BuildCardStatsGamePayload: the five skips.
    #[test]
    fn skips_without_a_local_player_archetype_format_date_or_cards() {
        let mut no_player = game();
        no_player.local_player = None;
        let mut no_archetype = game();
        no_archetype.player_archetype_uuid = None;
        let mut no_format = game();
        no_format.format = None;
        let mut no_date = game();
        no_date.game_started_at_ms = None;
        no_date.match_started_at_ms = None;
        let mut no_cards = game();
        no_cards.cards.clear();
        let mut only_unknown_cards = game();
        only_unknown_cards.cards = vec![card(None)];
        for (name, skipped) in [
            ("player", no_player),
            ("archetype", no_archetype),
            ("format", no_format),
            ("date", no_date),
            ("cards", no_cards),
            ("oracle ids", only_unknown_cards),
        ] {
            assert_eq!(card_stats_payload(&skipped), None, "{name}");
            assert!(card_stats_skip_reason(&skipped).is_some(), "{name}");
        }
        assert_eq!(card_stats_skip_reason(&game()), None);
        // The opponent's archetype may be missing.
        let mut no_opponent = game();
        no_opponent.opponent_archetype_uuid = None;
        assert_eq!(
            payload(&no_opponent)["opponent_archetype_uuid"],
            Value::Null
        );
    }

    #[test]
    fn played_on_is_the_utc_date_of_the_game_else_the_match() {
        assert_eq!(payload(&game())["played_on"], json!("2026-05-19"));
        let mut late = game();
        late.game_started_at_ms = Some(LATE + 3_600_000);
        assert_eq!(payload(&late)["played_on"], json!("2026-05-20"));
        let mut no_game_start = game();
        no_game_start.game_started_at_ms = None;
        no_game_start.match_started_at_ms = Some(LATE - 86_400_000);
        assert_eq!(payload(&no_game_start)["played_on"], json!("2026-05-18"));
    }

    #[test]
    fn a_later_game_is_postboard() {
        let mut second = game();
        second.game_number = 2;
        assert_eq!(payload(&second)["is_postboard"], json!(true));
    }

    #[test]
    fn each_card_has_the_36_columns_and_a_card_without_an_oracle_id_is_left_out() {
        let mut two = game();
        two.cards.push(card(None));
        let sent = payload(&two);
        let cards = sent["cards"].as_array().expect("cards");
        assert_eq!(cards.len(), 1);
        let first = cards[0].as_object().expect("card");
        assert_eq!(first.len(), 41, "oracle_id plus 40 columns: {first:?}");
        assert_eq!(cards[0]["oracle_id"], json!("q-oracle-1"));
        assert_eq!(cards[0]["quantity"], json!(4));
        assert_eq!(cards[0]["kept"], json!(1));
        assert_eq!(cards[0]["seen"], json!(2));
        assert_eq!(cards[0]["cast_turn"], json!(3));
        assert_eq!(cards[0]["has_zone_data"], json!(false));
        assert_eq!(cards[0]["cast"], json!(1));
        assert_eq!(cards[0]["played"], json!(0));
        // 0.x `(int) null`.
        assert_eq!(cards[0]["activated"], json!(0));
        assert_eq!(cards[0]["kicked"], json!(1));
        assert_eq!(cards[0]["flashback"], json!(0));
        assert_eq!(cards[0]["casualty"], json!(0));
        assert_eq!(cards[0]["sided_in"], json!(false));
        assert_eq!(cards[0]["sided_out"], json!(true));
        assert_eq!(cards[0]["pregame_revealed"], json!(false));
        assert_eq!(cards[0]["pregame_played"], json!(true));
        for method in CAST_METHODS {
            assert!(first.contains_key(method), "{method}");
        }
        let mut no_turn = game();
        no_turn.cards[0].cast_turn = None;
        assert_eq!(payload(&no_turn)["cards"][0]["cast_turn"], Value::Null);
    }

    // Plan S task 11 (the 0.x golden): 0.x's `card_game_stats` has one row
    // per oracle id, so two printings of one card are sent as one card:
    // counts added, the earliest cast turn, a flag when either has it.
    #[test]
    fn printings_of_one_oracle_are_sent_as_one_card() {
        let mut two = game();
        let mut other = card(Some("q-oracle-1"));
        other.quantity = 2;
        other.kept = 0;
        other.seen = 1;
        other.hand_seen = 1;
        other.cast_turn = Some(2);
        other.has_zone_data = true;
        other.played = None;
        other.activated = Some(3);
        other.casts = vec![("kicked".to_string(), 2), ("evoked".to_string(), 1)];
        other.sided_out = 0;
        other.sided_in = 1;
        other.pregame_played = 0;
        two.cards.push(card(Some("q-oracle-2")));
        two.cards.push(other);
        let sent = payload(&two);
        let cards = sent["cards"].as_array().expect("cards");
        assert_eq!(cards.len(), 2, "{cards:?}");
        let merged = &cards[0];
        assert_eq!(merged["oracle_id"], json!("q-oracle-1"));
        assert_eq!(merged["quantity"], json!(6));
        assert_eq!(merged["kept"], json!(1));
        assert_eq!(merged["seen"], json!(3));
        assert_eq!(merged["hand_seen"], json!(1));
        assert_eq!(merged["cast_turn"], json!(2));
        assert_eq!(merged["has_zone_data"], json!(true));
        assert_eq!(merged["cast"], json!(2));
        assert_eq!(merged["played"], json!(0));
        assert_eq!(merged["activated"], json!(3));
        assert_eq!(merged["kicked"], json!(3));
        assert_eq!(merged["evoked"], json!(1));
        assert_eq!(merged["sided_in"], json!(true));
        assert_eq!(merged["sided_out"], json!(true));
        assert_eq!(merged["pregame_played"], json!(true));
        assert_eq!(cards[1]["oracle_id"], json!("q-oracle-2"));
    }

    // Plan S task 11 (the 0.x golden): 0.x measured zones only for the
    // cards in the game's deck; its zero-copy rows (sided out, sideboard)
    // say "never measured".
    #[test]
    fn a_card_with_no_copies_sends_no_zone_data() {
        let mut zero = game();
        zero.cards[0].has_zone_data = true;
        zero.cards[0].quantity = 0;
        assert_eq!(payload(&zero)["cards"][0]["has_zone_data"], json!(false));
        zero.cards[0].quantity = 1;
        assert_eq!(payload(&zero)["cards"][0]["has_zone_data"], json!(true));
    }

    #[test]
    fn the_subject_is_the_match_token_and_game_id() {
        let row = card_stats_row(&game(), 55).expect("a row");
        assert_eq!(row.kind, crate::outbox::OutboxKind::CardStats);
        assert_eq!(row.subject, "q-match-0001:959764088");
        assert_eq!(row.now_ms, 55);
        let stored = zstd::decode_all(row.payload.expect("payload").as_slice()).expect("zstd");
        assert_eq!(
            serde_json::from_slice::<Value>(&stored).expect("json"),
            payload(&game())
        );
        let mut skipped = game();
        skipped.format = None;
        assert_eq!(card_stats_row(&skipped, 55), None);
    }

    fn claimed(count: usize) -> Vec<ClaimedRow> {
        (0..count)
            .map(|index| {
                let row = card_stats_row(&game(), 1).expect("row");
                ClaimedRow {
                    subject: format!("q-match:{index}"),
                    payload: row.payload.expect("payload"),
                }
            })
            .collect()
    }

    fn games_sent(request: &ApiRequest) -> Vec<Value> {
        let Body::GzipJson(bytes) = &request.body else {
            panic!("a gzip body: {:?}", request.body);
        };
        let body: Value = serde_json::from_slice(&gunzip(bytes)).expect("json");
        body["games"].as_array().expect("games").clone()
    }

    // 0.x ShipCardStatsTest "sends gzipped body with auth headers".
    #[test]
    fn sends_a_gzipped_body_to_the_report_path() {
        let requests = card_stats_requests(&claimed(1));
        assert_eq!(requests.len(), 1);
        let request = &requests[0].1;
        assert_eq!(request.method, Method::Post);
        assert_eq!(request.path, "/api/card-stats/report");
        assert_eq!(request.class, RequestClass::Community);
        assert_eq!(request.auth, AuthMode::DeviceOrBearer);
        assert_eq!(
            (request.timeout_ms, request.connect_timeout_ms),
            (30_000, 10_000)
        );
        assert!(
            request
                .body
                .content_headers()
                .contains(&("Content-Encoding".to_string(), "gzip".to_string()))
        );
        assert!(!request.body.bytes().is_empty());
    }

    // 0.x "chunks claimed rows into HTTP_CHUNK requests".
    #[test]
    fn chunks_claimed_rows_into_requests_of_100() {
        let rows = claimed(150);
        let requests = card_stats_requests(&rows);
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].0.len(), 100);
        assert_eq!(requests[1].0.len(), 50);
        assert_eq!(requests[0].0[0], "q-match:0");
        assert_eq!(requests[1].0[0], "q-match:100");
        assert_eq!(games_sent(&requests[0].1).len(), 100);
        assert_eq!(games_sent(&requests[1].1).len(), 50);
    }

    // 0.x "sends body shape that satisfies API validator".
    #[test]
    fn sends_a_body_shape_that_satisfies_the_api_validator() {
        let requests = card_stats_requests(&claimed(1));
        let games = games_sent(&requests[0].1);
        assert_eq!(games.len(), 1);
        let keys = games[0].as_object().expect("game");
        for key in [
            "player_archetype_uuid",
            "opponent_archetype_uuid",
            "format",
            "won",
            "on_play",
            "is_postboard",
            "played_on",
            "cards",
        ] {
            assert!(keys.contains_key(key), "{key}");
        }
        assert!(!games[0]["cards"].as_array().expect("cards").is_empty());
    }

    #[test]
    fn an_unreadable_row_is_left_out() {
        let mut rows = claimed(1);
        rows.push(ClaimedRow {
            subject: "q-broken".to_string(),
            payload: b"nope".to_vec(),
        });
        let requests = card_stats_requests(&rows);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, vec!["q-match:0".to_string()]);
    }
}
