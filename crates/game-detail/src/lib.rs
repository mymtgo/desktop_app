//! Per-game match detail ported from the 0.x app: game state frames, the
//! opening hand and mulligans (`ParseOpeningHand`), the local deck and the
//! opponent's revealed cards (`CreateGames::buildLocalDeck`,
//! `CreateGames::buildOpponentDeck`, `BuildMatchGameData::mergeLogOnlyCards`),
//! the turn estimate (`BuildMatchGameData::estimateTurns`) and sideboard
//! changes (`BuildMatchGameData::computeSideboardChanges`).
//!
//! The game log: MTGO's `.dat` file (`ParseGameLogBinary`), the MetaMessage
//! fallback (`DecodeMetaMessageText`, `ExtractMetaMessageEntries`), one
//! game's display lines (`GetGameLogEntries`), log-only cards and game facts
//! (`ExtractCardsFromGameLog`), and each game's result (`ExtractGameResults`).
//!
//! The tracker stores each game's card rows (`game_cards`, from
//! `game_card_rows`); the match page reads them, and still parses frames for
//! the board and turn estimate. Pure: no filesystem, clock, network or
//! database.
//! Callers pass in the event texts, which player is local, and card lookups
//! (oracle ids, which catalog ids are lands, names).

pub mod deck;
pub mod entries;
pub mod frame;
pub mod game_cards;
pub mod game_results;
pub mod gamelog;
pub mod imported_rule;
pub mod log_cards;
pub mod meta_text;
pub mod opening_hand;
pub mod reveals;
pub mod sideboard;
pub mod turns;
pub mod zones;

pub use deck::{local_deck, merge_cards_seen, opponent_cards};
pub use decks::DeckCard;
pub use entries::{clean_message, game_log_entries};
pub use frame::{
    Frame, FrameCard, FramePlayer, frame_content_value, frame_text, frame_time, frame_value,
    parse_frame, parse_frame_content, parse_frame_contents, parse_frames,
};
pub use game_cards::{
    GAME_CARDS_VERSION, GameCardCast, GameCardRow, GameCards, GameCardsInput, Side, game_card_rows,
    played_deck, seats,
};
pub use game_results::{LogEndReason, LogGameResult, LogGameResults, extract_game_results};
pub use gamelog::{GameLog, GameLogEntry, parse_game_log};
pub use imported_rule::apply_imported_rule;
pub use log_cards::{
    Counter, GameLogCard, GameLogCards, GameMeta, LogGame, PlayerCards, PlayerPregame, PlayerValue,
    PregameAction, PregameKind, detect_players, extract_log_cards, split_into_games,
};
pub use meta_text::{MetaEvent, decode_meta_text, meta_message_entries};
pub use opening_hand::{
    HandCard, OpeningHand, ShownCard, StoredHand, opening_hand, opening_hand_from_stored,
};
pub use reveals::{LogCard, RevealedCard, merge_log_only_cards};
pub use sideboard::{ChangeKind, SideboardChange, sideboard_changes};
pub use turns::estimate_turns;
pub use zones::{ZONES, ZoneCounts, count_zones};
