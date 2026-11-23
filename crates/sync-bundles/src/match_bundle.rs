//! The match bundle (0.x `MatchBundleBuilder`, survey section 12), built
//! from v1's stored rows (`matches`, `games`, `game_cards`,
//! `game_card_casts`, `game_mulligan_cards`, the frames and the
//! `match_archetypes` seats, archetypes spec section 14).
//!
//! Every enum value keeps 0.x's spelling (`state`, `outcome`, a game's
//! `won`), and each player's `deck_json` and `opening_hand_json` are 0.x's
//! JSON shapes rebuilt from the per-card rows: lists come out in catalog id
//! order (the stored order; v1 keeps no dealt order, target schema section
//! 17), each copy repeated in a hand.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::canonical::{bundle_hash, canonical_json};
use crate::carry::Carry;
use crate::client_id::deck_client_id;
use crate::datetime::bundle_datetime;
use crate::endstep_cards::EndstepCardRef;
use crate::loose;
use crate::platform::{BundlePlatform, add_platform, copy_platform};
use crate::sources::{MatchSources, sources_section};

/// 0.x's alternative cast columns, in `MEASUREMENT_COLUMNS` order (v1's
/// `game_card_casts.method` values).
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

/// One row of a deck version's list (`decks::decode_signature`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckRow {
    pub catalog_id: u32,
    pub quantity: u32,
    pub sideboard: bool,
}

/// One `game_cards` row, with its card's oracle id (`cards.oracle_id`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CardRowInput {
    /// `side = 'opponent'`.
    pub opponent: bool,
    pub catalog_id: u32,
    pub oracle_id: Option<String>,
    pub main_qty: u32,
    pub side_qty: u32,
    pub sided_in: u32,
    pub sided_out: u32,
    pub opening_hand: u32,
    pub bottomed: u32,
    pub starting_hand: u32,
    pub seen: u32,
    pub pregame_revealed: u32,
    pub pregame_played: u32,
    pub cast: u32,
    pub played: Option<u32>,
    pub activated: Option<u32>,
}

/// One `game_card_casts` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CastInput {
    pub opponent: bool,
    pub catalog_id: u32,
    pub method: String,
    pub count: u32,
}

/// One game state frame: its UTC time and its JSON (0.x
/// `game_timelines.timestamp` and `content`).
#[derive(Debug, Clone, PartialEq)]
pub struct FrameInput {
    pub at_ms: i64,
    pub content: Value,
}

/// One game and its rows.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchGameInput {
    pub game_number: u32,
    pub mtgo_game_id: Option<i64>,
    /// `win`, `loss`, `draw` or `unknown`.
    pub result: String,
    pub on_play: Option<bool>,
    pub local_mulligans: Option<i64>,
    pub opp_mulligans: Option<i64>,
    pub local_die_roll: Option<i64>,
    pub opp_die_roll: Option<i64>,
    pub turn_count: Option<i64>,
    pub local_starting_hand_size: Option<i64>,
    pub opp_starting_hand_size: Option<i64>,
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
    pub cards: Vec<CardRowInput>,
    pub casts: Vec<CastInput>,
    /// `game_mulligan_cards`: `(hand, catalog_id, quantity)`.
    pub mulligans: Vec<(u32, u32, u32)>,
    pub frames: Vec<FrameInput>,
}

/// One `match_archetypes` seat: the archetype's uuid, the row's confidence
/// and hand-set flag, and whether it is the opponent's seat.
#[derive(Debug, Clone, PartialEq)]
pub struct ArchetypeSide {
    pub uuid: String,
    pub confidence: f64,
    pub manual: bool,
    pub opponent: bool,
}

/// An Arena player's extra fields (spec section 5.2), each written only
/// when known, so an MTGO bundle keeps its bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArenaPlayerFields {
    pub display_name: Option<String>,
    pub arena_user_id: Option<String>,
}

/// An Endstep match's ranked result (`endstep_ranked_results`, ids aside),
/// written in the match section as `endstep_ranked` (spec section 12).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EndstepRankedFields {
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

/// What a match bundle is built from.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchBundleInput {
    /// The game the match was recorded on (MTGO writes no `platform` key).
    pub platform: BundlePlatform,
    /// Arena and Endstep: the raw source, written as `sources`.
    pub sources: Option<MatchSources>,
    /// Endstep: the card each Endstep catalog id the rows name stands for.
    /// An Endstep bundle writes the ref where the others write the id
    /// (spec section 5.3); other platforms never read it.
    pub endstep_cards: BTreeMap<u32, EndstepCardRef>,
    /// Endstep: the match's ranked result, if it has one.
    pub endstep_ranked: Option<EndstepRankedFields>,
    pub local_player_fields: ArenaPlayerFields,
    pub opponent_player_fields: ArenaPlayerFields,
    pub token: String,
    pub mtgo_match_id: Option<i64>,
    /// `matches.source`: `log`, `imported`, `manual` or `history`.
    pub source: String,
    pub format: Option<String>,
    pub match_type: Option<String>,
    /// `matches.game_structure`: an Arena or Endstep match's kind (`bo1`,
    /// `bo3`), written as `match.game_structure` on those bundles only (an
    /// MTGO bundle's bytes never change).
    pub game_structure: Option<String>,
    pub state: String,
    pub outcome: String,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    pub notes: Option<String>,
    pub games_won_imported: Option<i64>,
    pub games_lost_imported: Option<i64>,
    /// The deck version played: its deck's MTGO id and its signature.
    pub deck_mtgo_id: Option<String>,
    pub deck_version_signature: Option<String>,
    /// The deck version's list, for a manual game's postboard sideboard.
    pub registered: Vec<DeckRow>,
    /// The run of `matches.league_id` (`league_client_id`).
    pub league_client_id: Option<String>,
    pub tournament_event_id: Option<i64>,
    pub tournament_round: Option<i64>,
    pub local_username: String,
    pub opponent_username: Option<String>,
    /// In `game_number` order.
    pub games: Vec<MatchGameInput>,
    /// The seats' archetype rows, any order.
    pub archetypes: Vec<ArchetypeSide>,
    /// `matches.hidden_at`: the player deleted the match (unix ms).
    pub hidden_at_ms: Option<i64>,
    /// `matches.no_league`: the player took it out of every league.
    pub no_league: bool,
    /// The player linked it to its league (spec 4.1). The store passes the
    /// flag only with an edit: imports link their members too.
    pub league_linked: bool,
    /// `matches.edited_at`: the player's last edit (unix ms).
    pub edited_at_ms: Option<i64>,
}

impl MatchBundleInput {
    fn manual(&self) -> bool {
        self.source == "manual"
    }
}

/// 0.x's `mtgo_id` of a match: the MTGO match id as text. A manual match
/// has none in v1; 0.x gave it `manual_` and 24 random characters, so v1
/// writes `manual_` and the token's first 24 letters and digits (stable
/// across rebuilds).
fn match_mtgo_id(input: &MatchBundleInput) -> Option<String> {
    if input.manual() {
        let letters: String = input
            .token
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(24)
            .collect();
        return Some(format!("manual_{letters}"));
    }
    input.mtgo_match_id.map(|id| id.to_string())
}

/// 0.x's `mtgo_id` of a game: the MTGO game id as text, else (a manual
/// game, which 0.x gave a UUID) `{token}-{game number}`.
fn game_key(input: &MatchBundleInput, game: &MatchGameInput) -> String {
    match game.mtgo_game_id {
        Some(id) => id.to_string(),
        None => format!("{}-{}", input.token, game.game_number),
    }
}

/// PHP's `<=>` on two strings: numeric strings compare as numbers, any
/// other pair byte for byte.
pub(crate) fn php_compare(a: &str, b: &str) -> Ordering {
    match (a.parse::<i64>(), b.parse::<i64>()) {
        (Ok(a), Ok(b)) => a.cmp(&b),
        _ => a.as_bytes().cmp(b.as_bytes()),
    }
}

fn datetime(ms: Option<i64>) -> Value {
    ms.map_or(Value::Null, |ms| Value::String(bundle_datetime(ms)))
}

/// The games in bundle order (0.x `sortBy('mtgo_id')`) as each game's key
/// and its index in `input.games`.
fn sorted_game_order(input: &MatchBundleInput) -> Vec<(String, usize)> {
    let mut order: Vec<(String, usize)> = input
        .games
        .iter()
        .enumerate()
        .map(|(index, game)| (game_key(input, game), index))
        .collect();
    order.sort_by(|(a, _), (b, _)| php_compare(a, b));
    order
}

/// The games in bundle order, each with its key.
fn sorted_games<'a>(
    input: &'a MatchBundleInput,
    order: &[(String, usize)],
) -> Vec<(String, &'a MatchGameInput)> {
    order
        .iter()
        .map(|(key, index)| (key.clone(), &input.games[*index]))
        .collect()
}

/// The games won and lost: the game rows, else the imported score.
fn score(input: &MatchBundleInput) -> (i64, i64) {
    if input.games.is_empty() {
        return (
            input.games_won_imported.unwrap_or(0),
            input.games_lost_imported.unwrap_or(0),
        );
    }
    let won = input.games.iter().filter(|g| g.result == "win").count() as i64;
    let lost = input.games.iter().filter(|g| g.result == "loss").count() as i64;
    (won, lost)
}

fn match_section(input: &MatchBundleInput) -> Value {
    let (won, lost) = score(input);
    // 0.x writes `result` ("2-1") for a match read from the log or the MTGO
    // history; a manual match has none. v1 keeps no `result` column, so an
    // imported match (a 0.x or pulled copy, never pushed) gets the same
    // score text, which keeps a pulled match's rebuild equal to its bundle.
    let result = if input.manual() {
        Value::Null
    } else {
        Value::String(format!("{won}-{lost}"))
    };
    let mut section = json!({
        "token": input.token,
        "mtgo_id": match_mtgo_id(input),
        "format": input.format,
        "match_type": input.match_type,
        "result": result,
        "outcome": input.outcome,
        "games_won": won,
        "games_lost": lost,
        "started_at": bundle_datetime(input.started_at_ms),
        "ended_at": datetime(input.ended_at_ms),
        "state": input.state,
        "notes": input.notes,
        "imported": input.source == "history",
        "manual": input.manual(),
        "deck_mtgo_id": input.deck_mtgo_id,
        "deck_version_signature": input.deck_version_signature,
        "league_client_id": input.league_client_id,
        "tournament_mtgo_event_id": input.tournament_event_id,
        "tournament_round": input.tournament_round,
    });
    add_player_fields(&mut section, input);
    add_endstep_ranked(&mut section, input);
    add_game_structure(&mut section, input);
    section
}

/// An Arena or Endstep match's kind, as `game_structure`, when it has one.
/// Never on an MTGO bundle: its bytes stay those of the goldens.
fn add_game_structure(section: &mut Value, input: &MatchBundleInput) {
    if input.platform == BundlePlatform::Mtgo {
        return;
    }
    let (Some(kind), Some(fields)) = (&input.game_structure, section.as_object_mut()) else {
        return;
    };
    fields.insert("game_structure".to_string(), json!(kind));
}

/// An Endstep match's ranked result, as `endstep_ranked` (every column,
/// nulls too). Never on an MTGO or Arena bundle.
fn add_endstep_ranked(section: &mut Value, input: &MatchBundleInput) {
    if input.platform != BundlePlatform::Endstep {
        return;
    }
    let (Some(ranked), Some(fields)) = (&input.endstep_ranked, section.as_object_mut()) else {
        return;
    };
    let value = json!({
        "status": ranked.status,
        "format_id": ranked.format_id,
        "rating_before": ranked.rating_before,
        "rating_after": ranked.rating_after,
        "score_before": ranked.score_before,
        "score_after": ranked.score_after,
        "delta": ranked.delta,
        "rd_before": ranked.rd_before,
        "rd_after": ranked.rd_after,
        "rank": ranked.rank,
    });
    fields.insert("endstep_ranked".to_string(), value);
}

/// A pulled Arena or Endstep bundle's match kind (`match.game_structure`),
/// read loosely; `None` for an MTGO bundle, which never carries one.
pub fn match_game_structure(bundle: &Value) -> Option<String> {
    if !matches!(BundlePlatform::of_bundle(bundle), Ok(platform) if platform != BundlePlatform::Mtgo)
    {
        return None;
    }
    loose::text(&bundle["match"]["game_structure"])
}

/// A pulled Endstep bundle's ranked result, read loosely; `None` when the
/// match section has none.
pub fn match_endstep_ranked(bundle: &Value) -> Option<EndstepRankedFields> {
    let ranked = bundle["match"].get("endstep_ranked")?;
    if !ranked.is_object() {
        return None;
    }
    Some(EndstepRankedFields {
        status: loose::text(&ranked["status"]),
        format_id: loose::text(&ranked["format_id"]),
        rating_before: loose::int(&ranked["rating_before"]),
        rating_after: loose::int(&ranked["rating_after"]),
        score_before: loose::int(&ranked["score_before"]),
        score_after: loose::int(&ranked["score_after"]),
        delta: loose::int(&ranked["delta"]),
        rd_before: loose::int(&ranked["rd_before"]),
        rd_after: loose::int(&ranked["rd_after"]),
        rank: loose::text(&ranked["rank"]),
    })
}

/// The player's fields (league self-management spec 6.1), each only when
/// set, so a match without an edit keeps its bytes and hash. Times are
/// integer ms, not bundle datetimes: a delete and its Undo can fall in one
/// second.
fn add_player_fields(section: &mut Value, input: &MatchBundleInput) {
    let Some(fields) = section.as_object_mut() else {
        return;
    };
    if let Some(at) = input.hidden_at_ms {
        fields.insert("hidden_at".to_string(), json!(at));
    }
    if input.no_league {
        fields.insert("no_league".to_string(), Value::Bool(true));
    }
    if input.league_linked {
        fields.insert("league_linked".to_string(), Value::Bool(true));
    }
    if let Some(at) = input.edited_at_ms {
        fields.insert("edited_at".to_string(), json!(at));
    }
}

/// A pulled match's player fields (spec 6.3), read loosely: a value that
/// does not read is not recorded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PulledPlayerFields {
    pub hidden_at_ms: Option<i64>,
    pub no_league: bool,
    pub league_linked: bool,
    pub edited_at_ms: Option<i64>,
}

/// The player fields of a pulled match bundle's `match` section.
pub fn match_player_fields(bundle: &Value) -> PulledPlayerFields {
    let section = &bundle["match"];
    PulledPlayerFields {
        hidden_at_ms: loose::int(&section["hidden_at"]),
        no_league: loose::flag(&section["no_league"]),
        league_linked: loose::flag(&section["league_linked"]),
        edited_at_ms: loose::int(&section["edited_at"]),
    }
}

fn game_section(key: &str, game: &MatchGameInput) -> Value {
    let won = match game.result.as_str() {
        "win" => Value::Bool(true),
        "loss" | "draw" => Value::Bool(false),
        _ => Value::Null,
    };
    json!({
        "mtgo_id": key,
        "won": won,
        "started_at": datetime(game.started_at_ms),
        "ended_at": datetime(game.ended_at_ms),
        "turn_count": game.turn_count,
    })
}

/// One side's rows of a game, in catalog id order. In an Endstep bundle,
/// in card ref order instead (rows without a ref first), since local ids
/// differ between devices.
fn side_rows<'a>(
    input: &MatchBundleInput,
    game: &'a MatchGameInput,
    opponent: bool,
) -> Vec<&'a CardRowInput> {
    let mut rows: Vec<&CardRowInput> = game
        .cards
        .iter()
        .filter(|row| row.opponent == opponent)
        .collect();
    rows.sort_by_key(|row| row.catalog_id);
    if input.platform == BundlePlatform::Endstep {
        // Stable: rows with the same ref (or none) keep catalog id order.
        rows.sort_by_key(|row| input.endstep_cards.get(&row.catalog_id));
    }
    rows
}

/// How a bundle names one card: its catalog id, or in an Endstep bundle
/// the card's ref (spec section 5.3). An Endstep bundle never writes a
/// number: a card without a ref is null, as its local id means nothing on
/// another device.
fn card_value(input: &MatchBundleInput, catalog_id: u32) -> Value {
    if input.platform != BundlePlatform::Endstep {
        return json!(catalog_id);
    }
    match input.endstep_cards.get(&catalog_id) {
        Some(card) => card.to_value(),
        None => Value::Null,
    }
}

fn deck_entry(input: &MatchBundleInput, catalog_id: u32, quantity: u32, sideboard: bool) -> Value {
    json!({
        "mtgo_id": card_value(input, catalog_id),
        "quantity": quantity,
        "sideboard": sideboard,
    })
}

/// The local player's `deck_json`. A tracked game: each card's maindeck row
/// then its sideboard row (0.x `CreateGames::buildLocalDeck`). A manual
/// game: `[]` when played with the registered deck, else the postboard
/// maindeck then the registered deck's other copies as the sideboard (0.x
/// `UpdateManualGameSideboard`).
fn local_deck_json(input: &MatchBundleInput, game: &MatchGameInput) -> Value {
    let rows = side_rows(input, game, false);
    let mut entries = Vec::new();
    if input.manual() {
        let mains: Vec<&&CardRowInput> = rows.iter().filter(|row| row.main_qty > 0).collect();
        if mains.is_empty() {
            return json!([]);
        }
        let mut total: BTreeMap<u32, u32> = BTreeMap::new();
        for card in &input.registered {
            *total.entry(card.catalog_id).or_insert(0) += card.quantity;
        }
        for row in &mains {
            entries.push(deck_entry(input, row.catalog_id, row.main_qty, false));
        }
        for (catalog_id, quantity) in total {
            let main = mains
                .iter()
                .find(|row| row.catalog_id == catalog_id)
                .map_or(0, |row| row.main_qty);
            if quantity > main {
                entries.push(deck_entry(input, catalog_id, quantity - main, true));
            }
        }
        return Value::Array(entries);
    }
    for row in rows {
        if row.main_qty > 0 {
            entries.push(deck_entry(input, row.catalog_id, row.main_qty, false));
        }
        if row.side_qty > 0 {
            entries.push(deck_entry(input, row.catalog_id, row.side_qty, true));
        }
    }
    Value::Array(entries)
}

/// The opponent's `deck_json`: the cards seen (0.x
/// `CreateGames::buildOpponentDeck`), or a manual game's reveals (0.x
/// `UpdateManualGameReveals`, no sideboard key).
fn opponent_deck_json(input: &MatchBundleInput, game: &MatchGameInput) -> Value {
    let entries = side_rows(input, game, true)
        .into_iter()
        .filter(|row| row.seen > 0)
        .map(|row| {
            if input.manual() {
                json!({"mtgo_id": card_value(input, row.catalog_id), "quantity": row.seen})
            } else {
                deck_entry(input, row.catalog_id, row.seen, false)
            }
        })
        .collect();
    Value::Array(entries)
}

/// A manual game's hand-entered hand (0.x `UpdateManualGameHand`):
/// `{kept, bottomed, mulligans}`, each copy repeated, or null when none is
/// recorded. A tracked game's hand travels in its frames, as 0.x's did.
fn opening_hand_json(input: &MatchBundleInput, game: &MatchGameInput) -> Value {
    if !input.manual() || game.local_starting_hand_size.is_none() {
        return Value::Null;
    }
    let mut kept = Vec::new();
    let mut bottomed = Vec::new();
    for row in side_rows(input, game, false) {
        for _ in 0..row.starting_hand {
            kept.push(card_value(input, row.catalog_id));
        }
        for _ in 0..row.bottomed {
            bottomed.push(card_value(input, row.catalog_id));
        }
    }
    let mut mulligan_rows = game.mulligans.clone();
    mulligan_rows.sort_unstable();
    let recorded = mulligan_rows
        .iter()
        .map(|(hand, _, _)| *hand)
        .max()
        .unwrap_or(0);
    let hands = recorded.max(game.local_mulligans.unwrap_or(0).max(0) as u32);
    let mut mulligans = Vec::new();
    for hand in 1..=hands {
        let mut cards = Vec::new();
        for (number, catalog_id, quantity) in &mulligan_rows {
            if *number == hand {
                for _ in 0..*quantity {
                    cards.push(card_value(input, *catalog_id));
                }
            }
        }
        mulligans.push(cards);
    }
    json!({"kept": kept, "bottomed": bottomed, "mulligans": mulligans})
}

/// An Arena seat's `display_name` and `arena_user_id`, each only when set.
fn add_arena_fields(seat: &mut Value, fields: &ArenaPlayerFields) {
    let Some(seat) = seat.as_object_mut() else {
        return;
    };
    if let Some(name) = &fields.display_name {
        seat.insert("display_name".to_string(), json!(name));
    }
    if let Some(id) = &fields.arena_user_id {
        seat.insert("arena_user_id".to_string(), json!(id));
    }
}

fn players_section(input: &MatchBundleInput, games: &[(String, &MatchGameInput)]) -> Value {
    let mut players = Vec::new();
    for (key, game) in games {
        let mut seats = vec![json!({
            "game": key,
            "username": input.local_username,
            "is_local": true,
            "on_play": game.on_play.unwrap_or(false),
            "starting_hand_size": game.local_starting_hand_size.unwrap_or(7),
            "mulligan_count": game.local_mulligans,
            "dice_roll": game.local_die_roll,
            "deck_json": local_deck_json(input, game),
            "opening_hand_json": opening_hand_json(input, game),
        })];
        add_arena_fields(&mut seats[0], &input.local_player_fields);
        if let Some(opponent) = &input.opponent_username {
            let mut seat = json!({
                "game": key,
                "username": opponent,
                "is_local": false,
                "on_play": game.on_play.map(|on_play| !on_play).unwrap_or(false),
                "starting_hand_size": game.opp_starting_hand_size.unwrap_or(7),
                "mulligan_count": game.opp_mulligans,
                "dice_roll": game.opp_die_roll,
                "deck_json": opponent_deck_json(input, game),
                "opening_hand_json": Value::Null,
            });
            add_arena_fields(&mut seat, &input.opponent_player_fields);
            seats.push(seat);
        }
        seats.sort_by(|a, b| {
            php_compare(
                a["username"].as_str().unwrap_or_default(),
                b["username"].as_str().unwrap_or_default(),
            )
        });
        players.extend(seats);
    }
    Value::Array(players)
}

/// The sha256 of a frame's canonical content (0.x `contentHash`: a value
/// that is not a map or list hashes as `[]`).
fn content_hash(content: &Value) -> String {
    match content {
        Value::Object(_) | Value::Array(_) => bundle_hash(&canonical_json(content)),
        _ => bundle_hash("[]"),
    }
}

/// The timeline rows, game by game in bundle order (`frames` holds each
/// game's frames, by its index in `input.games`): by time, then content
/// hash. The hash only orders frames of the same millisecond, so only
/// those are hashed (both sorts are stable, so the order is the one a sort
/// by time and hash gives).
fn timelines_section(order: &[(String, usize)], mut frames: Vec<Vec<FrameInput>>) -> Value {
    let mut rows = Vec::new();
    for (key, index) in order {
        let index = *index;
        let mut game_frames = std::mem::take(&mut frames[index]);
        game_frames.sort_by_key(|frame| frame.at_ms);
        let mut start = 0;
        while start < game_frames.len() {
            let mut end = start + 1;
            while end < game_frames.len() && game_frames[end].at_ms == game_frames[start].at_ms {
                end += 1;
            }
            if end - start > 1 {
                game_frames[start..end].sort_by_cached_key(|frame| content_hash(&frame.content));
            }
            start = end;
        }
        for frame in game_frames {
            let mut row = serde_json::Map::new();
            row.insert("game".to_string(), Value::String(key.clone()));
            row.insert(
                "timestamp".to_string(),
                Value::String(bundle_datetime(frame.at_ms)),
            );
            row.insert("content".to_string(), frame.content);
            rows.push(Value::Object(row));
        }
    }
    Value::Array(rows)
}

/// One side's card stats of one oracle, summed over its printings.
#[derive(Default)]
struct Stat {
    quantity: u32,
    kept: u32,
    seen: u32,
    sided_in: bool,
    sided_out: bool,
    cast: u32,
    played: u32,
    activated: u32,
    pregame_revealed: bool,
    pregame_played: bool,
    methods: BTreeMap<String, u32>,
}

impl Stat {
    fn says_something(&self) -> bool {
        self.quantity > 0
            || self.kept > 0
            || self.seen > 0
            || self.sided_in
            || self.sided_out
            || self.cast > 0
            || self.played > 0
            || self.activated > 0
            || self.pregame_revealed
            || self.pregame_played
            || self.methods.values().any(|count| *count > 0)
    }
}

/// One game's stats by `(oracle_id, opponent)`, from its rows: the local
/// side's maindeck copies (`quantity`), starting hand (`kept`) and
/// sideboarding flags; both sides' `seen`, counters, alternative casts and
/// pregame flags (0.x `ComputeCardGameStats`). A card without an oracle id
/// is left out.
fn game_stats(game: &MatchGameInput) -> BTreeMap<(String, bool), Stat> {
    let mut stats: BTreeMap<(String, bool), Stat> = BTreeMap::new();
    let mut oracle_of: BTreeMap<(bool, u32), String> = BTreeMap::new();
    for row in &game.cards {
        let Some(oracle_id) = row.oracle_id.as_deref().map(str::trim) else {
            continue;
        };
        if oracle_id.is_empty() {
            continue;
        }
        oracle_of.insert((row.opponent, row.catalog_id), oracle_id.to_string());
        let stat = stats
            .entry((oracle_id.to_string(), row.opponent))
            .or_default();
        if !row.opponent {
            stat.quantity += row.main_qty;
            stat.kept += row.starting_hand;
            stat.sided_in |= row.sided_in > 0;
            stat.sided_out |= row.sided_out > 0;
        }
        stat.seen += row.seen;
        stat.cast += row.cast;
        stat.played += row.played.unwrap_or(0);
        stat.activated += row.activated.unwrap_or(0);
        stat.pregame_revealed |= row.pregame_revealed > 0;
        stat.pregame_played |= row.pregame_played > 0;
    }
    for cast in &game.casts {
        let Some(oracle_id) = oracle_of.get(&(cast.opponent, cast.catalog_id)) else {
            continue;
        };
        if let Some(stat) = stats.get_mut(&(oracle_id.clone(), cast.opponent)) {
            *stat.methods.entry(cast.method.clone()).or_insert(0) += cast.count;
        }
    }
    stats.retain(|_, stat| stat.says_something());
    stats
}

fn stat_row(
    key: &str,
    oracle_id: &str,
    opponent: bool,
    game: &MatchGameInput,
    stat: &Stat,
) -> Value {
    let mut row = Map::new();
    row.insert("game".to_string(), json!(key));
    row.insert("oracle_id".to_string(), json!(oracle_id));
    row.insert("quantity".to_string(), json!(stat.quantity));
    row.insert("kept".to_string(), json!(stat.kept));
    row.insert("seen".to_string(), json!(stat.seen));
    row.insert("won".to_string(), json!(game.result == "win"));
    row.insert("is_postboard".to_string(), json!(game.game_number > 1));
    row.insert("sided_out".to_string(), json!(stat.sided_out));
    row.insert("cast".to_string(), json!(stat.cast));
    row.insert("sided_in".to_string(), json!(stat.sided_in));
    row.insert("played".to_string(), json!(stat.played));
    row.insert("activated".to_string(), json!(stat.activated));
    row.insert("pregame_revealed".to_string(), json!(stat.pregame_revealed));
    row.insert("pregame_played".to_string(), json!(stat.pregame_played));
    row.insert("opponent".to_string(), json!(opponent));
    for method in CAST_METHODS {
        let count = stat.methods.get(method).copied().unwrap_or(0);
        row.insert(method.to_string(), json!(count));
    }
    Value::Object(row)
}

/// The card stats, sorted by `(game, oracle_id, is_postboard, opponent)`.
/// v1 keeps no stats for a manual match (0.x recomputed them from the hand
/// and lists; target schema section 6), so it has none.
fn card_stats_section(input: &MatchBundleInput, games: &[(String, &MatchGameInput)]) -> Value {
    if input.manual() {
        return json!([]);
    }
    let mut rows: Vec<(String, String, bool, Value)> = Vec::new();
    for (key, game) in games {
        for ((oracle_id, opponent), stat) in game_stats(game) {
            let row = stat_row(key, &oracle_id, opponent, game, &stat);
            rows.push((key.clone(), oracle_id, opponent, row));
        }
    }
    rows.sort_by(|a, b| {
        php_compare(&a.0, &b.0)
            .then_with(|| php_compare(&a.1, &b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    Value::Array(rows.into_iter().map(|(_, _, _, row)| row).collect())
}

/// PHP `number_format($value, 2, '.', '')`: PHP rounds half away from zero
/// after pre-rounding the scaled value to 15 significant digits, so 0.285
/// (0.28499... in binary) gives `0.29` as in 0.x.
pub fn confidence_text(value: f64) -> String {
    let scaled = value * 100.0;
    let digits_before_point = if scaled.abs() >= 1.0 {
        scaled.abs().log10().floor() as usize + 1
    } else {
        1
    };
    let places = 15usize.saturating_sub(digits_before_point);
    let pre: f64 = format!("{scaled:.places$}").parse().unwrap_or(scaled);
    let rounded = pre.round() / 100.0;
    let text = format!("{rounded:.2}");
    // PHP never writes a negative zero.
    if text == "-0.00" {
        return "0.00".to_string();
    }
    text
}

/// The archetype rows (0.x `archetypesPayload`): sorted by `(uuid,
/// username)` (the username breaks a mirror match's tie), the local seat
/// under the account's username, the opponent's under the opponent's.
fn archetypes_section(input: &MatchBundleInput) -> Value {
    let mut rows: Vec<(&ArchetypeSide, Option<&str>)> = input
        .archetypes
        .iter()
        .map(|side| {
            let username = if side.opponent {
                input.opponent_username.as_deref()
            } else {
                Some(input.local_username.as_str())
            };
            (side, username)
        })
        .collect();
    rows.sort_by(|(a, a_name), (b, b_name)| {
        php_compare(&a.uuid, &b.uuid)
            .then_with(|| php_compare(a_name.unwrap_or(""), b_name.unwrap_or("")))
    });
    let out: Vec<Value> = rows
        .into_iter()
        .map(|(side, username)| {
            json!({
                "uuid": side.uuid,
                "confidence": confidence_text(side.confidence),
                "manual": side.manual,
                "player_username": username,
                "player_is_player": !side.opponent,
            })
        })
        .collect();
    Value::Array(out)
}

/// The match bundle: `match`, `games` (by game id), `players` (by username
/// within a game), `timelines` (by time, then content hash), `card_stats`
/// and `archetypes`, all from the rows. A carry (now empty for matches)
/// still applies.
pub fn match_bundle(input: &MatchBundleInput, carry: Option<&Carry>) -> Value {
    let frames = input.games.iter().map(|game| game.frames.clone()).collect();
    build_match_bundle(input, frames, carry)
}

/// `match_bundle` from an input it may take apart: the frames move into
/// the bundle rather than being copied (the 1,000-match rebuild's speed
/// budget, foundation spec section 8).
pub fn match_bundle_owned(mut input: MatchBundleInput, carry: Option<&Carry>) -> Value {
    let frames = input
        .games
        .iter_mut()
        .map(|game| std::mem::take(&mut game.frames))
        .collect();
    build_match_bundle(&input, frames, carry)
}

/// The match bundle of `input`, with `frames` holding each game's frames
/// (by its index in `input.games`; the input's own are not read).
fn build_match_bundle(
    input: &MatchBundleInput,
    frames: Vec<Vec<FrameInput>>,
    carry: Option<&Carry>,
) -> Value {
    let order = sorted_game_order(input);
    let games = sorted_games(input, &order);
    let mut bundle = json!({
        "match": match_section(input),
        "games": games.iter().map(|(key, game)| game_section(key, game)).collect::<Vec<_>>(),
        "players": players_section(input, &games),
        "card_stats": card_stats_section(input, &games),
        "archetypes": archetypes_section(input),
    });
    if input.platform == BundlePlatform::Mtgo {
        // Placed after the `json!`, which would copy every frame.
        bundle["timelines"] = timelines_section(&order, frames);
    } else if let Some(sources) = &input.sources {
        bundle["sources"] = sources_section(sources);
    }
    add_platform(&mut bundle, input.platform);
    if let Some(carry) = carry {
        carry.apply(&mut bundle);
    }
    bundle
}

/// The archetype uuid of one side (0.x `sideArchetypeUuid`): the first
/// archetype row whose player is (or is not) a non-local player of a game.
fn side_archetype(bundle: &Value, opponent: bool) -> Value {
    let opponents: Vec<&Value> = bundle["players"]
        .as_array()
        .map(|players| {
            players
                .iter()
                .filter(|player| player["is_local"] == Value::Bool(false))
                .map(|player| &player["username"])
                .collect()
        })
        .unwrap_or_default();
    let found = bundle["archetypes"].as_array().and_then(|rows| {
        rows.iter().find(|row| {
            let username = &row["player_username"];
            opponents.contains(&username) == opponent
        })
    });
    found.map_or(Value::Null, |row| row["uuid"].clone())
}

/// The sidecar sent beside the blob (0.x `MatchBundleBuilder::sidecar`),
/// every field from the bundle, nulls dropped.
pub fn match_sidecar(bundle: &Value) -> Value {
    let section = &bundle["match"];
    let deck_client_id = section["deck_mtgo_id"]
        .as_str()
        .map_or(Value::Null, |id| Value::String(deck_client_id(id)));
    let fields = [
        ("occurred_at", section["started_at"].clone()),
        ("format", section["format"].clone()),
        ("match_type", section["match_type"].clone()),
        ("result", section["result"].clone()),
        ("outcome", section["outcome"].clone()),
        ("games_won", section["games_won"].clone()),
        ("games_lost", section["games_lost"].clone()),
        ("deck_client_id", deck_client_id),
        ("archetype_uuid", side_archetype(bundle, false)),
        ("opponent_archetype_uuid", side_archetype(bundle, true)),
    ];
    let mut sidecar = Map::new();
    for (key, value) in fields {
        if !value.is_null() {
            sidecar.insert(key.to_string(), value);
        }
    }
    let mut sidecar = Value::Object(sidecar);
    copy_platform(bundle, &mut sidecar);
    sidecar
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::{bundle_hash, canonical_json};
    use serde_json::{Value, json};

    #[test]
    fn mtgo_bundle_has_no_platform_key() {
        let bundle = match_bundle(&sample(), None);
        assert!(bundle.get("platform").is_none());
        assert!(bundle.get("sources").is_none());
        assert!(bundle.get("timelines").is_some());
        for seat in bundle["players"].as_array().expect("players") {
            assert!(seat.get("display_name").is_none());
            assert!(seat.get("arena_user_id").is_none());
        }
    }

    fn arena_sources() -> MatchSources {
        use crate::sources::{ArenaGameSource, ArenaSourceMessage, ArenaSources};
        MatchSources::Arena(ArenaSources {
            event_id: Some("Play_Brawl_Historic".to_string()),
            games: vec![ArenaGameSource {
                game_number: 1,
                messages: vec![ArenaSourceMessage {
                    source: "gre".to_string(),
                    logged_at_ms: T,
                    raw: "{}".to_string(),
                }],
            }],
        })
    }

    #[test]
    fn arena_bundle_carries_platform_and_sources() {
        let mut input = sample();
        input.platform = BundlePlatform::Arena;
        input.sources = Some(arena_sources());
        input.local_player_fields.display_name = Some("Fa".to_string());
        input.opponent_player_fields.arena_user_id = Some("BBB".to_string());
        let bundle = match_bundle(&input, None);
        assert_eq!(bundle["platform"], json!("arena"));
        assert!(bundle.get("timelines").is_none());
        let platform = BundlePlatform::of_bundle(&bundle).expect("platform");
        assert_eq!(
            crate::sources::read_sources(&bundle, platform),
            Ok(input.sources.clone())
        );
        let seats = bundle["players"].as_array().expect("players");
        let local = seats
            .iter()
            .find(|s| s["is_local"] == json!(true))
            .expect("local");
        let other = seats
            .iter()
            .find(|s| s["is_local"] == json!(false))
            .expect("other");
        assert_eq!(local["display_name"], json!("Fa"));
        assert!(local.get("arena_user_id").is_none());
        assert_eq!(other["arena_user_id"], json!("BBB"));
    }

    #[test]
    fn endstep_bundle_round_trips_sources() {
        use crate::sources::{EndstepSourceUnit, EndstepSources};
        let sources = MatchSources::Endstep(EndstepSources {
            forge_game_id: "g-1".to_string(),
            units: vec![EndstepSourceUnit {
                unit_id: "u1".to_string(),
                lines: vec!["a".to_string()],
            }],
        });
        let mut input = sample();
        input.platform = BundlePlatform::Endstep;
        input.sources = Some(sources.clone());
        let bundle = match_bundle(&input, None);
        assert_eq!(bundle["platform"], json!("endstep"));
        assert_eq!(
            crate::sources::read_sources(&bundle, BundlePlatform::Endstep),
            Ok(Some(sources))
        );
    }

    #[test]
    fn unknown_platform_is_an_error() {
        let mut bundle = match_bundle(&sample(), None);
        bundle["platform"] = json!("mtga");
        assert!(BundlePlatform::of_bundle(&bundle).is_err());
    }

    #[test]
    fn sidecar_copies_platform() {
        let mut input = sample();
        assert!(
            match_sidecar(&match_bundle(&input, None))
                .get("platform")
                .is_none()
        );
        input.platform = BundlePlatform::Arena;
        let sidecar = match_sidecar(&match_bundle(&input, None));
        assert_eq!(sidecar["platform"], json!("arena"));
    }

    /// Arena and Endstep bundles carry the match's kind (`bo1`, `bo3`), which
    /// the deck pages filter on; an MTGO bundle never does, so its bytes stay
    /// those of the goldens.
    #[test]
    fn only_non_mtgo_bundles_carry_the_game_structure() {
        let mut input = sample();
        input.game_structure = Some("bo1".to_string());
        assert!(
            match_bundle(&input, None)["match"]
                .get("game_structure")
                .is_none()
        );
        for platform in [BundlePlatform::Arena, BundlePlatform::Endstep] {
            input.platform = platform;
            let bundle = match_bundle(&input, None);
            assert_eq!(bundle["match"]["game_structure"], json!("bo1"));
            assert_eq!(match_game_structure(&bundle), Some("bo1".to_string()));
        }
        input.game_structure = None;
        let bundle = match_bundle(&input, None);
        assert!(bundle["match"].get("game_structure").is_none());
        assert_eq!(match_game_structure(&bundle), None);
    }

    /// The sample as an Endstep match whose cards are Endstep catalog ids
    /// (`ENDSTEP_ID_OFFSET` 200,000,000 plus a row id), each with its ref.
    fn endstep_sample() -> MatchBundleInput {
        let mut input = sample();
        input.platform = BundlePlatform::Endstep;
        for game in &mut input.games {
            for row in &mut game.cards {
                row.catalog_id += 200_000_000;
            }
            game.cards[0].side_qty = 1;
        }
        input.endstep_cards = BTreeMap::from([
            (
                200_001_001,
                EndstepCardRef {
                    name: "Qmine".to_string(),
                    set: "m10".to_string(),
                    number: "146".to_string(),
                },
            ),
            (
                200_002_001,
                EndstepCardRef {
                    name: "Qtheirs".to_string(),
                    set: String::new(),
                    number: String::new(),
                },
            ),
        ]);
        input
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
    fn an_endstep_bundle_names_each_card_by_its_ref() {
        let input = endstep_sample();
        let bundle = match_bundle(&input, None);
        let mut found = Vec::new();
        numbers(&bundle, &mut found);
        assert!(
            found.iter().all(|number| *number < 200_000_000),
            "{found:?}"
        );
        let seats = bundle["players"].as_array().expect("players");
        let local = seats
            .iter()
            .find(|s| s["is_local"] == json!(true))
            .expect("local");
        assert_eq!(
            local["deck_json"],
            json!([
                {"mtgo_id": {"name": "Qmine", "set": "m10", "number": "146"},
                 "quantity": 4, "sideboard": false},
                {"mtgo_id": {"name": "Qmine", "set": "m10", "number": "146"},
                 "quantity": 1, "sideboard": true},
            ])
        );
        let other = seats
            .iter()
            .find(|s| s["is_local"] == json!(false))
            .expect("other");
        assert_eq!(
            other["deck_json"][0]["mtgo_id"],
            json!({"name": "Qtheirs", "set": "", "number": ""})
        );
        assert_eq!(
            EndstepCardRef::from_value(&other["deck_json"][0]["mtgo_id"]),
            Some(input.endstep_cards[&200_002_001].clone())
        );
    }

    #[test]
    fn a_manual_endstep_hand_names_its_cards_by_ref() {
        let mut input = endstep_sample();
        input.source = "manual".to_string();
        input.games[0].mulligans = vec![(1, 200_001_001, 2)];
        input.games[0].local_mulligans = Some(1);
        let bundle = match_bundle(&input, None);
        let mut found = Vec::new();
        numbers(&bundle, &mut found);
        assert!(
            found.iter().all(|number| *number < 200_000_000),
            "{found:?}"
        );
        let mine = input.endstep_cards[&200_001_001].to_value();
        let local = bundle["players"]
            .as_array()
            .expect("players")
            .iter()
            .find(|s| s["is_local"] == json!(true) && s["game"] == json!("945500844"))
            .expect("game 1 local");
        assert_eq!(local["opening_hand_json"]["kept"], json!([mine.clone()]));
        assert_eq!(
            local["opening_hand_json"]["mulligans"],
            json!([[mine.clone(), mine]])
        );
    }

    #[test]
    fn an_endstep_card_without_a_ref_is_never_its_local_number() {
        let mut input = endstep_sample();
        let mut stray = card(false, 200_009_999, "oracle-stray");
        stray.main_qty = 1;
        input.games[0].cards.push(stray);
        let bundle = match_bundle(&input, None);
        let mut found = Vec::new();
        numbers(&bundle, &mut found);
        assert!(
            found.iter().all(|number| *number < 200_000_000),
            "{found:?}"
        );
        let local = bundle["players"]
            .as_array()
            .expect("players")
            .iter()
            .find(|s| s["is_local"] == json!(true) && s["game"] == json!("945500844"))
            .expect("game 1 local");
        let nulls = local["deck_json"]
            .as_array()
            .expect("deck")
            .iter()
            .filter(|entry| entry["mtgo_id"].is_null())
            .count();
        assert_eq!(nulls, 1);
    }

    #[test]
    fn endstep_deck_entries_follow_their_refs_not_local_ids() {
        let mut input = endstep_sample();
        // A second card whose local id sorts after Qmine but whose name
        // sorts before it.
        let mut early = card(false, 200_001_002, "oracle-early");
        early.main_qty = 2;
        input.games[0].cards.push(early);
        input.endstep_cards.insert(
            200_001_002,
            EndstepCardRef {
                name: "Qaaa".to_string(),
                set: String::new(),
                number: String::new(),
            },
        );
        let bundle = match_bundle(&input, None);
        let local = bundle["players"]
            .as_array()
            .expect("players")
            .iter()
            .find(|s| s["is_local"] == json!(true) && s["game"] == json!("945500844"))
            .expect("game 1 local");
        let names: Vec<&str> = local["deck_json"]
            .as_array()
            .expect("deck")
            .iter()
            .map(|entry| entry["mtgo_id"]["name"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(names, vec!["Qaaa", "Qmine", "Qmine"]);
    }

    #[test]
    fn only_an_endstep_bundle_writes_refs() {
        let mut input = endstep_sample();
        input.platform = BundlePlatform::Arena;
        let bundle = match_bundle(&input, None);
        let local = bundle["players"]
            .as_array()
            .expect("players")
            .iter()
            .find(|s| s["is_local"] == json!(true))
            .expect("local");
        assert_eq!(local["deck_json"][0]["mtgo_id"], json!(200_001_001));
    }

    fn ranked() -> EndstepRankedFields {
        EndstepRankedFields {
            status: Some("done".to_string()),
            format_id: Some("Modern".to_string()),
            rating_before: Some(1500),
            rating_after: Some(1488),
            score_before: None,
            score_after: None,
            delta: Some(-12),
            rd_before: Some(80),
            rd_after: Some(78),
            rank: Some("Gold".to_string()),
        }
    }

    #[test]
    fn an_endstep_bundle_carries_its_ranked_result() {
        let mut input = endstep_sample();
        input.endstep_ranked = Some(ranked());
        let bundle = match_bundle(&input, None);
        assert_eq!(
            bundle["match"]["endstep_ranked"],
            json!({
                "status": "done", "format_id": "Modern", "rating_before": 1500,
                "rating_after": 1488, "score_before": null, "score_after": null,
                "delta": -12, "rd_before": 80, "rd_after": 78, "rank": "Gold",
            })
        );
        assert_eq!(match_endstep_ranked(&bundle), Some(ranked()));
        assert_eq!(
            match_endstep_ranked(&match_bundle(&endstep_sample(), None)),
            None
        );
        // MTGO and Arena bundles never carry it.
        let mut mtgo = sample();
        mtgo.endstep_ranked = Some(ranked());
        let bundle = match_bundle(&mtgo, None);
        assert!(bundle["match"].get("endstep_ranked").is_none());
        assert_eq!(match_endstep_ranked(&bundle), None);
    }

    // League self-management spec 6.1: a match without a player edit keeps
    // every byte (the 0.x goldens prove the rest).
    #[test]
    fn a_match_without_player_fields_writes_none_of_their_keys() {
        let bundle = match_bundle(&sample(), None);
        for key in ["hidden_at", "no_league", "league_linked", "edited_at"] {
            assert!(bundle["match"].get(key).is_none(), "{key}");
        }
        assert_eq!(match_player_fields(&bundle), PulledPlayerFields::default());
    }

    // Spec 6.1: player times are integer ms (a delete and its Undo can fall
    // in one second); the flags are written only when true.
    #[test]
    fn writes_and_reads_the_player_fields_when_set() {
        let mut input = sample();
        input.hidden_at_ms = Some(T + 1);
        input.no_league = true;
        input.edited_at_ms = Some(T + 2);
        let bundle = match_bundle(&input, None);
        assert_eq!(bundle["match"]["hidden_at"], json!(T + 1));
        assert_eq!(bundle["match"]["no_league"], json!(true));
        assert!(bundle["match"].get("league_linked").is_none());
        assert_eq!(bundle["match"]["edited_at"], json!(T + 2));
        assert_ne!(
            canonical_json(&bundle),
            canonical_json(&match_bundle(&sample(), None))
        );
        assert_eq!(
            match_player_fields(&bundle),
            PulledPlayerFields {
                hidden_at_ms: Some(T + 1),
                no_league: true,
                league_linked: false,
                edited_at_ms: Some(T + 2),
            }
        );
        let mut linked = sample();
        linked.league_linked = true;
        linked.edited_at_ms = Some(T + 3);
        let bundle = match_bundle(&linked, None);
        assert_eq!(bundle["match"]["league_linked"], json!(true));
        assert!(match_player_fields(&bundle).league_linked);
    }

    // Read loosely, as every pulled value: numeric text is a number, a
    // value that is neither is not recorded.
    #[test]
    fn reads_player_fields_loosely() {
        let bundle = json!({"match": {
            "hidden_at": "1787133605001", "no_league": 1, "league_linked": false,
            "edited_at": "soon",
        }});
        assert_eq!(
            match_player_fields(&bundle),
            PulledPlayerFields {
                hidden_at_ms: Some(1_787_133_605_001),
                no_league: true,
                league_linked: false,
                edited_at_ms: None,
            }
        );
        assert_eq!(
            match_player_fields(&json!({})),
            PulledPlayerFields::default()
        );
    }

    const T: i64 = 1_787_133_605_000;

    fn card(opponent: bool, catalog_id: u32, oracle: &str) -> CardRowInput {
        CardRowInput {
            opponent,
            catalog_id,
            oracle_id: Some(oracle.to_string()),
            ..CardRowInput::default()
        }
    }

    fn game(number: u32, mtgo_game_id: i64, result: &str) -> MatchGameInput {
        let mut local = card(false, 1001, &format!("oracle-{number}-mine"));
        local.main_qty = 4;
        local.starting_hand = 1;
        local.opening_hand = 1;
        let mut theirs = card(true, 2001, &format!("oracle-{number}-theirs"));
        theirs.seen = 2;
        MatchGameInput {
            game_number: number,
            mtgo_game_id: Some(mtgo_game_id),
            result: result.to_string(),
            on_play: Some(number == 1),
            local_mulligans: Some(0),
            opp_mulligans: Some(1),
            local_die_roll: Some(5),
            opp_die_roll: Some(2),
            turn_count: Some(7 + i64::from(number)),
            local_starting_hand_size: Some(7),
            opp_starting_hand_size: Some(6),
            started_at_ms: Some(T + i64::from(number) * 60_000),
            ended_at_ms: Some(T + i64::from(number) * 60_000 + 30_000),
            cards: vec![local, theirs],
            casts: Vec::new(),
            mulligans: Vec::new(),
            frames: vec![FrameInput {
                at_ms: T + i64::from(number) * 60_000 + 1_000,
                content: json!({"turn": number}),
            }],
        }
    }

    fn sample() -> MatchBundleInput {
        MatchBundleInput {
            platform: BundlePlatform::Mtgo,
            sources: None,
            endstep_cards: BTreeMap::new(),
            endstep_ranked: None,
            local_player_fields: Default::default(),
            opponent_player_fields: Default::default(),
            token: "q-match-token".to_string(),
            mtgo_match_id: Some(285_746_550),
            source: "log".to_string(),
            format: Some("CMODERN".to_string()),
            match_type: Some("Constructed".to_string()),
            game_structure: None,
            state: "complete".to_string(),
            outcome: "win".to_string(),
            started_at_ms: T,
            ended_at_ms: Some(T + 600_000),
            notes: Some("Qnote".to_string()),
            games_won_imported: None,
            games_lost_imported: None,
            deck_mtgo_id: Some("12345".to_string()),
            deck_version_signature: Some("q-signature".to_string()),
            registered: Vec::new(),
            league_client_id: Some("q-league_20260819T100005Z".to_string()),
            tournament_event_id: None,
            tournament_round: None,
            local_username: "Qplayer".to_string(),
            opponent_username: Some("Qfoe".to_string()),
            games: vec![game(1, 945_500_844, "win"), game(2, 945_500_845, "loss")],
            archetypes: Vec::new(),
            hidden_at_ms: None,
            no_league: false,
            league_linked: false,
            edited_at_ms: None,
        }
    }

    fn side(uuid: &str, confidence: f64, manual: bool, opponent: bool) -> ArchetypeSide {
        ArchetypeSide {
            uuid: uuid.to_string(),
            confidence,
            manual,
            opponent,
        }
    }

    /// The sample with both seats' `match_archetypes` rows, opponent first.
    fn with_archetypes() -> MatchBundleInput {
        let mut input = sample();
        input.archetypes = vec![
            side("bbbbbbbb-0000-4000-8000-000000000000", 0.5, false, true),
            side("aaaaaaaa-0000-4000-8000-000000000000", 1.0, false, false),
        ];
        input
    }

    fn keys(value: &Value) -> Vec<String> {
        value.as_object().expect("a map").keys().cloned().collect()
    }

    // 0.x "builds a match bundle carrying games, players, timelines, card
    // stats and archetype sides".
    #[test]
    fn builds_a_match_bundle_carrying_games_players_timelines_card_stats_and_archetype_sides() {
        let bundle = match_bundle(&with_archetypes(), None);
        let mut top = keys(&bundle);
        top.sort();
        assert_eq!(
            top,
            [
                "archetypes",
                "card_stats",
                "games",
                "match",
                "players",
                "timelines"
            ]
        );
        let mut match_keys = keys(&bundle["match"]);
        match_keys.sort();
        assert_eq!(
            match_keys,
            [
                "deck_mtgo_id",
                "deck_version_signature",
                "ended_at",
                "format",
                "games_lost",
                "games_won",
                "imported",
                "league_client_id",
                "manual",
                "match_type",
                "mtgo_id",
                "notes",
                "outcome",
                "result",
                "started_at",
                "state",
                "token",
                "tournament_mtgo_event_id",
                "tournament_round",
            ]
        );
        assert_eq!(bundle["match"]["token"], json!("q-match-token"));
        assert_eq!(bundle["match"]["mtgo_id"], json!("285746550"));
        assert_eq!(bundle["match"]["result"], json!("1-1"));
        assert_eq!(bundle["match"]["games_won"], json!(1));
        assert_eq!(bundle["match"]["games_lost"], json!(1));
        assert_eq!(bundle["match"]["imported"], json!(false));
        assert_eq!(bundle["match"]["manual"], json!(false));
        assert_eq!(bundle["games"].as_array().expect("games").len(), 2);
        assert_eq!(bundle["games"][0]["mtgo_id"], json!("945500844"));
        assert_eq!(bundle["games"][0]["won"], json!(true));
        let players = bundle["players"].as_array().expect("players");
        assert_eq!(players.len(), 4);
        // Sorted by username within a game: Qfoe before Qplayer.
        assert_eq!(players[0]["username"], json!("Qfoe"));
        assert_eq!(players[0]["mulligan_count"], json!(1));
        assert_eq!(players[0]["dice_roll"], json!(2));
        assert_eq!(players[0]["on_play"], json!(false));
        assert_eq!(
            players[0]["deck_json"],
            json!([{"mtgo_id": 2001, "quantity": 2, "sideboard": false}])
        );
        assert_eq!(players[1]["username"], json!("Qplayer"));
        assert_eq!(players[1]["is_local"], json!(true));
        assert_eq!(players[1]["mulligan_count"], json!(0));
        assert_eq!(players[1]["dice_roll"], json!(5));
        assert_eq!(players[1]["starting_hand_size"], json!(7));
        assert_eq!(players[1]["opening_hand_json"], Value::Null);
        assert_eq!(
            players[1]["deck_json"],
            json!([{"mtgo_id": 1001, "quantity": 4, "sideboard": false}])
        );
        let stats = bundle["card_stats"].as_array().expect("stats");
        assert_eq!(stats.len(), 4);
        assert_eq!(keys(&stats[0]).len(), 38);
        assert_eq!(stats[0]["oracle_id"], json!("oracle-1-mine"));
        assert_eq!(stats[0]["quantity"], json!(4));
        assert_eq!(stats[0]["kept"], json!(1));
        assert_eq!(stats[0]["won"], json!(true));
        assert_eq!(stats[1]["oracle_id"], json!("oracle-1-theirs"));
        assert_eq!(stats[1]["opponent"], json!(true));
        assert_eq!(stats[1]["seen"], json!(2));
        assert_eq!(stats[2]["is_postboard"], json!(true));
        assert_eq!(bundle["timelines"].as_array().expect("timelines").len(), 2);
        assert_eq!(
            bundle["archetypes"],
            json!([
                {"uuid": "aaaaaaaa-0000-4000-8000-000000000000", "confidence": "1.00",
                 "manual": false, "player_username": "Qplayer", "player_is_player": true},
                {"uuid": "bbbbbbbb-0000-4000-8000-000000000000", "confidence": "0.50",
                 "manual": false, "player_username": "Qfoe", "player_is_player": false},
            ])
        );
    }

    // 0.x `archetypesPayload`: uuid alone ties on a mirror match, so the
    // username breaks the tie.
    #[test]
    fn a_mirror_match_sorts_its_archetype_rows_by_username() {
        let mut input = sample();
        input.archetypes = vec![
            side("q-same-uuid", 0.9, false, false),
            side("q-same-uuid", 0.25, true, true),
        ];
        let bundle = match_bundle(&input, None);
        let names: Vec<&str> = bundle["archetypes"]
            .as_array()
            .expect("rows")
            .iter()
            .map(|row| row["player_username"].as_str().expect("name"))
            .collect();
        assert_eq!(names, vec!["Qfoe", "Qplayer"]);
        assert_eq!(bundle["archetypes"][0]["manual"], json!(true));
        assert_eq!(bundle["archetypes"][0]["confidence"], json!("0.25"));
    }

    // 0.x `number_format($confidence, 2, '.', '')`: PHP rounds half away
    // from zero after pre-rounding, so 0.125 and 0.285 round up.
    #[test]
    fn confidence_is_written_as_php_number_format_does() {
        for (confidence, text) in [
            (1.0, "1.00"),
            (0.0, "0.00"),
            (0.5, "0.50"),
            (0.125, "0.13"),
            (0.285, "0.29"),
            (0.8449, "0.84"),
            (0.999, "1.00"),
            (0.1234, "0.12"),
        ] {
            assert_eq!(confidence_text(confidence), text, "{confidence}");
        }
    }

    // 0.x "is byte-stable across two builds of the same match".
    #[test]
    fn is_byte_stable_across_two_builds_of_the_same_match() {
        assert_eq!(
            canonical_json(&match_bundle(&with_archetypes(), None)),
            canonical_json(&match_bundle(&with_archetypes(), None))
        );
    }

    // 0.x "carries null deck references for a limited match".
    #[test]
    fn carries_null_deck_references_for_a_limited_match() {
        let mut input = sample();
        input.format = Some("DZNR".to_string());
        input.deck_mtgo_id = None;
        input.deck_version_signature = None;
        let bundle = match_bundle(&input, None);
        assert_eq!(bundle["match"]["deck_mtgo_id"], Value::Null);
        assert_eq!(bundle["match"]["deck_version_signature"], Value::Null);
        assert!(match_sidecar(&bundle).get("deck_client_id").is_none());
    }

    // 0.x "emits every datetime as UTC ISO-8601 seconds".
    #[test]
    fn emits_every_datetime_as_utc_iso_8601_seconds() {
        let bundle = match_bundle(&sample(), None);
        let utc = |value: &Value| {
            let text = value.as_str().expect("a datetime");
            text.len() == 20 && text.ends_with('Z') && text.as_bytes()[10] == b'T'
        };
        assert!(utc(&bundle["match"]["started_at"]));
        assert!(utc(&bundle["match"]["ended_at"]));
        assert!(utc(&bundle["games"][0]["started_at"]));
        assert!(utc(&bundle["timelines"][0]["timestamp"]));
        assert_eq!(bundle["match"]["started_at"], json!("2026-08-19T10:00:05Z"));
    }

    fn frame_hash(content: &Value) -> String {
        bundle_hash(&canonical_json(content))
    }

    // 0.x "orders same-second timelines by content hash, not insertion order
    // or row id".
    #[test]
    fn orders_same_second_timelines_by_content_hash_not_insertion_order() {
        let a = json!({"note": "alpha-marker"});
        let b = json!({"note": "bravo-marker"});
        let (lower, higher) = if frame_hash(&a) < frame_hash(&b) {
            (a, b)
        } else {
            (b, a)
        };
        let mut input = sample();
        input.games[0].frames = vec![
            FrameInput {
                at_ms: T,
                content: higher.clone(),
            },
            FrameInput {
                at_ms: T,
                content: lower.clone(),
            },
        ];
        let bundle = match_bundle(&input, None);
        let rows: Vec<&Value> = bundle["timelines"]
            .as_array()
            .expect("timelines")
            .iter()
            .filter(|row| row["game"] == json!("945500844"))
            .collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["content"], lower);
        assert_eq!(rows[1]["content"], higher);
    }

    // The rebuild speed fix (foundation spec section 8, the 1,000-match
    // rebuild): only frames sharing a millisecond are hashed, and the owned
    // build moves its frames; the order is still a sort by time, then hash.
    #[test]
    fn the_owned_build_orders_frames_by_time_then_hash_as_the_borrowed_one() {
        let mut input = sample();
        let times = [T + 5, T, T + 2, T, T + 5, T + 1, T, T + 5, T + 2, T + 9];
        input.games[0].frames = times
            .iter()
            .enumerate()
            .map(|(index, at_ms)| FrameInput {
                at_ms: *at_ms,
                content: json!({"frame": index, "Cards": [{"Id": index}]}),
            })
            .collect();
        let mut expected: Vec<(i64, String, Value)> = input.games[0]
            .frames
            .iter()
            .map(|frame| {
                (
                    frame.at_ms,
                    frame_hash(&frame.content),
                    frame.content.clone(),
                )
            })
            .collect();
        expected.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
        let borrowed = match_bundle(&input, None);
        let owned = match_bundle_owned(input, None);
        assert_eq!(canonical_json(&owned), canonical_json(&borrowed));
        let contents: Vec<Value> = owned["timelines"]
            .as_array()
            .expect("timelines")
            .iter()
            .filter(|row| row["game"] == json!("945500844"))
            .map(|row| row["content"].clone())
            .collect();
        let expected: Vec<Value> = expected
            .into_iter()
            .map(|(_, _, content)| content)
            .collect();
        assert_eq!(contents, expected);
    }

    // 0.x "orders same-second timelines by millisecond before falling back
    // to content hash".
    #[test]
    fn orders_same_second_timelines_by_millisecond_before_the_content_hash() {
        let a = json!({"note": "alpha-marker"});
        let b = json!({"note": "bravo-marker"});
        let (lower, higher) = if frame_hash(&a) < frame_hash(&b) {
            (a, b)
        } else {
            (b, a)
        };
        let mut input = sample();
        input.games[0].frames = vec![
            FrameInput {
                at_ms: T + 900,
                content: lower.clone(),
            },
            FrameInput {
                at_ms: T + 100,
                content: higher.clone(),
            },
        ];
        let bundle = match_bundle(&input, None);
        let rows: Vec<&Value> = bundle["timelines"]
            .as_array()
            .expect("timelines")
            .iter()
            .filter(|row| row["game"] == json!("945500844"))
            .collect();
        assert_eq!(rows[0]["content"], higher);
        assert_eq!(rows[1]["content"], lower);
    }

    // 0.x "dates frames of a game that crossed local midnight from its end,
    // in play order". v1's events carry their UTC time already (the log
    // ingest dates each line from its file across midnight), so a frame at
    // 23:48:07.250 local on the 26th and one at 00:00:39.500 local on the
    // 27th (Europe/London, BST) come in as those instants.
    #[test]
    fn dates_frames_of_a_game_that_crossed_local_midnight_in_play_order() {
        let before_midnight = 1_790_462_887_250; // 2026-09-26T22:48:07.250Z
        let after_midnight = 1_790_463_639_500; // 2026-09-26T23:00:39.500Z
        let mut input = sample();
        input.games[0].ended_at_ms = Some(1_790_463_670_000);
        input.games[0].frames = vec![
            FrameInput {
                at_ms: after_midnight,
                content: json!({"note": "after-midnight"}),
            },
            FrameInput {
                at_ms: before_midnight,
                content: json!({"note": "opening-hand"}),
            },
        ];
        let bundle = match_bundle(&input, None);
        let rows: Vec<&Value> = bundle["timelines"]
            .as_array()
            .expect("timelines")
            .iter()
            .filter(|row| row["game"] == json!("945500844"))
            .collect();
        assert_eq!(rows[0]["timestamp"], json!("2026-09-26T22:48:07Z"));
        assert_eq!(rows[1]["timestamp"], json!("2026-09-26T23:00:39Z"));
        assert_eq!(rows[0]["content"], json!({"note": "opening-hand"}));
    }

    fn manual_sample() -> MatchBundleInput {
        let mut input = sample();
        input.source = "manual".to_string();
        input.mtgo_match_id = None;
        input.registered = vec![
            DeckRow {
                catalog_id: 101,
                quantity: 4,
                sideboard: false,
            },
            DeckRow {
                catalog_id: 102,
                quantity: 4,
                sideboard: false,
            },
            DeckRow {
                catalog_id: 301,
                quantity: 2,
                sideboard: true,
            },
        ];
        for game in &mut input.games {
            game.mtgo_game_id = None;
            game.frames = Vec::new();
            game.cards = Vec::new();
            game.local_starting_hand_size = None;
            game.opp_starting_hand_size = None;
        }
        let first = &mut input.games[0];
        first.local_mulligans = Some(1);
        first.local_starting_hand_size = Some(6);
        let mut hand = |catalog_id: u32, starting: u32, bottomed: u32| {
            let mut row = card(false, catalog_id, "o");
            row.opening_hand = starting + bottomed;
            row.starting_hand = starting;
            row.bottomed = bottomed;
            first.cards.push(row);
        };
        hand(101, 3, 0);
        hand(102, 3, 1);
        first.mulligans = vec![(1, 101, 4), (1, 102, 3)];
        // Game 2: one Qcard 301 in for one 102.
        let second = &mut input.games[1];
        let mut main = |catalog_id: u32, quantity: u32| {
            let mut row = card(false, catalog_id, "o");
            row.main_qty = quantity;
            second.cards.push(row);
        };
        main(101, 4);
        main(102, 3);
        main(301, 1);
        let mut revealed = card(true, 501, "o");
        revealed.seen = 2;
        second.cards.push(revealed);
        input
    }

    // 0.x "carries the manual flag and each player's hand-entered opening
    // hand" (hands in the stored sorted order).
    #[test]
    fn carries_the_manual_flag_and_each_players_opening_hand() {
        let input = manual_sample();
        let bundle = match_bundle(&input, None);
        assert_eq!(bundle["match"]["manual"], json!(true));
        assert_eq!(bundle["match"]["mtgo_id"], json!("manual_qmatchtoken"));
        assert_eq!(bundle["match"]["result"], Value::Null);
        assert_eq!(bundle["games"][0]["mtgo_id"], json!("q-match-token-1"));
        let players = bundle["players"].as_array().expect("players");
        let local = &players[1];
        assert_eq!(local["username"], json!("Qplayer"));
        assert_eq!(
            local["opening_hand_json"],
            json!({
                "kept": [101, 101, 101, 102, 102, 102],
                "bottomed": [102],
                "mulligans": [[101, 101, 101, 101, 102, 102, 102]],
            })
        );
        assert_eq!(local["starting_hand_size"], json!(6));
        assert_eq!(local["mulligan_count"], json!(1));
        assert_eq!(local["deck_json"], json!([]));
        assert_eq!(players[0]["opening_hand_json"], Value::Null);
        // Game 2's postboard list: the maindeck, then the registered deck's
        // other copies as the sideboard.
        assert_eq!(
            players[3]["deck_json"],
            json!([
                {"mtgo_id": 101, "quantity": 4, "sideboard": false},
                {"mtgo_id": 102, "quantity": 3, "sideboard": false},
                {"mtgo_id": 301, "quantity": 1, "sideboard": false},
                {"mtgo_id": 102, "quantity": 1, "sideboard": true},
                {"mtgo_id": 301, "quantity": 1, "sideboard": true},
            ])
        );
        // The opponent's reveals, as 0.x `UpdateManualGameReveals` keeps them.
        assert_eq!(
            players[2]["deck_json"],
            json!([{"mtgo_id": 501, "quantity": 2}])
        );
        assert_eq!(players[3]["opening_hand_json"], Value::Null);
        assert_eq!(players[3]["starting_hand_size"], json!(7));
        assert_eq!(bundle["card_stats"], json!([]));
    }

    // 0.x "derives the match sidecar from bundle content with both archetype
    // sides".
    #[test]
    fn derives_the_match_sidecar_from_bundle_content_with_both_archetype_sides() {
        let bundle = match_bundle(&with_archetypes(), None);
        let sidecar = match_sidecar(&bundle);
        let mut found = keys(&sidecar);
        found.sort();
        assert_eq!(
            found,
            [
                "archetype_uuid",
                "deck_client_id",
                "format",
                "games_lost",
                "games_won",
                "match_type",
                "occurred_at",
                "opponent_archetype_uuid",
                "outcome",
                "result",
            ]
        );
        assert_eq!(
            sidecar["archetype_uuid"],
            json!("aaaaaaaa-0000-4000-8000-000000000000")
        );
        assert_eq!(
            sidecar["opponent_archetype_uuid"],
            json!("bbbbbbbb-0000-4000-8000-000000000000")
        );
    }

    // 0.x "sanitizes the sidecar deck_client_id the same way the deck client
    // id uploads".
    #[test]
    fn sanitizes_the_sidecar_deck_client_id_as_the_deck_uploads() {
        let mut input = sample();
        input.deck_mtgo_id = Some("limited:791bacca-caea-4d88-b6c7-3bc067d412c2".to_string());
        let sidecar = match_sidecar(&match_bundle(&input, None));
        assert_eq!(
            sidecar["deck_client_id"],
            json!("limited_791bacca-caea-4d88-b6c7-3bc067d412c2")
        );
    }

    #[test]
    fn a_match_without_games_keeps_its_imported_score() {
        let mut input = sample();
        input.source = "history".to_string();
        input.games = Vec::new();
        input.games_won_imported = Some(2);
        input.games_lost_imported = Some(1);
        let bundle = match_bundle(&input, None);
        assert_eq!(bundle["match"]["games_won"], json!(2));
        assert_eq!(bundle["match"]["games_lost"], json!(1));
        assert_eq!(bundle["match"]["result"], json!("2-1"));
        assert_eq!(bundle["match"]["imported"], json!(true));
    }

    /// A golden's `input` (the v1 columns the 0.x export seeded).
    fn input_from_json(input: &Value) -> Option<MatchBundleInput> {
        let text = |value: &Value| value.as_str().map(str::to_string);
        let int = |value: &Value| value.as_i64();
        let small = |value: &Value| value.as_u64().map(|n| n as u32);
        let mut games = Vec::new();
        for game in input["games"].as_array()? {
            let mut cards = Vec::new();
            for row in game["cards"].as_array()? {
                cards.push(CardRowInput {
                    opponent: row["opponent"].as_bool()?,
                    catalog_id: small(&row["catalog_id"])?,
                    oracle_id: text(&row["oracle_id"]),
                    main_qty: small(&row["main_qty"])?,
                    side_qty: small(&row["side_qty"])?,
                    sided_in: small(&row["sided_in"])?,
                    sided_out: small(&row["sided_out"])?,
                    opening_hand: small(&row["opening_hand"])?,
                    bottomed: small(&row["bottomed"])?,
                    starting_hand: small(&row["starting_hand"])?,
                    seen: small(&row["seen"])?,
                    pregame_revealed: small(&row["pregame_revealed"])?,
                    pregame_played: small(&row["pregame_played"])?,
                    cast: small(&row["cast"])?,
                    played: small(&row["played"]),
                    activated: small(&row["activated"]),
                });
            }
            let mut casts = Vec::new();
            for cast in game["casts"].as_array()? {
                casts.push(CastInput {
                    opponent: cast["opponent"].as_bool()?,
                    catalog_id: small(&cast["catalog_id"])?,
                    method: text(&cast["method"])?,
                    count: small(&cast["count"])?,
                });
            }
            let mut mulligans = Vec::new();
            for row in game["mulligans"].as_array()? {
                mulligans.push((small(&row[0])?, small(&row[1])?, small(&row[2])?));
            }
            let mut frames = Vec::new();
            for frame in game["frames"].as_array()? {
                frames.push(FrameInput {
                    at_ms: int(&frame["at_ms"])?,
                    content: frame["content"].clone(),
                });
            }
            games.push(MatchGameInput {
                game_number: small(&game["game_number"])?,
                mtgo_game_id: int(&game["mtgo_game_id"]),
                result: text(&game["result"])?,
                on_play: game["on_play"].as_bool(),
                local_mulligans: int(&game["local_mulligans"]),
                opp_mulligans: int(&game["opp_mulligans"]),
                local_die_roll: int(&game["local_die_roll"]),
                opp_die_roll: int(&game["opp_die_roll"]),
                turn_count: int(&game["turn_count"]),
                local_starting_hand_size: int(&game["local_starting_hand_size"]),
                opp_starting_hand_size: int(&game["opp_starting_hand_size"]),
                started_at_ms: int(&game["started_at_ms"]),
                ended_at_ms: int(&game["ended_at_ms"]),
                cards,
                casts,
                mulligans,
                frames,
            });
        }
        let mut registered = Vec::new();
        for row in input["registered"].as_array()? {
            registered.push(DeckRow {
                catalog_id: small(&row["catalog_id"])?,
                quantity: small(&row["quantity"])?,
                sideboard: row["sideboard"].as_bool()?,
            });
        }
        let mut archetypes = Vec::new();
        for row in input["archetypes"].as_array()? {
            archetypes.push(ArchetypeSide {
                uuid: text(&row["uuid"])?,
                confidence: row["confidence"].as_f64()?,
                manual: row["manual"].as_bool()?,
                opponent: row["opponent"].as_bool()?,
            });
        }
        Some(MatchBundleInput {
            platform: BundlePlatform::Mtgo,
            sources: None,
            endstep_cards: BTreeMap::new(),
            endstep_ranked: None,
            local_player_fields: Default::default(),
            opponent_player_fields: Default::default(),
            token: text(&input["token"])?,
            mtgo_match_id: int(&input["mtgo_match_id"]),
            source: text(&input["source"])?,
            format: text(&input["format"]),
            match_type: text(&input["match_type"]),
            game_structure: None,
            state: text(&input["state"])?,
            outcome: text(&input["outcome"])?,
            started_at_ms: int(&input["started_at_ms"])?,
            ended_at_ms: int(&input["ended_at_ms"]),
            notes: text(&input["notes"]),
            games_won_imported: int(&input["games_won_imported"]),
            games_lost_imported: int(&input["games_lost_imported"]),
            deck_mtgo_id: text(&input["deck_mtgo_id"]),
            deck_version_signature: text(&input["deck_version_signature"]),
            registered,
            league_client_id: text(&input["league_client_id"]),
            tournament_event_id: int(&input["tournament_event_id"]),
            tournament_round: int(&input["tournament_round"]),
            local_username: text(&input["local_username"])?,
            opponent_username: text(&input["opponent_username"]),
            games,
            archetypes,
            hidden_at_ms: int(&input["hidden_at_ms"]),
            no_league: input["no_league"].as_bool().unwrap_or(false),
            league_linked: input["league_linked"].as_bool().unwrap_or(false),
            edited_at_ms: int(&input["edited_at_ms"]),
        })
    }

    fn golden() -> Vec<Value> {
        let golden: Value = serde_json::from_str(include_str!(
            "../../../fixtures/parity/sync_match_bundles.golden.json"
        ))
        .expect("the match golden is JSON");
        golden["bundles"].as_array().expect("bundles").clone()
    }

    #[test]
    fn every_golden_match_bundle_matches_0x_byte_for_byte() {
        let bundles = golden();
        assert!(!bundles.is_empty());
        for case in bundles {
            let input = input_from_json(&case["input"]).expect("golden input");
            // From the rows alone (archetypes spec section 14: no carry).
            let bundle = match_bundle(&input, None);
            let canonical = canonical_json(&bundle);
            assert_eq!(canonical, case["canonical"].as_str().expect("canonical"));
            assert_eq!(
                canonical_json(&match_bundle_owned(input.clone(), None)),
                canonical,
                "the owned build gives the golden bytes"
            );
            assert_eq!(
                bundle_hash(&canonical),
                case["hash"].as_str().expect("hash")
            );
            assert_eq!(match_sidecar(&bundle), case["sidecar"]);
            assert_eq!(input.token, case["client_id"].as_str().expect("id"));
        }
    }

    /// Every field a v1 match bundle cannot write as 0.x does is justified
    /// in the parity allowlist (ruling 5).
    #[test]
    fn the_known_match_differences_are_allowlisted() {
        let allowlist: Value =
            serde_json::from_str(include_str!("../../../fixtures/parity/allowlist.json"))
                .expect("the allowlist is JSON");
        for field in [
            "players[].deck_json order",
            "players[].opening_hand_json order",
            "match.mtgo_id (manual)",
            "games[].mtgo_id (manual)",
            "card_stats (manual)",
            "players[].starting_hand_size (unknown)",
            "match.result (imported)",
        ] {
            let listed =
                allowlist.as_array().expect("a list").iter().any(|entry| {
                    entry["fixture"] == "sync_match_bundles" && entry["field"] == field
                });
            assert!(listed, "{field}");
        }
        // Imported frames are dated from their own times now (replays spec
        // section 10), as the golden's imported match proves.
        assert!(!allowlist.as_array().expect("a list").iter().any(|entry| {
            entry["fixture"] == "sync_match_bundles"
                && entry["field"] == "timelines[].timestamp (imported)"
        }));
        assert!(golden().iter().any(|case| {
            case["input"]["source"] == "imported"
                && !case["input"]["games"][0]["frames"]
                    .as_array()
                    .expect("frames")
                    .is_empty()
        }));
    }
}
