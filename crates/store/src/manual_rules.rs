//! The rules of manual matches (Plan L), ported from 0.x's form requests with
//! their messages: `StoreManualMatchRequest` (and `CreateManualMatch`'s
//! outcome and game times), `UpdateManualGameHandRequest`,
//! `UpdateManualGameSideboardRequest` (and `UpdateManualGameSideboard`'s
//! check that a recorded hand still fits) and `UpdateManualGameRevealsRequest`.
//!
//! Pure: no database, clock or file. Callers pass in the deck lists, the
//! timezone and a card lookup. Field names and messages are 0.x's: Laravel's
//! default messages for the plain rules, the requests' own for the rest. Like
//! the errors Inertia handed 0.x's forms, each field has at most one message
//! (its first). Rules the Rust types already enforce (integers, booleans, a
//! sideboard change's direction) are not checked again.

use std::collections::BTreeMap;

use chrono::NaiveDateTime;
use chrono_tz::Tz;

use crate::manual::{CardCount, Direction, ManualGame, SideboardChange};

/// A refused field, as a 0.x form showed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldError {
    /// 0.x's field key, for example `opponent_name` or `games.0.turns`.
    pub field: String,
    pub message: String,
}

/// 0.x `StoreController`'s message once a manual match is added.
pub const MATCH_ADDED: &str = "Manual match added.";
/// 0.x `UpdateHandController`'s message.
pub const HAND_SAVED: &str = "Opening hand saved.";
/// 0.x `UpdateSideboardController`'s message.
pub const SIDEBOARD_SAVED: &str = "Sideboard saved.";
/// 0.x `UpdateSideboardController`'s message when the edit cleared the hand.
pub const SIDEBOARD_SAVED_HAND_CLEARED: &str =
    "Sideboard saved. Opening hand cleared because it used a card you sided out.";
/// 0.x `UpdateRevealsController`'s message.
pub const REVEALS_SAVED: &str = "Revealed cards saved.";

/// One game as the form sent it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameInput {
    pub won: bool,
    pub on_play: bool,
    pub turns: Option<u32>,
}

/// The new match as the form sent it: the times are the zone-less local
/// date and time a `datetime-local` input gives (`2026-09-01T20:00`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateInput {
    pub opponent_name: String,
    pub started_at: String,
    pub ended_at: String,
    pub games: Vec<GameInput>,
    /// MTGO matches need an opponent; an Arena match may have none (a blank
    /// name is no opponent).
    pub opponent_required: bool,
}

/// A form that passed: the opponent trimmed, the times in UTC milliseconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidCreate {
    pub opponent: String,
    pub started_at_ms: i64,
    pub ended_at_ms: i64,
    pub games: Vec<ManualGame>,
}

/// The errors found so far, at most one per field (its first), in the order
/// found.
#[derive(Debug, Default)]
struct Errors {
    list: Vec<FieldError>,
}

impl Errors {
    fn add(&mut self, field: &str, message: impl Into<String>) {
        if self.has(field) {
            return;
        }
        self.list.push(FieldError {
            field: field.to_string(),
            message: message.into(),
        });
    }

    fn has(&self, field: &str) -> bool {
        self.list.iter().any(|error| error.field == field)
    }

    fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

/// The longest opponent name, in characters (0.x `max:255`).
pub const MAX_OPPONENT_CHARS: usize = 255;
/// A best-of-three match has at most three games (0.x `games` `max:3`).
pub const MAX_GAMES: usize = 3;
/// The cards in an opening hand (0.x `HAND_SIZE`).
pub const HAND_SIZE: usize = 7;

const ENDED_BEFORE_STARTED: &str =
    "The ended at field must be a date after or equal to started at.";

/// 0.x `StoreManualMatchRequest` (its rules and `validateGameRecord`), with
/// the times read in the player's timezone `tz` and stored as UTC, as 0.x
/// `CreateManualMatch` does. The deck and league checks need the database:
/// the store makes them when it creates the match.
///
/// 0.x compared the zone-less times before reading them in the timezone;
/// this compares the UTC times, so a stored match never ends before it
/// starts, even across a clock change.
pub fn validate_create(input: &CreateInput, tz: Tz) -> Result<ValidCreate, Vec<FieldError>> {
    let mut errors = Errors::default();
    let opponent = input.opponent_name.trim().to_string();
    if input.opponent_required || !opponent.is_empty() {
        check_opponent(&opponent, &mut errors);
    }
    let started = check_time(
        "started_at",
        "started at",
        &input.started_at,
        tz,
        &mut errors,
    );
    let ended = check_time("ended_at", "ended at", &input.ended_at, tz, &mut errors);
    if let (Some(started), Some(ended)) = (started, ended)
        && ended < started
    {
        errors.add("ended_at", ENDED_BEFORE_STARTED);
    }
    let games: Vec<ManualGame> = input
        .games
        .iter()
        .map(|game| ManualGame {
            won: game.won,
            on_play: game.on_play,
            turns: game.turns,
        })
        .collect();
    check_games(&games, &mut errors);
    // 0.x's after-hook runs only once every rule passed.
    if errors.is_empty() {
        check_record(&games, &mut errors);
    }
    match (started, ended) {
        (Some(started_at_ms), Some(ended_at_ms)) if errors.is_empty() => Ok(ValidCreate {
            opponent,
            started_at_ms,
            ended_at_ms,
            games,
        }),
        _ => Err(errors.list),
    }
}

/// The rules of a match to store, once its times are UTC: the opponent, the
/// end not before the start, one to three games with sensible turn counts,
/// and a best-of-three record. The store checks every new match with these.
/// Without `opponent_required` (an Arena match) a blank opponent is allowed;
/// a name given is checked as ever.
pub fn match_errors(
    opponent: &str,
    opponent_required: bool,
    started_at_ms: i64,
    ended_at_ms: i64,
    games: &[ManualGame],
) -> Vec<FieldError> {
    let mut errors = Errors::default();
    if opponent_required || !opponent.trim().is_empty() {
        check_opponent(opponent.trim(), &mut errors);
    }
    if ended_at_ms < started_at_ms {
        errors.add("ended_at", ENDED_BEFORE_STARTED);
    }
    check_games(games, &mut errors);
    if errors.is_empty() {
        check_record(games, &mut errors);
    }
    errors.list
}

/// The rules of a player's fix of a stored match (edit a match spec section
/// 3): `match_errors`, except that a match the log gave no opponent or no
/// end may keep none (`None`). A name given is checked as Add checks it, and
/// an end given is not before the start.
pub fn correction_errors(
    opponent: Option<&str>,
    started_at_ms: i64,
    ended_at_ms: Option<i64>,
    games: &[ManualGame],
) -> Vec<FieldError> {
    let mut errors = Errors::default();
    if let Some(opponent) = opponent {
        check_opponent(opponent.trim(), &mut errors);
    }
    if let Some(ended_at_ms) = ended_at_ms
        && ended_at_ms < started_at_ms
    {
        errors.add("ended_at", ENDED_BEFORE_STARTED);
    }
    check_games(games, &mut errors);
    if errors.is_empty() {
        check_record(games, &mut errors);
    }
    errors.list
}

/// `opponent_name`: `required|string|max:255` (trimmed, as Laravel trims).
fn check_opponent(opponent: &str, errors: &mut Errors) {
    if opponent.is_empty() {
        errors.add("opponent_name", "The opponent name field is required.");
    } else if opponent.chars().count() > MAX_OPPONENT_CHARS {
        errors.add(
            "opponent_name",
            format!(
                "The opponent name field must not be greater than {MAX_OPPONENT_CHARS} characters."
            ),
        );
    }
}

/// `started_at` and `ended_at`: `required|date`, read in `tz`.
fn check_time(field: &str, label: &str, text: &str, tz: Tz, errors: &mut Errors) -> Option<i64> {
    if text.trim().is_empty() {
        errors.add(field, format!("The {label} field is required."));
        return None;
    }
    let at = local_to_utc_ms(text, tz);
    if at.is_none() {
        errors.add(field, format!("The {label} field must be a valid date."));
    }
    at
}

/// `games`: `required|array|min:1|max:3`, and `games.*.turns`:
/// `nullable|integer|min:1|max:99`.
fn check_games(games: &[ManualGame], errors: &mut Errors) {
    if games.is_empty() {
        errors.add("games", "The games field is required.");
    } else if games.len() > MAX_GAMES {
        errors.add(
            "games",
            format!("The games field must not have more than {MAX_GAMES} items."),
        );
    }
    for (index, game) in games.iter().enumerate() {
        let field = format!("games.{index}.turns");
        match game.turns {
            Some(0) => errors.add(&field, format!("The {field} field must be at least 1.")),
            Some(turns) if turns > 99 => errors.add(
                &field,
                format!("The {field} field must not be greater than 99."),
            ),
            _ => {}
        }
    }
}

/// 0.x `validateGameRecord`: at most two wins or two losses, and no game
/// after the match was decided.
fn check_record(games: &[ManualGame], errors: &mut Errors) {
    let wins = games.iter().filter(|game| game.won).count();
    let losses = games.len() - wins;
    if wins > 2 || losses > 2 {
        errors.add(
            "games",
            "A best-of-three match cannot have more than two wins or two losses.",
        );
        return;
    }
    // A third game after a 2-0 is not a real match shape either.
    let before_last = &games[..games.len().saturating_sub(1)];
    let prior_wins = before_last.iter().filter(|game| game.won).count();
    let prior_losses = before_last.len() - prior_wins;
    if prior_wins >= 2 || prior_losses >= 2 {
        errors.add(
            "games",
            "The match was already decided before the last game.",
        );
    }
}

/// 0.x `MtgoMatch::determineOutcome`: `"win"`, `"loss"`, `"draw"` (1-1) or
/// `"unknown"` (no games).
pub fn outcome(wins: usize, losses: usize) -> &'static str {
    if wins > losses {
        "win"
    } else if losses > wins {
        "loss"
    } else if wins > 0 {
        "draw"
    } else {
        "unknown"
    }
}

/// 0.x `CreateManualMatch::createGames`: each game's start and end, evenly
/// spaced whole-second slices of the match in the order entered.
pub fn game_times(started_at_ms: i64, ended_at_ms: i64, games: usize) -> Vec<(i64, i64)> {
    let seconds = ((ended_at_ms - started_at_ms) / 1000).max(0);
    let slice_ms = (seconds / games.max(1) as i64) * 1000;
    let mut times = Vec::new();
    for index in 0..games as i64 {
        times.push((
            started_at_ms + slice_ms * index,
            started_at_ms + slice_ms * (index + 1),
        ));
    }
    times
}

/// The forms a `datetime-local` input (and 0.x's tests) send.
const LOCAL_FORMATS: [&str; 4] = [
    "%Y-%m-%dT%H:%M",
    "%Y-%m-%dT%H:%M:%S",
    "%Y-%m-%d %H:%M",
    "%Y-%m-%d %H:%M:%S",
];

/// A zone-less local date and time (`2026-09-01T20:00`, seconds optional, a
/// space instead of the `T` too) in `tz`, as UTC milliseconds. A time in a
/// clock change gap moves forward by the gap and a repeated time is the first
/// one, as PHP reads them. `None` when the text is not a date and time.
pub fn local_to_utc_ms(text: &str, tz: Tz) -> Option<i64> {
    let text = text.trim();
    let mut wall_clock = None;
    for format in LOCAL_FORMATS {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(text, format) {
            wall_clock = Some(parsed);
            break;
        }
    }
    mtgo_protocol::local_to_utc_ms(wall_clock?, tz)
}

/// The hand form (0.x `UpdateManualGameHandRequest`): catalog ids, one entry
/// per copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandInput {
    pub mulligan_count: u32,
    /// The starting hand (0.x `kept_hand`): the opening hand minus the
    /// bottomed cards.
    pub starting_hand: Vec<u32>,
    pub bottomed: Vec<u32>,
    pub mulliganed_hands: Vec<Vec<u32>>,
}

impl HandInput {
    /// An empty starting hand with no mulligans clears the record.
    pub fn is_clearing(&self) -> bool {
        self.starting_hand.is_empty() && self.mulligan_count == 0
    }
}

/// 0.x `UpdateManualGameHandRequest`: `maindeck` is the game's maindeck by
/// catalog id (see `game_maindeck`).
pub fn validate_hand(input: &HandInput, maindeck: &BTreeMap<u32, u32>) -> Vec<FieldError> {
    let mut errors = Errors::default();
    let mulligans = input.mulligan_count;
    if mulligans > 6 {
        errors.add(
            "mulligan_count",
            "The mulligan count field must not be greater than 6.",
        );
    }
    if input.starting_hand.len() > HAND_SIZE {
        errors.add(
            "kept_hand",
            "The kept hand field must not have more than 7 items.",
        );
    }
    if input.bottomed.len() > 6 {
        errors.add(
            "bottomed",
            "The bottomed field must not have more than 6 items.",
        );
    }
    if input.mulliganed_hands.len() > 6 {
        errors.add(
            "mulliganed_hands",
            "The mulliganed hands field must not have more than 6 items.",
        );
    }
    for (index, hand) in input.mulliganed_hands.iter().enumerate() {
        if hand.len() > HAND_SIZE {
            let field = format!("mulliganed_hands.{index}");
            errors.add(
                &field,
                format!("The {field} field must not have more than 7 items."),
            );
        }
    }
    if !errors.is_empty() || input.is_clearing() {
        return errors.list;
    }

    // The after-hook: the first failure only.
    if input.starting_hand.len() + input.bottomed.len() != HAND_SIZE {
        errors.add(
            "kept_hand",
            "The final hand is seven cards: the ones you kept plus the ones you bottomed.",
        );
        return errors.list;
    }
    if input.bottomed.len() != mulligans as usize {
        errors.add(
            "bottomed",
            format!("After {mulligans} mulligan(s) you bottom {mulligans} card(s)."),
        );
        return errors.list;
    }
    if input.mulliganed_hands.len() != mulligans as usize {
        errors.add(
            "mulliganed_hands",
            "Record one hand per mulligan, or leave a hand empty if you do not remember it.",
        );
        return errors.list;
    }
    let mut final_hand = input.starting_hand.clone();
    final_hand.extend_from_slice(&input.bottomed);
    if let Some(message) = hand_misfit(&final_hand, maindeck) {
        errors.add("kept_hand", message);
        return errors.list;
    }
    for (index, hand) in input.mulliganed_hands.iter().enumerate() {
        let field = format!("mulliganed_hands.{index}");
        if !hand.is_empty() && hand.len() != HAND_SIZE {
            errors.add(
                &field,
                "A mulliganed hand is seven cards, or empty if you do not remember it.",
            );
            return errors.list;
        }
        if let Some(message) = hand_misfit(hand, maindeck) {
            errors.add(&field, message);
            return errors.list;
        }
    }
    errors.list
}

/// Each card of `hand` with its copies, in the order first seen (PHP's
/// `array_count_values`).
fn copies_in(hand: &[u32]) -> Vec<(u32, u32)> {
    let mut counts: Vec<(u32, u32)> = Vec::new();
    for catalog_id in hand {
        match counts.iter_mut().find(|(id, _)| id == catalog_id) {
            Some((_, count)) => *count += 1,
            None => counts.push((*catalog_id, 1)),
        }
    }
    counts
}

/// 0.x `UpdateManualGameHandRequest::handFits`: why `hand` cannot be drawn
/// from `maindeck`, `None` when it can.
fn hand_misfit(hand: &[u32], maindeck: &BTreeMap<u32, u32>) -> Option<&'static str> {
    for (catalog_id, count) in copies_in(hand) {
        match maindeck.get(&catalog_id) {
            None => return Some("Every card in a hand must be in the maindeck for this game."),
            Some(copies) if count > *copies => {
                return Some("A card in a hand exceeds the number of copies in the maindeck.");
            }
            Some(_) => {}
        }
    }
    None
}

/// 0.x `ResolveManualGameMaindeck`: the game's maindeck after sideboarding
/// when it was recorded, else the registered maindeck.
pub fn game_maindeck(
    registered_mains: &BTreeMap<u32, u32>,
    postboard_main: Option<&[CardCount]>,
) -> BTreeMap<u32, u32> {
    let Some(postboard) = postboard_main else {
        return registered_mains.clone();
    };
    let mut maindeck = BTreeMap::new();
    for card in postboard {
        *maindeck.entry(card.catalog_id).or_insert(0) += card.quantity;
    }
    maindeck
}

/// 0.x `UpdateManualGameSideboardRequest`: game 1 is never sideboarded, and
/// each change moves copies the registered deck has, once per direction.
pub fn validate_sideboard(
    game_number: u32,
    changes: &[SideboardChange],
    registered_mains: &BTreeMap<u32, u32>,
    registered_sideboard: &BTreeMap<u32, u32>,
) -> Vec<FieldError> {
    let mut errors = Errors::default();
    for (index, change) in changes.iter().enumerate() {
        if change.quantity == 0 {
            let field = format!("changes.{index}.quantity");
            errors.add(&field, format!("The {field} field must be at least 1."));
        }
    }
    if !errors.is_empty() {
        return errors.list;
    }

    // The after-hook: the first failure only.
    if game_number == 1 {
        errors.add("changes", "Game 1 is played with the registered deck.");
        return errors.list;
    }
    let mut seen: Vec<(Direction, u32)> = Vec::new();
    for change in changes {
        let key = (change.direction, change.catalog_id);
        if seen.contains(&key) {
            errors.add(
                "changes",
                "Each card can only be listed once per direction.",
            );
            return errors.list;
        }
        seen.push(key);
        let (pool, missing) = match change.direction {
            Direction::Out => (
                registered_mains,
                "Cards sided out must be in the registered maindeck.",
            ),
            Direction::In => (
                registered_sideboard,
                "Cards sided in must be in the registered sideboard.",
            ),
        };
        let Some(copies) = pool.get(&change.catalog_id) else {
            errors.add("changes", missing);
            return errors.list;
        };
        if change.quantity > *copies {
            errors.add(
                "changes",
                "A change exceeds the registered number of copies.",
            );
            return errors.list;
        }
    }
    errors.list
}

/// 0.x `UpdateManualGameSideboard::buildDeckJson`, maindeck only: the
/// registered maindeck with the changes applied, copies above zero, by
/// catalog id.
pub fn postboard_main(
    registered_mains: &BTreeMap<u32, u32>,
    changes: &[SideboardChange],
) -> Vec<CardCount> {
    let mut counts: BTreeMap<u32, i64> = BTreeMap::new();
    for (catalog_id, quantity) in registered_mains {
        counts.insert(*catalog_id, i64::from(*quantity));
    }
    for change in changes {
        let entry = counts.entry(change.catalog_id).or_insert(0);
        match change.direction {
            Direction::In => *entry += i64::from(change.quantity),
            Direction::Out => *entry -= i64::from(change.quantity),
        }
    }
    let mut main = Vec::new();
    for (catalog_id, quantity) in counts {
        if quantity > 0 {
            main.push(CardCount {
                catalog_id,
                quantity: quantity as u32,
            });
        }
    }
    main
}

/// 0.x `UpdateManualGameSideboard::openingHandFits`: every recorded hand (the
/// kept and bottomed cards together, and each hand put back) can still be
/// drawn from `maindeck`.
pub fn hand_fits(
    starting: &[u32],
    bottomed: &[u32],
    mulligans: &[Vec<u32>],
    maindeck: &BTreeMap<u32, u32>,
) -> bool {
    let mut final_hand = starting.to_vec();
    final_hand.extend_from_slice(bottomed);
    let fits = |hand: &[u32]| {
        copies_in(hand)
            .into_iter()
            .all(|(catalog_id, count)| maindeck.get(&catalog_id).copied().unwrap_or(0) >= count)
    };
    fits(&final_hand) && mulligans.iter().all(|hand| fits(hand))
}

/// The most cards a reveals list holds (0.x `max:60`).
pub const MAX_REVEALS: usize = 60;
/// The most copies of one revealed card (0.x `max:20`).
pub const MAX_REVEALED_COPIES: u32 = 20;

/// 0.x `UpdateManualGameRevealsRequest`: `is_card` says whether a catalog id
/// is a known card (0.x `exists:cards,mtgo_id`).
pub fn validate_reveals(cards: &[CardCount], is_card: &dyn Fn(u32) -> bool) -> Vec<FieldError> {
    let mut errors = Errors::default();
    if cards.len() > MAX_REVEALS {
        errors.add(
            "cards",
            format!("The cards field must not have more than {MAX_REVEALS} items."),
        );
    }
    for (index, card) in cards.iter().enumerate() {
        if !is_card(card.catalog_id) {
            errors.add(
                &format!("cards.{index}.mtgo_id"),
                format!("The selected cards.{index}.mtgo_id is invalid."),
            );
        }
        let field = format!("cards.{index}.quantity");
        if card.quantity < 1 {
            errors.add(&field, format!("The {field} field must be at least 1."));
        } else if card.quantity > MAX_REVEALED_COPIES {
            errors.add(
                &field,
                format!("The {field} field must not be greater than {MAX_REVEALED_COPIES}."),
            );
        }
    }
    // 0.x's after-hook runs even when a rule failed.
    let mut seen: Vec<u32> = Vec::new();
    for card in cards {
        if seen.contains(&card.catalog_id) {
            errors.add("cards", "Each card can only be listed once.");
            break;
        }
        seen.push(card.catalog_id);
    }
    errors.list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn messages(errors: &[FieldError]) -> Vec<(&str, &str)> {
        errors
            .iter()
            .map(|error| (error.field.as_str(), error.message.as_str()))
            .collect()
    }

    fn game(won: bool, on_play: bool) -> GameInput {
        GameInput {
            won,
            on_play,
            turns: None,
        }
    }

    /// 0.x `manualMatchPayload`: Qkai, 20:00 to 20:45, won, lost, won.
    fn payload() -> CreateInput {
        CreateInput {
            opponent_name: "Qkai".to_string(),
            started_at: "2026-09-01T20:00".to_string(),
            ended_at: "2026-09-01T20:45".to_string(),
            games: vec![game(true, true), game(false, false), game(true, true)],
            opponent_required: true,
        }
    }

    fn ms(text: &str) -> i64 {
        chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S")
            .expect("time")
            .and_utc()
            .timestamp_millis()
    }

    fn create_errors(input: &CreateInput) -> Vec<FieldError> {
        validate_create(input, chrono_tz::UTC).expect_err("refused")
    }

    // StoreManualMatchTest: "creates a complete manual match with games,
    // players and archetypes" (the outcome and game times).
    #[test]
    fn a_two_one_match_is_a_win_with_games_sliced_evenly() {
        let valid = validate_create(&payload(), chrono_tz::UTC).expect("valid");
        assert_eq!(valid.opponent, "Qkai");
        assert_eq!(valid.started_at_ms, ms("2026-09-01 20:00:00"));
        assert_eq!(valid.ended_at_ms, ms("2026-09-01 20:45:00"));
        let won: Vec<bool> = valid.games.iter().map(|game| game.won).collect();
        assert_eq!(won, vec![true, false, true]);
        let on_play: Vec<bool> = valid.games.iter().map(|game| game.on_play).collect();
        assert_eq!(on_play, vec![true, false, true]);
        assert_eq!(outcome(2, 1), "win");
        let starts: Vec<i64> = game_times(valid.started_at_ms, valid.ended_at_ms, 3)
            .into_iter()
            .map(|(start, _)| start)
            .collect();
        assert_eq!(
            starts,
            vec![
                ms("2026-09-01 20:00:00"),
                ms("2026-09-01 20:15:00"),
                ms("2026-09-01 20:30:00")
            ]
        );
    }

    #[test]
    fn game_times_are_whole_second_slices_and_never_run_backwards() {
        assert_eq!(
            game_times(0, 10_999, 3),
            vec![(0, 3_000), (3_000, 6_000), (6_000, 9_000)],
            "10 whole seconds in three: 3 each"
        );
        assert_eq!(
            game_times(5_000, 5_000, 2),
            vec![(5_000, 5_000), (5_000, 5_000)]
        );
        assert_eq!(game_times(5_000, 1_000, 1), vec![(5_000, 5_000)]);
        assert_eq!(game_times(0, 60_000, 0), Vec::new());
    }

    // "reads the zone-less form times in the system timezone and stores UTC".
    #[test]
    fn form_times_are_read_in_the_players_timezone_and_stored_as_utc() {
        let london: Tz = "Europe/London".parse().expect("zone");
        let valid = validate_create(&payload(), london).expect("valid");
        assert_eq!(valid.started_at_ms, ms("2026-09-01 19:00:00"));
        assert_eq!(valid.ended_at_ms, ms("2026-09-01 19:45:00"));
        for text in [
            "2026-09-01T20:00:00",
            "2026-09-01 20:00",
            "2026-09-01 20:00:00",
        ] {
            assert_eq!(
                local_to_utc_ms(text, london),
                Some(ms("2026-09-01 19:00:00")),
                "{text}"
            );
        }
        for text in ["", "tomorrow", "2026-13-01T20:00", "2026-09-01"] {
            assert_eq!(local_to_utc_ms(text, london), None, "{text:?}");
        }
    }

    #[test]
    fn times_across_a_clock_change_are_read_as_php_reads_them() {
        let london: Tz = "Europe/London".parse().expect("zone");
        // 01:30 on 2026-03-29 does not exist in London: PHP moves it forward
        // by the gap, to 02:30 BST, which is 01:30 UTC.
        assert_eq!(
            local_to_utc_ms("2026-03-29T01:30", london),
            Some(ms("2026-03-29 01:30:00"))
        );
        // Lord Howe's gap is half an hour: 02:15 (+10:30) is 02:45 (+11:00).
        let lord_howe: Tz = "Australia/Lord_Howe".parse().expect("zone");
        assert_eq!(
            local_to_utc_ms("2026-10-04T02:15", lord_howe),
            Some(ms("2026-10-03 15:45:00"))
        );
        // 01:30 on 2026-10-25 happens twice: the first one, BST.
        assert_eq!(
            local_to_utc_ms("2026-10-25T01:30", london),
            Some(ms("2026-10-25 00:30:00"))
        );
        // A match over the change: 00:50 BST to 02:10 GMT is 140 minutes.
        let mut input = payload();
        input.started_at = "2026-10-25T00:50".to_string();
        input.ended_at = "2026-10-25T02:10".to_string();
        let valid = validate_create(&input, london).expect("valid");
        assert_eq!(valid.ended_at_ms - valid.started_at_ms, 140 * 60_000);
        // An end that is earlier in UTC is refused, whatever the wall clock.
        input.started_at = "2026-03-29T01:30".to_string();
        input.ended_at = "2026-03-29T02:15".to_string();
        assert_eq!(
            messages(&validate_create(&input, london).expect_err("refused")),
            vec![(
                "ended_at",
                "The ended at field must be a date after or equal to started at."
            )]
        );
    }

    // "records a draw for a 1-1 match".
    #[test]
    fn the_outcome_follows_the_games() {
        assert_eq!(outcome(2, 0), "win");
        assert_eq!(outcome(2, 1), "win");
        assert_eq!(outcome(1, 2), "loss");
        assert_eq!(outcome(0, 1), "loss");
        assert_eq!(outcome(1, 1), "draw");
        assert_eq!(outcome(0, 0), "unknown");
        let mut input = payload();
        input.games = vec![game(true, true), game(false, false)];
        assert!(validate_create(&input, chrono_tz::UTC).is_ok());
    }

    // "rejects impossible game records".
    #[test]
    fn impossible_game_records_are_refused() {
        let mut input = payload();
        input.games = vec![game(true, true), game(true, true), game(true, true)];
        assert_eq!(
            messages(&create_errors(&input)),
            vec![(
                "games",
                "A best-of-three match cannot have more than two wins or two losses."
            )]
        );
        input.games = vec![game(false, true), game(false, true), game(false, true)];
        assert_eq!(
            messages(&create_errors(&input)),
            vec![(
                "games",
                "A best-of-three match cannot have more than two wins or two losses."
            )]
        );

        input.games = Vec::new();
        assert_eq!(
            messages(&create_errors(&input)),
            vec![("games", "The games field is required.")]
        );
        input.games = vec![game(true, true); 4];
        assert_eq!(
            messages(&create_errors(&input)),
            vec![("games", "The games field must not have more than 3 items.")]
        );

        // 2-0 then a third game.
        input.games = vec![game(true, true), game(true, false), game(false, true)];
        assert_eq!(
            messages(&create_errors(&input)),
            vec![(
                "games",
                "The match was already decided before the last game."
            )]
        );
        input.games = vec![game(false, true), game(false, false), game(true, true)];
        assert_eq!(
            messages(&create_errors(&input)),
            vec![(
                "games",
                "The match was already decided before the last game."
            )]
        );

        // 2-1 is fine, so are 1-0 and 0-1.
        for games in [
            vec![game(true, true), game(false, false), game(true, true)],
            vec![game(true, true)],
            vec![game(false, true)],
        ] {
            input.games = games;
            assert!(validate_create(&input, chrono_tz::UTC).is_ok());
        }
    }

    // "rejects an end time before the start time".
    #[test]
    fn an_end_before_the_start_and_missing_or_bad_times_are_refused() {
        let mut input = payload();
        input.ended_at = "2026-09-01T19:00".to_string();
        assert_eq!(
            messages(&create_errors(&input)),
            vec![(
                "ended_at",
                "The ended at field must be a date after or equal to started at."
            )]
        );
        input.ended_at = input.started_at.clone();
        assert!(
            validate_create(&input, chrono_tz::UTC).is_ok(),
            "equal is fine"
        );

        input.started_at = "  ".to_string();
        input.ended_at = "soon".to_string();
        assert_eq!(
            messages(&create_errors(&input)),
            vec![
                ("started_at", "The started at field is required."),
                ("ended_at", "The ended at field must be a valid date."),
            ]
        );
    }

    #[test]
    fn the_opponent_is_required_trimmed_and_at_most_255_characters() {
        let mut input = payload();
        input.opponent_name = "  Qkai  ".to_string();
        assert_eq!(
            validate_create(&input, chrono_tz::UTC)
                .expect("valid")
                .opponent,
            "Qkai"
        );
        input.opponent_name = "   ".to_string();
        assert_eq!(
            messages(&create_errors(&input)),
            vec![("opponent_name", "The opponent name field is required.")]
        );
        input.opponent_name = "é".repeat(256);
        assert_eq!(
            messages(&create_errors(&input)),
            vec![(
                "opponent_name",
                "The opponent name field must not be greater than 255 characters."
            )]
        );
        input.opponent_name = "é".repeat(255);
        assert!(validate_create(&input, chrono_tz::UTC).is_ok());
    }

    // "stores optional turn counts per game" and "rejects turn counts outside
    // one to ninety nine" (0.x's "nine" cannot be typed as a number here).
    #[test]
    fn turn_counts_are_optional_and_one_to_ninety_nine() {
        let mut input = payload();
        input.games[0].turns = Some(8);
        let valid = validate_create(&input, chrono_tz::UTC).expect("valid");
        let turns: Vec<Option<u32>> = valid.games.iter().map(|game| game.turns).collect();
        assert_eq!(turns, vec![Some(8), None, None]);

        input.games = vec![GameInput {
            won: true,
            on_play: true,
            turns: Some(0),
        }];
        assert_eq!(
            messages(&create_errors(&input)),
            vec![(
                "games.0.turns",
                "The games.0.turns field must be at least 1."
            )]
        );
        input.games[0].turns = Some(100);
        assert_eq!(
            messages(&create_errors(&input)),
            vec![(
                "games.0.turns",
                "The games.0.turns field must not be greater than 99."
            )]
        );
        input.games[0].turns = Some(99);
        assert!(validate_create(&input, chrono_tz::UTC).is_ok());
    }

    #[test]
    fn the_record_is_only_checked_once_every_field_passes() {
        let mut input = payload();
        input.opponent_name = String::new();
        input.games = vec![game(true, true); 3];
        assert_eq!(
            messages(&create_errors(&input)),
            vec![("opponent_name", "The opponent name field is required.")]
        );
    }

    #[test]
    fn a_stored_match_is_checked_with_the_same_rules() {
        let games = |won: &[bool]| -> Vec<ManualGame> {
            won.iter()
                .map(|won| ManualGame {
                    won: *won,
                    on_play: true,
                    turns: None,
                })
                .collect()
        };
        assert_eq!(
            match_errors("Qkai", true, 0, 1, &games(&[true, false, true])),
            Vec::new()
        );
        assert_eq!(
            messages(&match_errors("Qkai", true, 5, 1, &games(&[true]))),
            vec![(
                "ended_at",
                "The ended at field must be a date after or equal to started at."
            )]
        );
        assert_eq!(
            messages(&match_errors(" ", true, 0, 1, &games(&[true, true, true]))),
            vec![("opponent_name", "The opponent name field is required.")]
        );
        assert_eq!(
            messages(&match_errors(
                "Qkai",
                true,
                0,
                1,
                &games(&[true, true, false])
            )),
            vec![(
                "games",
                "The match was already decided before the last game."
            )]
        );
        // An Arena match may have no opponent, but a name given is checked.
        assert_eq!(
            match_errors(" ", false, 0, 1, &games(&[true, false, true])),
            Vec::new()
        );
        let long = "Q".repeat(MAX_OPPONENT_CHARS + 1);
        assert_eq!(
            match_errors(&long, false, 0, 1, &games(&[true, false, true])).len(),
            1
        );
    }

    /// 0.x `createManualMatchFixture`'s maindeck: 4 Lightning Bolt (4001), 4
    /// Tarmogoyf (4002), 20 Mountain (4004).
    fn mains() -> BTreeMap<u32, u32> {
        BTreeMap::from([(4001, 4), (4002, 4), (4004, 20)])
    }

    /// Its sideboard: 3 Rest in Peace (4003).
    fn sideboard() -> BTreeMap<u32, u32> {
        BTreeMap::from([(4003, 3)])
    }

    fn hand(
        mulligan_count: u32,
        kept: &[u32],
        bottomed: &[u32],
        mulliganed: &[&[u32]],
    ) -> HandInput {
        HandInput {
            mulligan_count,
            starting_hand: kept.to_vec(),
            bottomed: bottomed.to_vec(),
            mulliganed_hands: mulliganed.iter().map(|hand| hand.to_vec()).collect(),
        }
    }

    const LANDS: [u32; 7] = [4004; 7];

    // UpdateManualGameHandTest: "saves kept, bottomed and mulliganed cards".
    #[test]
    fn a_hand_after_a_mulligan_with_its_bottomed_card_and_hand_put_back_passes() {
        let input = hand(
            1,
            &[4001, 4001, 4002, 4004, 4004, 4004],
            &[4002],
            &[&[4004, 4004, 4004, 4004, 4004, 4004, 4001]],
        );
        assert_eq!(validate_hand(&input, &mains()), Vec::new());
        assert!(!input.is_clearing());
    }

    // "accepts an empty mulliganed hand when the user does not remember it".
    #[test]
    fn a_mulliganed_hand_can_be_left_empty() {
        let input = hand(
            2,
            &[4001, 4002, 4004, 4004, 4004],
            &[4004, 4004],
            &[&[], &LANDS],
        );
        assert_eq!(validate_hand(&input, &mains()), Vec::new());
    }

    // "requires a seven card final hand and no extra draw on the draw".
    #[test]
    fn the_final_hand_is_seven_cards() {
        let message =
            "The final hand is seven cards: the ones you kept plus the ones you bottomed.";
        for kept in [vec![4004; 8], vec![4004; 6]] {
            assert_eq!(
                messages(&validate_hand(&hand(0, &kept, &[], &[]), &mains())),
                if kept.len() > 7 {
                    vec![(
                        "kept_hand",
                        "The kept hand field must not have more than 7 items.",
                    )]
                } else {
                    vec![("kept_hand", message)]
                }
            );
        }
        assert_eq!(
            validate_hand(&hand(0, &LANDS, &[], &[]), &mains()),
            Vec::new()
        );
    }

    // "requires bottomed and mulliganed hand counts to match the mulligan
    // count".
    #[test]
    fn bottomed_cards_and_hands_put_back_match_the_mulligans() {
        let six = [4004; 6];
        assert_eq!(
            messages(&validate_hand(&hand(1, &LANDS, &[], &[&[]]), &mains())),
            vec![("bottomed", "After 1 mulligan(s) you bottom 1 card(s).")]
        );
        assert_eq!(
            messages(&validate_hand(&hand(1, &six, &[4004], &[]), &mains())),
            vec![(
                "mulliganed_hands",
                "Record one hand per mulligan, or leave a hand empty if you do not remember it."
            )]
        );
        assert_eq!(
            messages(&validate_hand(
                &hand(1, &six, &[4004], &[&[4004, 4004]]),
                &mains()
            )),
            vec![(
                "mulliganed_hands.0",
                "A mulliganed hand is seven cards, or empty if you do not remember it."
            )]
        );
    }

    // "rejects unknown cards and exceeded quantities in any hand".
    #[test]
    fn every_hand_is_drawn_from_the_maindeck() {
        let not_in = "Every card in a hand must be in the maindeck for this game.";
        let too_many = "A card in a hand exceeds the number of copies in the maindeck.";
        let refused = |input: HandInput| validate_hand(&input, &mains());
        assert_eq!(
            messages(&refused(hand(
                0,
                &[9999, 4004, 4004, 4004, 4004, 4004, 4004],
                &[],
                &[]
            ))),
            vec![("kept_hand", not_in)]
        );
        assert_eq!(
            messages(&refused(hand(
                0,
                &[4001, 4001, 4001, 4001, 4001, 4004, 4004],
                &[],
                &[]
            ))),
            vec![("kept_hand", too_many)]
        );
        assert_eq!(
            messages(&refused(hand(
                0,
                &[4003, 4004, 4004, 4004, 4004, 4004, 4004],
                &[],
                &[]
            ))),
            vec![("kept_hand", not_in)],
            "a sideboard card"
        );
        assert_eq!(
            messages(&refused(hand(
                1,
                &[4001, 4001, 4001, 4001, 4004, 4004],
                &[4001],
                &[&[]]
            ))),
            vec![("kept_hand", too_many)],
            "kept and bottomed count together"
        );
        assert_eq!(
            messages(&refused(hand(
                1,
                &[4004; 6],
                &[4004],
                &[&[4003, 4004, 4004, 4004, 4004, 4004, 4004]]
            ))),
            vec![("mulliganed_hands.0", not_in)]
        );
    }

    // "validates against the postboard maindeck when set".
    #[test]
    fn a_sideboarded_game_draws_from_its_postboard_maindeck() {
        let postboard = [
            CardCount {
                catalog_id: 4001,
                quantity: 4,
            },
            CardCount {
                catalog_id: 4004,
                quantity: 20,
            },
            CardCount {
                catalog_id: 4003,
                quantity: 3,
            },
        ];
        let maindeck = game_maindeck(&mains(), Some(&postboard));
        assert_eq!(maindeck, BTreeMap::from([(4001, 4), (4003, 3), (4004, 20)]));
        assert_eq!(game_maindeck(&mains(), None), mains());
        assert_eq!(
            validate_hand(
                &hand(0, &[4003, 4004, 4004, 4004, 4004, 4004, 4004], &[], &[]),
                &maindeck
            ),
            Vec::new()
        );
        assert_eq!(
            messages(&validate_hand(
                &hand(0, &[4002, 4004, 4004, 4004, 4004, 4004, 4004], &[], &[]),
                &maindeck
            )),
            vec![(
                "kept_hand",
                "Every card in a hand must be in the maindeck for this game."
            )]
        );
    }

    // "clears the hand with an empty payload".
    #[test]
    fn an_empty_hand_with_no_mulligans_clears_and_passes() {
        let clearing = hand(0, &[], &[], &[]);
        assert!(clearing.is_clearing());
        assert_eq!(validate_hand(&clearing, &BTreeMap::new()), Vec::new());
        let mulliganed = hand(1, &[], &[], &[&[]]);
        assert!(!mulliganed.is_clearing());
    }

    #[test]
    fn hand_lists_have_their_laravel_limits() {
        assert_eq!(
            messages(&validate_hand(&hand(7, &[], &[], &[]), &mains())),
            vec![(
                "mulligan_count",
                "The mulligan count field must not be greater than 6."
            )]
        );
        assert_eq!(
            messages(&validate_hand(&hand(0, &[4004], &[4004; 7], &[]), &mains())),
            vec![(
                "bottomed",
                "The bottomed field must not have more than 6 items."
            )]
        );
        let seven: Vec<&[u32]> = vec![&[][..]; 7];
        assert_eq!(
            messages(&validate_hand(
                &hand(6, &[4004], &[4004; 6], &seven),
                &mains()
            )),
            vec![(
                "mulliganed_hands",
                "The mulliganed hands field must not have more than 6 items."
            )]
        );
        assert_eq!(
            messages(&validate_hand(
                &hand(1, &[4004; 6], &[4004], &[&[4004; 8]]),
                &mains()
            )),
            vec![(
                "mulliganed_hands.0",
                "The mulliganed_hands.0 field must not have more than 7 items."
            )]
        );
    }

    fn change(catalog_id: u32, quantity: u32, direction: Direction) -> SideboardChange {
        SideboardChange {
            catalog_id,
            quantity,
            direction,
        }
    }

    fn sideboard_errors(game: u32, changes: &[SideboardChange]) -> Vec<FieldError> {
        validate_sideboard(game, changes, &mains(), &sideboard())
    }

    // UpdateManualGameSideboardTest: "rejects game one, unknown cards, over
    // quantity and duplicates".
    #[test]
    fn sideboard_changes_are_refused_for_game_one_and_cards_the_deck_lacks() {
        let out = Direction::Out;
        let into = Direction::In;
        for (game, changes, message) in [
            (
                1,
                vec![change(4001, 1, out)],
                "Game 1 is played with the registered deck.",
            ),
            (1, Vec::new(), "Game 1 is played with the registered deck."),
            (
                2,
                vec![change(9999, 1, out)],
                "Cards sided out must be in the registered maindeck.",
            ),
            (
                2,
                vec![change(4003, 1, out)],
                "Cards sided out must be in the registered maindeck.",
            ),
            (
                2,
                vec![change(4001, 1, into)],
                "Cards sided in must be in the registered sideboard.",
            ),
            (
                2,
                vec![change(4001, 5, out)],
                "A change exceeds the registered number of copies.",
            ),
            (
                2,
                vec![change(4003, 4, into)],
                "A change exceeds the registered number of copies.",
            ),
            (
                2,
                vec![change(4001, 1, out), change(4001, 1, out)],
                "Each card can only be listed once per direction.",
            ),
        ] {
            assert_eq!(
                messages(&sideboard_errors(game, &changes)),
                vec![("changes", message)],
                "game {game}: {changes:?}"
            );
        }
        assert_eq!(
            messages(&sideboard_errors(2, &[change(4001, 0, out)])),
            vec![(
                "changes.0.quantity",
                "The changes.0.quantity field must be at least 1."
            )]
        );
    }

    // "allows uneven totals", and the same card both ways.
    #[test]
    fn uneven_totals_and_a_card_in_each_direction_pass() {
        assert_eq!(
            sideboard_errors(2, &[change(4001, 1, Direction::Out)]),
            Vec::new()
        );
        assert_eq!(
            sideboard_errors(
                3,
                &[
                    change(4001, 4, Direction::Out),
                    change(4003, 3, Direction::In)
                ]
            ),
            Vec::new()
        );
        assert_eq!(sideboard_errors(2, &[]), Vec::new(), "a reset");
    }

    // "writes a full postboard deck_json" (the maindeck part).
    #[test]
    fn the_postboard_maindeck_applies_the_changes_to_the_registered_one() {
        let count = |catalog_id, quantity| CardCount {
            catalog_id,
            quantity,
        };
        assert_eq!(
            postboard_main(
                &mains(),
                &[
                    change(4001, 2, Direction::Out),
                    change(4003, 2, Direction::In)
                ]
            ),
            vec![
                count(4001, 2),
                count(4002, 4),
                count(4003, 2),
                count(4004, 20)
            ]
        );
        assert_eq!(
            postboard_main(&mains(), &[change(4001, 4, Direction::Out)]),
            vec![count(4002, 4), count(4004, 20)],
            "none left: the card is gone"
        );
    }

    // "clears a kept hand that used a sided-out card", "keeps a hand that
    // still fits", "clears an opening hand whose mulliganed or bottomed cards
    // were sided out" and "... that only fit the postboard deck".
    #[test]
    fn a_hand_fits_while_every_recorded_hand_can_be_drawn() {
        let kept = [4001, 4004, 4004, 4004, 4004, 4004, 4004];
        let after = |changes: &[SideboardChange]| -> BTreeMap<u32, u32> {
            let main = postboard_main(&mains(), changes);
            game_maindeck(&mains(), Some(&main))
        };
        assert!(!hand_fits(
            &kept,
            &[],
            &[],
            &after(&[change(4001, 4, Direction::Out)])
        ));
        assert!(hand_fits(
            &kept,
            &[],
            &[],
            &after(&[change(4001, 2, Direction::Out)])
        ));
        assert!(
            hand_fits(&kept, &[], &[], &mains()),
            "reset to the registered deck"
        );

        let lands = [4004; 6];
        let put_back = vec![vec![4002, 4004, 4004, 4004, 4004, 4004, 4004]];
        assert!(!hand_fits(
            &lands,
            &[4004],
            &put_back,
            &after(&[change(4002, 4, Direction::Out)])
        ));
        assert!(!hand_fits(
            &lands,
            &[4002],
            &[],
            &after(&[change(4002, 4, Direction::Out)])
        ));
        assert!(hand_fits(&lands, &[4004], &[Vec::new()], &mains()));

        let postboard_only = [4003, 4004, 4004, 4004, 4004, 4004, 4004];
        assert!(hand_fits(
            &postboard_only,
            &[],
            &[],
            &after(&[
                change(4001, 3, Direction::Out),
                change(4003, 3, Direction::In)
            ])
        ));
        assert!(!hand_fits(&postboard_only, &[], &[], &mains()));
    }

    fn known(catalog_id: u32) -> bool {
        [4001, 4002, 4003, 4004].contains(&catalog_id)
    }

    fn card(catalog_id: u32, quantity: u32) -> CardCount {
        CardCount {
            catalog_id,
            quantity,
        }
    }

    // UpdateManualGameRevealsTest: "validates cards, quantities and
    // duplicates".
    #[test]
    fn reveals_need_known_cards_sensible_quantities_and_no_duplicates() {
        assert_eq!(
            messages(&validate_reveals(&[card(9999, 1)], &known)),
            vec![(
                "cards.0.mtgo_id",
                "The selected cards.0.mtgo_id is invalid."
            )]
        );
        assert_eq!(
            messages(&validate_reveals(&[card(4002, 0)], &known)),
            vec![(
                "cards.0.quantity",
                "The cards.0.quantity field must be at least 1."
            )]
        );
        assert_eq!(
            messages(&validate_reveals(&[card(4002, 21)], &known)),
            vec![(
                "cards.0.quantity",
                "The cards.0.quantity field must not be greater than 20."
            )]
        );
        assert_eq!(
            messages(&validate_reveals(&[card(4002, 1), card(4002, 1)], &known)),
            vec![("cards", "Each card can only be listed once.")]
        );
        assert_eq!(
            validate_reveals(&[card(4002, 2), card(4004, 5)], &known),
            Vec::new()
        );
        assert_eq!(validate_reveals(&[], &known), Vec::new(), "a clear");
    }

    #[test]
    fn reveals_are_at_most_sixty_cards_and_the_duplicate_check_always_runs() {
        let many: Vec<CardCount> = (0..61).map(|_| card(4004, 1)).collect();
        assert_eq!(
            messages(&validate_reveals(&many, &known)),
            vec![("cards", "The cards field must not have more than 60 items.")],
            "one message per field: the first"
        );
        assert_eq!(
            messages(&validate_reveals(&[card(9999, 1), card(9999, 1)], &known)),
            vec![
                (
                    "cards.0.mtgo_id",
                    "The selected cards.0.mtgo_id is invalid."
                ),
                (
                    "cards.1.mtgo_id",
                    "The selected cards.1.mtgo_id is invalid."
                ),
                ("cards", "Each card can only be listed once."),
            ]
        );
    }

    #[test]
    fn the_flash_messages_are_the_0x_ones() {
        assert_eq!(MATCH_ADDED, "Manual match added.");
        assert_eq!(HAND_SAVED, "Opening hand saved.");
        assert_eq!(SIDEBOARD_SAVED, "Sideboard saved.");
        assert_eq!(
            SIDEBOARD_SAVED_HAND_CLEARED,
            "Sideboard saved. Opening hand cleared because it used a card you sided out."
        );
        assert_eq!(REVEALS_SAVED, "Revealed cards saved.");
    }
}
