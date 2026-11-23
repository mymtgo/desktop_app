//! Reading the header tags of a `.mtgr` file (match token, ids, games).
//!
//! This is a line scan, not a full parser: it only needs the tags that say
//! which match and games a file holds. Each game starts at a `[Mtgr "..."]`
//! line and repeats its tags.

/// The match and game tags of a recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingHeader {
    /// The `Match` tag of the first game, lowercased.
    pub match_token: String,
    pub mtgo_match_id: Option<i64>,
    pub recorder: Option<String>,
    pub games: Vec<HeaderGame>,
}

/// One game in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderGame {
    /// Index of the game among the games in the file, from 0.
    pub position: u32,
    /// The `Game` tag (a file that joined at game 2 starts at 2).
    pub game_number: u32,
    pub mtgo_game_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderError {
    /// Not a `.mtgr` file, or a game without a numeric `Game` tag.
    NotMtgr,
    /// A `Mtgr` tag other than `0.3.0`.
    WrongVersion(String),
    NoMatchToken,
    NoGames,
}

const SUPPORTED_VERSION: &str = "0.3.0";

/// Read the header tags of every game in `text`.
pub fn read_header(text: &str) -> Result<RecordingHeader, HeaderError> {
    let mut blocks: Vec<Vec<(String, String)>> = Vec::new();
    for line in text.lines() {
        let Some((name, value)) = parse_tag(line) else {
            continue;
        };
        if name == "Mtgr" {
            if value != SUPPORTED_VERSION {
                return Err(HeaderError::WrongVersion(value));
            }
            blocks.push(Vec::new());
        }
        match blocks.last_mut() {
            Some(block) => block.push((name, value)),
            // A tag before any `Mtgr` line: not a file we know.
            None => return Err(HeaderError::NotMtgr),
        }
    }
    let Some(first) = blocks.first() else {
        return Err(HeaderError::NotMtgr);
    };

    let mut games = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        let game_number = tag(block, "Game")
            .and_then(|value| value.parse::<u32>().ok())
            .ok_or(HeaderError::NotMtgr)?;
        games.push(HeaderGame {
            position: index as u32,
            game_number,
            mtgo_game_id: tag(block, "MTGO_GameId").and_then(|value| value.parse().ok()),
        });
    }

    let match_token = tag(first, "Match")
        .filter(|value| !value.is_empty())
        .ok_or(HeaderError::NoMatchToken)?
        .to_lowercase();
    Ok(RecordingHeader {
        match_token,
        mtgo_match_id: tag(first, "MTGO_MatchId").and_then(|value| value.parse().ok()),
        recorder: tag(first, "Recorder").map(str::to_string),
        games,
    })
}

fn tag<'a>(block: &'a [(String, String)], name: &str) -> Option<&'a str> {
    block
        .iter()
        .find(|(tag_name, _)| tag_name == name)
        .map(|(_, value)| value.as_str())
}

/// `[Name "value"]` into its name and unescaped value.
fn parse_tag(line: &str) -> Option<(String, String)> {
    let inner = line.trim_end().strip_prefix('[')?.strip_suffix(']')?;
    let (name, quoted) = inner.split_once(' ')?;
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let body = quoted.strip_prefix('"')?.strip_suffix('"')?;
    let mut value = String::new();
    let mut chars = body.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            value.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => value.push('\n'),
            Some(other) => value.push(other),
            None => value.push('\\'),
        }
    }
    Some((name.to_string(), value))
}
