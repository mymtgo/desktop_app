//! Match reports (survey section 7, 07 spec section 8, rulings 3 and 4):
//! built, tested and live (`MATCH_REPORTS_LIVE`, on since #9); a match is
//! reported once its player seat has an archetype (archetypes spec section
//! 13). The body is 0.x's `SubmitMatchToApi` payload
//! from v1's stored rows: one `POST /api/matches/report` per match.

use serde_json::{Value, json};

pub use decks::DeckCard;

use crate::gate::RequestClass;
use crate::outbox::{ClaimedRow, NewOutboxRow, OutboxKind, pack_payload, readable_rows};
use crate::php::iso8601_utc;
use crate::request::{ApiRequest, AuthMode, Body, Method};

pub const REPORT_PATH: &str = "/api/matches/report";

/// 0.x's Homebrew fallback archetype (a client-side seed the API has no row
/// for).
pub const HOMEBREW_UUID: &str = "00000000-0000-0000-0000-000000000001";
/// 0.x's Rogue fallback archetype.
pub const ROGUE_UUID: &str = "00000000-0000-0000-0000-000000000002";

/// The per-game clock and timeout fields. v1 has no clock columns (target
/// schema section 6), so they are always null (parity allowlist).
pub const CLOCK_FIELDS: [&str; 4] = [
    "clock_remaining_ms",
    "opponent_clock_remaining_ms",
    "timed_out",
    "opponent_timed_out",
];

/// Most copies of one card the opponent's deck reports.
const MAX_OPPONENT_COPIES: i64 = 4;

/// One seat's archetype, read from `match_archetypes` and `archetypes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchetypeRef {
    pub uuid: String,
    /// Made on this machine (`archetypes.source = 'manual'`: a
    /// device-prefixed uuid the API refuses).
    pub manual: bool,
    /// A client-side fallback seed (`source = 'fallback'`: Homebrew, Rogue).
    pub is_fallback: bool,
}

/// Both seats' archetypes of a match; `None` for a seat with no row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MatchArchetypes {
    pub player: Option<ArchetypeRef>,
    pub opponent: Option<ArchetypeRef>,
}

/// One game, from `games` and its `game_cards`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportGame {
    pub game_number: i64,
    pub turn_count: Option<i64>,
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
    /// `games.local_mulligans`.
    pub mulligans: Option<i64>,
    /// `games.local_starting_hand_size`.
    pub starting_hand_size: Option<i64>,
    /// The local side's `starting_hand` counts `(catalog id, copies)`, in
    /// stored order.
    pub kept_hand: Vec<(u32, i64)>,
    /// `games.opp_mulligans`.
    pub opponent_mulligans: Option<i64>,
    pub on_play: Option<bool>,
    /// `games.result = 'win'`.
    pub won: bool,
    /// `games.result` is `draw` or `unknown` (Arena sends `won` null).
    pub undecided: bool,
    /// The opponent side's `seen` per catalog id, in stored order.
    pub opponent_seen: Vec<(u32, i64)>,
}

/// An Arena rank observation.
#[derive(Debug, Clone, PartialEq)]
pub struct ArenaRank {
    /// `constructed` or `limited`.
    pub ladder: String,
    /// `Platinum`, `Mythic`, and so on.
    pub class: String,
    pub level: i64,
    pub step: i64,
    pub mythic_percentile: Option<f64>,
    pub mythic_placement: Option<i64>,
    pub season: i64,
    pub observed_at_ms: i64,
}

/// One commander card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArenaCommander {
    pub arena_id: u32,
    pub name: Option<String>,
}

/// Both seats' commanders, sent for a commander format match (Arena Brawl).
/// The opponent's go in `opponent_deck` as zone `side`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArenaCommanders {
    pub player: Vec<ArenaCommander>,
    pub opponent: Vec<ArenaCommander>,
}

/// What an Arena report adds to the MTGO shape.
#[derive(Debug, Clone, PartialEq)]
pub struct ArenaReport {
    pub arena_user_id: Option<String>,
    pub event_name: String,
    /// `bo1` or `bo3`.
    pub best_of: String,
    /// `ranked`, `unranked` or `limited`.
    pub match_type: String,
    pub season: Option<i64>,
    pub rank: Option<ArenaRank>,
    pub commanders: Option<ArenaCommanders>,
}

/// Everything a report says, as v1 stores it.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchReportInput {
    pub match_token: String,
    /// The match's account (0.x: the first game's first local player).
    pub username: String,
    /// `accounts.login_id` of that account.
    pub login_id: Option<i64>,
    pub player_archetype: Option<ArchetypeRef>,
    pub opponent_archetype: Option<ArchetypeRef>,
    /// `matches.outcome`.
    pub outcome: String,
    pub format: Option<String>,
    pub tournament_event_id: Option<i64>,
    /// The token of the match's league row.
    pub league_token: Option<String>,
    /// `matches.league_id` (ruling 4).
    pub league_id: Option<i64>,
    /// The league's sync client id (`leagues.sync_client_id`), for a league
    /// with a token: the run's identity across devices, as the match bundle
    /// carries it.
    pub league_client_id: Option<String>,
    /// MTGO's league offering id (`leagues.event_id`, the log's `EventId`
    /// and the id in mtgo.com's league decklist page names), for a league
    /// from MTGO.
    pub league_playevent_id: Option<i64>,
    /// `matches.tournament_token`.
    pub tournament_token: Option<String>,
    pub tournament_round: Option<i64>,
    pub started_at_ms: i64,
    pub client_version: String,
    pub opponent_name: Option<String>,
    /// The deck version's signature, decoded.
    pub deck: Vec<DeckCard>,
    /// `(catalog id, oracle id)` of every `cards` row whose oracle id is one
    /// of the deck's, in catalog id order.
    pub oracle_cards: Vec<(u32, String)>,
    /// In game number order.
    pub games: Vec<ReportGame>,
    /// `Some` for an Arena match; `None` for MTGO.
    pub arena: Option<ArenaReport>,
}

/// The rank label the API stores: `platinum-4`, or `mythic` for Mythic.
pub fn rank_label(class: &str, level: i64) -> String {
    if class.eq_ignore_ascii_case("mythic") {
        return "mythic".to_string();
    }
    format!("{}-{}", class.to_lowercase(), level)
}

/// The archetype uuid the API can resolve, or null (0.x `reportableUuid`):
/// manual and fallback archetypes go as null.
fn reportable_uuid(archetype: Option<&ArchetypeRef>) -> Value {
    match archetype {
        Some(found)
            if !found.manual
                && !found.is_fallback
                && found.uuid != HOMEBREW_UUID
                && found.uuid != ROGUE_UUID =>
        {
            json!(found.uuid)
        }
        _ => Value::Null,
    }
}

/// The report body with 0.x's keys (survey section 7), plus
/// `league_client_id` for a match whose league has one (league run
/// identity, allowlisted). The key is left out otherwise: the server reads
/// an explicit null as "clear the stored id", and it may have one from the
/// sync. `league_playevent_id` (allowlisted) is likewise sent only when
/// known.
pub fn report_body(input: &MatchReportInput) -> Value {
    if let Some(arena) = &input.arena {
        return arena_body(input, arena);
    }
    let result = if input.outcome == "win" {
        "win"
    } else {
        "loss"
    };
    let games: Vec<Value> = input.games.iter().map(game_payload).collect();
    let mut body = json!({
        "match_token": input.match_token,
        "username": input.username,
        "login_id": input.login_id,
        "player_archetype_uuid": reportable_uuid(input.player_archetype.as_ref()),
        "opponent_archetype_uuid": reportable_uuid(input.opponent_archetype.as_ref()),
        "result": result,
        "format": input.format,
        "is_tournament": input.tournament_event_id.is_some(),
        "league_token": input.league_token,
        "league_run": input.league_id,
        "challenge_token": input.tournament_token,
        "tournament_round": input.tournament_round,
        "played_at": iso8601_utc(input.started_at_ms),
        "client_version": input.client_version,
        "opponent_name": input.opponent_name,
        "deck": deck_payload(&input.deck, &input.oracle_cards),
        "opponent_deck": opponent_deck_payload(&input.games),
        "games": games,
    });
    if let Some(league_client_id) = &input.league_client_id {
        body["league_client_id"] = json!(league_client_id);
    }
    if let Some(playevent_id) = input.league_playevent_id {
        body["league_playevent_id"] = json!(playevent_id);
    }
    body
}

fn game_payload(game: &ReportGame) -> Value {
    game_payload_with(game, Some, json!(game.won))
}

/// The id MTGO reports send for a stored catalog id: every id but 0 (the
/// opponent deck's rule; the kept hand sends every id).
fn mtgo_id(catalog_id: u32) -> Option<u32> {
    (catalog_id != 0).then_some(catalog_id)
}

/// One game's body: the kept hand as the ids `id_of` gives (a card it
/// skips is left out), `won` as the platform decides it.
fn game_payload_with(game: &ReportGame, id_of: fn(u32) -> Option<u32>, won: Value) -> Value {
    let mut kept_hand: Vec<u32> = Vec::new();
    let mut hand_size: i64 = 0;
    for (catalog_id, copies) in &game.kept_hand {
        for _ in 0..*copies {
            hand_size += 1;
            if let Some(id) = id_of(*catalog_id) {
                kept_hand.push(id);
            }
        }
    }
    game_json(game, kept_hand, hand_size, won)
}

/// `hand_size` is the kept hand's card count before any card was skipped.
fn game_json(game: &ReportGame, kept_hand: Vec<u32>, hand_size: i64, won: Value) -> Value {
    let starting_hand_size = game.starting_hand_size.unwrap_or(hand_size);
    json!({
        "game_number": game.game_number,
        "turn_count": game.turn_count,
        "started_at": game.started_at_ms.map(iso8601_utc),
        "ended_at": game.ended_at_ms.map(iso8601_utc),
        "mulligan_count": game.mulligans.unwrap_or(0),
        "starting_hand_size": starting_hand_size,
        "kept_hand": kept_hand,
        "opponent_mulligan_count": game.opponent_mulligans.unwrap_or(0),
        "on_play": game.on_play.unwrap_or(false),
        "won": won,
        "clock_remaining_ms": Value::Null,
        "opponent_clock_remaining_ms": Value::Null,
        "timed_out": Value::Null,
        "opponent_timed_out": Value::Null,
    })
}

/// The Arena id of a stored catalog id, or `None` for MTGO ids and for
/// Endstep ids (offset 200,000,000: their grpId would be above the offset),
/// and for grpId 0 (as MTGO skips id 0).
fn arena_id(catalog_id: u32) -> Option<u32> {
    cards::grp_id_of(i64::from(catalog_id))
        .map(|grp| grp.0)
        .filter(|grp| *grp > 0 && *grp < cards::ARENA_ID_OFFSET)
}

fn arena_game_payload(game: &ReportGame) -> Value {
    let won = if game.undecided {
        Value::Null
    } else {
        json!(game.won)
    };
    game_payload_with(game, arena_id, won)
}

fn arena_deck_payload(deck: &[DeckCard]) -> Value {
    let mut cards = Vec::new();
    for card in deck {
        // The API takes a quantity of at least 1.
        if card.quantity == 0 {
            continue;
        }
        let Some(id) = arena_id(card.catalog_id) else {
            continue;
        };
        let zone = if card.sideboard { "side" } else { "main" };
        cards.push(json!({ "arena_id": id, "quantity": card.quantity, "zone": zone }));
    }
    Value::Array(cards)
}

fn arena_opponent_deck_payload(games: &[ReportGame], commanders: &[ArenaCommander]) -> Value {
    let mut cards: Vec<Value> = Vec::new();
    for (id, total) in opponent_totals(games, arena_id) {
        // The API takes a quantity of at least 1.
        if total <= 0 {
            continue;
        }
        if commanders.iter().any(|c| c.arena_id == id) {
            continue;
        }
        cards.push(json!({
            "arena_id": id,
            "quantity": total.min(MAX_OPPONENT_COPIES),
            "zone": "main",
        }));
    }
    // The opponent's commanders are one copy each, zone `side`, whether or
    // not a game saw them.
    for commander in commanders {
        cards.push(json!({ "arena_id": commander.arena_id, "quantity": 1, "zone": "side" }));
    }
    Value::Array(cards)
}

/// The opponent's seen copies summed per id `id_of` gives (a card it skips
/// is left out), first seen first.
fn opponent_totals(games: &[ReportGame], id_of: fn(u32) -> Option<u32>) -> Vec<(u32, i64)> {
    let mut totals: Vec<(u32, i64)> = Vec::new();
    for game in games {
        for (catalog_id, seen) in &game.opponent_seen {
            let Some(id) = id_of(*catalog_id) else {
                continue;
            };
            match totals.iter_mut().find(|(other, _)| *other == id) {
                Some((_, total)) => *total += seen,
                None => totals.push((id, *seen)),
            }
        }
    }
    totals
}

fn commanders_payload(commanders: Option<&ArenaCommanders>) -> Value {
    let Some(found) = commanders else {
        return Value::Null;
    };
    let seat = |list: &[ArenaCommander]| -> Vec<Value> {
        list.iter()
            .map(|c| json!({ "arena_id": c.arena_id, "name": c.name }))
            .collect()
    };
    json!({ "player": seat(&found.player), "opponent": seat(&found.opponent) })
}

fn rank_payload(rank: Option<&ArenaRank>) -> Value {
    let Some(rank) = rank else {
        return Value::Null;
    };
    let mythic = rank.class.eq_ignore_ascii_case("mythic");
    let (level, step) = if mythic {
        (Value::Null, Value::Null)
    } else {
        (json!(rank.level), json!(rank.step))
    };
    json!({
        "ladder": rank.ladder,
        "label": rank_label(&rank.class, rank.level),
        "class": rank.class,
        "level": level,
        "step": step,
        "mythic_percentile": rank.mythic_percentile,
        "mythic_placement": rank.mythic_placement,
        "season": rank.season,
        "observed_at": iso8601_utc(rank.observed_at_ms),
    })
}

/// The Arena body: the common keys, Arena ids, and the `arena` object.
/// `league_playevent_id` is MTGO only and never sent.
fn arena_body(input: &MatchReportInput, arena: &ArenaReport) -> Value {
    let result = match input.outcome.as_str() {
        "win" => "win",
        "draw" => "draw",
        _ => "loss",
    };
    let games: Vec<Value> = input.games.iter().map(arena_game_payload).collect();
    let mut body = json!({
        "platform": "arena",
        "match_token": input.match_token,
        "username": input.username,
        "arena_user_id": arena.arena_user_id,
        "login_id": input.login_id,
        "player_archetype_uuid": reportable_uuid(input.player_archetype.as_ref()),
        "opponent_archetype_uuid": reportable_uuid(input.opponent_archetype.as_ref()),
        "result": result,
        "format": input.format,
        "is_tournament": input.tournament_event_id.is_some(),
        "league_token": input.league_token,
        "league_run": input.league_id,
        "challenge_token": input.tournament_token,
        "tournament_round": input.tournament_round,
        "played_at": iso8601_utc(input.started_at_ms),
        "client_version": input.client_version,
        "opponent_name": input.opponent_name,
        "deck": arena_deck_payload(&input.deck),
        "opponent_deck": arena_opponent_deck_payload(
            &input.games,
            arena.commanders.as_ref().map_or(&[], |c| c.opponent.as_slice()),
        ),
        "games": games,
        "arena": {
            "event_name": arena.event_name,
            "best_of": arena.best_of,
            "match_type": arena.match_type,
            "season": arena.season,
            "rank": rank_payload(arena.rank.as_ref()),
            "commanders": commanders_payload(arena.commanders.as_ref()),
        },
    });
    if let Some(league_client_id) = &input.league_client_id {
        body["league_client_id"] = json!(league_client_id);
    }
    body
}

/// 0.x `buildDeckPayload`: each card's oracle id maps to one catalog id,
/// the last `cards` row of that oracle id (`keyBy('oracle_id')`); `main`
/// when the sideboard flag is false; a card without a card row (or without
/// an oracle id) is dropped.
fn deck_payload(deck: &[DeckCard], oracle_cards: &[(u32, String)]) -> Value {
    let oracle_of = |catalog_id: u32| {
        oracle_cards
            .iter()
            .find(|(id, oracle)| *id == catalog_id && !oracle.is_empty())
            .map(|(_, oracle)| oracle.as_str())
    };
    let mut cards = Vec::new();
    for card in deck {
        let Some(oracle) = oracle_of(card.catalog_id) else {
            continue;
        };
        let Some((mapped, _)) = oracle_cards.iter().rev().find(|(_, other)| other == oracle) else {
            continue;
        };
        let zone = if card.sideboard { "side" } else { "main" };
        cards.push(json!({ "mtgo_id": mapped, "quantity": card.quantity, "zone": zone }));
    }
    Value::Array(cards)
}

/// 0.x `buildOpponentDeckPayload`: the opponent's seen copies summed per
/// catalog id across games (first seen first), capped at 4, all `main`.
fn opponent_deck_payload(games: &[ReportGame]) -> Value {
    let cards: Vec<Value> = opponent_totals(games, mtgo_id)
        .into_iter()
        .map(|(catalog_id, total)| {
            json!({
                "mtgo_id": catalog_id,
                "quantity": total.min(MAX_OPPONENT_COPIES),
                "zone": "main",
            })
        })
        .collect();
    Value::Array(cards)
}

/// `POST /api/matches/report`: JSON, class `Community`, 30 s total and
/// 10 s connect.
pub fn report_request(body: &Value) -> ApiRequest {
    let mut request = ApiRequest::new(
        Method::Post,
        REPORT_PATH,
        RequestClass::Community,
        AuthMode::DeviceOrBearer,
    );
    request.body = Body::json(body);
    request
}

/// The outbox row of a report: subject the match token, payload the zstd
/// JSON body (built at enqueue time).
pub fn report_row(input: &MatchReportInput, now_ms: i64) -> NewOutboxRow {
    NewOutboxRow {
        kind: OutboxKind::MatchReport,
        subject: input.match_token.clone(),
        payload: pack_payload(&report_body(input)),
        now_ms,
    }
}

/// The requests for a claim of reports: one per match, as 0.x. A row whose
/// stored payload cannot be read is left out (the worker fails it as
/// unreadable).
pub fn report_requests(rows: &[ClaimedRow]) -> Vec<(Vec<String>, ApiRequest)> {
    readable_rows(rows)
        .into_iter()
        .map(|(subject, body)| (vec![subject], report_request(&body)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::RequestClass;
    use crate::request::{AuthMode, Body, Method};
    use serde_json::{Value, json};

    /// 2026-08-19T10:00:00Z.
    const STARTED: i64 = 1_787_133_600_000;

    fn archetype(uuid: &str) -> ArchetypeRef {
        ArchetypeRef {
            uuid: uuid.to_string(),
            manual: false,
            is_fallback: false,
        }
    }

    fn game(number: i64, seen: &[(u32, i64)]) -> ReportGame {
        ReportGame {
            game_number: number,
            turn_count: Some(9),
            started_at_ms: Some(STARTED + number * 60_000),
            ended_at_ms: Some(STARTED + number * 60_000 + 30_000),
            mulligans: Some(0),
            starting_hand_size: Some(7),
            kept_hand: vec![(1001, 2), (1002, 1)],
            opponent_mulligans: Some(1),
            on_play: Some(true),
            won: true,
            undecided: false,
            opponent_seen: seen.to_vec(),
        }
    }

    /// 0.x `makeSubmittableMatch`: a won match with a deck version, both
    /// archetype sides and one game per opponent card list.
    fn submittable(per_game: &[&[(u32, i64)]]) -> MatchReportInput {
        MatchReportInput {
            match_token: "q-match-0001".to_string(),
            username: "Qplayer".to_string(),
            login_id: None,
            player_archetype: Some(archetype("aaaaaaaa-1111-4111-8111-111111111111")),
            opponent_archetype: Some(archetype("bbbbbbbb-2222-4222-8222-222222222222")),
            outcome: "win".to_string(),
            format: Some("CMODERN".to_string()),
            tournament_event_id: None,
            league_token: None,
            league_id: None,
            league_client_id: None,
            league_playevent_id: None,
            tournament_token: None,
            tournament_round: None,
            started_at_ms: STARTED,
            client_version: "1.0.0-beta.2".to_string(),
            opponent_name: Some("Qfoe".to_string()),
            deck: Vec::new(),
            oracle_cards: Vec::new(),
            games: per_game
                .iter()
                .enumerate()
                .map(|(index, seen)| game(index as i64 + 1, seen))
                .collect(),
            arena: None,
        }
    }

    fn body(input: &MatchReportInput) -> Value {
        report_body(input)
    }

    const ARENA_CARD: u32 = 100_087_654;
    const ARENA_OPP: u32 = 100_081_234;

    fn arena_rank() -> ArenaRank {
        ArenaRank {
            ladder: "constructed".to_string(),
            class: "Platinum".to_string(),
            level: 4,
            step: 2,
            mythic_percentile: None,
            mythic_placement: None,
            season: 95,
            // 2026-11-10T19:02:11Z
            observed_at_ms: 1_794_337_331_000,
        }
    }

    fn arena_input() -> MatchReportInput {
        let mut input = submittable(&[&[(ARENA_OPP, 1)]]);
        input.format = Some("CPIONEER".to_string());
        input.deck = vec![DeckCard {
            catalog_id: ARENA_CARD,
            quantity: 4,
            sideboard: false,
        }];
        input.games[0].kept_hand = vec![(ARENA_CARD, 1)];
        input.arena = Some(ArenaReport {
            arena_user_id: Some("ARENAUSER1".to_string()),
            event_name: "Traditional_Explorer_Ladder".to_string(),
            best_of: "bo3".to_string(),
            match_type: "ranked".to_string(),
            season: Some(95),
            rank: Some(arena_rank()),
            commanders: None,
        });
        input
    }

    #[test]
    fn an_arena_body_sends_the_platform_and_arena_ids() {
        let sent = body(&arena_input());
        assert_eq!(sent["platform"], json!("arena"));
        assert_eq!(sent["arena_user_id"], json!("ARENAUSER1"));
        assert_eq!(
            sent["deck"],
            json!([{"arena_id": 87654, "quantity": 4, "zone": "main"}])
        );
        assert_eq!(
            sent["opponent_deck"][0],
            json!({"arena_id": 81234, "quantity": 1, "zone": "main"})
        );
        assert_eq!(sent["games"][0]["kept_hand"], json!([87654]));
        assert!(!sent.to_string().contains("mtgo_id"));
    }

    #[test]
    fn an_arena_body_carries_the_arena_object() {
        let sent = body(&arena_input());
        assert_eq!(
            sent["arena"],
            json!({
                "event_name": "Traditional_Explorer_Ladder",
                "best_of": "bo3",
                "match_type": "ranked",
                "season": 95,
                "rank": {
                    "ladder": "constructed", "label": "platinum-4",
                    "class": "Platinum", "level": 4, "step": 2,
                    "mythic_percentile": null, "mythic_placement": null,
                    "season": 95, "observed_at": "2026-11-10T19:02:11+00:00"
                },
                "commanders": null
            })
        );
    }

    #[test]
    fn commanders_serialise_when_present() {
        let mut input = arena_input();
        input.arena.as_mut().expect("arena").commanders = Some(ArenaCommanders {
            player: vec![ArenaCommander {
                arena_id: 90123,
                name: Some("Some Commander".to_string()),
            }],
            opponent: vec![ArenaCommander {
                arena_id: 90456,
                name: None,
            }],
        });
        assert_eq!(
            body(&input)["arena"]["commanders"],
            json!({
                "player": [{"arena_id": 90123, "name": "Some Commander"}],
                "opponent": [{"arena_id": 90456, "name": null}]
            })
        );
    }

    #[test]
    fn a_mythic_rank_has_no_level_or_step() {
        let mut input = arena_input();
        let rank = input
            .arena
            .as_mut()
            .expect("arena")
            .rank
            .as_mut()
            .expect("rank");
        rank.class = "Mythic".to_string();
        rank.level = 1;
        rank.step = 2;
        rank.mythic_percentile = Some(91.5);
        rank.mythic_placement = Some(120);
        let sent = body(&input);
        let rank = &sent["arena"]["rank"];
        assert_eq!(rank["label"], json!("mythic"));
        assert_eq!(rank["level"], Value::Null);
        assert_eq!(rank["step"], Value::Null);
        assert_eq!(rank["mythic_percentile"], json!(91.5));
        assert_eq!(rank["mythic_placement"], json!(120));
    }

    #[test]
    fn rank_label_lowercases_the_class() {
        assert_eq!(rank_label("Platinum", 4), "platinum-4");
        assert_eq!(rank_label("Bronze", 0), "bronze-0");
        assert_eq!(rank_label("Mythic", 1), "mythic");
    }

    #[test]
    fn an_arena_draw_is_a_draw() {
        let mut input = arena_input();
        input.outcome = "draw".to_string();
        input.games[0].undecided = true;
        let sent = body(&input);
        assert_eq!(sent["result"], json!("draw"));
        let game = sent["games"][0].as_object().expect("game");
        assert_eq!(game.get("won"), Some(&Value::Null));
        let mut mtgo = submittable(&[&[(1001, 1)]]);
        mtgo.outcome = "draw".to_string();
        assert_eq!(body(&mtgo)["result"], json!("loss"));
    }

    #[test]
    fn an_arena_body_drops_ids_that_are_not_arena_ids() {
        let mut input = arena_input();
        let mine = |id: u32, quantity: u32| DeckCard {
            catalog_id: id,
            quantity,
            sideboard: false,
        };
        input.deck = vec![mine(126_519, 1), mine(200_000_005, 1), mine(ARENA_CARD, 4)];
        input.games[0].opponent_seen = vec![(126_519, 1), (200_000_005, 1), (ARENA_OPP, 1)];
        input.games[0].kept_hand = vec![(126_519, 1), (200_000_005, 1), (ARENA_CARD, 1)];
        let sent = body(&input);
        assert_eq!(sent["deck"].as_array().expect("deck").len(), 1);
        assert_eq!(sent["opponent_deck"].as_array().expect("opp").len(), 1);
        assert_eq!(sent["games"][0]["kept_hand"], json!([87654]));
    }

    // The MTGO kept hand has always sent every id, 0 included.
    #[test]
    fn an_mtgo_kept_hand_sends_id_zero() {
        let mut input = submittable(&[&[(1001, 1)]]);
        input.games[0].kept_hand = vec![(0, 1), (1001, 1)];
        assert_eq!(body(&input)["games"][0]["kept_hand"], json!([0, 1001]));
    }

    // grpId 0 is no card, as MTGO skips id 0.
    #[test]
    fn an_arena_body_drops_grp_id_zero() {
        let mut input = arena_input();
        let zero = cards::ARENA_ID_OFFSET;
        input.deck = vec![DeckCard {
            catalog_id: zero,
            quantity: 1,
            sideboard: false,
        }];
        input.games[0].opponent_seen = vec![(zero, 1)];
        input.games[0].kept_hand = vec![(zero, 1), (ARENA_CARD, 1)];
        let sent = body(&input);
        assert_eq!(sent["deck"], json!([]));
        assert_eq!(sent["opponent_deck"], json!([]));
        assert_eq!(sent["games"][0]["kept_hand"], json!([87654]));
    }

    // The hand size fallback counts the kept hand before ids are filtered.
    #[test]
    fn the_hand_size_fallback_counts_the_unfiltered_kept_hand() {
        let mut input = arena_input();
        input.games[0].starting_hand_size = None;
        input.games[0].kept_hand = vec![(126_519, 2), (ARENA_CARD, 5)];
        let sent = body(&input);
        assert_eq!(
            sent["games"][0]["kept_hand"]
                .as_array()
                .expect("hand")
                .len(),
            5
        );
        assert_eq!(sent["games"][0]["starting_hand_size"], json!(7));
    }

    // The API takes a quantity of at least 1.
    #[test]
    fn an_arena_body_drops_cards_with_no_copies() {
        let mut input = arena_input();
        input.deck = vec![
            DeckCard {
                catalog_id: ARENA_OPP,
                quantity: 0,
                sideboard: true,
            },
            DeckCard {
                catalog_id: ARENA_CARD,
                quantity: 4,
                sideboard: false,
            },
        ];
        input.games[0].opponent_seen = vec![(ARENA_CARD, 0), (ARENA_OPP, 1)];
        let sent = body(&input);
        assert_eq!(
            sent["deck"],
            json!([{"arena_id": 87654, "quantity": 4, "zone": "main"}])
        );
        assert_eq!(
            sent["opponent_deck"],
            json!([{"arena_id": 81234, "quantity": 1, "zone": "main"}])
        );
    }

    #[test]
    fn an_arena_body_keeps_league_keys_and_drops_the_playevent_id() {
        let mut input = arena_input();
        input.league_token = Some("arena:abc".to_string());
        input.league_id = Some(7);
        input.league_client_id = Some("client-1".to_string());
        input.league_playevent_id = Some(99);
        let sent = body(&input);
        assert_eq!(sent["league_token"], json!("arena:abc"));
        assert_eq!(sent["league_run"], json!(7));
        assert_eq!(sent["league_client_id"], json!("client-1"));
        assert!(sent.get("league_playevent_id").is_none());
    }

    #[test]
    fn a_null_rank_and_season_are_sent_as_null() {
        let mut input = arena_input();
        let arena = input.arena.as_mut().expect("arena");
        arena.rank = None;
        arena.season = None;
        let sent = body(&input);
        assert!(sent["arena"].get("rank").expect("rank key").is_null());
        assert!(sent["arena"].get("season").expect("season key").is_null());
    }

    #[test]
    fn an_mtgo_body_has_no_platform_or_arena_key() {
        let sent = body(&submittable(&[&[(1001, 1)]]));
        assert!(sent.get("platform").is_none());
        assert!(sent.get("arena_user_id").is_none());
        assert!(sent.get("arena").is_none());
    }

    // 0.x SubmitMatchToApiTest: "sends the client version with the report".
    #[test]
    fn sends_the_client_version_with_the_report() {
        let input = submittable(&[&[(1001, 1)]]);
        assert_eq!(body(&input)["client_version"], json!("1.0.0-beta.2"));
    }

    // 0.x "sends league_run and opponent_deck when reporting a match".
    #[test]
    fn sends_league_run_and_opponent_deck_when_reporting_a_match() {
        let mut input = submittable(&[&[(1001, 2), (1002, 1)]]);
        input.league_id = Some(42);
        input.league_token = Some("q-league-token".to_string());
        let sent = body(&input);
        assert_eq!(sent["league_run"], json!(42));
        assert_eq!(sent["league_token"], json!("q-league-token"));
        assert_eq!(
            sent["opponent_deck"],
            json!([
                {"mtgo_id": 1001, "quantity": 2, "zone": "main"},
                {"mtgo_id": 1002, "quantity": 1, "zone": "main"},
            ])
        );
    }

    // League run identity (server spec 2026-10-23): a league match names
    // its run by the league's sync client id, so two devices' local league
    // ids never merge two runs on the server.
    #[test]
    fn a_league_match_sends_its_league_client_id() {
        let mut input = submittable(&[&[(1001, 1)]]);
        input.league_id = Some(12);
        input.league_token = Some("q-league-token".to_string());
        input.league_client_id = Some("q-league-token_20270101T000000Z".to_string());
        let sent = body(&input);
        assert_eq!(
            sent["league_client_id"],
            json!("q-league-token_20270101T000000Z")
        );
        assert_eq!(sent["league_run"], json!(12));
    }

    // An explicit null would clear an id the server took from the sync, so a
    // match with no league id leaves the key out.
    #[test]
    fn a_match_without_a_league_client_id_leaves_the_key_out() {
        let sent = body(&submittable(&[&[(1001, 1)]]));
        assert!(sent.get("league_client_id").is_none(), "{sent}");
    }

    // MTGO's league offering id (the log's `EventId`, the id in the
    // mtgo.com decklist page names), so the server can tell offerings apart.
    #[test]
    fn a_league_match_sends_its_playevent_id() {
        let mut input = submittable(&[&[(1001, 1)]]);
        input.league_id = Some(12);
        input.league_token = Some("q-league-token".to_string());
        input.league_playevent_id = Some(10397);
        let sent = body(&input);
        assert_eq!(sent["league_playevent_id"], json!(10397));
    }

    // Optional on the server: a match with no offering id leaves the key out.
    #[test]
    fn a_match_without_a_playevent_id_leaves_the_key_out() {
        let sent = body(&submittable(&[&[(1001, 1)]]));
        assert!(sent.get("league_playevent_id").is_none(), "{sent}");
    }

    // 0.x "aggregates opponent cards across games and caps quantity at 4".
    #[test]
    fn aggregates_opponent_cards_across_games_and_caps_quantity_at_4() {
        let input = submittable(&[&[(2001, 3)], &[(2001, 3)]]);
        assert_eq!(
            body(&input)["opponent_deck"],
            json!([{"mtgo_id": 2001, "quantity": 4, "zone": "main"}])
        );
    }

    #[test]
    fn opponent_cards_keep_their_first_seen_order_and_drop_catalog_id_zero() {
        let input = submittable(&[&[(3002, 1), (0, 2)], &[(3001, 1), (3002, 1)]]);
        assert_eq!(
            body(&input)["opponent_deck"],
            json!([
                {"mtgo_id": 3002, "quantity": 2, "zone": "main"},
                {"mtgo_id": 3001, "quantity": 1, "zone": "main"},
            ])
        );
    }

    // 0.x "sends per-game turn count and timing".
    #[test]
    fn sends_per_game_turn_count_and_timing() {
        let mut input = submittable(&[&[(4001, 1)]]);
        input.games[0].turn_count = Some(12);
        input.games[0].started_at_ms = Some(STARTED);
        input.games[0].ended_at_ms = Some(STARTED + 870_000);
        let game = &body(&input)["games"][0];
        assert_eq!(game["turn_count"], json!(12));
        assert_eq!(game["started_at"], json!("2026-08-19T10:00:00+00:00"));
        assert_eq!(game["ended_at"], json!("2026-08-19T10:14:30+00:00"));
    }

    // 0.x "sends null turn count and ended_at for games that never recorded
    // them".
    #[test]
    fn sends_null_turn_count_and_ended_at_for_games_that_never_recorded_them() {
        let mut input = submittable(&[&[(4002, 1)]]);
        input.games[0].turn_count = None;
        input.games[0].ended_at_ms = None;
        let game = &body(&input)["games"][0];
        let keys = game.as_object().expect("a game object");
        for key in ["turn_count", "started_at", "ended_at"] {
            assert!(keys.contains_key(key), "{key}");
        }
        assert_eq!(game["turn_count"], Value::Null);
        assert_eq!(game["ended_at"], Value::Null);
        assert_ne!(game["started_at"], Value::Null);
    }

    // 0.x "sends a null league_run for non-league matches".
    #[test]
    fn sends_a_null_league_run_for_non_league_matches() {
        let sent = body(&submittable(&[&[(3001, 1)]]));
        assert_eq!(sent["league_run"], Value::Null);
        assert_eq!(sent["league_token"], Value::Null);
    }

    // 0.x "sends a null opponent archetype uuid when the opponent archetype
    // is local to this client".
    #[test]
    fn sends_a_null_opponent_archetype_uuid_when_the_opponent_archetype_is_local() {
        let mut input = submittable(&[&[(3001, 1)]]);
        input.opponent_archetype = Some(ArchetypeRef {
            uuid: "a1b2c3d4-qqqq".to_string(),
            manual: true,
            is_fallback: false,
        });
        let sent = body(&input);
        assert_eq!(sent["opponent_archetype_uuid"], Value::Null);
        assert_eq!(
            sent["player_archetype_uuid"],
            json!("aaaaaaaa-1111-4111-8111-111111111111")
        );
    }

    // 0.x "sends a null player archetype uuid when the player archetype is
    // local to this client".
    #[test]
    fn sends_a_null_player_archetype_uuid_when_the_player_archetype_is_local() {
        let mut input = submittable(&[&[(3002, 1)]]);
        input.player_archetype = Some(ArchetypeRef {
            uuid: "b2c3d4e5-qqqq".to_string(),
            manual: true,
            is_fallback: false,
        });
        assert_eq!(body(&input)["player_archetype_uuid"], Value::Null);
    }

    // 0.x "sends a null opponent archetype uuid for the fallback archetypes".
    #[test]
    fn sends_a_null_opponent_archetype_uuid_for_the_fallback_archetypes() {
        for uuid in [HOMEBREW_UUID, ROGUE_UUID] {
            let mut input = submittable(&[&[(3003, 1)]]);
            input.opponent_archetype = Some(archetype(uuid));
            assert_eq!(
                body(&input)["opponent_archetype_uuid"],
                Value::Null,
                "{uuid}"
            );
            input.opponent_archetype = Some(ArchetypeRef {
                uuid: "cccccccc-3333-4333-8333-333333333333".to_string(),
                manual: false,
                is_fallback: true,
            });
            assert_eq!(body(&input)["opponent_archetype_uuid"], Value::Null);
        }
        let mut input = submittable(&[&[(3003, 1)]]);
        input.opponent_archetype = None;
        assert_eq!(body(&input)["opponent_archetype_uuid"], Value::Null);
    }

    // 0.x "sends the login id of the reporting account when the client
    // knows it".
    #[test]
    fn sends_the_login_id_of_the_reporting_account_when_the_client_knows_it() {
        let mut input = submittable(&[&[(1001, 1)]]);
        input.login_id = Some(6_031_972);
        let sent = body(&input);
        assert_eq!(sent["login_id"], json!(6_031_972));
        assert_eq!(sent["username"], json!("Qplayer"));
    }

    // 0.x "sends a null login id when the reporting account has none".
    #[test]
    fn sends_a_null_login_id_when_the_reporting_account_has_none() {
        let sent = body(&submittable(&[&[(1001, 1)]]));
        assert!(sent.as_object().expect("object").contains_key("login_id"));
        assert_eq!(sent["login_id"], Value::Null);
    }

    // 0.x "sends per-game clocks, timeout flags and the opponent name": v1
    // has no clock columns (target schema section 6), so the opponent name
    // goes and every clock and timeout field is null (parity allowlist).
    #[test]
    fn sends_the_opponent_name_and_null_clocks() {
        let sent = body(&submittable(&[&[(5001, 1)]]));
        assert_eq!(sent["opponent_name"], json!("Qfoe"));
        for key in CLOCK_FIELDS {
            assert_eq!(sent["games"][0][key], Value::Null, "{key}");
        }
    }

    // 0.x "sends null clocks and timeouts for games with no recorded clock"
    // and "sends null for a side with no recorded clock".
    #[test]
    fn sends_null_clocks_and_timeouts_for_every_game() {
        let sent = body(&submittable(&[&[(5002, 1)], &[(5003, 1)]]));
        for game in sent["games"].as_array().expect("games") {
            for key in CLOCK_FIELDS {
                assert!(game.as_object().expect("game").contains_key(key), "{key}");
                assert_eq!(game[key], Value::Null, "{key}");
            }
        }
    }

    #[test]
    fn the_body_has_exactly_the_0x_keys() {
        let sent = body(&submittable(&[&[(1001, 1)]]));
        let mut keys: Vec<&str> = sent
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        let mut expected = vec![
            "match_token",
            "username",
            "login_id",
            "player_archetype_uuid",
            "opponent_archetype_uuid",
            "result",
            "format",
            "is_tournament",
            "league_token",
            "league_run",
            "challenge_token",
            "tournament_round",
            "played_at",
            "client_version",
            "opponent_name",
            "deck",
            "opponent_deck",
            "games",
        ];
        expected.sort_unstable();
        assert_eq!(keys, expected);
        // A league match adds two keys 0.x does not send (allowlisted).
        let mut league = submittable(&[&[(1001, 1)]]);
        league.league_client_id = Some("q-league-token_20270101T000000Z".to_string());
        league.league_playevent_id = Some(10397);
        let league_sent = body(&league);
        let mut league_keys: Vec<&str> = league_sent
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        league_keys.sort_unstable();
        expected.push("league_client_id");
        expected.push("league_playevent_id");
        expected.sort_unstable();
        assert_eq!(league_keys, expected);
        let mut game_keys: Vec<&str> = sent["games"][0]
            .as_object()
            .expect("game")
            .keys()
            .map(String::as_str)
            .collect();
        game_keys.sort_unstable();
        let mut expected_game = vec![
            "game_number",
            "turn_count",
            "started_at",
            "ended_at",
            "mulligan_count",
            "starting_hand_size",
            "kept_hand",
            "opponent_mulligan_count",
            "on_play",
            "won",
            "clock_remaining_ms",
            "opponent_clock_remaining_ms",
            "timed_out",
            "opponent_timed_out",
        ];
        expected_game.sort_unstable();
        assert_eq!(game_keys, expected_game);
    }

    // Ruling 4: a draw reports "loss", as 0.x's `isWin() ? win : loss`.
    #[test]
    fn only_a_win_reports_win_and_a_draw_reports_loss() {
        for (outcome, result) in [("win", "win"), ("loss", "loss"), ("draw", "loss")] {
            let mut input = submittable(&[&[(1001, 1)]]);
            input.outcome = outcome.to_string();
            assert_eq!(body(&input)["result"], json!(result), "{outcome}");
        }
    }

    #[test]
    fn match_fields_are_the_stored_values() {
        let mut input = submittable(&[&[(1001, 1)]]);
        input.tournament_event_id = Some(12_840_476);
        input.tournament_round = Some(2);
        input.tournament_token = Some("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb".to_string());
        let sent = body(&input);
        assert_eq!(sent["match_token"], json!("q-match-0001"));
        assert_eq!(sent["format"], json!("CMODERN"));
        assert_eq!(sent["is_tournament"], json!(true));
        assert_eq!(
            sent["challenge_token"],
            json!("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb")
        );
        assert_eq!(sent["tournament_round"], json!(2));
        assert_eq!(sent["played_at"], json!("2026-08-19T10:00:00+00:00"));
        assert_eq!(body(&submittable(&[]))["is_tournament"], json!(false));
    }

    #[test]
    fn games_carry_the_hand_and_the_mulligans() {
        let mut input = submittable(&[&[(1001, 1)], &[(1001, 1)]]);
        input.games[1].won = false;
        input.games[1].on_play = None;
        input.games[1].mulligans = None;
        input.games[1].opponent_mulligans = Some(2);
        input.games[1].starting_hand_size = Some(6);
        let games = body(&input)["games"].clone();
        assert_eq!(games[0]["game_number"], json!(1));
        assert_eq!(games[0]["kept_hand"], json!([1001, 1001, 1002]));
        assert_eq!(games[0]["starting_hand_size"], json!(7));
        assert_eq!(games[0]["mulligan_count"], json!(0));
        assert_eq!(games[0]["opponent_mulligan_count"], json!(1));
        assert_eq!(games[0]["on_play"], json!(true));
        assert_eq!(games[0]["won"], json!(true));
        assert_eq!(games[1]["game_number"], json!(2));
        assert_eq!(games[1]["won"], json!(false));
        assert_eq!(games[1]["on_play"], json!(false));
        assert_eq!(games[1]["mulligan_count"], json!(0));
        assert_eq!(games[1]["opponent_mulligan_count"], json!(2));
        assert_eq!(games[1]["starting_hand_size"], json!(6));
        // No stored size: the kept hand's count, as 0.x `count($keptHand)`.
        input.games[0].starting_hand_size = None;
        assert_eq!(body(&input)["games"][0]["starting_hand_size"], json!(3));
    }

    // 0.x `buildDeckPayload`: each card's oracle id maps to one catalog id
    // (`keyBy('oracle_id')`: the last row per oracle id), zone `main` when
    // the sideboard flag is false, cards without a card row dropped.
    #[test]
    fn the_deck_maps_each_oracle_id_to_one_catalog_id() {
        let mut input = submittable(&[&[(1001, 1)]]);
        input.deck = vec![
            DeckCard {
                catalog_id: 500,
                quantity: 4,
                sideboard: false,
            },
            DeckCard {
                catalog_id: 600,
                quantity: 2,
                sideboard: true,
            },
            DeckCard {
                catalog_id: 700,
                quantity: 1,
                sideboard: false,
            },
            DeckCard {
                catalog_id: 800,
                quantity: 3,
                sideboard: false,
            },
        ];
        input.oracle_cards = vec![
            (500, "q-oracle-a".to_string()),
            (501, "q-oracle-a".to_string()),
            (600, "q-oracle-b".to_string()),
            (800, String::new()),
        ];
        assert_eq!(
            body(&input)["deck"],
            json!([
                {"mtgo_id": 501, "quantity": 4, "zone": "main"},
                {"mtgo_id": 600, "quantity": 2, "zone": "side"},
            ])
        );
    }

    #[test]
    fn the_report_request_posts_json_to_the_report_path() {
        let sent = body(&submittable(&[&[(1001, 1)]]));
        let request = report_request(&sent);
        assert_eq!(request.method, Method::Post);
        assert_eq!(request.path, "/api/matches/report");
        assert_eq!(request.class, RequestClass::Community);
        assert_eq!(request.auth, AuthMode::DeviceOrBearer);
        assert_eq!(
            (request.timeout_ms, request.connect_timeout_ms),
            (30_000, 10_000)
        );
        let Body::Json(bytes) = &request.body else {
            panic!("a JSON body: {:?}", request.body);
        };
        assert_eq!(serde_json::from_slice::<Value>(bytes).expect("json"), sent);
    }

    #[test]
    fn a_report_row_is_the_zstd_body_keyed_by_the_match_token() {
        let input = submittable(&[&[(1001, 1)]]);
        let row = report_row(&input, 77);
        assert_eq!(row.kind, crate::outbox::OutboxKind::MatchReport);
        assert_eq!(row.subject, "q-match-0001");
        assert_eq!(row.now_ms, 77);
        let stored = zstd::decode_all(row.payload.expect("a payload").as_slice()).expect("zstd");
        assert_eq!(
            serde_json::from_slice::<Value>(&stored).expect("json"),
            report_body(&input)
        );
        let claimed = crate::outbox::ClaimedRow {
            subject: row.subject.clone(),
            payload: zstd::encode_all(stored.as_slice(), 0).expect("zstd"),
        };
        let requests = report_requests(&[claimed.clone(), claimed]);
        assert_eq!(requests.len(), 2, "one request per match, as 0.x");
        assert_eq!(requests[0].0, vec!["q-match-0001".to_string()]);
        assert_eq!(requests[0].1, report_request(&report_body(&input)));
        let unreadable = crate::outbox::ClaimedRow {
            subject: "q-broken".to_string(),
            payload: b"not zstd".to_vec(),
        };
        assert!(report_requests(&[unreadable]).is_empty());
    }

    // The report's keys 0.x does not send are justified in the parity
    // allowlist.
    #[test]
    fn the_league_client_id_is_allowlisted() {
        let allowlist: Value =
            serde_json::from_str(include_str!("../../../fixtures/parity/allowlist.json"))
                .expect("the allowlist is JSON");
        for field in ["league_client_id", "league_playevent_id"] {
            let listed = allowlist
                .as_array()
                .expect("a list")
                .iter()
                .any(|entry| entry["fixture"] == "match_report" && entry["field"] == field);
            assert!(listed, "{field}");
        }
    }

    // The null clock fields are justified in the parity allowlist.
    #[test]
    fn the_null_clock_fields_are_allowlisted() {
        let allowlist: Value =
            serde_json::from_str(include_str!("../../../fixtures/parity/allowlist.json"))
                .expect("the allowlist is JSON");
        for field in CLOCK_FIELDS {
            let listed = allowlist.as_array().expect("a list").iter().any(|entry| {
                entry["fixture"] == "match_report"
                    && entry["field"] == format!("games[].{field}")
                    && entry["reason"] == "0.x clock columns dropped (target schema section 6)"
            });
            assert!(listed, "{field}");
        }
    }
}
