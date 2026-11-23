//! Redaction (package `RedactPlayer`): an opponent's name replaced with
//! "Opponent" in every frame and log line, then proven gone.
//!
//! The package matches log mentions with the PCRE pattern
//! `(?<![A-Za-z0-9_])(?<![A-Za-z0-9_][.\-])USER(?![A-Za-z0-9_])(?![.\-][A-Za-z0-9_])`
//! and `/i` (no `/u`). The `regex` crate has no look-around, so this is a
//! plain loop over the bytes doing what PCRE does in byte mode: ASCII letters
//! fold case, every other byte must match exactly, and the four neighbour
//! conditions are checked on the original text (replays spec ruling 6).

use std::fmt;

use serde_json::Value;

use crate::snapshot::{REDACTED_NAME, Snapshot};

/// Why a snapshot could not be redacted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedactError {
    /// The username was blank after trimming (package
    /// `InvalidArgumentException`).
    EmptyUsername,
    /// The username was still present after redaction (package
    /// `RedactionFailed`), with the package's message.
    NameSurvived(String),
}

impl fmt::Display for RedactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RedactError::EmptyUsername => formatter.write_str("A username is required."),
            RedactError::NameSurvived(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for RedactError {}

/// Package `RedactPlayer::run`: replaces a player's name with "Opponent" in
/// every frame and every log line of every game, then proves it is gone.
///
/// The username is trimmed as PHP's `trim` does. A player `Name` equal to it
/// ignoring ASCII case (`strcasecmp`) becomes "Opponent"; each whole-token
/// mention in a log message is replaced. Card `name`, `type` and `image` are
/// left alone and are not checked: a username can be part of a card name.
pub fn redact_player(snapshot: Snapshot, username: &str) -> Result<Snapshot, RedactError> {
    let username = php_trim(username);
    if username.is_empty() {
        return Err(RedactError::EmptyUsername);
    }

    let mut snapshot = snapshot;
    for game in &mut snapshot.games {
        for frame in &mut game.frames {
            if let Some(players) = frame
                .content
                .get_mut("Players")
                .and_then(Value::as_array_mut)
            {
                for player in players {
                    let matches = player
                        .get("Name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| name.eq_ignore_ascii_case(username));
                    if matches && let Some(player) = player.as_object_mut() {
                        player.insert("Name".to_string(), Value::String(REDACTED_NAME.to_string()));
                    }
                }
            }
        }

        for entry in &mut game.log {
            entry.message = replace_whole_tokens(&entry.message, username);
        }
    }

    assert_gone(&snapshot, username)?;
    Ok(snapshot)
}

/// Package `assertGone`: fails when any player `Name` or log message still
/// contains the username (ASCII case-insensitive substring, PHP `stripos`).
pub fn assert_gone(snapshot: &Snapshot, username: &str) -> Result<(), RedactError> {
    for game in &snapshot.games {
        for frame in &game.frames {
            if let Some(players) = frame.content.get("Players").and_then(Value::as_array) {
                for player in players {
                    if let Some(name) = player.get("Name").and_then(Value::as_str)
                        && contains_ignoring_ascii_case(name, username)
                    {
                        return Err(RedactError::NameSurvived(
                            "A player name still contains the redacted username.".to_string(),
                        ));
                    }
                }
            }
        }

        for entry in &game.log {
            if contains_ignoring_ascii_case(&entry.message, username) {
                return Err(RedactError::NameSurvived(
                    "A log line still contains the redacted username.".to_string(),
                ));
            }
        }
    }

    Ok(())
}

/// PHP `trim` with its default characters: space, tab, newline, carriage
/// return, NUL and vertical tab.
pub(crate) fn php_trim(text: &str) -> &str {
    text.trim_matches(|character| matches!(character, ' ' | '\t' | '\n' | '\r' | '\0' | '\x0B'))
}

/// `A-Za-z0-9_`: characters that can start or end an MTGO username.
fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// `.` and `-`: characters a username may contain between word characters.
fn is_joiner(byte: u8) -> bool {
    byte == b'.' || byte == b'-'
}

/// Whether the match at `start..end` is a whole token: no word character
/// touches it, directly or through a joiner. A full stop or dash with nothing
/// after it is punctuation ("targeting Qfoe."), not more of the name.
fn is_whole_token(text: &[u8], start: usize, end: usize) -> bool {
    // (?<![A-Za-z0-9_])
    if start >= 1 && is_word(text[start - 1]) {
        return false;
    }
    // (?<![A-Za-z0-9_][.\-])
    if start >= 2 && is_word(text[start - 2]) && is_joiner(text[start - 1]) {
        return false;
    }
    // (?![A-Za-z0-9_])
    if end < text.len() && is_word(text[end]) {
        return false;
    }
    // (?![.\-][A-Za-z0-9_])
    if end + 1 < text.len() && is_joiner(text[end]) && is_word(text[end + 1]) {
        return false;
    }
    true
}

/// `preg_replace` of the package pattern: every non-overlapping whole-token
/// match, left to right, becomes "Opponent".
fn replace_whole_tokens(text: &str, username: &str) -> String {
    let bytes = text.as_bytes();
    let name = username.as_bytes();
    if name.is_empty() || name.len() > bytes.len() {
        return text.to_string();
    }

    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut copied_to = 0;
    let mut index = 0;
    while index + name.len() <= bytes.len() {
        let end = index + name.len();
        if bytes[index..end].eq_ignore_ascii_case(name) && is_whole_token(bytes, index, end) {
            out.extend_from_slice(&bytes[copied_to..index]);
            out.extend_from_slice(REDACTED_NAME.as_bytes());
            index = end;
            copied_to = end;
        } else {
            index += 1;
        }
    }
    out.extend_from_slice(&bytes[copied_to..]);

    // A match starts and ends on character boundaries (ASCII folds only to
    // ASCII and other bytes match exactly), so the text stays UTF-8.
    match String::from_utf8(out) {
        Ok(redacted) => redacted,
        Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
    }
}

/// PHP `stripos(...) !== false` on bytes, ASCII case folding only.
fn contains_ignoring_ascii_case(haystack: &str, needle: &str) -> bool {
    let haystack = haystack.as_bytes();
    let needle = needle.as_bytes();
    if needle.is_empty() {
        return true;
    }
    haystack
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}
