//! Cards and per-game facts read from a match's game log, port of 0.x
//! `ExtractCardsFromGameLog` (with the parts of `ExtractGameResults` it uses:
//! `detectPlayers` and `splitIntoGames`).
//!
//! The game log names cards as `@[Name@:RAW,INSTANCE:@]`, where RAW is the
//! catalog id doubled (front face `id * 2`, back face `id * 2 + 1`), so the
//! catalog id is `RAW >> 1`. A card is credited to a player only by phrases
//! where it surely belongs to them; phrases where the card may be the other
//! player's are skipped (see `owned_cards`).

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use mtgo_protocol::entries::{JOINED, ROLLED, TURN, player_re};
use regex::Regex;

use crate::gamelog::GameLogEntry;

/// The longest player name read from a game log, in bytes. MTGO names are
/// far shorter; a longer one comes from a crafted log and is ignored.
pub const MAX_PLAYER_NAME_BYTES: usize = 64;

/// The most players read from one game log. A match has two; the rest of a
/// crafted log's names are ignored, so it cannot build a regex set per name.
pub const MAX_PLAYERS: usize = 8;

/// A card reference, capturing the name, the raw id and the instance id.
const CARD: &str = r"@\[([^@]+)@:([0-9]+),([0-9]+):@\]";

static GAME_END: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"wins the game|has conceded from the game").expect("valid regex"));
/// One digit, as 0.x reads the roll (protocol's `ROLLED` reads them all).
static DICE: LazyLock<Regex> = LazyLock::new(|| player_re(r"^@P({P}) rolled a ([0-9])"));
static MULLIGAN: LazyLock<Regex> = LazyLock::new(|| player_re(r"^@P({P}) mulligans to"));
static ANY_CARD: LazyLock<Regex> = LazyLock::new(|| Regex::new(CARD).expect("valid regex"));
static AFTER_COLON: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r":\s*(.+)$").expect("valid regex"));

/// Every per-card counter, in 0.x's `COUNTER_FIELDS` order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Counter {
    Cast,
    Played,
    Kicked,
    Flashback,
    Madness,
    Evoked,
    Warp,
    FreeCast,
    Bargained,
    Dashed,
    Bestowed,
    Replicated,
    Spectacle,
    Rebound,
    Escaped,
    Ninjutsu,
    Suspended,
    Buyback,
    Disturb,
    Foretold,
    Retraced,
    Mayhem,
    Miracle,
    Gifted,
    Casualty,
    Activated,
}

/// How many counters there are.
pub const COUNTER_COUNT: usize = 26;

impl Counter {
    pub const ALL: [Counter; COUNTER_COUNT] = [
        Counter::Cast,
        Counter::Played,
        Counter::Kicked,
        Counter::Flashback,
        Counter::Madness,
        Counter::Evoked,
        Counter::Warp,
        Counter::FreeCast,
        Counter::Bargained,
        Counter::Dashed,
        Counter::Bestowed,
        Counter::Replicated,
        Counter::Spectacle,
        Counter::Rebound,
        Counter::Escaped,
        Counter::Ninjutsu,
        Counter::Suspended,
        Counter::Buyback,
        Counter::Disturb,
        Counter::Foretold,
        Counter::Retraced,
        Counter::Mayhem,
        Counter::Miracle,
        Counter::Gifted,
        Counter::Casualty,
        Counter::Activated,
    ];

    /// The 0.x field name (`free_cast`, `activated`, ...).
    pub fn field_name(self) -> &'static str {
        match self {
            Counter::Cast => "cast",
            Counter::Played => "played",
            Counter::Kicked => "kicked",
            Counter::Flashback => "flashback",
            Counter::Madness => "madness",
            Counter::Evoked => "evoked",
            Counter::Warp => "warp",
            Counter::FreeCast => "free_cast",
            Counter::Bargained => "bargained",
            Counter::Dashed => "dashed",
            Counter::Bestowed => "bestowed",
            Counter::Replicated => "replicated",
            Counter::Spectacle => "spectacle",
            Counter::Rebound => "rebound",
            Counter::Escaped => "escaped",
            Counter::Ninjutsu => "ninjutsu",
            Counter::Suspended => "suspended",
            Counter::Buyback => "buyback",
            Counter::Disturb => "disturb",
            Counter::Foretold => "foretold",
            Counter::Retraced => "retraced",
            Counter::Mayhem => "mayhem",
            Counter::Miracle => "miracle",
            Counter::Gifted => "gifted",
            Counter::Casualty => "casualty",
            Counter::Activated => "activated",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// Casting methods read from the text after the card in a "casts" line:
/// counter, then needles matched case-sensitively (any hit). The casing is
/// exactly what MTGO writes.
const CAST_SUFFIX_FLAGS: [(Counter, &[&str]); 23] = [
    (Counter::Kicked, &["with kicker", "kicked "]),
    (Counter::Flashback, &["Flashback"]),
    (Counter::Madness, &["Madness"]),
    (Counter::Evoked, &["evoke"]),
    (Counter::Warp, &["with warp", "for its warp cost"]),
    (Counter::FreeCast, &["without paying its mana cost"]),
    (Counter::Bargained, &["with Bargain"]),
    (Counter::Dashed, &["with dash", "for its dash cost"]),
    (Counter::Bestowed, &["with Bestow", "for its bestow cost"]),
    (Counter::Replicated, &["with Replicate", "(Replicated"]),
    (
        Counter::Spectacle,
        &["with Spectacle", "for its spectacle cost"],
    ),
    (Counter::Rebound, &["with Rebound"]),
    (Counter::Escaped, &["for its escape cost"]),
    (Counter::Ninjutsu, &["with sneak"]),
    (Counter::Suspended, &["with suspend"]),
    (Counter::Buyback, &["with buyback"]),
    (Counter::Disturb, &["with disturb"]),
    (Counter::Foretold, &["foretell cost"]),
    (Counter::Retraced, &["with Retrace"]),
    (Counter::Mayhem, &["with Mayhem"]),
    (Counter::Miracle, &["with Miracle"]),
    (Counter::Gifted, &["with Gift"]),
    (Counter::Casualty, &["with Casualty"]),
];

/// A card a player was seen with in the game log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameLogCard {
    /// Catalog id (`RAW >> 1`).
    pub mtgo_id: u32,
    /// The name the log gave when the card was first seen.
    pub name: String,
    /// The turn the card was first cast on (`None` when never cast after a
    /// turn marker). At match level, the first game's value.
    pub cast_turn: Option<u32>,
    /// Counts, indexed by `Counter` order.
    pub counts: [u32; COUNTER_COUNT],
    /// The distinct copies (instance ids, `@[Name@:RAW,INSTANCE:@]`) the
    /// player's lines name in the game. MTGO keeps a copy's instance id
    /// across zone changes (owner ruling, issue #6). At match level, the
    /// most of any game.
    pub copies_seen: u32,
}

impl GameLogCard {
    pub fn count(&self, counter: Counter) -> u32 {
        self.counts[counter.index()]
    }
}

/// One player's cards, in first-seen order, one entry per catalog id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerCards {
    pub player: String,
    pub cards: Vec<GameLogCard>,
}

/// A number per player (a die roll, a mulligan count), in first-seen order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerValue {
    pub player: String,
    pub value: u32,
}

/// Per-game facts from the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameMeta {
    /// Each player's last die roll in the game.
    pub dice_rolls: Vec<PlayerValue>,
    /// Mulligans per player (every detected player, 0 when none).
    pub mulligans: Vec<PlayerValue>,
    /// The highest `Turn N:` marker, `None` when there is none.
    pub turn_count: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PregameKind {
    /// Revealed from the opening hand (Chancellors, Devourer of Destiny).
    Revealed,
    /// Put onto the battlefield before turn 1 (Leylines, Gemstone Caverns).
    Played,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PregameAction {
    pub mtgo_id: u32,
    pub kind: PregameKind,
}

/// One player's actions between the last "begins the game with" line and
/// the first `Turn 1:` marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerPregame {
    pub player: String,
    pub actions: Vec<PregameAction>,
}

/// One game of the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogGame {
    /// Cards per detected player, in `GameLogCards::players` order.
    pub cards: Vec<PlayerCards>,
    pub meta: GameMeta,
    /// Pregame actions per detected player, in `GameLogCards::players` order.
    pub pregame: Vec<PlayerPregame>,
}

/// Everything read from a match's game log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameLogCards {
    /// Players in first-seen order (from join and roll lines).
    pub players: Vec<String>,
    /// Cards per player over the whole match, counters summed.
    pub cards_by_player: Vec<PlayerCards>,
    /// Per game, in log order.
    pub games: Vec<LogGame>,
}

impl GameLogCards {
    /// A player's cards in one game (0-based), empty when unknown.
    pub fn game_cards(&self, game_index: usize, player: &str) -> &[GameLogCard] {
        let Some(game) = self.games.get(game_index) else {
            return &[];
        };
        cards_of(&game.cards, player)
    }

    /// A player's cards over the whole match, empty when unknown.
    pub fn match_cards(&self, player: &str) -> &[GameLogCard] {
        cards_of(&self.cards_by_player, player)
    }
}

fn cards_of<'a>(lists: &'a [PlayerCards], player: &str) -> &'a [GameLogCard] {
    lists
        .iter()
        .find(|list| list.player == player)
        .map(|list| list.cards.as_slice())
        .unwrap_or(&[])
}

/// The two players' names, in first-seen order, from the join and roll lines
/// (0.x `ExtractGameResults::detectPlayers`). Names longer than
/// [`MAX_PLAYER_NAME_BYTES`] are skipped, and at most [`MAX_PLAYERS`] are
/// kept.
pub fn detect_players(entries: &[GameLogEntry]) -> Vec<String> {
    let mut players: Vec<String> = Vec::new();
    for entry in entries {
        if players.len() >= MAX_PLAYERS {
            break;
        }
        let caps = JOINED
            .captures(&entry.message)
            .or_else(|| ROLLED.captures(&entry.message));
        if let Some(caps) = caps {
            let name = caps[1].to_string();
            if name.len() <= MAX_PLAYER_NAME_BYTES && !players.contains(&name) {
                players.push(name);
            }
        }
    }
    players
}

/// The entries split into games (0.x `ExtractGameResults::splitIntoGames`).
/// After a win or concede line, the next roll or join line starts a new game.
/// A lost connection does not end a game (the player may reconnect).
pub fn split_into_games(entries: &[GameLogEntry]) -> Vec<&[GameLogEntry]> {
    let mut games = Vec::new();
    let mut start = 0;
    let mut game_end_seen = false;
    for (index, entry) in entries.iter().enumerate() {
        let message = &entry.message;
        if GAME_END.is_match(message) {
            game_end_seen = true;
        }
        if game_end_seen && (ROLLED.is_match(message) || JOINED.is_match(message)) {
            games.push(&entries[start..index]);
            start = index;
            game_end_seen = false;
        }
    }
    if start < entries.len() {
        games.push(&entries[start..]);
    }
    games
}

/// Cards per player and game, game facts and pregame actions from a match's
/// game log entries (0.x `ExtractCardsFromGameLog::run`).
pub fn extract_log_cards(entries: &[GameLogEntry]) -> GameLogCards {
    // A player whose patterns cannot be built is left out.
    let patterns: Vec<PlayerPatterns> = detect_players(entries)
        .iter()
        .filter_map(|player| PlayerPatterns::new(player))
        .collect();
    let players: Vec<String> = patterns.iter().map(|p| p.player.clone()).collect();

    let mut games = Vec::new();
    let mut cards_by_player: Vec<PlayerCards> = players
        .iter()
        .map(|player| PlayerCards {
            player: player.clone(),
            cards: Vec::new(),
        })
        .collect();

    for game_entries in split_into_games(entries) {
        let game_cards = cards_from_entries(game_entries, &patterns);
        for (total, game) in cards_by_player.iter_mut().zip(&game_cards) {
            add_to_match_totals(total, &game.cards);
        }
        games.push(LogGame {
            cards: game_cards,
            meta: game_meta(game_entries, &players),
            pregame: pregame_actions(game_entries, &patterns),
        });
    }

    GameLogCards {
        players,
        cards_by_player,
        games,
    }
}

/// Sum a game's cards into the match totals: a card already there gets its
/// counters added (its name and cast turn stay); a new one is appended.
fn add_to_match_totals(total: &mut PlayerCards, game_cards: &[GameLogCard]) {
    for card in game_cards {
        match total.cards.iter_mut().find(|c| c.mtgo_id == card.mtgo_id) {
            Some(existing) => {
                for counter in Counter::ALL {
                    existing.counts[counter.index()] += card.count(counter);
                }
                existing.copies_seen = existing.copies_seen.max(card.copies_seen);
            }
            None => total.cards.push(card.clone()),
        }
    }
}

/// The per-player regexes, built once per extraction.
struct PlayerPatterns {
    player: String,
    marker: String,
    casts: Regex,
    plays: Regex,
    ninjutsu: Regex,
    activates: Regex,
    triggered: Regex,
    reveals_opening_hand: Regex,
    reveals_cards_with: Regex,
    reveals: Regex,
    discards: Regex,
    into_graveyard: Regex,
    possessive: Regex,
    names_for: Regex,
    onto_battlefield: Regex,
}

impl PlayerPatterns {
    /// The patterns for `player`; `None` when one cannot be built.
    fn new(player: &str) -> Option<Self> {
        let p = regex::escape(player);
        let re = |rest: &str| Regex::new(&format!("@P{p}{}", rest.replace("{CARD}", CARD))).ok();
        Some(PlayerPatterns {
            player: player.to_string(),
            marker: format!("@P{player}"),
            casts: re(" casts {CARD}(.*)")?,
            plays: re(" plays {CARD}")?,
            ninjutsu: re(" activates Ninjutsu ability of {CARD}")?,
            activates: re(" activates an ability of {CARD}")?,
            triggered: re(" puts a triggered ability from {CARD}")?,
            reveals_opening_hand: re(" reveals {CARD} from their opening hand")?,
            reveals_cards_with: re(r" reveals [0-9]+ cards with @\[")?,
            reveals: re(" reveals {CARD}")?,
            discards: re(" discards {CARD}")?,
            into_graveyard: re(" puts {CARD} into their graveyard")?,
            possessive: re("'s {CARD}")?,
            names_for: re(" names .+ for {CARD}")?,
            onto_battlefield: re(" puts {CARD} onto the battlefield")?,
        })
    }
}

/// A card seen in one message, with the counters that message sets.
struct SeenCard {
    mtgo_id: u32,
    name: String,
    /// The copy's instance id, `None` when the digits do not fit.
    instance: Option<u64>,
    flags: Vec<Counter>,
}

/// `RAW >> 1` as a catalog id, `None` when the digits do not fit.
fn catalog_id(raw: &str) -> Option<u32> {
    let raw: u64 = raw.parse().ok()?;
    u32::try_from(raw >> 1).ok()
}

/// A card from a regex match whose groups 1 to 3 are the name, the raw id
/// and the instance id.
fn seen(caps: &regex::Captures, flags: Vec<Counter>) -> Option<SeenCard> {
    Some(SeenCard {
        mtgo_id: catalog_id(&caps[2])?,
        name: caps[1].to_string(),
        instance: caps[3].parse().ok(),
        flags,
    })
}

fn one(card: Option<SeenCard>) -> Vec<SeenCard> {
    card.into_iter().collect()
}

/// The cards in `message` that surely belong to the player, by the first
/// phrase that matches (0.x `extractOwnedCards`).
///
/// Skipped on purpose (the card may be the other player's, or ownership is
/// unclear), so these return nothing:
/// - "removes N counters from @[Card]": combat damage to a planeswalker logs
///   the attacker removing loyalty from the defender's planeswalker.
/// - "exiles @[Card1] with @[Card2]" and "returns @[Card1] with @[Card2]": the
///   affected player is logged with the opponent's ability source (and under
///   Mindslaver the controller uses the opponent's cards).
/// - "draws a card with @[Card]": the source is often the opponent's.
/// - "is being attacked by @[Card]": the attacker is the opponent's.
/// - "chooses to use @[Card]'s ability", "declines to use @[Card]'s
///   ability", "chooses X for @[Card]": ambiguous.
fn owned_cards(message: &str, p: &PlayerPatterns) -> Vec<SeenCard> {
    if let Some(caps) = p.casts.captures(message) {
        let suffix = &caps[4];
        let mut flags = vec![Counter::Cast];
        for (counter, needles) in CAST_SUFFIX_FLAGS {
            if needles.iter().any(|needle| suffix.contains(needle)) {
                flags.push(counter);
            }
        }
        return one(seen(&caps, flags));
    }
    if let Some(caps) = p.plays.captures(message) {
        return one(seen(&caps, vec![Counter::Played]));
    }
    // Ninjutsu puts the creature onto the battlefield without a cast line.
    if let Some(caps) = p.ninjutsu.captures(message) {
        return one(seen(&caps, vec![Counter::Ninjutsu]));
    }
    if let Some(caps) = p.activates.captures(message) {
        return one(seen(&caps, vec![Counter::Activated]));
    }
    // The ability source is the player's; cards after "targeting" are not.
    if let Some(caps) = p.triggered.captures(message) {
        return one(seen(&caps, Vec::new()));
    }
    if let Some(caps) = p.reveals_opening_hand.captures(message) {
        return one(seen(&caps, Vec::new()));
    }
    // "reveals N cards with @[Source]: @[Card1], @[Card2]": the player reveals
    // their own cards, but the source may be the other player's, so only the
    // list after the colon counts.
    if p.reveals_cards_with.is_match(message) {
        let mut cards = Vec::new();
        if let Some(list) = AFTER_COLON.captures(message) {
            for caps in ANY_CARD.captures_iter(&list[1]) {
                cards.extend(seen(&caps, Vec::new()));
            }
        }
        return cards;
    }
    for regex in [
        &p.reveals,
        &p.discards,
        &p.into_graveyard,
        &p.possessive,
        &p.names_for,
    ] {
        if let Some(caps) = regex.captures(message) {
            return one(seen(&caps, Vec::new()));
        }
    }
    Vec::new()
}

/// One player's running tally while reading a game.
struct Tally {
    cards: Vec<GameLogCard>,
    counts: HashMap<u32, [u32; COUNTER_COUNT]>,
    cast_turns: HashMap<u32, u32>,
    /// The distinct instance ids per card.
    instances: HashMap<u32, HashSet<u64>>,
}

/// Cards per player in `entries` (0.x `extractFromEntries`).
fn cards_from_entries(entries: &[GameLogEntry], patterns: &[PlayerPatterns]) -> Vec<PlayerCards> {
    let mut tallies: Vec<Tally> = patterns
        .iter()
        .map(|_| Tally {
            cards: Vec::new(),
            counts: HashMap::new(),
            cast_turns: HashMap::new(),
            instances: HashMap::new(),
        })
        .collect();
    // The turn a card was first cast on, from the `Turn N:` markers read in
    // order with the cast lines.
    let mut turn: u32 = 0;

    for entry in entries {
        let message = &entry.message;
        if let Some(caps) = TURN.captures(message) {
            turn = caps[1].parse().unwrap_or(u32::MAX);
        }
        for (p, tally) in patterns.iter().zip(tallies.iter_mut()) {
            if !message.contains(&p.marker) {
                continue;
            }
            for card in owned_cards(message, p) {
                let counts = tally
                    .counts
                    .entry(card.mtgo_id)
                    .or_insert([0; COUNTER_COUNT]);
                for flag in &card.flags {
                    counts[flag.index()] += 1;
                }
                // First cast only: how early it came down, not how often.
                if card.flags.contains(&Counter::Cast) && turn > 0 {
                    tally.cast_turns.entry(card.mtgo_id).or_insert(turn);
                }
                if let Some(instance) = card.instance {
                    tally
                        .instances
                        .entry(card.mtgo_id)
                        .or_default()
                        .insert(instance);
                }
                if tally.cards.iter().any(|c| c.mtgo_id == card.mtgo_id) {
                    continue;
                }
                tally.cards.push(GameLogCard {
                    mtgo_id: card.mtgo_id,
                    name: card.name,
                    cast_turn: None,
                    counts: [0; COUNTER_COUNT],
                    copies_seen: 0,
                });
            }
        }
    }

    patterns
        .iter()
        .zip(tallies)
        .map(|(p, tally)| {
            let mut cards = tally.cards;
            for card in &mut cards {
                card.counts = tally
                    .counts
                    .get(&card.mtgo_id)
                    .copied()
                    .unwrap_or([0; COUNTER_COUNT]);
                card.cast_turn = tally.cast_turns.get(&card.mtgo_id).copied();
                card.copies_seen = tally
                    .instances
                    .get(&card.mtgo_id)
                    .map_or(0, |ids| ids.len() as u32);
            }
            PlayerCards {
                player: p.player.clone(),
                cards,
            }
        })
        .collect()
}

/// Add one to `player`'s count, appending the player when new.
fn bump(values: &mut Vec<PlayerValue>, player: &str) {
    match values.iter_mut().find(|v| v.player == player) {
        Some(existing) => existing.value += 1,
        None => values.push(PlayerValue {
            player: player.to_string(),
            value: 1,
        }),
    }
}

/// Dice rolls, mulligans and turn count of one game (0.x `extractGameMeta`).
fn game_meta(entries: &[GameLogEntry], players: &[String]) -> GameMeta {
    let mut dice_rolls: Vec<PlayerValue> = Vec::new();
    let mut mulligans: Vec<PlayerValue> = players
        .iter()
        .map(|player| PlayerValue {
            player: player.clone(),
            value: 0,
        })
        .collect();
    let mut turn_count: Option<u32> = None;

    for entry in entries {
        let message = &entry.message;
        if let Some(caps) = DICE.captures(message) {
            let roll: u32 = caps[2].parse().unwrap_or(0);
            match dice_rolls.iter_mut().find(|d| d.player == caps[1]) {
                Some(existing) => existing.value = roll,
                None => dice_rolls.push(PlayerValue {
                    player: caps[1].to_string(),
                    value: roll,
                }),
            }
        }
        if let Some(caps) = MULLIGAN.captures(message) {
            bump(&mut mulligans, &caps[1]);
        }
        if let Some(caps) = TURN.captures(message) {
            let turn: u32 = caps[1].parse().unwrap_or(u32::MAX);
            if turn_count.is_none_or(|highest| turn > highest) {
                turn_count = Some(turn);
            }
        }
    }

    GameMeta {
        dice_rolls,
        mulligans,
        turn_count,
    }
}

/// Actions between the last "begins the game with" line and the first
/// `Turn 1:` marker after it (0.x `extractPregameActions`).
fn pregame_actions(entries: &[GameLogEntry], patterns: &[PlayerPatterns]) -> Vec<PlayerPregame> {
    let mut out: Vec<PlayerPregame> = patterns
        .iter()
        .map(|p| PlayerPregame {
            player: p.player.clone(),
            actions: Vec::new(),
        })
        .collect();

    let mut window_start: Option<usize> = None;
    let mut window_end: Option<usize> = None;
    for (index, entry) in entries.iter().enumerate() {
        if entry.message.contains("begins the game with") {
            window_start = Some(index + 1);
        }
        if window_start.is_some() && entry.message.starts_with("@PTurn 1:") {
            window_end = Some(index);
            break;
        }
    }
    let (Some(start), Some(end)) = (window_start, window_end) else {
        return out;
    };
    if start >= end {
        return out;
    }

    for entry in &entries[start..end] {
        for (p, pregame) in patterns.iter().zip(out.iter_mut()) {
            if let Some(caps) = p.reveals_opening_hand.captures(&entry.message)
                && let Some(mtgo_id) = catalog_id(&caps[2])
            {
                pregame.actions.push(PregameAction {
                    mtgo_id,
                    kind: PregameKind::Revealed,
                });
            }
            if let Some(caps) = p.onto_battlefield.captures(&entry.message)
                && let Some(mtgo_id) = catalog_id(&caps[2])
            {
                pregame.actions.push(PregameAction {
                    mtgo_id,
                    kind: PregameKind::Played,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(messages: &[&str]) -> Vec<GameLogEntry> {
        messages
            .iter()
            .enumerate()
            .map(|(index, message)| GameLogEntry {
                timestamp_ms: index as i64 * 1000,
                message: message.to_string(),
            })
            .collect()
    }

    fn joined(extra: &[&str]) -> Vec<GameLogEntry> {
        let mut messages = vec!["@P@PAlpha joined the game.", "@P@PBravo joined the game."];
        messages.extend_from_slice(extra);
        entries(&messages)
    }

    fn ids(cards: &[GameLogCard]) -> Vec<u32> {
        cards.iter().map(|c| c.mtgo_id).collect()
    }

    fn named<'a>(cards: &'a [GameLogCard], name: &str) -> &'a GameLogCard {
        cards
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("no card named {name}"))
    }

    fn counts(list: &[PlayerValue]) -> Vec<(&str, u32)> {
        list.iter().map(|c| (c.player.as_str(), c.value)).collect()
    }

    #[test]
    fn does_not_attribute_a_planeswalker_to_the_player_removing_loyalty_counters_via_combat() {
        let result = extract_log_cards(&joined(&[
            "@PBravo casts @[Karn, the Great Creator@:155958,100:@].",
            "@PAlpha removes a loyalty counter from @[Karn, the Great Creator@:155958,100:@].",
        ]));
        assert!(!ids(result.match_cards("Alpha")).contains(&77979));
        assert!(ids(result.match_cards("Bravo")).contains(&77979));
    }

    #[test]
    fn does_not_attribute_the_ability_source_to_the_affected_player_in_exiles_with_messages() {
        let result = extract_log_cards(&joined(&[
            "@PBravo casts @[Subtlety@:181014,200:@] by exiling a blue card from your hand with evoke.",
            "@PAlpha exiles @[Sowing Mycospawn@:251694,201:@] with @[Subtlety@:181014,200:@]'s ability.",
        ]));
        assert!(!ids(result.match_cards("Alpha")).contains(&90507));
        assert!(ids(result.match_cards("Bravo")).contains(&90507));
    }

    #[test]
    fn does_not_attribute_the_ability_source_to_the_affected_player_in_returns_with_messages() {
        let result = extract_log_cards(&joined(&[
            "@PBravo casts @[Teferi, Time Raveler@:143200,300:@].",
            "@PAlpha returns @[Sowing Mycospawn@:251694,301:@] to its owner's hand with @[Teferi, Time Raveler@:143200,300:@]'s ability.",
        ]));
        assert!(!ids(result.match_cards("Alpha")).contains(&71600));
        assert!(ids(result.match_cards("Bravo")).contains(&71600));
    }

    #[test]
    fn separates_land_plays_from_spell_casts() {
        let result = extract_log_cards(&joined(&[
            "@PAlpha plays @[Forest@:281774,100:@].",
            "@PAlpha casts @[Llanowar Elves@:1000,101:@].",
        ]));
        let alpha = result.match_cards("Alpha");
        let forest = named(alpha, "Forest");
        assert_eq!(forest.count(Counter::Cast), 0);
        assert_eq!(forest.count(Counter::Played), 1);
        let elves = named(alpha, "Llanowar Elves");
        assert_eq!(elves.count(Counter::Cast), 1);
        assert_eq!(elves.count(Counter::Played), 0);
    }

    #[test]
    fn tracks_kicked_casts() {
        let result = extract_log_cards(&joined(&[
            "@PAlpha casts @[Sowing Mycospawn@:251694,100:@] with kicker.",
            "@PAlpha casts @[Sowing Mycospawn@:251694,101:@].",
        ]));
        let card = named(result.match_cards("Alpha"), "Sowing Mycospawn");
        assert_eq!(card.count(Counter::Cast), 2);
        assert_eq!(card.count(Counter::Kicked), 1);
    }

    #[test]
    fn tracks_flashback_casts() {
        let result = extract_log_cards(&joined(&[
            "@PAlpha casts @[Eviscerator's Insight@:252406,100:@] with Flashback {4B} from the graveyard.",
        ]));
        let card = named(result.match_cards("Alpha"), "Eviscerator's Insight");
        assert_eq!(card.count(Counter::Cast), 1);
        assert_eq!(card.count(Counter::Flashback), 1);
    }

    #[test]
    fn tracks_madness_casts() {
        let result = extract_log_cards(&joined(&[
            "@PAlpha casts @[Fiery Temper@:119868,100:@] by paying {R} with Madness {R} targeting Bravo.",
        ]));
        let card = named(result.match_cards("Alpha"), "Fiery Temper");
        assert_eq!(card.count(Counter::Cast), 1);
        assert_eq!(card.count(Counter::Madness), 1);
    }

    #[test]
    fn tracks_evoke_casts() {
        let result = extract_log_cards(&joined(&[
            "@PAlpha casts @[Solitude@:182406,100:@] by exiling a white card from your hand with evoke.",
        ]));
        let card = named(result.match_cards("Alpha"), "Solitude");
        assert_eq!(card.count(Counter::Cast), 1);
        assert_eq!(card.count(Counter::Evoked), 1);
    }

    #[test]
    fn tracks_activated_ability_count() {
        let result = extract_log_cards(&joined(&[
            "@PAlpha casts @[Karn, the Great Creator@:155958,100:@].",
            "@PAlpha activates an ability of @[Karn, the Great Creator@:155958,100:@] (You may reveal an artifact card...).",
            "@PAlpha activates an ability of @[Karn, the Great Creator@:155958,100:@] (You may reveal an artifact card...).",
        ]));
        let card = named(result.match_cards("Alpha"), "Karn, the Great Creator");
        assert_eq!(card.count(Counter::Cast), 1);
        assert_eq!(card.count(Counter::Activated), 2);
    }

    #[test]
    fn extracts_dice_rolls_per_player_per_game() {
        let result = extract_log_cards(&entries(&[
            "@PAlpha rolled a 5.",
            "@PBravo rolled a 3.",
            "@P@PAlpha joined the game.",
            "@P@PBravo joined the game.",
            "@PAlpha begins the game with seven cards in hand.",
            "@PBravo begins the game with seven cards in hand.",
            "@PAlpha wins the game.",
        ]));
        assert_eq!(result.games.len(), 1);
        assert_eq!(
            counts(&result.games[0].meta.dice_rolls),
            vec![("Alpha", 5), ("Bravo", 3)]
        );
    }

    #[test]
    fn extracts_mulligan_count_per_player() {
        let result = extract_log_cards(&joined(&[
            "@PAlpha mulligans to six cards.",
            "@PAlpha begins the game with six cards in hand.",
            "@PBravo begins the game with seven cards in hand.",
            "@PAlpha wins the game.",
        ]));
        assert_eq!(
            counts(&result.games[0].meta.mulligans),
            vec![("Alpha", 1), ("Bravo", 0)]
        );
    }

    fn pregame<'a>(result: &'a GameLogCards, game: usize, player: &str) -> &'a [PregameAction] {
        &result.games[game]
            .pregame
            .iter()
            .find(|p| p.player == player)
            .expect("player")
            .actions
    }

    #[test]
    fn detects_pregame_reveal_from_opening_hand() {
        let result = extract_log_cards(&entries(&[
            "@PAlpha rolled a 3.",
            "@PBravo rolled a 4.",
            "@P@PAlpha joined the game.",
            "@P@PBravo joined the game.",
            "@PBravo chooses to play first.",
            "@PBravo begins the game with seven cards in hand.",
            "@PAlpha begins the game with seven cards in hand.",
            "@PAlpha reveals @[Devourer of Destiny@:252042,427:@] from their opening hand.",
            "@PTurn 1: Bravo",
            "@PAlpha wins the game.",
        ]));
        assert_eq!(
            pregame(&result, 0, "Alpha"),
            &[PregameAction {
                mtgo_id: 126021,
                kind: PregameKind::Revealed,
            }]
        );
        assert!(pregame(&result, 0, "Bravo").is_empty());
    }

    #[test]
    fn detects_pregame_put_onto_battlefield_gemstone_caverns() {
        let result = extract_log_cards(&entries(&[
            "@PAlpha rolled a 3.",
            "@PBravo rolled a 4.",
            "@P@PAlpha joined the game.",
            "@P@PBravo joined the game.",
            "@PBravo chooses to play first.",
            "@PBravo begins the game with seven cards in hand.",
            "@PAlpha begins the game with seven cards in hand.",
            "@PAlpha puts @[Gemstone Caverns@:51778,419:@] onto the battlefield.",
            "@PTurn 1: Bravo",
            "@PAlpha wins the game.",
        ]));
        assert_eq!(
            pregame(&result, 0, "Alpha"),
            &[PregameAction {
                mtgo_id: 25889,
                kind: PregameKind::Played,
            }]
        );
        assert!(pregame(&result, 0, "Bravo").is_empty());
    }

    #[test]
    fn detects_pregame_put_onto_battlefield_leyline() {
        let result = extract_log_cards(&joined(&[
            "@PAlpha chooses to play first.",
            "@PAlpha begins the game with seven cards in hand.",
            "@PBravo begins the game with seven cards in hand.",
            "@PBravo puts @[Leyline of the Guildpact@:242604,414:@] onto the battlefield.",
            "@PTurn 1: Alpha",
            "@PAlpha wins the game.",
        ]));
        assert!(pregame(&result, 0, "Alpha").is_empty());
        let bravo = pregame(&result, 0, "Bravo");
        assert_eq!(bravo.len(), 1);
        assert_eq!(bravo[0].kind, PregameKind::Played);
    }

    #[test]
    fn does_not_flag_post_turn_1_actions_as_pregame() {
        let result = extract_log_cards(&joined(&[
            "@PAlpha begins the game with seven cards in hand.",
            "@PBravo begins the game with seven cards in hand.",
            "@PTurn 1: Alpha",
            "@PAlpha plays @[Gemstone Caverns@:51778,419:@].",
            "@PAlpha wins the game.",
        ]));
        assert!(pregame(&result, 0, "Alpha").is_empty());
        assert!(pregame(&result, 0, "Bravo").is_empty());
    }

    #[test]
    fn detects_pregame_actions_in_multiple_games_independently() {
        let result = extract_log_cards(&entries(&[
            // Game 1: Alpha reveals Devourer.
            "@PAlpha rolled a 3.",
            "@PBravo rolled a 5.",
            "@P@PAlpha joined the game.",
            "@P@PBravo joined the game.",
            "@PBravo chooses to play first.",
            "@PBravo begins the game with seven cards in hand.",
            "@PAlpha begins the game with seven cards in hand.",
            "@PAlpha reveals @[Devourer of Destiny@:252042,427:@] from their opening hand.",
            "@PTurn 1: Bravo",
            "@PBravo wins the game.",
            // Game 2: no pregame actions.
            "@P@PAlpha joined the game.",
            "@P@PBravo joined the game.",
            "@PAlpha chooses to play first.",
            "@PAlpha begins the game with seven cards in hand.",
            "@PBravo begins the game with seven cards in hand.",
            "@PTurn 1: Alpha",
            "@PAlpha wins the game.",
        ]));
        assert_eq!(result.games.len(), 2);
        let alpha = pregame(&result, 0, "Alpha");
        assert_eq!(alpha.len(), 1);
        assert_eq!(alpha[0].kind, PregameKind::Revealed);
        assert!(pregame(&result, 1, "Alpha").is_empty());
        assert!(pregame(&result, 1, "Bravo").is_empty());
    }

    #[test]
    fn extracts_turn_count_from_turn_markers() {
        let result = extract_log_cards(&joined(&[
            "@PTurn 1: Alpha",
            "@PTurn 1: Bravo",
            "@PTurn 2: Alpha",
            "@PTurn 2: Bravo",
            "@PTurn 3: Alpha",
            "@PAlpha wins the game.",
        ]));
        assert_eq!(result.games[0].meta.turn_count, Some(3));
    }

    #[test]
    fn tracks_warp_casts_in_both_logged_phrasings_without_card_name_false_positives() {
        let result = extract_log_cards(&joined(&[
            "@PAlpha casts @[Pinnacle Emissary@:280000,100:@] by paying {#ur-} with warp.",
            "@PAlpha casts @[Perigee Beckoner@:280002,101:@] for its warp cost.",
            "@PAlpha casts @[Snap@:280004,102:@] targeting @[Warped Tusker@:280006,103:@].",
        ]));
        let alpha = result.match_cards("Alpha");
        for name in ["Pinnacle Emissary", "Perigee Beckoner"] {
            assert_eq!(named(alpha, name).count(Counter::Cast), 1, "{name}");
            assert_eq!(named(alpha, name).count(Counter::Warp), 1, "{name}");
        }
        assert_eq!(named(alpha, "Snap").count(Counter::Cast), 1);
        assert_eq!(named(alpha, "Snap").count(Counter::Warp), 0);
    }

    #[test]
    fn tracks_alternative_cost_casting_methods() {
        let cases = [
            (
                "@PAlpha casts @[Test Card@:2000,100:@] without paying its mana cost.",
                Counter::FreeCast,
            ),
            (
                "@PAlpha casts @[Test Card@:2000,100:@] with Bargain.",
                Counter::Bargained,
            ),
            (
                "@PAlpha casts @[Test Card@:2000,100:@] by paying {1R} with dash.",
                Counter::Dashed,
            ),
            (
                "@PAlpha casts @[Test Card@:2000,100:@] for its dash cost.",
                Counter::Dashed,
            ),
            (
                "@PAlpha casts @[Test Card@:2000,100:@] with Bestow targeting @[Other@:2002,101:@].",
                Counter::Bestowed,
            ),
            (
                "@PAlpha casts @[Test Card@:2000,100:@] with Replicate {1U} (Replicated 1 time) targeting Bravo.",
                Counter::Replicated,
            ),
            (
                "@PAlpha casts @[Test Card@:2000,100:@] with Spectacle for its spectacle cost.",
                Counter::Spectacle,
            ),
            (
                "@PAlpha casts @[Test Card@:2000,100:@] without paying its mana cost with Rebound targeting Bravo.",
                Counter::Rebound,
            ),
            (
                "@PAlpha casts @[Test Card@:2000,100:@] from the graveyard for its escape cost.",
                Counter::Escaped,
            ),
            (
                "@PAlpha casts @[Test Card@:2000,100:@] by paying {W} and returning an unblocked attacking creature you control to its owner's hand with sneak.",
                Counter::Ninjutsu,
            ),
            (
                "@PAlpha casts @[Test Card@:2000,100:@] without paying its mana cost with suspend.",
                Counter::Suspended,
            ),
            (
                "@PAlpha casts @[Test Card@:2000,100:@] with buyback.",
                Counter::Buyback,
            ),
            (
                "@PAlpha casts @[Test Card@:2000,100:@] by paying {1W} with disturb from the graveyard.",
                Counter::Disturb,
            ),
            (
                "@PAlpha casts @[Test Card@:2000,100:@] by paying its foretell cost.",
                Counter::Foretold,
            ),
            (
                "@PAlpha casts @[Test Card@:2000,100:@] with Retrace from the graveyard targeting Bravo.",
                Counter::Retraced,
            ),
            (
                "@PAlpha casts @[Test Card@:2000,100:@] by paying {B} with Mayhem {B} from the graveyard.",
                Counter::Mayhem,
            ),
            (
                "@PAlpha casts @[Test Card@:2000,100:@] with Miracle {W} targeting @[Other@:2002,101:@].",
                Counter::Miracle,
            ),
            (
                "@PAlpha casts @[Test Card@:2000,100:@] with Gift with a gift promised to Bravo.",
                Counter::Gifted,
            ),
            (
                "@PAlpha casts @[Test Card@:2000,100:@] with Casualty X.",
                Counter::Casualty,
            ),
        ];
        for (message, counter) in cases {
            let result = extract_log_cards(&joined(&[message]));
            let card = named(result.match_cards("Alpha"), "Test Card");
            assert_eq!(card.count(Counter::Cast), 1, "{}", counter.field_name());
            assert_eq!(card.count(counter), 1, "{}", counter.field_name());
        }
    }

    #[test]
    fn tracks_ninjutsu_activations_as_ninjutsu_without_counting_a_cast() {
        let result = extract_log_cards(&joined(&[
            "@PAlpha activates Ninjutsu ability of @[Ninja of the Deep Hours@:87308,100:@].",
        ]));
        let card = named(result.match_cards("Alpha"), "Ninja of the Deep Hours");
        assert_eq!(card.count(Counter::Ninjutsu), 1);
        assert_eq!(card.count(Counter::Cast), 0);
        assert_eq!(card.count(Counter::Activated), 0);
    }

    #[test]
    fn attributes_cards_to_players_with_spaces_in_usernames() {
        let result = extract_log_cards(&entries(&[
            "@P@PQab Cd joined the game.",
            "@P@PBravo joined the game.",
            "@PQab Cd casts @[Behold the Multiverse@:174834,100:@] by paying its foretell cost.",
        ]));
        let card = named(result.match_cards("Qab Cd"), "Behold the Multiverse");
        assert_eq!(card.count(Counter::Cast), 1);
        assert_eq!(card.count(Counter::Foretold), 1);
    }

    // v1 tests beyond the 0.x file.

    #[test]
    fn every_owned_phrase_credits_the_player_and_skipped_phrases_do_not() {
        let owned = [
            "@PAlpha puts a triggered ability from @[Card@:2000,1:@] onto the stack.",
            "@PAlpha reveals @[Card@:2000,1:@] from their opening hand.",
            "@PAlpha reveals @[Card@:2000,1:@].",
            "@PAlpha discards @[Card@:2000,1:@].",
            "@PAlpha puts @[Card@:2000,1:@] into their graveyard.",
            "@PAlpha's @[Card@:2000,1:@] creates a token.",
            "@PAlpha names Bolt for @[Card@:2000,1:@].",
        ];
        for message in owned {
            let result = extract_log_cards(&joined(&[message]));
            assert_eq!(ids(result.match_cards("Alpha")), vec![1000], "{message}");
            assert!(result.match_cards("Bravo").is_empty(), "{message}");
        }
        let skipped = [
            "@PAlpha removes two loyalty counters from @[Card@:2000,1:@].",
            "@PAlpha exiles @[Card@:2000,1:@] with @[Other@:2002,2:@]'s ability.",
            "@PAlpha returns @[Card@:2000,1:@] to its owner's hand with @[Other@:2002,2:@].",
            "@PAlpha draws a card with @[Card@:2000,1:@].",
            "@PAlpha is being attacked by @[Card@:2000,1:@]",
            "@PAlpha chooses to use @[Card@:2000,1:@]'s ability.",
            "@PAlpha declines to use @[Card@:2000,1:@]'s ability.",
            "@PAlpha chooses red for @[Card@:2000,1:@].",
            "@PAlpha puts @[Card@:2000,1:@] onto the battlefield.",
        ];
        for message in skipped {
            let result = extract_log_cards(&joined(&[message]));
            assert!(result.match_cards("Alpha").is_empty(), "{message}");
        }
    }

    #[test]
    fn a_reveal_with_a_source_credits_only_the_revealed_list() {
        let result = extract_log_cards(&joined(&[
            "@PAlpha reveals 2 cards with @[Thought-Knot Seer@:5000,9:@]'s ability: @[Bolt@:2000,1:@] and @[Fire // Ice@:2003,2:@].",
        ]));
        assert_eq!(ids(result.match_cards("Alpha")), vec![1000, 1001]);
        assert_eq!(
            named(result.match_cards("Alpha"), "Fire // Ice").mtgo_id,
            1001
        );
    }

    #[test]
    fn cast_turn_is_the_first_cast_after_a_turn_marker() {
        let result = extract_log_cards(&joined(&[
            "@PAlpha casts @[Early@:2000,1:@].",
            "@PTurn 2: Alpha",
            "@PAlpha casts @[Bolt@:2002,2:@].",
            "@PAlpha discards @[Late@:2004,3:@].",
            "@PTurn 3: Bravo",
            "@PAlpha casts @[Bolt@:2002,4:@].",
            "@PAlpha casts @[Early@:2000,5:@].",
            "@PAlpha casts @[Late@:2004,6:@].",
        ]));
        let alpha = result.match_cards("Alpha");
        assert_eq!(named(alpha, "Early").cast_turn, Some(3));
        assert_eq!(named(alpha, "Bolt").cast_turn, Some(2));
        assert_eq!(named(alpha, "Bolt").count(Counter::Cast), 2);
        assert_eq!(named(alpha, "Late").cast_turn, Some(3));
        assert_eq!(ids(alpha), vec![1000, 1001, 1002]);
    }

    #[test]
    fn match_totals_sum_counters_and_keep_the_first_games_cast_turn() {
        let result = extract_log_cards(&entries(&[
            "@P@PAlpha joined the game.",
            "@P@PBravo joined the game.",
            "@PTurn 2: Alpha",
            "@PAlpha casts @[Bolt@:2002,2:@].",
            "@PAlpha wins the game.",
            "@P@PAlpha joined the game.",
            "@P@PBravo joined the game.",
            "@PTurn 1: Alpha",
            "@PAlpha casts @[Bolt@:2002,2:@].",
            "@PAlpha casts @[Bolt@:2002,3:@].",
            "@PBravo wins the game.",
        ]));
        assert_eq!(result.games.len(), 2);
        assert_eq!(result.game_cards(1, "Alpha")[0].cast_turn, Some(1));
        let bolt = &result.match_cards("Alpha")[0];
        assert_eq!(bolt.count(Counter::Cast), 3);
        assert_eq!(bolt.cast_turn, Some(2));
        assert!(result.game_cards(5, "Alpha").is_empty());
        assert!(result.game_cards(0, "Nobody").is_empty());
    }

    #[test]
    fn players_come_from_join_and_roll_lines_in_first_seen_order() {
        let log = entries(&[
            "@PBravo rolled a 3.",
            "@P@PAlpha joined the game.",
            "@P@PBravo joined the game.",
            "@PQa.bcd-x_1 rolled a 2.",
            "@PCharlie wins the game.",
        ]);
        assert_eq!(detect_players(&log), vec!["Bravo", "Alpha", "Qa.bcd-x_1"]);
    }

    #[test]
    fn overlong_names_and_players_past_the_cap_are_ignored() {
        let long = "Q".repeat(MAX_PLAYER_NAME_BYTES + 1);
        let at_cap = "R".repeat(MAX_PLAYER_NAME_BYTES);
        let mut messages = vec![
            format!("@P{long} rolled a 3."),
            format!("@P@P{at_cap} joined the game."),
        ];
        for n in 0..20 {
            messages.push(format!("@PQp{n} rolled a 1."));
        }
        let messages: Vec<&str> = messages.iter().map(String::as_str).collect();
        let log = entries(&messages);
        let players = detect_players(&log);
        assert_eq!(players.len(), MAX_PLAYERS);
        assert_eq!(players[0], at_cap);
        assert_eq!(players[1], "Qp0");
        assert_eq!(extract_log_cards(&log).players, players);
    }

    #[test]
    fn games_split_on_a_roll_or_join_after_a_win_or_concede() {
        let log = entries(&[
            "@PAlpha rolled a 3.",
            "@P@PAlpha joined the game.",
            "@PAlpha has lost connection to the game.",
            "@P@PAlpha joined the game.",
            "@PAlpha wins the game.",
            "@PAlpha leads the match 1-0",
            "@P@PAlpha joined the game.",
            "@PBravo has conceded from the game.",
            "@PBravo rolled a 2.",
        ]);
        let games = split_into_games(&log);
        let sizes: Vec<usize> = games.iter().map(|g| g.len()).collect();
        assert_eq!(sizes, vec![6, 2, 1]);
        assert!(split_into_games(&[]).is_empty());
    }

    #[test]
    fn the_counter_list_matches_0x() {
        let names: Vec<&str> = Counter::ALL.iter().map(|c| c.field_name()).collect();
        assert_eq!(
            names,
            vec![
                "cast",
                "played",
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
                "activated",
            ]
        );
        for (index, counter) in Counter::ALL.iter().enumerate() {
            assert_eq!(counter.index(), index);
        }
    }

    #[test]
    fn a_raw_id_too_large_for_a_catalog_id_is_skipped() {
        let result = extract_log_cards(&joined(&[
            "@PAlpha casts @[Huge@:99999999999999999999999,1:@].",
            "@PAlpha casts @[Bolt@:2002,2:@].",
        ]));
        assert_eq!(ids(result.match_cards("Alpha")), vec![1001]);
    }
}
