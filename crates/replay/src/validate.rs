//! Snapshot validation (package `ValidateReplaySnapshot`): the rules the
//! website applies to an upload, in one hand-walked pass, every failure
//! collected under its dotted path with the package's message.
//!
//! PHP decodes JSON objects and lists alike into arrays, so the mapping is:
//! where the package wants an array of keys (`is_array`), a JSON object is
//! accepted and a JSON list behaves as one without those keys; where it wants
//! a list (`array_is_list`), only a JSON array is. A JSON `null` counts as a
//! missing key (`?? null`) except where the package asks
//! `array_key_exists`. Integers are JSON integers that fit in 64 bits (PHP
//! decodes `2.0` and larger numbers as floats).
//!
//! Not validated, as the package: card `name` and `type`, optional player
//! and card keys, sizes, counts, string lengths, `game_number` order or
//! uniqueness. No size limit of any kind (replays spec ruling 14).

use std::collections::BTreeMap;

use serde_json::Value;

use crate::snapshot::VERSION;

/// Failures by dotted path (`games.0.frames.3.content.Cards.1.Zone`), each
/// with its messages in the order found.
pub type ValidationErrors = BTreeMap<String, Vec<String>>;

/// Package `ValidateReplaySnapshot::run`: `Ok` when the website would accept
/// the snapshot, else every failure by path.
pub fn validate_snapshot(snapshot: &Value) -> Result<(), ValidationErrors> {
    let mut errors = ValidationErrors::new();

    if int(get(snapshot, "version")) != Some(i64::from(VERSION)) {
        fail(&mut errors, "version", "Unsupported snapshot version.");
    }

    meta(get(snapshot, "meta"), &mut errors);
    games(get(snapshot, "games"), &mut errors);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn fail(errors: &mut ValidationErrors, path: &str, message: &str) {
    errors
        .entry(path.to_string())
        .or_default()
        .push(message.to_string());
}

/// `$value[$key] ?? null`: the value under a key of a JSON object, `None`
/// when missing, null or not an object.
fn get<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_object()?.get(key).filter(|found| !found.is_null())
}

/// Same as `get` for an optional parent.
fn get_in<'a>(value: Option<&'a Value>, key: &str) -> Option<&'a Value> {
    get(value?, key)
}

/// `array_key_exists($key, $value)`: present, even when null.
fn has_key(value: &Value, key: &str) -> bool {
    value
        .as_object()
        .is_some_and(|object| object.contains_key(key))
}

/// PHP `is_array`: a JSON object or list.
fn is_php_array(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Object(_)) | Some(Value::Array(_)))
}

/// PHP `is_int`.
fn int(value: Option<&Value>) -> Option<i64> {
    value?.as_i64()
}

fn is_string(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::String(_)))
}

/// `is_string($value) || $value === null` for a key that must exist.
fn is_string_or_null_key(value: &Value, key: &str) -> bool {
    has_key(value, key) && matches!(value.get(key), Some(Value::String(_)) | Some(Value::Null))
}

/// `null`, or a string starting `https://`.
fn is_remote_or_null(value: Option<&Value>) -> bool {
    match value {
        None => true,
        Some(Value::String(url)) => url.starts_with("https://"),
        Some(_) => false,
    }
}

fn is_positive_int(value: Option<&Value>) -> bool {
    int(value).is_some_and(|number| number >= 1)
}

fn meta(meta: Option<&Value>, errors: &mut ValidationErrors) {
    if !is_php_array(meta) {
        fail(errors, "meta", "Required.");
        return;
    }
    let Some(meta) = meta else {
        return;
    };

    if !is_string(get(meta, "played_at")) {
        fail(errors, "meta.played_at", "Must be a string.");
    }

    if !is_positive_int(get(meta, "games_in_match")) {
        fail(errors, "meta.games_in_match", "Must be a positive integer.");
    }

    for key in ["format", "local_archetype", "opponent_archetype"] {
        if !is_string_or_null_key(meta, key) {
            fail(errors, &format!("meta.{key}"), "Must be a string or null.");
        }
    }
}

fn games(games: Option<&Value>, errors: &mut ValidationErrors) {
    let Some(list) = games
        .and_then(Value::as_array)
        .filter(|list| !list.is_empty())
    else {
        fail(errors, "games", "Must be a non-empty list.");
        return;
    };

    for (g, game) in list.iter().enumerate() {
        let path = format!("games.{g}");
        if !is_php_array(Some(game)) {
            fail(errors, &path, "Must be an object.");
            continue;
        }

        if !is_positive_int(get(game, "game_number")) {
            fail(
                errors,
                &format!("{path}.game_number"),
                "Must be a positive integer.",
            );
        }

        let won_ok = has_key(game, "won")
            && matches!(game.get("won"), Some(Value::Bool(_)) | Some(Value::Null));
        if !won_ok {
            fail(
                errors,
                &format!("{path}.won"),
                "Must be true, false or null.",
            );
        }

        frames(get(game, "frames"), &format!("{path}.frames"), errors);
        log(get(game, "log"), &format!("{path}.log"), errors);

        if has_key(game, "sideboard") {
            sideboard(game.get("sideboard"), &format!("{path}.sideboard"), errors);
        }
    }
}

/// Your sideboard as the game began; optional, since older snapshots predate
/// it.
fn sideboard(sideboard: Option<&Value>, path: &str, errors: &mut ValidationErrors) {
    let Some(list) = sideboard.and_then(Value::as_array) else {
        fail(errors, path, "Must be a list.");
        return;
    };

    for (s, entry) in list.iter().enumerate() {
        if int(get(entry, "catalog_id")).is_none() {
            fail(
                errors,
                &format!("{path}.{s}.catalog_id"),
                "Must be an integer.",
            );
        }

        if !is_positive_int(get(entry, "quantity")) {
            fail(
                errors,
                &format!("{path}.{s}.quantity"),
                "Must be a positive integer.",
            );
        }

        for key in ["name", "type"] {
            if !is_string_or_null_key(entry, key) {
                fail(
                    errors,
                    &format!("{path}.{s}.{key}"),
                    "Must be a string or null.",
                );
            }
        }

        if !is_remote_or_null(get(entry, "image")) {
            fail(
                errors,
                &format!("{path}.{s}.image"),
                "Must be a remote https URL or null.",
            );
        }
    }
}

fn frames(frames: Option<&Value>, path: &str, errors: &mut ValidationErrors) {
    let Some(list) = frames
        .and_then(Value::as_array)
        .filter(|list| !list.is_empty())
    else {
        fail(errors, path, "Must be a non-empty list.");
        return;
    };

    for (f, frame) in list.iter().enumerate() {
        if !is_string(get(frame, "timestamp")) {
            fail(
                errors,
                &format!("{path}.{f}.timestamp"),
                "Must be a string.",
            );
        }

        let content = get(frame, "content");
        if !is_php_array(content) {
            fail(errors, &format!("{path}.{f}.content"), "Required.");
            continue;
        }

        players(
            get_in(content, "Players"),
            &format!("{path}.{f}.content.Players"),
            errors,
        );
        cards(
            get_in(content, "Cards"),
            &format!("{path}.{f}.content.Cards"),
            errors,
        );
    }
}

fn players(players: Option<&Value>, path: &str, errors: &mut ValidationErrors) {
    let Some(list) = players.and_then(Value::as_array) else {
        fail(errors, path, "Must be a list.");
        return;
    };

    for (p, player) in list.iter().enumerate() {
        if int(get(player, "Id")).is_none() {
            fail(errors, &format!("{path}.{p}.Id"), "Must be an integer.");
        }

        if !is_string(get(player, "Name")) {
            fail(errors, &format!("{path}.{p}.Name"), "Must be a string.");
        }
    }
}

fn cards(cards: Option<&Value>, path: &str, errors: &mut ValidationErrors) {
    let Some(list) = cards.and_then(Value::as_array) else {
        fail(errors, path, "Must be a list.");
        return;
    };

    for (c, card) in list.iter().enumerate() {
        for key in ["Id", "CatalogID", "Owner"] {
            if int(get(card, key)).is_none() {
                fail(errors, &format!("{path}.{c}.{key}"), "Must be an integer.");
            }
        }

        if !is_string(get(card, "Zone")) {
            fail(errors, &format!("{path}.{c}.Zone"), "Must be a string.");
        }

        for key in ["image", "other_image"] {
            if !is_remote_or_null(get(card, key)) {
                fail(
                    errors,
                    &format!("{path}.{c}.{key}"),
                    "Must be a remote https URL or null.",
                );
            }
        }
    }
}

fn log(log: Option<&Value>, path: &str, errors: &mut ValidationErrors) {
    let Some(list) = log.and_then(Value::as_array) else {
        fail(errors, path, "Must be a list.");
        return;
    };

    for (l, entry) in list.iter().enumerate() {
        if !is_string(get(entry, "timestamp")) {
            fail(
                errors,
                &format!("{path}.{l}.timestamp"),
                "Must be a string.",
            );
        }

        if !is_string(get(entry, "message")) {
            fail(errors, &format!("{path}.{l}.message"), "Must be a string.");
        }
    }
}
