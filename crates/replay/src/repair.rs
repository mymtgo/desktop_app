//! The package's frame repairs (`RepairMidnightWrap`, `RepairActivePlayer`),
//! every constant and bail-out as the package has them.

use chrono::{DateTime, Days, NaiveDate, NaiveDateTime};
use serde_json::Value;

use crate::snapshot::ReplayFrame;

/// No real game waits this long between two frames; a gap this wide is a
/// midnight wrap (package `WRAP_GAP_SECONDS`, 12 hours).
pub const WRAP_GAP_SECONDS: i64 = 12 * 60 * 60;

/// The main phase 1 rank in `step_rank` (package `MAIN_ONE`).
const MAIN_ONE: u8 = 3;

/// Package `RepairMidnightWrap::run`: a game's frames back in play order when
/// a client dated them wrongly past midnight.
///
/// Older clients dated every time-of-day frame on one day when building the
/// sync bundle, then sorted, so a game that crossed local midnight opens with
/// its final frames and jumps more than twelve hours forward to its real
/// start. Those leading frames move to the end, a day later.
///
/// Only ISO 8601 timestamps can wrap this way. v1 frames are timed
/// `HH:MM:SS` (the log line's time), come in play order already and are
/// returned as they came, as is any game without such a gap. Kept so the
/// builder equals the package.
pub fn repair_midnight_wrap(frames: Vec<ReplayFrame>) -> Vec<ReplayFrame> {
    let Some(wrap_at) = wrap_index(&frames) else {
        return frames;
    };

    let mut frames = frames;
    let after_wrap = frames.split_off(wrap_at);
    let mut repaired = after_wrap;
    for mut frame in frames {
        frame.timestamp = next_day(&frame.timestamp);
        repaired.push(frame);
    }
    repaired
}

/// The first frame after the wrapping gap, or `None` when the frames do not
/// wrap. One frame that is not an ISO timestamp means no wrap.
fn wrap_index(frames: &[ReplayFrame]) -> Option<usize> {
    let mut previous: Option<i64> = None;

    for (index, frame) in frames.iter().enumerate() {
        let time = iso_seconds(&frame.timestamp)?;

        if let Some(previous) = previous
            && time - previous > WRAP_GAP_SECONDS
        {
            return Some(index);
        }

        previous = Some(time);
    }

    None
}

/// Unix seconds of an ISO 8601 timestamp (package `isoSeconds`, PHP
/// `strtotime`); `None` for a bare time of day or text that does not parse.
/// A timestamp without a zone is read as UTC (the package's app timezone).
fn iso_seconds(timestamp: &str) -> Option<i64> {
    if !starts_like_iso_date(timestamp) {
        return None;
    }

    if let Ok(time) = DateTime::parse_from_rfc3339(timestamp) {
        return Some(time.timestamp());
    }

    for format in ["%Y-%m-%dT%H:%M:%S%.f%z", "%Y-%m-%dT%H:%M:%S%z"] {
        if let Ok(time) = DateTime::parse_from_str(timestamp, format) {
            return Some(time.timestamp());
        }
    }

    for format in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M"] {
        if let Ok(time) = NaiveDateTime::parse_from_str(timestamp, format) {
            return Some(time.and_utc().timestamp());
        }
    }

    None
}

/// The package's `^\d{4}-\d{2}-\d{2}T` check.
fn starts_like_iso_date(timestamp: &str) -> bool {
    let bytes = timestamp.as_bytes();
    if bytes.len() < 11 {
        return false;
    }

    for (index, byte) in bytes[..11].iter().enumerate() {
        let fits = match index {
            4 | 7 => *byte == b'-',
            10 => *byte == b'T',
            _ => byte.is_ascii_digit(),
        };
        if !fits {
            return false;
        }
    }

    true
}

/// The same timestamp a calendar day later, its time and zone untouched
/// (package `nextDay`: only the first ten characters change).
fn next_day(timestamp: &str) -> String {
    // Only called on timestamps `starts_like_iso_date` accepted, so the first
    // ten bytes are ASCII and slicing there is safe.
    let date = &timestamp[..10];
    let rest = &timestamp[10..];

    match NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .and_then(|day| day.checked_add_days(Days::new(1)))
    {
        Some(next) => format!("{}{rest}", next.format("%Y-%m-%d")),
        None => timestamp.to_string(),
    }
}

/// Package `STEP_RANK`: steps in turn order; MTGO names without a step of
/// their own share a rank.
fn step_rank(step: &str) -> Option<u8> {
    let rank = match step {
        "Untap" => 0,
        "Upkeep" => 1,
        "Draw" => 2,
        "PreCombatMain" => 3,
        "BeginCombat" => 4,
        "DeclareAttackers" => 5,
        "DeclareBlockers" => 6,
        "FirstStrikeDamage" | "CombatDamage" => 7,
        "EndOfCombat" => 8,
        "PostCombatMain" => 9,
        "EndOfTurn" => 10,
        "Cleanup" => 11,
        _ => return None,
    };
    Some(rank)
}

/// Package `RepairActivePlayer::run`: a game's frames with `ActivePlayer`
/// naming whose turn it is.
///
/// Sidecar recordings carry the player who went first as the active player
/// on every frame, and MTGO's `Turn` counts rounds, so neither says when the
/// second player's turn begins. Turn starts are found from the frames: the
/// first step, `Turn` rising, the step going back to main phase 1 or
/// earlier, or, after a turn that never left main phase 1, the next player
/// drawing on an empty stack. The active player starts from the one recorded
/// and alternates at each start; frames before the first start are left
/// alone.
///
/// Returned as they came: no frames, a first frame without exactly two seats,
/// more than one distinct recorded `ActivePlayer`, no turn start, or a first
/// active player who is not a seat.
pub fn repair_active_player(frames: Vec<ReplayFrame>) -> Vec<ReplayFrame> {
    let Some(first_frame) = frames.first() else {
        return frames;
    };

    let seats = seat_ids(&first_frame.content);
    if seats.len() != 2 || distinct_recorded_players(&frames) > 1 {
        return frames;
    }
    let seats = [seats[0].clone(), seats[1].clone()];

    let starts = turn_starts(&frames, &seats);
    let Some(first_start) = starts.iter().position(|start| *start) else {
        return frames;
    };

    let first = frames[first_start]
        .content
        .get("ActivePlayer")
        .filter(|seat| !seat.is_null())
        .cloned()
        .unwrap_or_else(|| seats[0].clone());
    if first != seats[0] && first != seats[1] {
        return frames;
    }

    let mut frames = frames;
    let mut active: Option<Value> = None;
    for (index, frame) in frames.iter_mut().enumerate() {
        if starts[index] {
            active = Some(match active {
                None => first.clone(),
                Some(seat) if seat == seats[0] => seats[1].clone(),
                Some(_) => seats[0].clone(),
            });
        }

        if let (Some(seat), Some(content)) = (&active, frame.content.as_object_mut()) {
            content.insert("ActivePlayer".to_string(), seat.clone());
        }
    }

    frames
}

/// Package `array_column($players, 'Id')`: the `Id` of every player that has
/// one, in order.
fn seat_ids(content: &Value) -> Vec<Value> {
    let Some(players) = content.get("Players").and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut seats = Vec::new();
    for player in players {
        if let Some(id) = player.get("Id") {
            seats.push(id.clone());
        }
    }
    seats
}

/// How many different non-null `ActivePlayer` values the frames recorded.
fn distinct_recorded_players(frames: &[ReplayFrame]) -> usize {
    let mut seen: Vec<&Value> = Vec::new();
    for frame in frames {
        if let Some(seat) = frame.content.get("ActivePlayer")
            && !seat.is_null()
            && !seen.contains(&seat)
        {
            seen.push(seat);
        }
    }
    seen.len()
}

/// A JSON number as `f64` for the package's loose `>` on `Turn`.
fn number(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64)
}

/// Package `turnStarts`: one flag per frame, true where a turn begins.
fn turn_starts(frames: &[ReplayFrame], seats: &[Value; 2]) -> Vec<bool> {
    let mut starts = vec![false; frames.len()];
    let mut any_start = false;
    let mut previous_rank: Option<u8> = None;
    let mut previous_turn: Option<&Value> = None;
    let mut previous: Option<&Value> = None;
    let mut past_main_one = false;
    let mut turns_this_round = 0;
    let mut first: Option<Value> = None;

    for (index, frame) in frames.iter().enumerate() {
        let content = &frame.content;
        let rank = content
            .get("Step")
            .and_then(Value::as_str)
            .and_then(step_rank);
        let turn = content.get("Turn").filter(|turn| !turn.is_null());

        if let Some(rank) = rank {
            if first.is_none() {
                first = Some(
                    content
                        .get("ActivePlayer")
                        .filter(|seat| !seat.is_null())
                        .cloned()
                        .unwrap_or_else(|| seats[0].clone()),
                );
            }

            let turn_rose = match (number(turn), number(previous_turn)) {
                (Some(now), Some(before)) => now > before,
                _ => false,
            };
            let new_round = !any_start || turn_rose;

            let step_went_back =
                previous_rank.is_some_and(|before| before > MAIN_ONE) && rank <= MAIN_ONE;
            let drew_after_main_one = !past_main_one
                && turns_this_round == 1
                && rank <= MAIN_ONE
                && match (previous, &first) {
                    (Some(before), Some(first)) => next_player_drew(before, content, first, seats),
                    _ => false,
                };
            let handover = !new_round && (step_went_back || drew_after_main_one);

            if new_round || handover {
                starts[index] = true;
                any_start = true;
                past_main_one = false;
                turns_this_round = if new_round { 1 } else { turns_this_round + 1 };
            }

            if rank > MAIN_ONE {
                past_main_one = true;
            }

            previous_rank = Some(rank);
        }

        if turn.is_some() {
            previous_turn = turn;
        }

        previous = Some(content);
    }

    starts
}

/// Package `nextPlayerDrew`: whether the player who is not the round's first
/// drew exactly one card between two frames, with nothing on the stack in
/// either. A turn cannot end with the stack full, which rules out
/// instant-speed draws there.
fn next_player_drew(before: &Value, after: &Value, first: &Value, seats: &[Value; 2]) -> bool {
    for content in [before, after] {
        if let Some(cards) = content.get("Cards").and_then(Value::as_array) {
            for card in cards {
                if card.get("Zone").and_then(Value::as_str) == Some("Stack") {
                    return false;
                }
            }
        }
    }

    let second = if *first == seats[0] {
        &seats[1]
    } else {
        &seats[0]
    };
    let (Some(was), Some(now)) = (player(before, second), player(after, second)) else {
        return false;
    };

    let count = |player: &Value, key: &str| player.get(key).and_then(Value::as_i64).unwrap_or(0);
    count(now, "HandCount") - count(was, "HandCount") == 1
        && count(was, "LibraryCount") - count(now, "LibraryCount") == 1
}

/// The player in a frame whose `Id` is the seat.
fn player<'a>(content: &'a Value, seat: &Value) -> Option<&'a Value> {
    let players = content.get("Players")?.as_array()?;
    players.iter().find(|player| player.get("Id") == Some(seat))
}
