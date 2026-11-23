//! Match commands (dashboard spec section 4, Plan K): the match lists'
//! filters and sorts (0.x `BuildMatchListProps`), the match detail page
//! (0.x `BuildMatchShowProps` and `BuildMatchGameData`), a game's log (0.x
//! `GetGameLogEntries`), and the match edits (notes, "Remove from stats"),
//! which go through the tracker (the only writer).
//!
//! Each game's hand, deck, sideboarding and reveals are its stored rows
//! (`game_cards`, `game_mulligan_cards`, the hand sizes; target schema
//! section 6): the tracker's game sync writes a log match's, the imports a
//! 0.x or history match's, the player a manual match's. The board for the
//! turn estimate still comes from the frames (the match's events, or a 0.x
//! import's `game_timelines`), and the game log from `game_logs`, else
//! MTGO's `Match_GameLog` file, else the MetaMessages.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;

use crate::platform::BestOfView;
use chrono::{DateTime, NaiveDate};
use chrono_tz::Tz;
use decks::DeckCard;
use decks::stats::{Range, end_of_day, local_midnight};
use game_detail::{
    ChangeKind, Frame, GameCardRow, GameCards, GameLogEntry, MetaEvent, Side, estimate_turns,
    extract_log_cards, game_log_entries, meta_message_entries, parse_frame_contents, parse_frames,
    sideboard_changes,
};
use mtgo_log::Env;
use mtgo_protocol::{EventKind, PreparedEvent};
use serde::{Deserialize, Serialize};
use store::BestOf;
use store::Connection;
use store::archetype_edit::ArchetypeEdit;
use store::decks::{CardRow, card_infos};
use store::game_sync::{game_log as stored_game_log, match_game_cards, timeline};
use store::manual::{ManualGameDetail, manual_game_rows};
use store::manual_rules::game_maindeck;
use store::match_detail::{
    GameRow, MatchHeader, match_games, match_header, opponent_color_identities,
};
use store::match_edits::{MatchEdit, MatchEditOutcome};
use store::match_list::{
    ArchetypeFilter, KindFilter, MatchListFilter, MatchListPage, ResultFilter, Scope, SortDir,
    SortKey, list_archetypes, listed_match_ids, opponent_archetype,
};
use tauri::{AppHandle, Manager, State};
use tracker::history::History;

use crate::bridge::lock;
use crate::commands::{
    AppState, CommandError, Startup, off_async_workers, read_settings, ready, storage,
};
use crate::decks::color_list;
use crate::logging::Timer;
use crate::matches::log_cache::{GameLogCache, Step};
pub use crate::summary::{ArchetypeCountView, CommanderCardView, OpponentArchetypeView};
use crate::summary::{
    MatchPage, SummaryOutcome, SummaryState, archetype_colors, format_label, from_row,
    match_type_label, outcome_from_str, state_from_str,
};

// ---- Lists -------------------------------------------------------------------

/// The list's result filter (0.x `filter_result`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum MatchResultFilter {
    Win,
    Loss,
}

/// The list's type filter (0.x `filter_type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum MatchKindFilter {
    /// A match with a league token, or in an imported or manual league run.
    League,
    Casual,
    /// Entered by hand (`source = 'manual'`).
    Manual,
    /// An Arena ranked match (the UI says "Ranked").
    Ranked,
    /// An Arena unranked match (the UI says "Casual").
    Unranked,
}

/// The list's sort column (0.x `sort`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum MatchSortKey {
    #[default]
    #[serde(rename = "started_at")]
    StartedAt,
    #[serde(rename = "outcome")]
    Outcome,
    #[serde(rename = "duration")]
    Duration,
    /// The opponent archetype's name.
    #[serde(rename = "archetype")]
    Archetype,
    #[serde(rename = "game_1")]
    Game1,
    #[serde(rename = "game_2")]
    Game2,
    #[serde(rename = "game_3")]
    Game3,
}

/// The list's sort direction (0.x `sort_dir`; anything but `asc` is `desc`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum MatchSortDir {
    Asc,
    #[default]
    Desc,
}

/// What a match list shows: the main Matches page and a deck's Matches page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchListQuery {
    /// First local day (`YYYY-MM-DD`, OS timezone); `null` or blank for none.
    pub from: Option<String>,
    /// Last local day, inclusive.
    pub to: Option<String>,
    pub result: Option<MatchResultFilter>,
    pub kind: Option<MatchKindFilter>,
    /// Arena Bo1 or Bo3 only; `null` for every match. Ignored for MTGO.
    #[serde(default)]
    pub best_of: Option<BestOfView>,
    /// The archetype filter (0.x `filter_archetype`): `none` for matches
    /// with no opponent archetype, else an archetype id; `null` or anything
    /// else for every match.
    pub archetype: Option<String>,
    pub sort: MatchSortKey,
    pub sort_dir: MatchSortDir,
    /// 1-based.
    pub page: u32,
}

impl MatchListQuery {
    /// No filter, newest first, page `page`.
    pub fn newest_first(page: u32) -> Self {
        Self {
            from: None,
            to: None,
            result: None,
            kind: None,
            best_of: None,
            archetype: None,
            sort: MatchSortKey::StartedAt,
            sort_dir: MatchSortDir::Desc,
            page,
        }
    }

    /// No filter and the default sort: live matches can join the first page.
    pub fn is_unfiltered(&self) -> bool {
        blank(self.from.as_deref())
            && blank(self.to.as_deref())
            && self.result.is_none()
            && self.kind.is_none()
            && self.best_of.is_none()
            && archetype_filter(self.archetype.as_deref()).is_none()
            && self.sort == MatchSortKey::StartedAt
            && self.sort_dir == MatchSortDir::Desc
    }
}

/// The store's archetype filter for the UI's text: `none`, or an id.
fn archetype_filter(text: Option<&str>) -> Option<ArchetypeFilter> {
    let text = text?.trim();
    if text == "none" {
        return Some(ArchetypeFilter::Unknown);
    }
    text.parse::<i64>().ok().map(ArchetypeFilter::Is)
}

fn blank(text: Option<&str>) -> bool {
    text.is_none_or(|text| text.trim().is_empty())
}

/// A local day from the UI, `None` when blank.
fn local_day(text: Option<&str>, which: &str) -> Result<Option<NaiveDate>, CommandError> {
    let Some(text) = text.map(str::trim).filter(|text| !text.is_empty()) else {
        return Ok(None);
    };
    match NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        Ok(day) => Ok(Some(day)),
        Err(_) => Err(CommandError::Refused(format!(
            "The {which} date must be a date like 2026-09-07."
        ))),
    }
}

/// The store filter for `query`, inside `base` (a deck page's timeframe, or
/// all time): 0.x `filter_from` is the start of that local day and
/// `filter_to` the end of its day, in the OS timezone.
pub fn match_filter(
    query: &MatchListQuery,
    base: Range,
    tz: Tz,
) -> Result<MatchListFilter, CommandError> {
    let mut range = base;
    if let Some(day) = local_day(query.from.as_deref(), "from")? {
        range.from_ms = range.from_ms.max(local_midnight(day, tz));
    }
    if let Some(day) = local_day(query.to.as_deref(), "to")? {
        range.to_ms = range.to_ms.min(end_of_day(day, tz));
    }
    let result = match query.result {
        Some(MatchResultFilter::Win) => Some(ResultFilter::Win),
        Some(MatchResultFilter::Loss) => Some(ResultFilter::Loss),
        None => None,
    };
    let kind = match query.kind {
        Some(MatchKindFilter::League) => Some(KindFilter::League),
        Some(MatchKindFilter::Casual) => Some(KindFilter::Casual),
        Some(MatchKindFilter::Manual) => Some(KindFilter::Manual),
        Some(MatchKindFilter::Ranked) => Some(KindFilter::Ranked),
        Some(MatchKindFilter::Unranked) => Some(KindFilter::Unranked),
        None => None,
    };
    let sort = match query.sort {
        MatchSortKey::StartedAt => SortKey::StartedAt,
        MatchSortKey::Outcome => SortKey::Outcome,
        MatchSortKey::Duration => SortKey::Duration,
        MatchSortKey::Archetype => SortKey::Archetype,
        MatchSortKey::Game1 => SortKey::Game1,
        MatchSortKey::Game2 => SortKey::Game2,
        MatchSortKey::Game3 => SortKey::Game3,
    };
    let dir = match query.sort_dir {
        MatchSortDir::Asc => SortDir::Asc,
        MatchSortDir::Desc => SortDir::Desc,
    };
    Ok(MatchListFilter {
        range,
        result,
        kind,
        best_of: query.best_of.map(BestOf::from),
        archetype: archetype_filter(query.archetype.as_deref()),
        sort,
        dir,
    })
}

/// A store page as the UI's page; a row the list cannot show is skipped.
pub fn page_view(found: MatchListPage) -> MatchPage {
    let mut rows = Vec::new();
    for row in &found.rows {
        match from_row(row) {
            Some(summary) => rows.push(summary),
            None => {
                tracing::warn!(token = %row.token, state = %row.state, "skipped a match row the list cannot show")
            }
        }
    }
    MatchPage {
        rows,
        total: found.total,
        page: found.page,
        per_page: found.per_page,
        unknown_archetype_count: 0,
        archetypes: Vec::new(),
    }
}

// ---- Archetypes ----------------------------------------------------------------

/// One archetype a picker offers (0.x `ArchetypeData`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ArchetypePickerOption {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    pub name: String,
    /// The archetypes format key (`modern`); `null` for a fallback.
    pub format: Option<String>,
    /// Colour identity letters in WUBRG order.
    pub colors: Vec<String>,
    /// Homebrew or Rogue (shown as "System").
    pub is_fallback: bool,
    /// The player's own archetype (shown as "Custom"), so it stands apart
    /// from a MyMTGO archetype of the same name. Not one the offline
    /// download holds as Custom: that one is MyMTGO's.
    pub is_custom: bool,
}

/// The archetypes a picker offers for a match or deck format (an MTGO
/// code such as `CMODERN`, or its label): the format's archetypes that are
/// not retired, by name, then the fallbacks. No format (a bulk set over
/// several formats) offers every archetype that is not retired, as 0.x's
/// picker did; a limited format only the fallbacks.
pub fn archetype_option_views(
    conn: &Connection,
    format: Option<&str>,
) -> Result<Vec<ArchetypePickerOption>, CommandError> {
    let options = match format {
        None => store::match_list::every_archetype_option(conn).map_err(storage)?,
        Some(format) => {
            let key = archetypes::format_key(format);
            store::archetypes::archetype_options(conn, key.as_deref()).map_err(storage)?
        }
    };
    let mut views = Vec::new();
    for option in options {
        views.push(ArchetypePickerOption {
            id: option.id,
            name: option.name,
            format: option.format,
            colors: archetype_colors(option.color_identity.as_deref()),
            is_fallback: option.source == archetypes::Source::Fallback,
            is_custom: option.source == archetypes::Source::Manual && !option.offline_held,
        });
    }
    Ok(views)
}

/// Fill a list page's archetype filter: `scope`'s complete matches in
/// `base` (the page's timeframe, before the list's own filters) with no
/// opponent archetype, and the options with their counts. `format` is the
/// list's format code (a deck's), `None` for the main list.
pub fn with_list_archetypes(
    conn: &Connection,
    mut page: MatchPage,
    scope: Scope,
    base: Range,
    format: Option<&str>,
) -> Result<MatchPage, CommandError> {
    let key = format.and_then(archetypes::format_key);
    let found = list_archetypes(conn, scope, base, key.as_deref()).map_err(storage)?;
    page.unknown_archetype_count = found.unknown;
    page.archetypes = found
        .options
        .into_iter()
        .map(|option| ArchetypeCountView {
            id: option.id,
            name: option.name,
            colors: archetype_colors(option.color_identity.as_deref()),
            is_fallback: option.is_fallback,
            match_count: option.matches,
        })
        .collect();
    Ok(page)
}

pub(crate) fn archetype_editor(state: &AppState) -> Result<tracker::ArchetypeEditor, CommandError> {
    state.tracker_part(tracker::TrackerHandle::archetype_editor)
}

/// The opponent archetype set by hand (0.x `UpdateArchetypeController`,
/// `BulkUpdateArchetypeController`, archetypes spec section 8): each
/// match's opponent seat becomes `archetype_id`, hand-set at confidence 1;
/// `None` deletes the seat and keeps detection from refilling it. Matches
/// with no opponent, and tokens with no listed match, are skipped. Through
/// the tracker (`ArchetypeEdit::SetOpponent`).
pub fn set_opponent(
    state: &AppState,
    tokens: &[String],
    archetype_id: Option<u32>,
    now_ms: i64,
) -> Result<(), CommandError> {
    if tokens.is_empty() {
        return Err(CommandError::Refused(
            "The match ids field is required.".to_string(),
        ));
    }
    let match_ids = listed_match_ids(&lock(&state.reader), tokens).map_err(storage)?;
    let outcome = archetype_editor(state)?
        .edit(ArchetypeEdit::SetOpponent {
            match_ids,
            archetype_id: archetype_id.map(i64::from),
            now_ms,
        })
        .map_err(CommandError::Tracker)?;
    match outcome.refusal() {
        Some(message) => Err(CommandError::Refused(message.to_string())),
        None => Ok(()),
    }
}

// ---- Detail ------------------------------------------------------------------

/// The deck a match was played with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchDeckView {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    pub name: String,
    /// Colour identity letters, e.g. `["U", "R"]`.
    pub colors: Vec<String>,
    #[specta(type = specta_typescript::Number)]
    pub version_id: i64,
}

/// A card of a hand (0.x `keptHand`, `mulliganedHands`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HandCardView {
    pub catalog_id: u32,
    /// `Card <id>` for a card with no card data.
    pub name: String,
    pub image: Option<String>,
    /// The file in v1's images folder (`card` scheme), when downloaded.
    pub local_image: Option<String>,
    /// Put on the bottom after a mulligan (always false in mulliganed hands).
    pub bottomed: bool,
    /// The turn 1 draw on the draw, shown after the opening hand (always
    /// false in mulliganed hands).
    pub first_draw: bool,
}

/// Which way a sideboard change moved cards (0.x `type`): shown on the match
/// page, and sent by the manual sideboard edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum SideboardKind {
    In,
    Out,
}

/// A sideboard change against the registered list (0.x `sideboardChanges`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SideboardChangeView {
    pub catalog_id: u32,
    pub name: String,
    pub image: Option<String>,
    /// The file in v1's images folder (`card` scheme), when downloaded.
    pub local_image: Option<String>,
    pub quantity: u32,
    pub kind: SideboardKind,
}

/// A card the opponent showed (0.x `opponentCardsSeen`). The rail groups
/// them by type and merges games in the UI, as 0.x did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RevealView {
    pub catalog_id: u32,
    /// The card's name; the log's name or `Card <id>` without card data.
    pub name: String,
    pub image: Option<String>,
    /// The file in v1's images folder (`card` scheme), when downloaded.
    pub local_image: Option<String>,
    /// The card's type line.
    pub type_line: Option<String>,
    /// Colour identity letters.
    pub colors: Vec<String>,
    pub quantity: u32,
}

/// One game of the detail page (0.x `BuildMatchGameData`, no clocks).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct GameDetailView {
    /// The `games.id` (the replay window opens it).
    pub id: u32,
    /// 1-based, as the tabs show it.
    pub number: u32,
    pub result: SummaryOutcome,
    /// `null` when unknown.
    pub on_the_play: Option<bool>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub started_at_ms: Option<i64>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub ended_at_ms: Option<i64>,
    /// The game log's last turn, else the lands on the battlefield at the
    /// end (0.x `estimateTurns`); `null` when neither is known.
    pub turns: Option<u32>,
    pub local_mulligans: u32,
    pub opponent_mulligans: u32,
    /// The opening hand (the cards drawn for the hand that was kept, before
    /// bottoming) in a fixed order: mana value, then name, then catalog id,
    /// each card's bottomed copies marked, then the turn 1 draw on the draw
    /// (`first_draw`) last.
    pub opening_hand: Vec<HandCardView>,
    /// Each hand sent back, in order (empty placeholders when unrecorded).
    pub mulliganed_hands: Vec<Vec<HandCardView>>,
    /// Game 2 and later only.
    pub sideboard_changes: Vec<SideboardChangeView>,
    pub opponent_cards_seen: Vec<RevealView>,
    /// The game log has lines for this game (the "Game log" button).
    pub has_game_log: bool,
    /// The game's own deck list is known, so its sideboard changes are real
    /// even when there are none.
    pub has_deck_list: bool,
    /// The game has a frame to replay (the "Replay" button; never for a
    /// manual or history match).
    pub has_frames: bool,
}

/// The match detail page (0.x `BuildMatchShowProps`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchDetail {
    /// The match's row id ("Create archetype from this match" sends it).
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    pub token: String,
    /// The platform of the match's account.
    pub platform: crate::platform::PlatformView,
    /// Where the match came from: `log`, `imported`, `manual` or `history`.
    pub source: String,
    pub state: SummaryState,
    pub outcome: SummaryOutcome,
    /// Games won and lost.
    pub wins: u32,
    pub losses: u32,
    pub opponent: Option<String>,
    /// Display label, e.g. "Modern".
    pub format: Option<String>,
    pub match_type: Option<String>,
    #[specta(type = specta_typescript::Number)]
    pub started_at_ms: i64,
    #[specta(type = Option<specta_typescript::Number>)]
    pub ended_at_ms: Option<i64>,
    /// The league run the match is in.
    pub league_name: Option<String>,
    /// The deck the match was played with; the page sits in its deck layout.
    pub deck: Option<MatchDeckView>,
    /// The opponent seat's archetype; `null` shows "Unknown archetype", or
    /// "Not detected yet" when `archetype_pending`.
    pub opponent_archetype: Option<OpponentArchetypeView>,
    /// No opponent archetype, and detection has not decided the match yet
    /// (it runs in the background and retries when MyMTGO can be reached).
    pub archetype_pending: bool,
    /// An Endstep ranked match's rating change; `null` for any other match.
    #[specta(optional)]
    pub rating: Option<crate::summary::RatingView>,
    pub notes: Option<String>,
    /// Imported (0.x or MTGO's game history). Only a 0.x import can have
    /// game detail; the page says it is missing when no game has any.
    pub imported: bool,
    /// Entered by hand: no game log or replay, and the per-game hand,
    /// sideboard and reveals are the player's records.
    pub manual: bool,
    /// The player fixed the match ("Edit match"): the page shows "Set by
    /// you" beside the result.
    pub corrected: bool,
    pub games: Vec<GameDetailView>,
    /// What the manual edit dialogs pick from (0.x `manualEditing`); `null`
    /// for a match that was not entered by hand.
    pub manual_editing: Option<ManualEditingView>,
    /// A commander format match's opponent commanders (partners both, in
    /// seat order), shown above the reveals; empty for every other format.
    pub opponent_commanders: Vec<CommanderCardView>,
}

/// A card of the registered deck the manual edit dialogs offer (0.x
/// `DeckCardOption`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeckCardOptionView {
    pub catalog_id: u32,
    /// `Card <id>` for a card with no card data.
    pub name: String,
    pub image: Option<String>,
    /// The file in v1's images folder (`card` scheme), when downloaded.
    pub local_image: Option<String>,
    pub quantity: u32,
}

/// One game's maindeck as the hand dialog offers it: the registered maindeck
/// with the game's recorded sideboard changes applied (0.x `effectiveMains`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ManualGameEditingView {
    pub number: u32,
    pub mains: Vec<DeckCardOptionView>,
}

/// The options of the manual edit dialogs (0.x `BuildManualEditingData`):
/// the registered deck split into maindeck and sideboard, and each game's
/// maindeck, and the opponent archetype's list (the quick picks); the
/// reveals dialog also copies from the other games' `opponentCardsSeen`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ManualEditingView {
    pub mains: Vec<DeckCardOptionView>,
    pub sideboard: Vec<DeckCardOptionView>,
    pub games: Vec<ManualGameEditingView>,
    /// The opponent archetype's list (0.x `archetypeDecklist`): list 1, or a
    /// manual archetype's first variant; `null` without one.
    pub archetype_decklist: Option<Vec<ArchetypeCardView>>,
}

/// A card of the opponent archetype's list (0.x `GetArchetypeDecklistForMatch`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ArchetypeCardView {
    pub catalog_id: u32,
    /// `Card <id>` for a card with no card data.
    pub name: String,
    pub image: Option<String>,
    /// The file in v1's images folder (`card` scheme), when downloaded.
    pub local_image: Option<String>,
    pub quantity: u32,
    pub sideboard: bool,
}

/// One line of a game's log (0.x `GetGameLogEntries`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct GameLogLine {
    /// Local time `HH:MM:SS` (OS timezone).
    pub time: String,
    pub message: String,
}

/// Where MTGO's game log files are searched (deck sync's roots).
#[derive(Debug, Clone)]
pub struct GameLogFiles {
    pub env: Env,
    pub data_folder: Option<PathBuf>,
}

/// What the page reads from a log match's stored events: each game's state
/// frame texts (the board, for the turn estimate) and the MetaMessage game
/// log. Hands, decks, sideboarding and reveals come from the game rows.
#[derive(Debug, Default)]
struct MatchEvents {
    /// Each game's state frame texts, in log order, by Game ID.
    frames: HashMap<u64, Vec<String>>,
    /// The `game_management_json` events (the MetaMessage game log).
    meta: Vec<MetaEvent>,
}

/// An MTGO log match's events (live or archived, in `(file, offset)` order).
/// Other matches (Arena and Endstep ones too) have none.
fn match_events(conn: &Connection, header: &MatchHeader) -> Result<MatchEvents, CommandError> {
    if header.source != "log" || header.platform != store::Platform::Mtgo {
        return Ok(MatchEvents::default());
    }
    let events = History::default()
        .match_events(conn, &header.token)
        .map_err(storage)?;
    let mut found = MatchEvents::default();
    for event in events {
        match event.kind {
            EventKind::GameStateUpdate => {
                if let Some(game_id) = event.subjects.game_id {
                    found.frames.entry(game_id).or_default().push(event.text);
                }
            }
            EventKind::GameManagementJson => found.meta.push(MetaEvent {
                at_ms: event.at_ms,
                text: event.text,
            }),
            _ => {}
        }
    }
    Ok(found)
}

/// Each game's first `deck_used` list, by Game ID: MTGO often logs the line
/// twice, and 0.x read only the first (events in `(file, offset)` order).
pub fn first_deck_lists(events: &[PreparedEvent]) -> HashMap<u64, Vec<DeckCard>> {
    let mut lists: HashMap<u64, Vec<DeckCard>> = HashMap::new();
    for event in events {
        if event.kind != EventKind::DeckUsed {
            continue;
        }
        if let Some((game_id, cards)) = decks::parse_deck_used(&event.text) {
            lists.entry(game_id).or_insert(cards);
        }
    }
    lists
}

/// The last frame of a game's texts that reads.
fn last_frame(texts: &[String], bare_json: bool) -> Option<Frame> {
    texts.iter().rev().find_map(|text| {
        let parsed = if bare_json {
            parse_frame_contents(&[text.as_str()])
        } else {
            parse_frames(&[text.as_str()])
        };
        parsed.into_iter().next()
    })
}

/// A game's last frame: from the events of a log match, or the frames a
/// 0.x import kept (`game_timelines`). A stored timeline that cannot be read
/// counts as none.
fn game_last_frame(conn: &Connection, row: &GameRow, events: &MatchEvents) -> Option<Frame> {
    if let Some(texts) = row.mtgo_game_id.and_then(|id| events.frames.get(&id)) {
        return last_frame(texts, false);
    }
    match timeline(conn, row.id) {
        Ok(Some(frames)) => {
            let texts: Vec<String> = frames.into_iter().map(|frame| frame.text).collect();
            last_frame(&texts, true)
        }
        Ok(None) => None,
        Err(error) => {
            tracing::warn!(%error, "an imported game's frames could not be read");
            None
        }
    }
}

/// The match's game log for the page: its stored `game_logs` row (the
/// tracker's decoded `.dat` file, or 0.x's), else the `.dat` file's entries
/// (`dat`), else the MetaMessages (`meta`, log matches). A stored log that
/// cannot be read counts as none.
struct PageLog {
    dat: Vec<GameLogEntry>,
    meta: Vec<GameLogEntry>,
}

fn page_log(
    conn: &Connection,
    header: &MatchHeader,
    dat: &[GameLogEntry],
    events: &MatchEvents,
) -> PageLog {
    let stored = match stored_game_log(conn, header.id) {
        Ok(stored) => stored,
        Err(error) => {
            tracing::warn!(%error, "a stored game log could not be read");
            None
        }
    };
    let dat = stored.unwrap_or_else(|| dat.to_vec());
    let meta = if dat.is_empty() {
        meta_message_entries(&events.meta)
    } else {
        Vec::new()
    };
    PageLog { dat, meta }
}

/// Card lookups for the views: names (`Card <id>` when unknown), images.
struct Cards {
    rows: HashMap<u32, CardRow>,
}

impl Cards {
    fn read(conn: &Connection, ids: &mut Vec<u32>) -> Result<Cards, CommandError> {
        ids.sort_unstable();
        ids.dedup();
        Ok(Cards {
            rows: card_infos(conn, ids).map_err(storage)?,
        })
    }

    fn name(&self, id: u32) -> Option<String> {
        self.rows
            .get(&id)
            .and_then(|card| card.name.clone())
            .filter(|name| !name.is_empty())
    }

    fn name_or_id(&self, id: u32) -> String {
        self.name(id).unwrap_or_else(|| format!("Card {id}"))
    }

    fn image(&self, id: u32) -> Option<String> {
        self.rows
            .get(&id)
            .and_then(|card| card.image.clone())
            .filter(|image| !image.is_empty())
    }

    fn local_image(&self, id: u32) -> Option<String> {
        self.rows.get(&id).and_then(|card| card.local_image.clone())
    }

    fn oracle(&self, id: u32) -> Option<String> {
        self.rows
            .get(&id)
            .and_then(|card| card.oracle_id.clone())
            .filter(|oracle| !oracle.is_empty())
    }

    fn hand_card(&self, id: u32, bottomed: bool, first_draw: bool) -> HandCardView {
        HandCardView {
            catalog_id: id,
            name: self.name_or_id(id),
            image: self.image(id),
            local_image: self.local_image(id),
            bottomed,
            first_draw,
        }
    }

    fn deck_option(&self, id: u32, quantity: u32) -> DeckCardOptionView {
        DeckCardOptionView {
            catalog_id: id,
            name: self.name_or_id(id),
            image: self.image(id),
            local_image: self.local_image(id),
            quantity,
        }
    }

    fn lands(&self) -> HashSet<u32> {
        let mut lands = HashSet::new();
        for card in self.rows.values() {
            if card
                .card_type
                .as_deref()
                .is_some_and(|line| line.contains("Land"))
            {
                lands.insert(card.catalog_id);
            }
        }
        lands
    }

    /// The hand order (spec section 17): mana value (unknown last), then
    /// name, then catalog id.
    fn sort_for_hand(&self, ids: &mut [u32]) {
        ids.sort_by(|a, b| {
            let value = |id: &u32| {
                self.rows
                    .get(id)
                    .and_then(|card| card.cmc)
                    .unwrap_or(f64::MAX)
            };
            value(a)
                .total_cmp(&value(b))
                .then_with(|| self.name_or_id(*a).cmp(&self.name_or_id(*b)))
                .then_with(|| a.cmp(b))
        });
    }
}

fn result_of(text: &str) -> SummaryOutcome {
    outcome_from_str(text).unwrap_or(SummaryOutcome::Unknown)
}

/// The local side's rows of a game.
fn local_rows(cards: &GameCards) -> impl Iterator<Item = &GameCardRow> {
    cards.cards.iter().filter(|row| row.side == Side::Local)
}

/// The opening hand as the page shows it: the cards in hand order, each
/// card's bottomed copies after its kept ones and marked, then the turn 1
/// draw last.
fn opening_hand_view(cards: &GameCards, lookup: &Cards) -> Vec<HandCardView> {
    let mut ids: Vec<u32> = local_rows(cards)
        .filter(|row| row.opening_hand > 0)
        .map(|row| row.catalog_id)
        .collect();
    lookup.sort_for_hand(&mut ids);
    let mut shown = Vec::new();
    for id in &ids {
        let Some(row) = cards.row(Side::Local, *id) else {
            continue;
        };
        for _ in 0..row.starting_hand {
            shown.push(lookup.hand_card(*id, false, false));
        }
        for _ in 0..row.bottomed {
            shown.push(lookup.hand_card(*id, true, false));
        }
    }
    let mut drawn: Vec<u32> = local_rows(cards)
        .filter(|row| row.first_draw > 0)
        .map(|row| row.catalog_id)
        .collect();
    lookup.sort_for_hand(&mut drawn);
    for id in drawn {
        let copies = cards.row(Side::Local, id).map_or(0, |row| row.first_draw);
        for _ in 0..copies {
            shown.push(lookup.hand_card(id, false, true));
        }
    }
    shown
}

/// Each hand sent back, in order, in hand order within; `count` hands, the
/// ones with no rows (not remembered) empty.
fn mulliganed_views(cards: &GameCards, lookup: &Cards, count: u32) -> Vec<Vec<HandCardView>> {
    let mut hands = Vec::new();
    for hand in 1..=count {
        let mut ids: Vec<u32> = cards
            .mulligans
            .iter()
            .filter(|(number, _, _)| *number == hand)
            .map(|(_, catalog_id, _)| *catalog_id)
            .collect();
        lookup.sort_for_hand(&mut ids);
        let mut shown = Vec::new();
        for id in ids {
            let quantity = cards
                .mulligans
                .iter()
                .find(|(number, catalog_id, _)| *number == hand && *catalog_id == id)
                .map_or(0, |(_, _, quantity)| *quantity);
            for _ in 0..quantity {
                shown.push(lookup.hand_card(id, false, false));
            }
        }
        hands.push(shown);
    }
    hands
}

/// The local deck as played that game (main and side rows, catalog order).
fn game_deck(cards: &GameCards) -> Vec<DeckCard> {
    let mut deck = Vec::new();
    for row in local_rows(cards) {
        if row.main_qty > 0 {
            deck.push(DeckCard {
                catalog_id: row.catalog_id,
                quantity: row.main_qty,
                sideboard: false,
            });
        }
        if row.side_qty > 0 {
            deck.push(DeckCard {
                catalog_id: row.catalog_id,
                quantity: row.side_qty,
                sideboard: true,
            });
        }
    }
    deck
}

/// The opponent's cards seen `(catalog id, copies)`, catalog order.
fn opponent_seen(cards: &GameCards) -> Vec<(u32, u32)> {
    cards
        .cards
        .iter()
        .filter(|row| row.side == Side::Opponent && row.seen > 0)
        .map(|row| (row.catalog_id, row.seen))
        .collect()
}

/// Every catalog id a game's view names.
fn card_ids(cards: &GameCards, ids: &mut Vec<u32>) {
    ids.extend(cards.cards.iter().map(|row| row.catalog_id));
    ids.extend(cards.mulligans.iter().map(|(_, catalog_id, _)| *catalog_id));
}

/// One game's facts before card names are looked up.
struct RawGame {
    row: GameRow,
    cards: GameCards,
    last_frame: Option<Frame>,
    log_turns: Option<u32>,
    has_game_log: bool,
    has_frames: bool,
}

/// One game of the page from its stored rows (spec section 6). A manual
/// match's mulligan counts are the recorded ones, with an empty hand for
/// each one not remembered. Otherwise a game with a hand from its frames
/// counts the hands sent back and the opponent's bottomed cards (0.x read
/// both from the frames); without one, the stored counts. An Endstep game
/// (`endstep`) shows the stored count when it is larger than the hands sent
/// back: its stored count already takes every source of the fold (trap 63),
/// and Game Stats reads it (#707).
fn game_view(
    raw: RawGame,
    lookup: &Cards,
    log_names: &HashMap<u32, String>,
    lands: &HashSet<u32>,
    registered: &[DeckCard],
    manual: bool,
    endstep: bool,
) -> GameDetailView {
    let row = &raw.row;
    let cards = &raw.cards;
    let recorded_hands = cards
        .mulligans
        .iter()
        .map(|(hand, _, _)| *hand)
        .max()
        .unwrap_or(0);
    let has_hand = local_rows(cards).any(|row| row.opening_hand > 0);
    let (local_mulligans, opponent_mulligans) = if !manual && has_hand {
        let opponent = match (cards.opp_opening_hand_size, cards.opp_starting_hand_size) {
            (Some(opening), Some(starting)) => opening.saturating_sub(starting),
            _ => row.opp_mulligans.unwrap_or(0),
        };
        let local = if endstep {
            recorded_hands.max(row.local_mulligans.unwrap_or(0))
        } else {
            recorded_hands
        };
        (local, opponent)
    } else {
        (
            row.local_mulligans.unwrap_or(0),
            row.opp_mulligans.unwrap_or(0),
        )
    };
    let hand_count = if manual && cards.starting_hand_size.is_some() {
        recorded_hands.max(local_mulligans)
    } else {
        recorded_hands
    };

    let deck = game_deck(cards);
    let sideboard = sideboard_views(row.game_number, &deck, registered, lookup);
    let mut reveals = Vec::new();
    for (catalog_id, quantity) in opponent_seen(cards) {
        // A card with no card data shows the game log's name (0.x
        // `mergeLogOnlyCards`).
        let name = lookup
            .name(catalog_id)
            .or_else(|| log_names.get(&catalog_id).cloned())
            .unwrap_or_else(|| format!("Card {catalog_id}"));
        reveals.push(reveal_view(catalog_id, name, quantity, lookup));
    }
    let turns = raw.log_turns.or(row.turn_count).or_else(|| {
        raw.last_frame
            .as_ref()
            .and_then(|last| estimate_turns(last, lands))
    });

    GameDetailView {
        id: u32::try_from(row.id).unwrap_or(0),
        number: row.game_number,
        result: result_of(&row.result),
        on_the_play: row.on_play,
        started_at_ms: row.started_at_ms,
        ended_at_ms: row.ended_at_ms,
        turns,
        local_mulligans,
        opponent_mulligans,
        opening_hand: opening_hand_view(cards, lookup),
        mulliganed_hands: mulliganed_views(cards, lookup, hand_count),
        sideboard_changes: sideboard,
        opponent_cards_seen: reveals,
        has_game_log: raw.has_game_log,
        has_deck_list: !deck.is_empty(),
        has_frames: raw.has_frames,
    }
}

// ---- Manual matches ----------------------------------------------------------

/// The sideboard changes of a game deck against the registered list, as
/// the page shows them (game 2 and later only).
fn sideboard_views(
    game_number: u32,
    game_deck: &[DeckCard],
    registered: &[DeckCard],
    cards: &Cards,
) -> Vec<SideboardChangeView> {
    let mut views = Vec::new();
    if game_number < 2 || game_deck.is_empty() || registered.is_empty() {
        return views;
    }
    let oracle_of = |id: u32| cards.oracle(id);
    for change in sideboard_changes(game_deck, registered, &oracle_of) {
        views.push(SideboardChangeView {
            catalog_id: change.catalog_id,
            name: cards.name_or_id(change.catalog_id),
            image: cards.image(change.catalog_id),
            local_image: cards.local_image(change.catalog_id),
            quantity: change.quantity,
            kind: match change.kind {
                ChangeKind::In => SideboardKind::In,
                ChangeKind::Out => SideboardKind::Out,
            },
        });
    }
    views
}

/// A card the opponent showed, `quantity` copies.
fn reveal_view(catalog_id: u32, name: String, quantity: u32, cards: &Cards) -> RevealView {
    let row = cards.rows.get(&catalog_id);
    RevealView {
        catalog_id,
        image: cards.image(catalog_id),
        local_image: cards.local_image(catalog_id),
        type_line: row.and_then(|row| row.card_type.clone()),
        colors: color_list(row.and_then(|row| row.color_identity.as_deref())),
        name,
        quantity,
    }
}

/// The registered maindeck's copies by catalog id.
fn registered_mains(registered: &[DeckCard]) -> BTreeMap<u32, u32> {
    let mut mains = BTreeMap::new();
    for card in registered {
        if !card.sideboard {
            *mains.entry(card.catalog_id).or_insert(0) += card.quantity;
        }
    }
    mains
}

/// The cards of a game's maindeck as dialog options: the registered
/// maindeck's order with the game's copies, then the cards brought in (0.x
/// `effectiveMains`). A card with no copies left is left out.
fn game_mains(
    registered: &[DeckCard],
    maindeck: &BTreeMap<u32, u32>,
    cards: &Cards,
) -> Vec<DeckCardOptionView> {
    let mut options: Vec<DeckCardOptionView> = Vec::new();
    let add = |catalog_id: u32, options: &mut Vec<DeckCardOptionView>| {
        let quantity = maindeck.get(&catalog_id).copied().unwrap_or(0);
        if quantity > 0 && !options.iter().any(|option| option.catalog_id == catalog_id) {
            options.push(cards.deck_option(catalog_id, quantity));
        }
    };
    for card in registered.iter().filter(|card| !card.sideboard) {
        add(card.catalog_id, &mut options);
    }
    for catalog_id in maindeck.keys() {
        add(*catalog_id, &mut options);
    }
    options
}

/// The registered deck's rows of one side as dialog options, in list order
/// (0.x `BuildManualEditingData`).
fn deck_options(
    registered: &[DeckCard],
    sideboard: bool,
    cards: &Cards,
) -> Vec<DeckCardOptionView> {
    let mut options = Vec::new();
    for card in registered.iter().filter(|card| card.sideboard == sideboard) {
        options.push(cards.deck_option(card.catalog_id, card.quantity));
    }
    options
}

/// The edit options of a manual match (0.x `BuildManualEditingData`): the
/// registered deck and each game's maindeck (its recorded postboard
/// maindeck, else the registered one).
fn manual_editing(
    conn: &Connection,
    header: &MatchHeader,
    games: &[GameRow],
    registered: &[DeckCard],
    cards: &Cards,
) -> Result<ManualEditingView, CommandError> {
    let mut postboards: HashMap<u32, ManualGameDetail> = HashMap::new();
    for detail in manual_game_rows(conn, &header.token).map_err(storage)? {
        postboards.insert(detail.game_number, detail);
    }
    let mains = registered_mains(registered);
    let mut game_options = Vec::new();
    for row in games {
        let postboard = postboards
            .get(&row.game_number)
            .and_then(|detail| detail.postboard_main.as_deref());
        let maindeck = game_maindeck(&mains, postboard);
        game_options.push(ManualGameEditingView {
            number: row.game_number,
            mains: game_mains(registered, &maindeck, cards),
        });
    }
    let archetype_decklist =
        match store::manual::archetype_decklist(conn, &header.token).map_err(storage)? {
            Some(list) => {
                let mut ids: Vec<u32> = list.iter().map(|card| card.catalog_id).collect();
                let lookup = Cards::read(conn, &mut ids)?;
                let mut views = Vec::new();
                for card in list {
                    views.push(ArchetypeCardView {
                        catalog_id: card.catalog_id,
                        name: card
                            .name
                            .clone()
                            .filter(|name| !name.is_empty())
                            .unwrap_or_else(|| format!("Card {}", card.catalog_id)),
                        image: lookup.image(card.catalog_id),
                        local_image: lookup.local_image(card.catalog_id),
                        quantity: card.quantity,
                        sideboard: card.sideboard,
                    });
                }
                Some(views)
            }
            None => None,
        };
    Ok(ManualEditingView {
        mains: deck_options(registered, false, cards),
        sideboard: deck_options(registered, true, cards),
        games: game_options,
        archetype_decklist,
    })
}

/// The match detail page; `None` when there is no such match or it is
/// hidden. Each game's hand, deck, sideboarding and reveals are its stored
/// rows; the board (turn estimate) comes from its frames. `dat` is the
/// match's `.dat` game log (`dat_entries`), used when no game log is
/// stored, empty when there is none.
pub fn detail(
    conn: &Connection,
    token: &str,
    dat: &[GameLogEntry],
) -> Result<Option<MatchDetail>, CommandError> {
    let Some(header) = match_header(conn, token).map_err(storage)? else {
        return Ok(None);
    };
    let Some(state) = state_from_str(&header.state) else {
        // `pending` is never shown.
        return Ok(None);
    };
    let manual = header.source == "manual";
    let games = match_games(conn, header.id).map_err(storage)?;
    let registered = match &header.deck {
        Some(deck) => decks::decode_signature(&deck.signature),
        None => Vec::new(),
    };
    let mut stored = match_game_cards(conn, header.id).map_err(storage)?;
    let events = match_events(conn, &header)?;
    // An Arena match has no `.dat` log, MetaMessages or frames: those reads
    // are skipped. Its game log is its stored game messages.
    let arena = header.platform == store::Platform::Arena;
    let arena_log_games = if arena {
        store::arena_games::game_numbers_with_messages(conn, token).map_err(storage)?
    } else {
        Default::default()
    };
    let log = if arena {
        PageLog {
            dat: Vec::new(),
            meta: Vec::new(),
        }
    } else {
        page_log(conn, &header, dat, &events)
    };
    // An Endstep game's log is read from the match's stored frames when the
    // dialog opens (`game_log_lines`): finding which games have lines would
    // walk the whole match here, so every game with a start offers it (a
    // placeholder game has none; a game without lines shows the dialog's
    // empty state).
    let endstep_log = header.platform == store::Platform::Endstep && !manual;
    // 0.x took turn counts from the `.dat` file only.
    let log_cards = (!log.dat.is_empty()).then(|| extract_log_cards(&log.dat));
    // The Replay button: one read for every game of the match.
    let replayable = if arena {
        // An Arena game has a replay when it has stored messages.
        games
            .iter()
            .filter(|row| arena_log_games.contains(&row.game_number))
            .map(|row| row.id)
            .collect()
    } else if endstep_log {
        // An Endstep game's replay is built from the match's frames when it
        // opens: every game that was seen offers it (a placeholder game has
        // no start), without walking the match here.
        games
            .iter()
            .filter(|row| row.started_at_ms.is_some())
            .map(|row| row.id)
            .collect()
    } else {
        let mut with_frames = store::replay::games_with_frames(conn, header.id).map_err(storage)?;
        // A recorded game opens in the .mtgr viewer.
        with_frames
            .extend(store::recorded_replays::recorded_game_ids(conn, header.id).map_err(storage)?);
        with_frames
    };

    let mut raw_games = Vec::new();
    for (index, row) in games.iter().enumerate() {
        let log_turns = log_cards
            .as_ref()
            .and_then(|cards| cards.games.get(index))
            .and_then(|game| game.meta.turn_count);
        // An Endstep game's log is read from its frames: a placeholder game
        // (never seen, no start) has none, as it has no replay.
        let has_game_log = if endstep_log {
            row.started_at_ms.is_some()
        } else {
            arena_log_games.contains(&row.game_number)
                || !game_log_entries(&log.dat, &log.meta, row.started_at_ms, row.ended_at_ms)
                    .is_empty()
        };
        raw_games.push(RawGame {
            cards: stored.remove(&row.id).unwrap_or_default(),
            last_frame: game_last_frame(conn, row, &events),
            row: row.clone(),
            log_turns,
            has_game_log,
            has_frames: replayable.contains(&row.id),
        });
    }

    let mut ids: Vec<u32> = registered.iter().map(|card| card.catalog_id).collect();
    for raw in &raw_games {
        card_ids(&raw.cards, &mut ids);
        if let Some(last) = &raw.last_frame {
            ids.extend(
                last.cards
                    .iter()
                    .filter(|card| card.zone == "Battlefield")
                    .map(|card| card.catalog_id),
            );
        }
    }
    let lookup = Cards::read(conn, &mut ids)?;
    let lands = lookup.lands();
    let mut log_names: HashMap<u32, String> = HashMap::new();
    if let (Some(cards), Some(opponent)) = (&log_cards, &header.opponent) {
        for card in cards.match_cards(opponent) {
            log_names
                .entry(card.mtgo_id)
                .or_insert_with(|| card.name.clone());
        }
    }
    let mut game_views = Vec::new();
    for raw in raw_games {
        game_views.push(game_view(
            raw,
            &lookup,
            &log_names,
            &lands,
            &registered,
            manual,
            header.platform == store::Platform::Endstep,
        ));
    }
    let editing = if manual {
        Some(manual_editing(conn, &header, &games, &registered, &lookup)?)
    } else {
        None
    };
    let opponent = opponent_archetype(conn, header.id)
        .map_err(storage)?
        .as_ref()
        .map(OpponentArchetypeView::from_seat);
    let rating = store::endstep_sync::endstep_rating(conn, header.id)
        .map_err(storage)?
        .and_then(|ranked| crate::summary::RatingView::from_ranked(&ranked));
    let mut page = detail_view(&header, state, game_views, editing, opponent, rating)?;
    page.opponent_commanders = opponent_commanders(conn, &header)?;
    Ok(Some(page))
}

/// The opponent's stored commanders of a commander format match, empty for
/// any other format (`store::match_commanders`).
fn opponent_commanders(
    conn: &Connection,
    header: &MatchHeader,
) -> Result<Vec<CommanderCardView>, CommandError> {
    if !store::match_commanders::is_commander_format(header.format.as_deref()) {
        return Ok(Vec::new());
    }
    let stored = store::match_commanders::commanders_of(conn, header.id).map_err(storage)?;
    let mut ids = Vec::new();
    for id in stored.opponent {
        if let Ok(id) = u32::try_from(id) {
            ids.push(id);
        }
    }
    let rows = card_infos(conn, &ids).map_err(storage)?;
    let mut views = Vec::new();
    for id in ids {
        let row = rows.get(&id);
        let text = |value: Option<&String>| value.filter(|text| !text.is_empty()).cloned();
        views.push(CommanderCardView {
            catalog_id: id,
            name: row
                .and_then(|card| text(card.name.as_ref()))
                .unwrap_or_else(|| format!("Card {id}")),
            image: row.and_then(|card| text(card.image.as_ref())),
            local_image: row.and_then(|card| card.local_image.clone()),
            colors: archetype_colors(row.and_then(|card| card.color_identity.as_deref())),
        });
    }
    Ok(views)
}

/// The page around its games: the hero, the deck, the league and the note.
fn detail_view(
    header: &MatchHeader,
    state: SummaryState,
    games: Vec<GameDetailView>,
    manual_editing: Option<ManualEditingView>,
    opponent_archetype: Option<OpponentArchetypeView>,
    rating: Option<crate::summary::RatingView>,
) -> Result<MatchDetail, CommandError> {
    let mut wins = 0;
    let mut losses = 0;
    for game in &games {
        match game.result {
            SummaryOutcome::Win => wins += 1,
            SummaryOutcome::Loss => losses += 1,
            _ => {}
        }
    }
    let deck = header.deck.as_ref().map(|deck| MatchDeckView {
        id: deck.deck_id,
        name: deck.name.clone(),
        colors: color_list(deck.color_identity.as_deref()),
        version_id: deck.version_id,
    });
    Ok(MatchDetail {
        id: header.id,
        token: header.token.clone(),
        platform: header.platform.into(),
        source: header.source.clone(),
        state,
        outcome: result_of(&header.outcome),
        wins,
        losses,
        opponent: header.opponent.clone(),
        format: format_label(header.format.as_deref()),
        match_type: match_type_label(header.match_type.as_deref()),
        started_at_ms: header.started_at_ms,
        ended_at_ms: header.ended_at_ms,
        league_name: header.league_name.clone(),
        deck,
        archetype_pending: opponent_archetype.is_none() && header.detection_pending,
        opponent_archetype,
        rating,
        notes: header.notes.clone(),
        // A 0.x import or a match from MTGO's match history.
        imported: matches!(header.source.as_str(), "imported" | "history"),
        manual: header.source == "manual",
        corrected: header.corrected,
        games,
        manual_editing,
        opponent_commanders: Vec::new(),
    })
}

/// `HH:MM:SS` of an instant in `tz` (0.x `toLocal()->format('H:i:s')`).
fn local_time(ms: i64, tz: Tz) -> String {
    match DateTime::from_timestamp_millis(ms) {
        Some(at) => at.with_timezone(&tz).format("%H:%M:%S").to_string(),
        None => String::new(),
    }
}

/// One game's log lines (0.x `GetGameLogEntries`): the stored game log
/// (`game_logs`), else the `.dat` file's entries, else the MetaMessages,
/// within the game's window, cleaned. An Arena game's lines are folded from
/// its stored game messages, an Endstep game's read from its stored frames.
/// Empty when the match, the game or its log is unknown.
pub fn game_log_lines(
    conn: &Connection,
    token: &str,
    game_number: u32,
    dat: &[GameLogEntry],
    tz: Tz,
) -> Result<Vec<GameLogLine>, CommandError> {
    let Some(header) = match_header(conn, token).map_err(storage)? else {
        return Ok(Vec::new());
    };
    let games = match_games(conn, header.id).map_err(storage)?;
    let Some(game) = games.iter().find(|game| game.game_number == game_number) else {
        return Ok(Vec::new());
    };
    if header.platform == store::Platform::Arena {
        return arena_log_lines(conn, token, game_number, tz);
    }
    if header.platform == store::Platform::Endstep {
        return endstep_log_lines(conn, token, game_number, tz);
    }
    let mut log = page_log(conn, &header, dat, &MatchEvents::default());
    if log.dat.is_empty() {
        let events = match_events(conn, &header)?;
        log.meta = meta_message_entries(&events.meta);
    }
    let mut lines = Vec::new();
    for entry in game_log_entries(&log.dat, &log.meta, game.started_at_ms, game.ended_at_ms) {
        lines.push(GameLogLine {
            time: local_time(entry.timestamp_ms, tz),
            message: entry.message,
        });
    }
    Ok(lines)
}

/// One Arena game's log lines, folded from its stored game messages. A card
/// with no card data shows as `Card <id>`, as on the rest of the page.
fn arena_log_lines(
    conn: &Connection,
    token: &str,
    game_number: u32,
    tz: Tz,
) -> Result<Vec<GameLogLine>, CommandError> {
    let lines =
        store::arena_game_sync::game_log_lines(conn, token, game_number).map_err(storage)?;
    let mut ids: Vec<u32> = lines.iter().map(|line| line.catalog_id).collect();
    let lookup = Cards::read(conn, &mut ids)?;
    let mut views = Vec::new();
    for line in lines {
        views.push(GameLogLine {
            time: local_time(line.at_ms, tz),
            message: arena_log_message(&line, &lookup.name_or_id(line.catalog_id)),
        });
    }
    Ok(views)
}

/// One Endstep game's log lines, read from the match's stored frames.
fn endstep_log_lines(
    conn: &Connection,
    token: &str,
    game_number: u32,
    tz: Tz,
) -> Result<Vec<GameLogLine>, CommandError> {
    let lines = store::endstep_sync::game_log_lines(conn, token, game_number).map_err(storage)?;
    let mut views = Vec::new();
    for line in lines {
        views.push(GameLogLine {
            time: local_time(line.at_ms, tz),
            message: line.message,
        });
    }
    Ok(views)
}

/// `You drew X`, `You played X`, `You cast X`, `Opponent played X` or
/// `Opponent cast X` (the opponent's draws are never lines).
fn arena_log_message(line: &arena_game::LogLine, name: &str) -> String {
    let who = if line.local { "You" } else { "Opponent" };
    let verb = match line.verb {
        arena_game::LogVerb::Drew => "drew",
        arena_game::LogVerb::Played => "played",
        arena_game::LogVerb::Cast => "cast",
    };
    format!("{who} {verb} {name}")
}

/// The entries of the match's `.dat` game log, empty when there is no file
/// or it cannot be read. Searches the deck files' roots (the chosen MTGO
/// data folder, else the detected ones); ticks are local wall-clock in `tz`.
/// `cache` remembers the file and its entries, so a refetch neither walks
/// the data folder nor re-parses an unchanged file.
pub fn dat_entries(
    cache: &Mutex<GameLogCache>,
    files: &GameLogFiles,
    token: &str,
    tz: Tz,
    now_ms: i64,
) -> Vec<GameLogEntry> {
    let data_folder = files.data_folder.as_deref();
    // The cache is not held while the data folder is walked, so another
    // match's request does not wait behind a cold search.
    let step = lock(cache).begin(data_folder, token, now_ms);
    let located = match step {
        Step::Known(path, stamp) => Some((path, stamp)),
        Step::FreshMiss => None,
        Step::Search => {
            let found = mtgo_log::find_game_log(&files.env, data_folder, token);
            lock(cache).record(data_folder, token, now_ms, found)
        }
    };
    let Some((path, stamp)) = located else {
        return Vec::new();
    };
    lock(cache).entries_of(&path, stamp, tz)
}

// ---- Limited boards ----------------------------------------------------------

/// What the Limited pages read from one game of a match (0.x's
/// `game_player.deck_json` rows and card game stats).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameBoard {
    /// `win`, `loss`, `draw` or `unknown`.
    pub result: String,
    /// The local deck as played, main and side rows; empty when MTGO gave no
    /// board data for the game.
    pub local_deck: Vec<DeckCard>,
    /// The catalog ids of the opponent's cards seen.
    pub opponent_seen: Vec<u32>,
    /// The catalog ids the local player cast, from the game log (empty when
    /// `with_cast` was not asked or there is no log).
    pub cast: Vec<u32>,
}

/// Each stored game of the match, in game order, as the Limited pages read
/// it from the game rows: the local deck as played, the opponent's cards
/// seen and, with `with_cast`, the cards the local player cast. Empty for an
/// unknown or hidden match.
pub fn game_boards(
    conn: &Connection,
    token: &str,
    with_cast: bool,
) -> Result<Vec<GameBoard>, CommandError> {
    let Some(header) = match_header(conn, token).map_err(storage)? else {
        return Ok(Vec::new());
    };
    let games = match_games(conn, header.id).map_err(storage)?;
    let mut stored = match_game_cards(conn, header.id).map_err(storage)?;
    let mut boards = Vec::new();
    for row in games {
        let cards = stored.remove(&row.id).unwrap_or_default();
        let cast = if with_cast {
            local_rows(&cards)
                .filter(|row| row.cast > 0)
                .map(|row| row.catalog_id)
                .collect()
        } else {
            Vec::new()
        };
        boards.push(GameBoard {
            result: row.result,
            local_deck: game_deck(&cards),
            opponent_seen: opponent_seen(&cards)
                .into_iter()
                .map(|(catalog_id, _)| catalog_id)
                .collect(),
            cast,
        });
    }
    Ok(boards)
}

/// The five colours, in 0.x's WUBRG order.
const WUBRG: [&str; 5] = ["W", "U", "B", "R", "G"];

/// The colours each match's opponent was seen with, by match token (0.x
/// `ResolveOpponentColorIdentities`): the colour identities of every card
/// the opponent showed in any game (the game rows joined to the card table),
/// in WUBRG order, comma separated (`B,G`). A match whose opponent showed
/// only colourless cards, or cards with no card data, has no entry.
pub fn opponent_colors(
    conn: &Connection,
    tokens: &[String],
) -> Result<HashMap<String, String>, CommandError> {
    let mut colors = HashMap::new();
    for token in tokens {
        let identities = opponent_color_identities(conn, token).map_err(storage)?;
        let mut symbols: HashSet<String> = HashSet::new();
        for identity in identities {
            for symbol in identity.split(',') {
                symbols.insert(symbol.trim().to_uppercase());
            }
        }
        let ordered: Vec<&str> = WUBRG
            .into_iter()
            .filter(|symbol| symbols.contains(*symbol))
            .collect();
        if !ordered.is_empty() {
            colors.insert(token.clone(), ordered.join(","));
        }
    }
    Ok(colors)
}

// ---- Edits -------------------------------------------------------------------

fn edit_result(outcome: MatchEditOutcome) -> Result<(), CommandError> {
    match outcome.refusal() {
        Some(message) => Err(CommandError::Refused(message.to_string())),
        None => Ok(()),
    }
}

fn send_edit(state: &AppState, edit: MatchEdit) -> Result<(), CommandError> {
    let editor = state.tracker_part(tracker::TrackerHandle::match_editor)?;
    let outcome = editor.edit(edit).map_err(CommandError::Tracker)?;
    edit_result(outcome)
}

/// 0.x `UpdateNotesController`: at most 5000 characters, trimmed; blank
/// clears the note.
pub fn set_notes(state: &AppState, token: &str, notes: &str) -> Result<(), CommandError> {
    send_edit(
        state,
        MatchEdit::SetNotes {
            token: token.to_string(),
            notes: notes.to_string(),
        },
    )
}

/// "Delete match" (0.x `DeleteController`): v1 hides the match.
pub fn hide(state: &AppState, token: &str, now_ms: i64) -> Result<(), CommandError> {
    send_edit(
        state,
        MatchEdit::Hide {
            token: token.to_string(),
            now_ms,
        },
    )
}

/// Restore a deleted match (the Undo toast, Settings, Archives, Deleted).
pub fn restore(state: &AppState, token: &str, now_ms: i64) -> Result<(), CommandError> {
    send_edit(
        state,
        MatchEdit::Restore {
            token: token.to_string(),
            now_ms,
        },
    )
}

/// A deleted league in Settings, Archives (league self-management spec 5.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeletedLeagueView {
    pub run_key: String,
    pub name: String,
    #[specta(type = specta_typescript::Number)]
    pub started_at_ms: i64,
    #[specta(type = specta_typescript::Number)]
    pub removed_at_ms: i64,
    pub wins: u32,
    pub losses: u32,
    pub deck_name: Option<String>,
    /// Shown when the league has no deck.
    pub format: Option<String>,
}

/// A deleted match in Settings, Archives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeletedMatchView {
    pub token: String,
    #[specta(type = specta_typescript::Number)]
    pub started_at_ms: i64,
    #[specta(type = specta_typescript::Number)]
    pub hidden_at_ms: i64,
    pub opponent: Option<String>,
    /// The match outcome.
    pub result: SummaryOutcome,
    pub deck_name: Option<String>,
    pub league_name: Option<String>,
    /// Whether that league is deleted too.
    pub league_removed: bool,
    /// The deleted league this match was deleted together with: the match
    /// comes back only with it (issue #418), so the row offers "Restore
    /// league" instead of Restore.
    pub deleted_with_league: Option<DeletedWithLeagueView>,
}

/// The deleted league a deleted match was deleted together with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeletedWithLeagueView {
    pub run_key: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeletedItemsView {
    pub leagues: Vec<DeletedLeagueView>,
    pub matches: Vec<DeletedMatchView>,
}

/// The Deleted lists, as `store::deleted::deleted_items` orders them.
pub fn deleted(conn: &Connection) -> Result<DeletedItemsView, CommandError> {
    let items = store::deleted::deleted_items(
        conn,
        crate::platform::page_scope(conn).map_err(storage)?.platform,
    )
    .map_err(storage)?;
    let mut leagues = Vec::new();
    for league in items.leagues {
        leagues.push(DeletedLeagueView {
            run_key: league.run_key,
            name: league.name,
            started_at_ms: league.started_at,
            removed_at_ms: league.removed_at,
            wins: league.wins,
            losses: league.losses,
            deck_name: league.deck_name,
            format: league.format,
        });
    }
    let mut matches = Vec::new();
    for row in items.matches {
        matches.push(DeletedMatchView {
            token: row.token,
            started_at_ms: row.started_at,
            hidden_at_ms: row.hidden_at,
            opponent: row.opponent,
            result: result_of(&row.result),
            deck_name: row.deck_name,
            league_name: row.league_name,
            league_removed: row.league_removed,
            deleted_with_league: row.deleted_with_league.map(|league| DeletedWithLeagueView {
                run_key: league.run_key,
                name: league.name,
            }),
        });
    }
    Ok(DeletedItemsView { leagues, matches })
}

/// "Set deck" on the Matches page (issue #332): the matches were played with
/// deck version `deck_version_id`, replacing any link. Missing and hidden
/// matches are skipped by the store.
pub fn set_deck(
    state: &AppState,
    tokens: &[String],
    deck_version_id: u32,
) -> Result<(), CommandError> {
    if tokens.is_empty() {
        return Err(CommandError::Refused(
            "Choose at least one match.".to_string(),
        ));
    }
    send_edit(
        state,
        MatchEdit::SetDeckVersion {
            tokens: tokens.to_vec(),
            deck_version_id: i64::from(deck_version_id),
        },
    )
}

// ---- Commands ----------------------------------------------------------------

fn game_log_files(state: &AppState) -> Result<GameLogFiles, CommandError> {
    Ok(GameLogFiles {
        env: Env::current(),
        data_folder: read_settings(state)?.mtgo_data_folder_path(),
    })
}

/// Whether MTGO's data folder is not searched for the match's `.dat` game
/// log: a manual match has none, a match with a stored one (`game_logs`)
/// reads that, and an Arena or Endstep match never has one (its game data
/// comes from its stored game messages or captures, or from nothing yet).
fn skips_dat_search(conn: &Connection, token: &str) -> Result<bool, CommandError> {
    Ok(store::manual::is_manual(conn, token).map_err(storage)?
        || store::game_sync::has_game_log(conn, token).map_err(storage)?
        || store::platform::match_is_non_mtgo(conn, token).map_err(storage)?)
}

/// The match's `.dat` game log entries, empty without a search of MTGO's
/// data folder where `skips_dat_search` says so.
pub(crate) fn match_dat_entries(
    state: &AppState,
    token: &str,
) -> Result<Vec<GameLogEntry>, CommandError> {
    let skip = skips_dat_search(&lock(&state.reader), token)?;
    if skip {
        return Ok(Vec::new());
    }
    Ok(dat_entries(
        &state.game_logs,
        &game_log_files(state)?,
        token,
        state.timezone,
        crate::wall_clock_ms(),
    ))
}

/// The match detail page. Async: it searches MTGO's data folder for the
/// game log, which must not block the window.
#[tauri::command]
#[specta::specta]
pub async fn match_detail(
    app: AppHandle,
    token: String,
) -> Result<Option<MatchDetail>, CommandError> {
    let _timer = Timer::start("match_detail");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let dat = match_dat_entries(state, &token)?;
        detail(&lock(&state.reader), &token, &dat)
    })
    .await
}

/// One game's log (`game_number` 1-based). Async, as above.
#[tauri::command]
#[specta::specta]
pub async fn game_log(
    app: AppHandle,
    token: String,
    game_number: u32,
) -> Result<Vec<GameLogLine>, CommandError> {
    let _timer = Timer::start("game_log");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let dat = match_dat_entries(state, &token)?;
        game_log_lines(
            &lock(&state.reader),
            &token,
            game_number,
            &dat,
            state.timezone,
        )
    })
    .await
}

/// Store the match's notes. Async so the wait for the tracker's commit never
/// blocks the window. The command answers after the commit; no event is
/// pushed, so the page refetches on success.
#[tauri::command]
#[specta::specta]
pub async fn set_match_notes(
    app: AppHandle,
    token: String,
    notes: String,
) -> Result<(), CommandError> {
    let _timer = Timer::start("set_match_notes");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        set_notes(ready(&startup)?, &token, &notes)
    })
    .await
}

/// "Delete match". Async, as above. The tracker pushes `MatchRemoved`,
/// `DecksChanged` and `LeaguesChanged` once committed.
#[tauri::command]
#[specta::specta]
pub async fn hide_match(app: AppHandle, token: String) -> Result<(), CommandError> {
    let _timer = Timer::start("hide_match");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        hide(ready(&startup)?, &token, crate::wall_clock_ms())
    })
    .await
}

/// Restore a deleted match. Async, as above.
#[tauri::command]
#[specta::specta]
pub async fn restore_match(app: AppHandle, token: String) -> Result<(), CommandError> {
    let _timer = Timer::start("restore_match");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        restore(ready(&startup)?, &token, crate::wall_clock_ms())
    })
    .await
}

/// Settings, Archives: the deleted leagues and matches.
#[tauri::command]
#[specta::specta]
pub fn deleted_items(startup: State<'_, Startup>) -> Result<DeletedItemsView, CommandError> {
    let _timer = Timer::start("deleted_items");
    deleted(&lock(&ready(&startup)?.reader))
}

/// Set the deck version the matches were played with (the Matches row menu
/// and selection bar). Async, as the notes. The tracker pushes `DecksChanged`
/// and `LeaguesChanged` once committed; the archetypes worker is woken, so
/// a match whose player seat follows the deck is detected again.
#[tauri::command]
#[specta::specta]
pub async fn set_match_deck(
    app: AppHandle,
    tokens: Vec<String>,
    deck_version_id: u32,
) -> Result<(), CommandError> {
    let _timer = Timer::start("set_match_deck");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        set_deck(ready(&startup)?, &tokens, deck_version_id)?;
        crate::archetypes::worker::signal(
            &app,
            crate::archetypes::worker::ArchetypeSignal::MatchCompleted,
        );
        Ok(())
    })
    .await
}

/// The archetypes a picker offers for a format (`CMODERN` or its label;
/// `null` for every archetype).
#[tauri::command]
#[specta::specta]
pub fn archetype_options(
    startup: tauri::State<'_, Startup>,
    format: Option<String>,
) -> Result<Vec<ArchetypePickerOption>, CommandError> {
    let state = ready(&startup)?;
    archetype_option_views(&lock(&state.reader), format.as_deref())
}

/// Set (or with `null` clear) the opponent archetype of matches by token.
/// Async, as the notes: it waits for the tracker's commit; the page
/// refetches on success.
#[tauri::command]
#[specta::specta]
pub async fn set_opponent_archetype(
    app: AppHandle,
    tokens: Vec<String>,
    archetype_id: Option<u32>,
) -> Result<(), CommandError> {
    let _timer = Timer::start("set_opponent_archetype");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        set_opponent(
            ready(&startup)?,
            &tokens,
            archetype_id,
            crate::wall_clock_ms(),
        )
    })
    .await
}

/// "Detect" on matches by token (the matches table menu, archetypes spec
/// section 6.3): their archetypes are detected again on the archetypes
/// worker, answering how many were detected (0.x
/// `DetectArchetypeController`, synchronous). Tokens with no listed match
/// are skipped.
#[tauri::command]
#[specta::specta]
pub async fn detect_archetypes(app: AppHandle, tokens: Vec<String>) -> Result<u32, CommandError> {
    let _timer = Timer::start("detect_archetypes");
    off_async_workers(move || {
        let match_ids = {
            let startup = app.state::<Startup>();
            let state = ready(&startup)?;
            listed_match_ids(&lock(&state.reader), &tokens).map_err(storage)?
        };
        crate::archetypes::worker::ask(&app, |reply| {
            crate::archetypes::worker::ArchetypeSignal::Detect { match_ids, reply }
        })
        .map_err(CommandError::Refused)
    })
    .await
}

pub mod log_cache;

#[cfg(test)]
pub(crate) mod tests;
