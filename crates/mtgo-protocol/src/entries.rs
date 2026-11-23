//! MetaMessage decoding (port of 0.x `DecodeMetaMessageText`) and typed gameplay
//! entries (port of the phrases 0.x `ExtractGameResults` reads).

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

/// MTGO usernames: letters, digits, space, underscore, period, hyphen (0.x
/// `ExtractGameResults::PLAYER_PATTERN`).
pub const PLAYER: &str = r"[A-Za-z0-9 _.\-]+";

const DECODE_PATTERNS: [&str; 23] = [
    r"@P{P} rolled a \d+\.",
    r"@P@P{P} joined the game\.",
    r"@P{P} chooses to play (?:first|second)\.",
    r"@P{P} begins the game with \w+ cards? in hand\.",
    r"@P{P} wins the game\.",
    r"@P{P} has conceded from the game\.",
    r"@P{P} has lost connection to the game\.",
    r"@P{P} (?:leads|wins) the match \d+-\d+",
    r"Match Tied \d+-\d+",
    r"@P{P} casts @\[[^@]+@:\d+,\d+:@\].*",
    r"@P{P} plays @\[[^@]+@:\d+,\d+:@\]",
    r"@P{P} activates an ability of @\[[^@]+@:\d+,\d+:@\]",
    r"@P{P} puts a triggered ability from @\[[^@]+@:\d+,\d+:@\]",
    r"@P{P} reveals @\[[^@]+@:\d+,\d+:@\] from their opening hand",
    r"@P{P} reveals \d+ cards? with @\[.*",
    r"@P{P} reveals @\[[^@]+@:\d+,\d+:@\]",
    r"@P{P} discards @\[[^@]+@:\d+,\d+:@\]",
    r"@P{P} puts @\[[^@]+@:\d+,\d+:@\] into their graveyard",
    r"@P{P} puts @\[[^@]+@:\d+,\d+:@\] onto the battlefield",
    r"@P{P}'s @\[[^@]+@:\d+,\d+:@\]",
    r"@P{P} names .+ for @\[[^@]+@:\d+,\d+:@\]",
    r"@P{P} mulligans to .+",
    r"@PTurn \d+:",
];

static DECODERS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    DECODE_PATTERNS
        .iter()
        .map(|pattern| Regex::new(&pattern.replace("{P}", PLAYER)).expect("valid regex"))
        .collect()
});

/// Compile `pattern` with every `{P}` replaced by [`PLAYER`].
pub fn player_re(pattern: &str) -> Regex {
    Regex::new(&pattern.replace("{P}", PLAYER)).expect("valid regex")
}

// The gameplay line patterns, shared with `game-detail`. Digits are `[0-9]`
// because 0.x's PCRE `\d` (no `/u`) is ASCII and Rust's `\d` is not.

/// Captures the player and the roll.
pub static ROLLED: LazyLock<Regex> = LazyLock::new(|| player_re(r"^@P({P}) rolled a ([0-9]+)"));
/// Captures the player.
pub static JOINED: LazyLock<Regex> = LazyLock::new(|| player_re(r"^@P@P({P}) joined the game"));
/// Captures the player and `first` or `second`.
pub static CHOOSES: LazyLock<Regex> =
    LazyLock::new(|| player_re(r"^@P({P}) chooses to play (first|second)"));
/// Captures the player and the hand size word (read it with [`hand_size`]).
///
/// After a London mulligan MTGO writes "@PName puts a card on the bottom of their
/// library and begins the game with six cards in hand." The name is matched lazily
/// so the bottoming clause is not swallowed into it (names may contain spaces).
/// 0.x's pattern is greedy and takes the clause as the name; this is the fix.
pub static BEGINS: LazyLock<Regex> = LazyLock::new(|| {
    player_re(
        r"^@P({P}?)(?: puts .+? on the bottom of their library and)? begins the game with (\w+) cards? in hand",
    )
});
/// Captures the player.
pub static WINS_GAME: LazyLock<Regex> = LazyLock::new(|| player_re(r"^@P({P}) wins the game"));
/// Captures the player.
pub static CONCEDED: LazyLock<Regex> =
    LazyLock::new(|| player_re(r"^@P({P}) has conceded from the game"));
/// Captures the player.
pub static LOST_CONNECTION: LazyLock<Regex> =
    LazyLock::new(|| player_re(r"^@P({P}) has lost connection to the game"));
/// Captures the player, `leads` or `wins`, and the player's and the other
/// player's game wins.
pub static MATCH_SCORE: LazyLock<Regex> =
    LazyLock::new(|| player_re(r"^@P({P}) (leads|wins) the match ([0-9]+)-([0-9]+)"));
/// Captures the two game win counts.
pub static MATCH_TIED: LazyLock<Regex> =
    LazyLock::new(|| player_re(r"^Match Tied ([0-9]+)-([0-9]+)"));
/// Captures the turn number.
pub static TURN: LazyLock<Regex> = LazyLock::new(|| player_re(r"^@PTurn ([0-9]+):"));

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GameEntry {
    Rolled {
        player: String,
        value: u32,
    },
    Joined {
        player: String,
    },
    ChoosesToPlay {
        player: String,
        first: bool,
    },
    BeginsGame {
        player: String,
        hand_size: u32,
    },
    WinsGame {
        player: String,
    },
    ConcededGame {
        player: String,
    },
    LostConnection {
        player: String,
    },
    MatchScore {
        player: String,
        wins: u32,
        losses: u32,
        is_final: bool,
    },
    MatchTied {
        first: u32,
        second: u32,
    },
    Turn {
        number: u32,
    },
    Other {
        text: String,
    },
}

impl GameEntry {
    /// The player this entry is about, when it names one.
    pub fn player(&self) -> Option<&str> {
        match self {
            GameEntry::Rolled { player, .. }
            | GameEntry::Joined { player }
            | GameEntry::ChoosesToPlay { player, .. }
            | GameEntry::BeginsGame { player, .. }
            | GameEntry::WinsGame { player }
            | GameEntry::ConcededGame { player }
            | GameEntry::LostConnection { player }
            | GameEntry::MatchScore { player, .. } => Some(player),
            GameEntry::MatchTied { .. } | GameEntry::Turn { .. } | GameEntry::Other { .. } => None,
        }
    }
}

/// Extract the readable gameplay phrase from a MetaMessage byte array, or `None`
/// when the frame carries no recognised text.
pub fn decode_meta_message(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 24 {
        return None;
    }
    // Non-printable bytes become a sentinel so a match cannot span binary fields.
    let candidate: String = bytes
        .iter()
        .map(|&b| {
            if (32..127).contains(&b) {
                b as char
            } else {
                '\0'
            }
        })
        .collect();
    DECODERS
        .iter()
        .find_map(|regex| regex.find(&candidate).map(|m| m.as_str().to_string()))
}

/// A starting hand size: digits, or a number word from one to seven, any case.
pub fn hand_size(word: &str) -> Option<u32> {
    match word.to_ascii_lowercase().as_str() {
        "one" => Some(1),
        "two" => Some(2),
        "three" => Some(3),
        "four" => Some(4),
        "five" => Some(5),
        "six" => Some(6),
        "seven" => Some(7),
        digits => digits.parse().ok(),
    }
}

fn num(text: &str) -> u32 {
    text.parse().unwrap_or(0)
}

/// Turn a decoded phrase into a typed entry.
pub fn parse_entry(message: &str) -> GameEntry {
    if let Some(c) = JOINED.captures(message) {
        return GameEntry::Joined {
            player: c[1].to_string(),
        };
    }
    if let Some(c) = ROLLED.captures(message) {
        return GameEntry::Rolled {
            player: c[1].to_string(),
            value: num(&c[2]),
        };
    }
    if let Some(c) = CHOOSES.captures(message) {
        return GameEntry::ChoosesToPlay {
            player: c[1].to_string(),
            first: &c[2] == "first",
        };
    }
    if let Some(c) = BEGINS.captures(message)
        && let Some(size) = hand_size(&c[2])
    {
        return GameEntry::BeginsGame {
            player: c[1].to_string(),
            hand_size: size,
        };
    }
    if let Some(c) = WINS_GAME.captures(message) {
        return GameEntry::WinsGame {
            player: c[1].to_string(),
        };
    }
    if let Some(c) = CONCEDED.captures(message) {
        return GameEntry::ConcededGame {
            player: c[1].to_string(),
        };
    }
    if let Some(c) = LOST_CONNECTION.captures(message) {
        return GameEntry::LostConnection {
            player: c[1].to_string(),
        };
    }
    if let Some(c) = MATCH_SCORE.captures(message) {
        return GameEntry::MatchScore {
            player: c[1].to_string(),
            wins: num(&c[3]),
            losses: num(&c[4]),
            is_final: &c[2] == "wins",
        };
    }
    if let Some(c) = MATCH_TIED.captures(message) {
        return GameEntry::MatchTied {
            first: num(&c[1]),
            second: num(&c[2]),
        };
    }
    if let Some(c) = TURN.captures(message) {
        return GameEntry::Turn { number: num(&c[1]) };
    }
    GameEntry::Other {
        text: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framed(message: &str) -> Vec<u8> {
        let mut bytes = vec![0u8; 24];
        bytes.extend_from_slice(message.as_bytes());
        bytes.push(0);
        bytes
    }

    #[test]
    fn decodes_result_phrases_from_bytes() {
        assert_eq!(
            decode_meta_message(&framed("@PSteve O wins the game.")).as_deref(),
            Some("@PSteve O wins the game.")
        );
        assert_eq!(decode_meta_message(&framed("x")), None);
        assert_eq!(decode_meta_message(&[64, 80]), None);
    }

    #[test]
    fn parses_every_result_phrase() {
        let cases = [
            (
                "@Pa.b-c rolled a 6.",
                GameEntry::Rolled {
                    player: "a.b-c".into(),
                    value: 6,
                },
            ),
            (
                "@P@PSteve O joined the game.",
                GameEntry::Joined {
                    player: "Steve O".into(),
                },
            ),
            (
                "@PSteve O chooses to play second.",
                GameEntry::ChoosesToPlay {
                    player: "Steve O".into(),
                    first: false,
                },
            ),
            (
                "@PSteve O begins the game with six cards in hand.",
                GameEntry::BeginsGame {
                    player: "Steve O".into(),
                    hand_size: 6,
                },
            ),
            (
                "@PSteve O puts a card on the bottom of their library and begins the game with six cards in hand.",
                GameEntry::BeginsGame {
                    player: "Steve O".into(),
                    hand_size: 6,
                },
            ),
            (
                "@Pabc puts two cards on the bottom of their library and begins the game with five cards in hand.",
                GameEntry::BeginsGame {
                    player: "abc".into(),
                    hand_size: 5,
                },
            ),
            (
                "@Pabc begins the game with 5 cards in hand.",
                GameEntry::BeginsGame {
                    player: "abc".into(),
                    hand_size: 5,
                },
            ),
            (
                "@Pabc wins the game.",
                GameEntry::WinsGame {
                    player: "abc".into(),
                },
            ),
            (
                "@Pabc has conceded from the game.",
                GameEntry::ConcededGame {
                    player: "abc".into(),
                },
            ),
            (
                "@Pabc has lost connection to the game.",
                GameEntry::LostConnection {
                    player: "abc".into(),
                },
            ),
            (
                "@Pabc leads the match 1-0",
                GameEntry::MatchScore {
                    player: "abc".into(),
                    wins: 1,
                    losses: 0,
                    is_final: false,
                },
            ),
            (
                "@Pabc wins the match 2-1",
                GameEntry::MatchScore {
                    player: "abc".into(),
                    wins: 2,
                    losses: 1,
                    is_final: true,
                },
            ),
            (
                "Match Tied 1-1",
                GameEntry::MatchTied {
                    first: 1,
                    second: 1,
                },
            ),
            ("@PTurn 7:", GameEntry::Turn { number: 7 }),
        ];
        for (message, expected) in cases {
            assert_eq!(parse_entry(message), expected, "message {message}");
        }
    }

    #[test]
    fn card_actions_are_kept_as_other() {
        let entry = parse_entry("@Pabc casts @[Lightning Bolt@:123,4:@].");
        assert!(matches!(entry, GameEntry::Other { .. }));
    }
}
