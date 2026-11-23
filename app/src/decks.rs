//! Deck page commands (decks spec 8): the store's deck queries mapped to the
//! types the UI receives, and the deck edits, which go through the tracker
//! (the only writer). Each command is a thin shell over a plain function that
//! the tests call directly.
//!
//! Ids cross to the UI as numbers. Deck, version and catalog ids that come
//! from the UI are `u32` (row ids never get near that limit), because the
//! TypeScript export refuses `i64` arguments.

use cards::standout::{Standout, StandoutCard};
use chrono_tz::Tz;
use decks::dek::{DekCard, dek_file};
use decks::stats::{
    self, BoardingSplit, DayRow, GameRecord, PlayDraw, Range, Record, record_label,
};
use serde::{Deserialize, Serialize};
use store::Connection;
use store::deck_pages::{
    self, Dashboard, DeckHeader, DeckListFilter, DeckListPage, DeckSort, DeckSummary, Decklist,
    ListCard, TypeGroup, VersionStats,
};
use store::decks::{
    DeckEdit, EditOutcome, MAX_NAME_CHARS, card_infos, cover_options, deck_by_id, versions_of,
};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::bridge::lock;
use crate::commands::{AppState, CommandError, Startup, off_async_workers, ready, storage};
use crate::live::PER_PAGE;
use crate::logging::Timer;
use crate::matches::{MatchListQuery, match_filter, page_view};
use crate::platform::{BestOfView, PageScope, PlatformView, page_scope};
use crate::settings::DECKS_PER_PAGE_OPTIONS;
use crate::summary::MatchPage;
use store::match_list::{Scope, match_list};

pub mod archetype_pages;
pub mod cover;
pub mod screenshot;

/// The colours a manual identity can pick, in 0.x order.
const IDENTITY_LETTERS: [&str; 6] = ["W", "U", "B", "R", "G", "C"];
/// The text that confirms an archive (0.x `DeleteDeckRequest`).
pub const ARCHIVE_CONFIRMATION: &str = "DELETE";

/// A match record (0.x `MatchRecord`): draws include unknown outcomes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RecordView {
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
    pub total: u32,
    pub win_rate: u32,
    /// "W - L", or "W - L - D" when there are draws.
    pub label: String,
}

/// One deck card on the list, and the deck pages' sidebar header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeckView {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    pub name: String,
    /// The deck file's name while the deck is renamed (for "revert").
    pub original_name: Option<String>,
    /// The raw format code, e.g. `CMODERN`.
    pub format: String,
    /// The display format, e.g. "Modern".
    pub format_label: String,
    /// Colour identity letters in order, e.g. `["U", "R"]`; empty for none.
    pub colors: Vec<String>,
    /// The cover card's art crop URL.
    pub cover_art: Option<String>,
    /// Its file in v1's images folder (`card` scheme), when downloaded.
    pub local_cover_art: Option<String>,
    /// The deck's own cover image: a file name in the deck covers folder
    /// (`deckcover` scheme). It shows before the cover card.
    pub custom_cover_art: Option<String>,
    pub archived: bool,
    /// Every complete match, all time.
    pub record: RecordView,
    /// The kinds the deck has a match of, Bo3 first then Bo1; empty for an
    /// MTGO deck.
    pub kinds: Vec<BestOfView>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub last_played_at_ms: Option<i64>,
    /// The deck's archetype; `null` is "Unclassified" (archetypes spec 12).
    pub archetype: Option<archetype_pages::DeckArchetypeView>,
    /// An Arena Brawl deck (`store::match_commanders::is_commander_format`):
    /// it takes its commander's archetype, so it cannot be set by hand.
    pub commander_format: bool,
}

/// Deck list sort (0.x `sort` query values).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum DeckSortKey {
    #[default]
    LastPlayed,
    WinRate,
    MatchCount,
    Name,
}

/// What the deck list shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeckListQuery {
    /// A raw format code; `None` or blank for every format.
    pub format: Option<String>,
    /// Only the 8 decks played most recently (falls back to every deck when
    /// none has a complete match; see `DeckListView::recent`).
    pub recent: bool,
    /// Part of a deck name; `None` or blank for every deck.
    pub search: Option<String>,
    pub hide_archived: bool,
    pub sort: DeckSortKey,
    /// 1-based; past the end gives the last page.
    pub page: u32,
    /// 12, 24 or 48 (anything else is 12).
    pub per_page: u32,
    /// The archetype filter (0.x `archetype`): `none` for decks without one,
    /// an archetype id, or `null` (anything else) for every deck. It replaces
    /// `recent` when set.
    pub archetype: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct FormatOptionView {
    pub code: String,
    pub label: String,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeckListView {
    pub decks: Vec<DeckView>,
    pub total: u32,
    /// The page shown (a page past the end shows the last one).
    pub page: u32,
    pub per_page: u32,
    pub format_options: Vec<FormatOptionView>,
    /// Whether the "recently played" filter applied.
    pub recent: bool,
    /// The archetype filter that applied (`none` or an id); a stale id is
    /// dropped (0.x).
    pub archetype: Option<String>,
    /// The sidebar's archetype rows (scope: account, format, archived).
    pub archetype_options: Vec<archetype_pages::DeckArchetypeOptionView>,
    pub unclassified_count: u32,
    /// The header band when an archetype (or Unclassified) is chosen.
    pub archetype_header: Option<archetype_pages::ArchetypeHeaderView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct GameRecordView {
    pub won: u32,
    pub lost: u32,
    pub win_rate: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PlayDrawView {
    pub otp_won: u32,
    pub otp_lost: u32,
    pub otp_rate: u32,
    pub otd_won: u32,
    pub otd_lost: u32,
    pub otd_rate: u32,
    /// Games whose play or draw is known.
    pub play_draw_games: u32,
}

/// One version row in the sidebar ("All versions" first, then v1..vN).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct VersionView {
    /// `None` for "All versions".
    #[specta(type = Option<specta_typescript::Number>)]
    pub id: Option<i64>,
    pub label: String,
    pub is_current: bool,
    /// `MMM d - MMM d`, or ending in `now` for the current version.
    pub date_label: Option<String>,
    pub record: RecordView,
    pub games: GameRecordView,
    pub play_draw: PlayDrawView,
}

/// The deck pages' sidebar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeckHeaderView {
    pub deck: DeckView,
    pub versions: Vec<VersionView>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub current_version_id: Option<i64>,
    /// Complete league runs in which the deck won every one of its matches
    /// (0.x `GetDeckViewSharedProps`), all time; `None` for an Arena deck
    /// (Arena has no leagues).
    pub trophies: Option<u32>,
    /// The deck's platform (its account's; a deck without one is MTGO).
    pub platform: PlatformView,
    /// An Arena deck's matches by kind; `None` for an MTGO deck.
    pub kinds: Option<DeckKindsView>,
}

/// How many of an Arena deck's matches are Bo1 and Bo3, and the kind of the
/// most recent one (`store::deck_pages::DeckKinds`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeckKindsView {
    pub bo1: u32,
    pub bo3: u32,
    pub latest: Option<BestOfView>,
}

/// The win rate change against the previous window (0.x `winrateDelta`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct WinRateDeltaView {
    pub previous_rate: Option<u32>,
    pub previous_total: u32,
    /// `None` for all time and for an empty previous window.
    pub delta: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct BoardingSplitView {
    pub game_one_won: u32,
    pub game_one_lost: u32,
    pub game_one_rate: u32,
    pub post_board_won: u32,
    pub post_board_lost: u32,
    pub post_board_rate: u32,
    /// Post-board rate minus game 1 rate.
    pub delta: i32,
    pub games: u32,
}

/// One day of the daily chart (local day `YYYY-MM-DD`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DayView {
    pub day: String,
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
    /// `None` on a day without matches.
    pub win_rate: Option<u32>,
    /// The win rate trend after the day's last match, kept on days without
    /// one.
    pub trend: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DashboardView {
    pub record: RecordView,
    pub delta: WinRateDeltaView,
    pub games: GameRecordView,
    pub play_draw: PlayDrawView,
    pub boarding: BoardingSplitView,
    pub daily: Vec<DayView>,
    /// The 7-day rolling win rate per chart day, 0 to 100.
    pub sparkline: Vec<f64>,
    /// The six standout cards (0.x `GetStandoutCards`); not timeframe bound.
    pub standout: StandoutView,
    /// The deck's matchups in the window, for the best and worst (0.x
    /// `matchupSpread`).
    pub matchup_spread: Vec<archetype_pages::MatchupView>,
    /// The other decks of the deck's archetype, day by day (0.x
    /// `peerChart`); `null` without any.
    pub peer_chart: Option<archetype_pages::PeerChartView>,
}

/// One standout pick.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct StandoutCardView {
    pub name: String,
    pub image: Option<String>,
    pub local_image: Option<String>,
    /// "80% cast win rate", "10 of 10 games".
    pub stat: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct StandoutView {
    pub top_performer: Option<StandoutCardView>,
    pub most_cast: Option<StandoutCardView>,
    pub most_seen: Option<StandoutCardView>,
    pub most_played_land: Option<StandoutCardView>,
    pub most_sided_in: Option<StandoutCardView>,
    pub most_sided_out: Option<StandoutCardView>,
}

fn standout_card_view(card: &Option<StandoutCard>) -> Option<StandoutCardView> {
    card.as_ref().map(|card| StandoutCardView {
        name: card.name.clone(),
        image: card.image.clone(),
        local_image: card.local_image.clone(),
        stat: card.stat.clone(),
    })
}

fn standout_view(standout: &Standout) -> StandoutView {
    StandoutView {
        top_performer: standout_card_view(&standout.top_performer),
        most_cast: standout_card_view(&standout.most_cast),
        most_seen: standout_card_view(&standout.most_seen),
        most_played_land: standout_card_view(&standout.most_played_land),
        most_sided_in: standout_card_view(&standout.most_sided_in),
        most_sided_out: standout_card_view(&standout.most_sided_out),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ListCardView {
    pub catalog_id: u32,
    /// `Card <id>` for a card with no card data.
    pub name: String,
    pub type_line: Option<String>,
    pub colors: Vec<String>,
    pub image: Option<String>,
    pub art_crop: Option<String>,
    pub local_image: Option<String>,
    pub local_art_crop: Option<String>,
    pub cmc: Option<f64>,
    /// Scryfall's form (`{1}{G}`); `None` without card data or a stored cost.
    pub mana_cost: Option<String>,
    pub quantity: u32,
    pub sideboard: bool,
    /// The card has card data.
    pub known: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct TypeGroupView {
    pub type_name: String,
    pub count: u32,
    pub cards: Vec<ListCardView>,
}

/// Mana value bucket: 0 to 6, and 7 for 7 or more.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CurvePoint {
    pub cmc: u32,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ColourCount {
    /// W U B R G or C.
    pub colour: String,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DecklistView {
    /// The version shown (`None` when the deck has none).
    #[specta(type = Option<specta_typescript::Number>)]
    pub version_id: Option<i64>,
    /// A commander format deck's commanders, shown before the main deck;
    /// empty for every other format (its sideboard is `side`).
    pub commanders: Vec<ListCardView>,
    pub main: Vec<TypeGroupView>,
    pub side: Vec<ListCardView>,
    pub curve: Vec<CurvePoint>,
    pub colours: Vec<ColourCount>,
    /// Main deck cards that are not lands: the colour percentages' base.
    pub nonland_total: u32,
}

/// A printing a deck cover can use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CoverOptionView {
    pub catalog_id: u32,
    pub set_name: Option<String>,
    pub set_code: Option<String>,
    pub art_crop: String,
    pub local_art_crop: Option<String>,
}

/// The deck's current cover: the printing and its card name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CoverCardView {
    pub catalog_id: u32,
    pub name: String,
}

/// What the Settings page's cover picker starts from (0.x `SettingsController`
/// `coverArt` and `cardNames`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CoverSettingsView {
    /// The latest version's card names that have a printing with art, sorted.
    pub card_names: Vec<String>,
    /// The current cover, when it has a card row with a name.
    pub cover: Option<CoverCardView>,
    /// The deck's own cover image, a file name in the deck covers folder;
    /// `None` when the cover is the printing.
    pub custom_cover_art: Option<String>,
}

/// What the `.dek` save dialog did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DekExport {
    Saved { path: String },
    Cancelled,
}

// ---- Validation -------------------------------------------------------------

/// The store's refusal as a message for the user.
fn refused(outcome: EditOutcome) -> CommandError {
    CommandError::Refused(outcome.refusal().unwrap_or("Nothing changed.").to_string())
}

/// Trimmed text, or `None` when blank (0.x trims every input).
pub(crate) fn not_blank(text: Option<&str>) -> Option<String> {
    let text = text?.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

/// A deck name, trimmed, with 1 to 255 characters (0.x `max:255`).
pub fn checked_name(name: &str) -> Result<String, CommandError> {
    let name = name.trim();
    let length = name.chars().count();
    if length == 0 || length > MAX_NAME_CHARS {
        return Err(refused(EditOutcome::InvalidName));
    }
    Ok(name.to_string())
}

/// A manual colour identity: every letter one of W U B R G C (0.x
/// `Rule::in`), normalised by `decks::normalize_manual_identity`.
pub fn checked_identity(letters: &[String]) -> Result<Option<String>, CommandError> {
    for letter in letters {
        if !IDENTITY_LETTERS.contains(&letter.as_str()) {
            return Err(CommandError::Refused(
                "Colours must be W, U, B, R, G or C.".to_string(),
            ));
        }
    }
    Ok(decks::identity::normalize_manual_identity(letters))
}

/// An archive is confirmed by typing DELETE (0.x trims the input first).
pub fn check_confirmation(text: &str) -> Result<(), CommandError> {
    if text.trim() == ARCHIVE_CONFIRMATION {
        Ok(())
    } else {
        Err(CommandError::Refused("Type DELETE to confirm.".to_string()))
    }
}

/// A done edit, or the store's refusal as a message for the user.
pub fn edit_result(outcome: EditOutcome) -> Result<(), CommandError> {
    match outcome {
        EditOutcome::Done => Ok(()),
        refusal => Err(refused(refusal)),
    }
}

/// Archiving an archived deck changes nothing and is not an error (0.x).
pub fn archive_result(outcome: EditOutcome) -> Result<(), CommandError> {
    match outcome {
        EditOutcome::Archived => Ok(()),
        other => edit_result(other),
    }
}

/// The longest `.dek` file name stem offered, in characters.
const MAX_DEK_STEM_CHARS: usize = 100;

/// The `.dek` file name offered by the save dialog: the deck name with
/// characters no file system takes replaced by `-`, at most 100 characters
/// before `.dek`, and `_` before a Windows device name.
pub fn dek_file_name(deck_name: &str) -> String {
    let mut cleaned = String::new();
    for c in deck_name.chars() {
        if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
            cleaned.push('-');
        } else {
            cleaned.push(c);
        }
    }
    let cut: String = trim_dots_and_spaces(&cleaned)
        .chars()
        .take(MAX_DEK_STEM_CHARS)
        .collect();
    // Trimmed again: the cut can end in dots or spaces.
    let stem = trim_dots_and_spaces(&cut);
    if stem.is_empty() {
        "deck.dek".to_string()
    } else if is_windows_device_name(stem) {
        format!("_{stem}.dek")
    } else {
        format!("{stem}.dek")
    }
}

/// Windows drops trailing dots and spaces; leading ones look odd.
fn trim_dots_and_spaces(text: &str) -> &str {
    text.trim_matches(|c: char| c == '.' || c == ' ')
}

/// Windows opens a device, not a file, for these names in any case, also
/// with an extension: it reads the part before the first dot, trailing
/// spaces ignored.
fn is_windows_device_name(stem: &str) -> bool {
    let base = stem.split('.').next().unwrap_or("").trim_end_matches(' ');
    let upper = base.to_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    let digit = upper
        .strip_prefix("COM")
        .or_else(|| upper.strip_prefix("LPT"));
    match digit {
        Some(digit) => matches!(
            digit,
            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "\u{b9}" | "\u{b2}" | "\u{b3}"
        ),
        None => false,
    }
}

// ---- Mapping ----------------------------------------------------------------

/// Colour letters from a stored identity (`U,R`).
pub(crate) fn color_list(identity: Option<&str>) -> Vec<String> {
    let mut colors = Vec::new();
    for letter in identity.unwrap_or("").split(',') {
        let letter = letter.trim();
        if !letter.is_empty() {
            colors.push(letter.to_string());
        }
    }
    colors
}

pub fn record_view(record: &Record) -> RecordView {
    RecordView {
        wins: record.wins,
        losses: record.losses,
        draws: record.draws,
        total: record.total,
        win_rate: record.win_rate,
        label: record_label(record),
    }
}

pub fn deck_view(deck: &DeckSummary) -> DeckView {
    DeckView {
        id: deck.id,
        name: deck.name.clone(),
        original_name: deck.original_name.clone(),
        format: deck.format.clone(),
        format_label: deck.format_label.clone(),
        colors: color_list(deck.color_identity.as_deref()),
        cover_art: deck.cover_art.clone(),
        local_cover_art: deck.local_cover_art.clone(),
        custom_cover_art: deck.cover_file.clone(),
        archived: deck.archived,
        record: record_view(&deck.record),
        kinds: deck.kinds.iter().copied().map(BestOfView::from).collect(),
        last_played_at_ms: deck.last_played_at,
        archetype: deck
            .archetype
            .as_ref()
            .map(archetype_pages::deck_archetype_view),
        commander_format: store::match_commanders::is_commander_format(Some(&deck.format)),
    }
}

/// The store filter for a list query: blank text means no filter, an unknown
/// page size is 12.
pub(crate) fn list_filter(query: &DeckListQuery, scope: PageScope) -> DeckListFilter {
    let sort = match query.sort {
        DeckSortKey::LastPlayed => DeckSort::LastPlayed,
        DeckSortKey::WinRate => DeckSort::WinRate,
        DeckSortKey::MatchCount => DeckSort::MatchCount,
        DeckSortKey::Name => DeckSort::Name,
    };
    let per_page = if DECKS_PER_PAGE_OPTIONS.contains(&query.per_page) {
        query.per_page
    } else {
        DECKS_PER_PAGE_OPTIONS[0]
    };
    DeckListFilter {
        account_id: scope.account,
        platform: scope.platform,
        format: not_blank(query.format.as_deref()),
        recent: query.recent,
        search: not_blank(query.search.as_deref()),
        hide_archived: query.hide_archived,
        sort,
        page: query.page,
        per_page,
        archetype: archetype_pages::archetype_filter(query.archetype.as_deref()),
    }
}

pub fn list_view(page: DeckListPage) -> DeckListView {
    let mut decks = Vec::new();
    for deck in &page.decks {
        decks.push(deck_view(deck));
    }
    let mut format_options = Vec::new();
    for option in page.format_options {
        format_options.push(FormatOptionView {
            code: option.code,
            label: option.label,
            count: option.count,
        });
    }
    DeckListView {
        decks,
        total: page.total,
        page: page.page,
        per_page: page.per_page,
        format_options,
        recent: page.recent,
        archetype: archetype_pages::filter_key(page.archetype),
        archetype_options: Vec::new(),
        unclassified_count: 0,
        archetype_header: None,
    }
}

pub fn game_record_view(games: &GameRecord) -> GameRecordView {
    GameRecordView {
        won: games.won,
        lost: games.lost,
        win_rate: games.win_rate,
    }
}

fn play_draw_view(play_draw: &PlayDraw) -> PlayDrawView {
    PlayDrawView {
        otp_won: play_draw.otp_won,
        otp_lost: play_draw.otp_lost,
        otp_rate: play_draw.otp_rate,
        otd_won: play_draw.otd_won,
        otd_lost: play_draw.otd_lost,
        otd_rate: play_draw.otd_rate,
        play_draw_games: play_draw.play_draw_games,
    }
}

fn version_view(version: &VersionStats) -> VersionView {
    VersionView {
        id: version.id,
        label: version.label.clone(),
        is_current: version.is_current,
        date_label: version.date_label.clone(),
        record: record_view(&version.record),
        games: game_record_view(&version.games),
        play_draw: play_draw_view(&version.play_draw),
    }
}

pub fn header_view(header: &DeckHeader, platform: PlatformView) -> DeckHeaderView {
    let mut versions = Vec::new();
    for version in &header.versions {
        versions.push(version_view(version));
    }
    DeckHeaderView {
        deck: deck_view(&header.summary),
        versions,
        current_version_id: header.current_version_id,
        trophies: None,
        platform,
        kinds: None,
    }
}

fn boarding_view(boarding: &BoardingSplit) -> BoardingSplitView {
    BoardingSplitView {
        game_one_won: boarding.game_one_won,
        game_one_lost: boarding.game_one_lost,
        game_one_rate: boarding.game_one_rate,
        post_board_won: boarding.post_board_won,
        post_board_lost: boarding.post_board_lost,
        post_board_rate: boarding.post_board_rate,
        delta: boarding.delta,
        games: boarding.games,
    }
}

pub fn day_view(day: &DayRow) -> DayView {
    DayView {
        day: day.day.clone(),
        wins: day.wins,
        losses: day.losses,
        draws: day.draws,
        win_rate: day.win_rate,
        trend: day.trend,
    }
}

pub fn dashboard_view(dashboard: &Dashboard) -> DashboardView {
    let mut daily = Vec::new();
    for day in &dashboard.daily {
        daily.push(day_view(day));
    }
    DashboardView {
        record: record_view(&dashboard.record),
        delta: WinRateDeltaView {
            previous_rate: dashboard.delta.previous_rate,
            previous_total: dashboard.delta.previous_total,
            delta: dashboard.delta.delta,
        },
        games: game_record_view(&dashboard.games),
        play_draw: play_draw_view(&dashboard.play_draw),
        boarding: boarding_view(&dashboard.boarding),
        daily,
        sparkline: dashboard.sparkline.clone(),
        standout: standout_view(&dashboard.standout),
        matchup_spread: Vec::new(),
        peer_chart: None,
    }
}

fn card_view(card: &ListCard) -> ListCardView {
    ListCardView {
        catalog_id: card.catalog_id,
        name: card.name.clone(),
        type_line: card.type_line.clone(),
        colors: color_list(card.color_identity.as_deref()),
        image: card.image.clone(),
        art_crop: card.art_crop.clone(),
        local_image: card.local_image.clone(),
        local_art_crop: card.local_art_crop.clone(),
        cmc: card.cmc,
        mana_cost: card.mana_cost.clone(),
        quantity: card.quantity,
        sideboard: card.sideboard,
        known: card.known,
    }
}

fn group_view(group: &TypeGroup) -> TypeGroupView {
    let mut cards = Vec::new();
    for card in &group.cards {
        cards.push(card_view(card));
    }
    TypeGroupView {
        type_name: group.type_name.clone(),
        count: group.count,
        cards,
    }
}

pub fn decklist_view(list: &Decklist) -> DecklistView {
    let mut main = Vec::new();
    for group in &list.main {
        main.push(group_view(group));
    }
    let mut side = Vec::new();
    for card in &list.side {
        side.push(card_view(card));
    }
    let mut commanders = Vec::new();
    for card in &list.commanders {
        commanders.push(card_view(card));
    }
    let mut curve = Vec::new();
    for (cmc, count) in &list.curve {
        curve.push(CurvePoint {
            cmc: u32::from(*cmc),
            count: *count,
        });
    }
    let mut colours = Vec::new();
    for (colour, count) in &list.colours {
        colours.push(ColourCount {
            colour: colour.clone(),
            count: *count,
        });
    }
    DecklistView {
        version_id: list.version_id,
        commanders,
        main,
        side,
        curve,
        colours,
        nonland_total: list.nonland_total,
    }
}

// ---- Windows ----------------------------------------------------------------

/// The window the sidebar's version rows count: the page's timeframe
/// (Dashboard, Matches), or 0.x's default two months for a page without one
/// (Decklist, Settings).
pub fn header_range(timeframe: Option<&str>, now_ms: i64, tz: Tz) -> Range {
    match timeframe {
        None => stats::default_version_range(now_ms),
        Some(key) => stats::timeframe_range(key, now_ms, tz).unwrap_or_else(Range::all_time),
    }
}

/// A timeframe's window and the previous one of the same length (none for
/// all time or an unknown key).
pub fn timeframe_ranges(key: &str, now_ms: i64, tz: Tz) -> (Range, Option<Range>) {
    let current = stats::timeframe_range(key, now_ms, tz);
    let range = current.unwrap_or_else(Range::all_time);
    let previous = current.and_then(|window| stats::previous_range(key, window.from_ms, tz));
    (range, previous)
}

// ---- Reads ------------------------------------------------------------------

/// One deck list page, scoped to the current account.
pub fn list(conn: &Connection, query: &DeckListQuery) -> Result<DeckListView, CommandError> {
    let scope = page_scope(conn).map_err(storage)?;
    let filter = list_filter(query, scope);
    let page = deck_pages::deck_list(conn, &filter).map_err(storage)?;
    let applied = page.archetype;
    let mut view = list_view(page);
    let scope = store::deck_index::DeckScope {
        account_id: scope.account,
        platform: filter.platform,
        format: filter.format.clone(),
        hide_archived: filter.hide_archived,
    };
    let shell = archetype_pages::index_shell(conn, &scope, applied, Range::all_time())?;
    view.archetype_options = shell.archetype_options;
    view.unclassified_count = shell.unclassified_count;
    view.archetype_header = shell.archetype_header;
    Ok(view)
}

/// The deck's platform: its account's (MTGO for a deck without one).
fn deck_platform(conn: &Connection, deck_id: i64) -> Result<store::Platform, CommandError> {
    store::platform::deck_platform(conn, deck_id).map_err(storage)
}

/// The kind a deck page reads: the asked kind for an Arena or Endstep deck,
/// never one for an MTGO deck (its matches hold MTGO's structure text).
pub(crate) fn deck_best_of(
    conn: &Connection,
    deck_id: i64,
    best_of: Option<BestOfView>,
) -> Result<Option<store::BestOf>, CommandError> {
    let Some(best_of) = best_of else {
        return Ok(None);
    };
    Ok(platform_best_of(
        deck_platform(conn, deck_id)?,
        Some(best_of),
    ))
}

/// The asked kind on Arena or Endstep, never one on MTGO.
pub(crate) fn platform_best_of(
    platform: store::Platform,
    best_of: Option<BestOfView>,
) -> Option<store::BestOf> {
    match platform {
        store::Platform::Arena | store::Platform::Endstep => best_of.map(store::BestOf::from),
        store::Platform::Mtgo => None,
    }
}

/// The sidebar of a deck page, its version rows counting the matches of
/// `best_of` (every match when `None`); `None` when there is no such deck.
pub fn header(
    conn: &Connection,
    deck_id: i64,
    timeframe: Option<&str>,
    now_ms: i64,
    tz: Tz,
    best_of: Option<BestOfView>,
) -> Result<Option<DeckHeaderView>, CommandError> {
    let range = header_range(timeframe, now_ms, tz);
    let platform = deck_platform(conn, deck_id)?;
    let kind = platform_best_of(platform, best_of);
    let found = deck_pages::deck_header(conn, deck_id, range, tz, kind).map_err(storage)?;
    let Some(found) = found else {
        return Ok(None);
    };
    let mut view = header_view(&found, platform.into());
    match platform {
        store::Platform::Arena | store::Platform::Endstep => {
            let kinds = deck_pages::deck_kinds(conn, deck_id).map_err(storage)?;
            view.kinds = Some(DeckKindsView {
                bo1: kinds.bo1,
                bo3: kinds.bo3,
                latest: kinds.latest.map(BestOfView::from),
            });
        }
        store::Platform::Mtgo => {
            view.trophies = Some(crate::leagues::deck_trophies(conn, deck_id)?);
        }
    }
    Ok(Some(view))
}

/// The deck's Dashboard over the matches of `best_of` (every match when
/// `None`).
pub fn dashboard(
    conn: &Connection,
    deck_id: i64,
    version: Option<i64>,
    timeframe: &str,
    now_ms: i64,
    tz: Tz,
    best_of: Option<BestOfView>,
) -> Result<DashboardView, CommandError> {
    let (range, previous) = timeframe_ranges(timeframe, now_ms, tz);
    let kind = deck_best_of(conn, deck_id, best_of)?;
    let numbers = deck_pages::deck_dashboard(conn, deck_id, version, range, previous, tz, kind)
        .map_err(storage)?;
    let mut view = dashboard_view(&numbers);
    view.matchup_spread = archetype_pages::spread_views(conn, deck_id, version, range, kind)?;
    let scope = page_scope(conn).map_err(storage)?;
    view.peer_chart = archetype_pages::peer_chart_view(conn, deck_id, scope, range, tz, kind)?;
    Ok(view)
}

pub fn decklist(
    conn: &Connection,
    deck_id: i64,
    version: Option<i64>,
) -> Result<DecklistView, CommandError> {
    let list = deck_pages::deck_list_cards(conn, deck_id, version).map_err(storage)?;
    Ok(decklist_view(&list))
}

/// One page (50 rows, as 0.x) of the deck's complete matches in the
/// timeframe of `best_of` (every match when `None`), through the list's
/// filters and sort (0.x `BuildMatchListProps`).
#[allow(clippy::too_many_arguments)]
pub fn matches(
    conn: &Connection,
    deck_id: i64,
    version: Option<i64>,
    timeframe: &str,
    query: &MatchListQuery,
    now_ms: i64,
    tz: Tz,
    best_of: Option<BestOfView>,
) -> Result<MatchPage, CommandError> {
    let (range, _previous) = timeframe_ranges(timeframe, now_ms, tz);
    let filter = match_filter(query, range, tz)?;
    let scope = Scope::Deck {
        deck_id,
        version,
        best_of: deck_best_of(conn, deck_id, best_of)?,
    };
    let found = match_list(conn, scope.clone(), &filter, query.page, PER_PAGE).map_err(storage)?;
    let format = deck_by_id(conn, deck_id)
        .map_err(storage)?
        .map(|deck| deck.format);
    crate::matches::with_list_archetypes(conn, page_view(found), scope, range, format.as_deref())
}

/// The printings of a card name with art, for the cover picker.
pub fn covers(conn: &Connection, card_name: &str) -> Result<Vec<CoverOptionView>, CommandError> {
    let mut options = Vec::new();
    for option in cover_options(conn, card_name).map_err(storage)? {
        options.push(CoverOptionView {
            catalog_id: option.catalog_id,
            set_name: option.set_name,
            set_code: option.set_code,
            art_crop: option.art_crop,
            local_art_crop: option.local_art_crop,
        });
    }
    Ok(options)
}

/// The cover picker's card names and the current cover.
pub fn cover_settings(conn: &Connection, deck_id: i64) -> Result<CoverSettingsView, CommandError> {
    let Some(deck) = deck_by_id(conn, deck_id).map_err(storage)? else {
        return Err(refused(EditOutcome::NotFound));
    };

    // Oldest first, so the last is the latest.
    let versions = versions_of(conn, deck_id).map_err(storage)?;
    let mut catalog_ids = Vec::new();
    if let Some(latest) = versions.last() {
        for card in decks::decode_signature(&latest.signature) {
            catalog_ids.push(card.catalog_id);
        }
    }
    let infos = card_infos(conn, &catalog_ids).map_err(storage)?;
    let mut names: Vec<String> = Vec::new();
    for info in infos.values() {
        if let Some(name) = &info.name
            && !name.is_empty()
            && !names.contains(name)
        {
            names.push(name.clone());
        }
    }
    // 0.x offers a name when any printing of it has art.
    let mut card_names = Vec::new();
    for name in names {
        if !cover_options(conn, &name).map_err(storage)?.is_empty() {
            card_names.push(name);
        }
    }
    card_names.sort();

    let mut cover = None;
    if let Some(catalog_id) = deck.cover_catalog_id {
        let cards = card_infos(conn, &[catalog_id]).map_err(storage)?;
        if let Some(name) = cards.get(&catalog_id).and_then(|card| card.name.clone()) {
            cover = Some(CoverCardView { catalog_id, name });
        }
    }

    Ok(CoverSettingsView {
        card_names,
        cover,
        custom_cover_art: deck.cover_file,
    })
}

/// A cover must be a printing with art (0.x); `None` clears it.
pub fn checked_cover(
    conn: &Connection,
    catalog_id: Option<u32>,
) -> Result<Option<u32>, CommandError> {
    let Some(catalog_id) = catalog_id else {
        return Ok(None);
    };
    let cards = card_infos(conn, &[catalog_id]).map_err(storage)?;
    let has_art = cards
        .get(&catalog_id)
        .and_then(|card| card.art_crop.as_deref())
        .is_some_and(|art| !art.is_empty());
    if has_art {
        Ok(Some(catalog_id))
    } else {
        Err(CommandError::Refused(
            "That printing has no art to use as a cover.".to_string(),
        ))
    }
}

/// The `.dek` file for a version (`None`, or a version not the deck's: the
/// latest, as 0.x exports): its file name and text.
pub fn dek_export(
    conn: &Connection,
    deck_id: i64,
    version: Option<i64>,
) -> Result<(String, String), CommandError> {
    let Some(deck) = deck_by_id(conn, deck_id).map_err(storage)? else {
        return Err(refused(EditOutcome::NotFound));
    };
    let versions = versions_of(conn, deck_id).map_err(storage)?;
    // Oldest first, so the last is the latest.
    let chosen = versions
        .iter()
        .find(|row| Some(row.id) == version)
        .or(versions.last());
    let cards = match chosen {
        Some(row) => decks::decode_signature(&row.signature),
        None => Vec::new(),
    };
    if cards.is_empty() {
        return Err(CommandError::Refused(
            "This deck has no cards to export.".to_string(),
        ));
    }
    let mut catalog_ids = Vec::new();
    for card in &cards {
        catalog_ids.push(card.catalog_id);
    }
    let known = card_infos(conn, &catalog_ids).map_err(storage)?;
    let mut rows = Vec::new();
    for card in &cards {
        // 0.x leaves out a card with no card row.
        if let Some(info) = known.get(&card.catalog_id) {
            rows.push(DekCard {
                catalog_id: card.catalog_id,
                quantity: card.quantity,
                sideboard: card.sideboard,
                name: info.name.clone().unwrap_or_default(),
            });
        }
    }
    Ok((dek_file_name(&deck.name), dek_file(&rows)))
}

// ---- Edits ------------------------------------------------------------------

/// A deck editor for the running tracker. The tracker lock is held only to
/// take it, never while waiting for an answer.
fn deck_editor(state: &AppState) -> Result<tracker::DeckEditor, CommandError> {
    state.tracker_part(tracker::TrackerHandle::deck_editor)
}

pub(crate) fn send_edit(state: &AppState, edit: DeckEdit) -> Result<EditOutcome, CommandError> {
    deck_editor(state)?
        .edit(edit)
        .map_err(CommandError::Tracker)
}

pub fn rename(state: &AppState, deck_id: i64, name: &str) -> Result<(), CommandError> {
    let name = checked_name(name)?;
    edit_result(send_edit(state, DeckEdit::Rename { deck_id, name })?)
}

pub fn set_identity(
    state: &AppState,
    deck_id: i64,
    letters: &[String],
) -> Result<(), CommandError> {
    let identity = checked_identity(letters)?;
    edit_result(send_edit(
        state,
        DeckEdit::SetIdentity { deck_id, identity },
    )?)
}

pub fn set_cover(
    state: &AppState,
    deck_id: i64,
    catalog_id: Option<u32>,
) -> Result<(), CommandError> {
    let catalog_id = checked_cover(&lock(&state.reader), catalog_id)?;
    edit_result(send_edit(
        state,
        DeckEdit::SetCover {
            deck_id,
            catalog_id,
        },
    )?)
}

pub fn archive(
    state: &AppState,
    deck_id: i64,
    confirmation: &str,
    now_ms: i64,
) -> Result<(), CommandError> {
    check_confirmation(confirmation)?;
    archive_result(send_edit(
        state,
        DeckEdit::Archive {
            deck_id,
            at_ms: now_ms,
        },
    )?)
}

pub fn restore(state: &AppState, deck_id: i64) -> Result<(), CommandError> {
    edit_result(send_edit(state, DeckEdit::Restore { deck_id })?)
}

/// Read the deck files now. The tracker's count is not shown: the
/// `DecksChanged` event refreshes the pages when anything changed.
pub fn sync(state: &AppState) -> Result<(), CommandError> {
    deck_editor(state)?
        .sync()
        .map(|_written| ())
        .map_err(CommandError::Tracker)
}

// ---- Commands ---------------------------------------------------------------

#[tauri::command]
#[specta::specta]
pub fn list_decks(
    query: DeckListQuery,
    startup: State<'_, Startup>,
) -> Result<DeckListView, CommandError> {
    let _timer = Timer::start("list_decks");
    list(&lock(&ready(&startup)?.reader), &query)
}

/// `timeframe`: the page's timeframe key (Dashboard, Matches), or `null` for
/// pages without one (Decklist, Settings: the default two months).
/// `best_of`: an Arena deck's kind the version rows count, `null` for every
/// match (ignored for an MTGO deck).
#[tauri::command]
#[specta::specta]
pub fn deck_header(
    id: u32,
    timeframe: Option<String>,
    best_of: Option<BestOfView>,
    startup: State<'_, Startup>,
) -> Result<Option<DeckHeaderView>, CommandError> {
    let _timer = Timer::start("deck_header");
    let state = ready(&startup)?;
    header(
        &lock(&state.reader),
        i64::from(id),
        timeframe.as_deref(),
        crate::wall_clock_ms(),
        state.timezone,
        best_of,
    )
}

/// `best_of`: an Arena deck's kind, `null` for every match (ignored for an
/// MTGO deck).
#[tauri::command]
#[specta::specta]
pub fn deck_dashboard(
    id: u32,
    version: Option<u32>,
    timeframe: String,
    best_of: Option<BestOfView>,
    startup: State<'_, Startup>,
) -> Result<DashboardView, CommandError> {
    let _timer = Timer::start("deck_dashboard");
    let state = ready(&startup)?;
    dashboard(
        &lock(&state.reader),
        i64::from(id),
        version.map(i64::from),
        &timeframe,
        crate::wall_clock_ms(),
        state.timezone,
        best_of,
    )
}

#[tauri::command]
#[specta::specta]
pub fn deck_decklist(
    id: u32,
    version: Option<u32>,
    startup: State<'_, Startup>,
) -> Result<DecklistView, CommandError> {
    let _timer = Timer::start("deck_decklist");
    decklist(
        &lock(&ready(&startup)?.reader),
        i64::from(id),
        version.map(i64::from),
    )
}

#[tauri::command]
#[specta::specta]
pub fn deck_matches(
    id: u32,
    version: Option<u32>,
    timeframe: String,
    query: MatchListQuery,
    best_of: Option<BestOfView>,
    startup: State<'_, Startup>,
) -> Result<MatchPage, CommandError> {
    let _timer = Timer::start("deck_matches");
    let state = ready(&startup)?;
    matches(
        &lock(&state.reader),
        i64::from(id),
        version.map(i64::from),
        &timeframe,
        &query,
        crate::wall_clock_ms(),
        state.timezone,
        best_of,
    )
}

#[tauri::command]
#[specta::specta]
pub fn deck_cover_options(
    card_name: String,
    startup: State<'_, Startup>,
) -> Result<Vec<CoverOptionView>, CommandError> {
    let _timer = Timer::start("deck_cover_options");
    covers(&lock(&ready(&startup)?.reader), &card_name)
}

/// The Settings page's cover picker: card names and the current cover.
#[tauri::command]
#[specta::specta]
pub fn deck_cover_settings(
    id: u32,
    startup: State<'_, Startup>,
) -> Result<CoverSettingsView, CommandError> {
    let _timer = Timer::start("deck_cover_settings");
    cover_settings(&lock(&ready(&startup)?.reader), i64::from(id))
}

/// Async so the wait for the tracker's commit never blocks the window.
#[tauri::command]
#[specta::specta]
pub async fn rename_deck(app: AppHandle, id: u32, name: String) -> Result<(), CommandError> {
    let _timer = Timer::start("rename_deck");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        rename(ready(&startup)?, i64::from(id), &name)
    })
    .await
}

/// Async so the wait for the tracker's commit never blocks the window.
#[tauri::command]
#[specta::specta]
pub async fn set_deck_identity(
    app: AppHandle,
    id: u32,
    letters: Vec<String>,
) -> Result<(), CommandError> {
    let _timer = Timer::start("set_deck_identity");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        set_identity(ready(&startup)?, i64::from(id), &letters)
    })
    .await
}

/// Async so the wait for the tracker's commit never blocks the window.
#[tauri::command]
#[specta::specta]
pub async fn set_deck_cover(
    app: AppHandle,
    id: u32,
    catalog_id: Option<u32>,
) -> Result<(), CommandError> {
    let _timer = Timer::start("set_deck_cover");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        set_cover(ready(&startup)?, i64::from(id), catalog_id)
    })
    .await
}

/// Async so the wait for the tracker's commit never blocks the window.
#[tauri::command]
#[specta::specta]
pub async fn archive_deck(
    app: AppHandle,
    id: u32,
    confirmation: String,
) -> Result<(), CommandError> {
    let _timer = Timer::start("archive_deck");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        archive(
            ready(&startup)?,
            i64::from(id),
            &confirmation,
            crate::wall_clock_ms(),
        )
    })
    .await
}

/// Async so the wait for the tracker's commit never blocks the window.
#[tauri::command]
#[specta::specta]
pub async fn restore_deck(app: AppHandle, id: u32) -> Result<(), CommandError> {
    let _timer = Timer::start("restore_deck");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        restore(ready(&startup)?, i64::from(id))
    })
    .await
}

/// The "Sync decks" button. Async: reading the deck files can take a moment.
#[tauri::command]
#[specta::specta]
pub async fn sync_decks(app: AppHandle) -> Result<(), CommandError> {
    let _timer = Timer::start("sync_decks");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        sync(ready(&startup)?)
    })
    .await
}

// ---- Deck sync (Account page) ---------------------------------------------

/// One deck in the Account page's deck sync list (0.x `BuildDeckSyncList`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeckSyncRowView {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    pub name: String,
    pub format: String,
    pub format_label: String,
    /// The deck's archetype (#9 gives decks one); none yet.
    pub archetype: Option<String>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub last_played_at_ms: Option<i64>,
    pub cloud_sync_enabled: bool,
    /// A slot turned off but still held: the server's date it frees.
    pub frees_at: Option<String>,
    /// The deck's platform, shown beside its name.
    pub platform: crate::platform::PlatformView,
}

/// The Account page's deck sync list (0.x `AccountController`'s props).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeckSyncListView {
    pub decks: Vec<DeckSyncRowView>,
    pub slots: crate::api::sync_runner::SlotCounts,
    pub linked: bool,
}

/// The list's rows with their cooldowns from the stored ledger (0.x
/// `cooldowns`: decks turned off whose slot is still held).
pub fn deck_sync_view(
    rows: &[store::sync::DeckSyncRow],
    slots: Option<&mymtgo_api::Slots>,
    linked: bool,
) -> DeckSyncListView {
    let cooling: std::collections::HashMap<&str, Option<String>> = slots
        .and_then(|slots| slots.decks.as_ref())
        .into_iter()
        .flatten()
        .filter(|deck| deck.disabled_at.is_some())
        .map(|deck| (deck.client_id.as_str(), deck.frees_at.clone()))
        .collect();
    let decks = rows
        .iter()
        .map(|row| DeckSyncRowView {
            id: row.id,
            name: row.name.clone(),
            format: row.format.clone(),
            format_label: decks::display_format(&row.format),
            archetype: None,
            last_played_at_ms: row.last_played_at,
            cloud_sync_enabled: row.cloud_sync_enabled,
            frees_at: cooling.get(row.client_id.as_str()).cloned().flatten(),
            platform: row.platform.into(),
        })
        .collect();
    let slots = crate::api::sync_runner::SlotCounts {
        limit: slots.and_then(|slots| slots.limit),
        used: slots.map_or(0, |slots| slots.used),
    };
    DeckSyncListView {
        decks,
        slots,
        linked,
    }
}

/// The Account page's deck sync list. Async: reading whether this device is
/// signed in may open the keychain.
#[tauri::command]
#[specta::specta]
pub async fn deck_sync_list(app: AppHandle) -> Result<DeckSyncListView, CommandError> {
    let _timer = Timer::start("deck_sync_list");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let linked = app
            .state::<crate::api::session::ApiState>()
            .session()
            .linked();
        let conn = lock(&state.reader);
        let rows = deck_sync_rows(&conn).map_err(storage)?;
        let slots = store::sync::stored_slots(&conn).map_err(storage)?;
        Ok(deck_sync_view(&rows, slots.as_ref(), linked))
    })
    .await
}

/// The Settings sync list, whatever the page context is: the decks of the
/// current account of each platform (cross-device sync spec section 6).
/// When no platform has a current account, every deck (0.x listed every
/// deck then).
pub(crate) fn deck_sync_rows(
    conn: &Connection,
) -> Result<Vec<store::sync::DeckSyncRow>, store::StoreError> {
    let mut account_ids = Vec::new();
    for platform in [
        store::Platform::Mtgo,
        store::Platform::Arena,
        store::Platform::Endstep,
    ] {
        account_ids.extend(store::accounts::current_account_id(conn, platform)?);
    }
    if account_ids.is_empty() {
        return store::sync::deck_sync_list(conn, None);
    }
    store::sync::deck_sync_list(conn, Some(&account_ids))
}

/// Turn a deck's cloud sync on or off (0.x `UpdateCloudSyncController`):
/// the server decides, the ledger it answers is mirrored, and enabling
/// queues a sync run. Refusals carry 0.x's messages.
#[tauri::command]
#[specta::specta]
pub async fn set_deck_cloud_sync(
    app: AppHandle,
    deck_id: u32,
    enabled: bool,
) -> Result<crate::api::sync_runner::SlotCounts, CommandError> {
    let _timer = Timer::start("set_deck_cloud_sync");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let editor = state.tracker_part(tracker::TrackerHandle::sync_editor)?;
        let conn = store::open_reader(&state.db_path).map_err(storage)?;
        let slots = crate::api::sync_runner::set_deck_sync(
            &app.state::<crate::api::session::ApiState>(),
            &editor,
            &conn,
            i64::from(deck_id),
            enabled,
            state.timezone,
            crate::wall_clock_ms(),
        )?;
        if enabled {
            crate::api::worker::signal(&app, crate::api::worker::WorkerSignal::SyncNow);
        }
        Ok(slots)
    })
    .await
}

/// Save a version's `.dek` file where the user picks (0.x
/// `ExportDekController`). Async: a blocking dialog must not run on the main
/// thread.
#[tauri::command]
#[specta::specta]
pub async fn export_deck_dek(
    app: AppHandle,
    id: u32,
    version: Option<u32>,
) -> Result<DekExport, CommandError> {
    let _timer = Timer::start("export_deck_dek");
    let (file_name, text) = {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        dek_export(&lock(&state.reader), i64::from(id), version.map(i64::from))?
    };
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Save Deck File")
        .set_file_name(file_name)
        .add_filter("MTGO Deck", &["dek"]);
    // Modal, like the folder pickers.
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    let Some(picked) = dialog.blocking_save_file() else {
        return Ok(DekExport::Cancelled);
    };
    let path = picked
        .into_path()
        .map_err(|error| CommandError::Storage(format!("could not save the deck file: {error}")))?;
    std::fs::write(&path, text)
        .map_err(|error| CommandError::Storage(format!("could not save the deck file: {error}")))?;
    Ok(DekExport::Saved {
        path: path.to_string_lossy().into_owned(),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::commands::tests::ready_state;
    use crate::summary::testing::view;
    use decks::DeckCard;
    use fixture_tools::TestDir;
    use mtgo_match::MatchState;
    use std::time::{Duration, Instant};
    use store::decks::DeckWrite;
    use store::write::{MatchWrite, WriteBatch, commit};
    use tracker::TrackerEvent;

    fn strings(letters: &[&str]) -> Vec<String> {
        letters.iter().map(|letter| letter.to_string()).collect()
    }

    fn refused(message: &str) -> CommandError {
        CommandError::Refused(message.to_string())
    }

    /// The Settings list's deck ids on a database with `sql` run on it.
    fn listed(name: &str, sql: &str) -> Vec<i64> {
        let dir = TestDir::new(name);
        let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        conn.execute_batch(sql).expect("rows");
        let mut ids: Vec<i64> = deck_sync_rows(&conn)
            .expect("rows")
            .into_iter()
            .map(|row| row.id)
            .collect();
        ids.sort();
        ids
    }

    const TWO_DECKS: &str = "INSERT INTO decks (id, mtgo_id, account_id, name, format,
             created_at, updated_at, source)
         VALUES (1, 'q-mtgo', NULL, 'Qmtgo', 'CMODERN', 0, 0, 'file'),
                (2, 'q-arena', (SELECT id FROM accounts WHERE platform = 'arena'), 'Qarena',
                 'CPIONEER', 0, 0, 'file');";

    #[test]
    fn the_deck_sync_list_reads_each_platforms_current_account_without_mtgo() {
        let arena = format!(
            "INSERT INTO accounts (username, first_seen_at, platform, arena_user_id)
             VALUES ('Qarena#12345', 0, 'arena', 'Qarena-id'); {TWO_DECKS}"
        );
        assert_eq!(
            listed("decks-sync-list-arena-only", &arena),
            vec![2],
            "the current Arena account's decks, as when MTGO has one"
        );
        assert_eq!(
            listed("decks-sync-list-no-account", TWO_DECKS),
            vec![1, 2],
            "no platform has a current account: every deck"
        );
    }

    #[test]
    fn the_deck_sync_list_shows_each_decks_cooldown_from_the_ledger() {
        let rows = vec![
            store::sync::DeckSyncRow {
                id: 1,
                name: "Qon".to_string(),
                format: "CMODERN".to_string(),
                client_id: "111".to_string(),
                last_played_at: Some(5_000),
                cloud_sync_enabled: true,
                platform: store::Platform::Mtgo,
            },
            store::sync::DeckSyncRow {
                id: 2,
                name: "Qcooling".to_string(),
                format: "CPAUPER".to_string(),
                client_id: "222".to_string(),
                last_played_at: None,
                cloud_sync_enabled: false,
                platform: store::Platform::Arena,
            },
        ];
        let slots = mymtgo_api::Slots {
            limit: Some(1),
            used: 1,
            tier: mymtgo_api::Tier::Free,
            decks: Some(vec![
                mymtgo_api::SlotDeck {
                    client_id: "111".to_string(),
                    enabled_at: Some("x".to_string()),
                    disabled_at: None,
                    frees_at: None,
                },
                mymtgo_api::SlotDeck {
                    client_id: "222".to_string(),
                    enabled_at: Some("x".to_string()),
                    disabled_at: Some("y".to_string()),
                    frees_at: Some("2026-11-05T09:30:00+00:00".to_string()),
                },
            ]),
        };
        let list = deck_sync_view(&rows, Some(&slots), true);
        assert!(list.linked);
        assert_eq!(
            list.slots,
            crate::api::sync_runner::SlotCounts {
                limit: Some(1),
                used: 1
            }
        );
        assert_eq!(list.decks[0].frees_at, None);
        assert_eq!(list.decks[0].format_label, "Modern");
        assert_eq!(list.decks[0].last_played_at_ms, Some(5_000));
        assert_eq!(
            [list.decks[0].platform, list.decks[1].platform],
            [
                crate::platform::PlatformView::Mtgo,
                crate::platform::PlatformView::Arena
            ],
            "each row names its deck's platform"
        );
        assert_eq!(
            list.decks[1].frees_at.as_deref(),
            Some("2026-11-05T09:30:00+00:00")
        );
        let empty = deck_sync_view(&[], None, false);
        assert_eq!(
            empty.slots,
            crate::api::sync_runner::SlotCounts {
                limit: None,
                used: 0
            },
            "0.x: no stored ledger reads as no limit, none used"
        );
    }

    #[test]
    fn a_name_is_trimmed_and_needs_1_to_255_characters() {
        assert_eq!(checked_name("  Qburn  "), Ok("Qburn".to_string()));
        let message = "A deck name needs 1 to 255 characters.";
        assert_eq!(checked_name(""), Err(refused(message)));
        assert_eq!(checked_name("   "), Err(refused(message)));
        // Characters, not bytes: 255 two-byte letters fit.
        let longest = "é".repeat(255);
        assert_eq!(checked_name(&longest), Ok(longest.clone()));
        assert_eq!(checked_name(&"q".repeat(256)), Err(refused(message)));
    }

    #[test]
    fn an_identity_takes_only_wubrgc_and_is_normalised() {
        assert_eq!(
            checked_identity(&strings(&["R", "U", "R"])),
            Ok(Some("U,R".to_string()))
        );
        assert_eq!(
            checked_identity(&strings(&["C", "G", "B", "W"])),
            Ok(Some("W,B,G,C".to_string()))
        );
        assert_eq!(checked_identity(&[]), Ok(None));
        let message = "Colours must be W, U, B, R, G or C.";
        assert_eq!(checked_identity(&strings(&["X"])), Err(refused(message)));
        assert_eq!(
            checked_identity(&strings(&["u"])),
            Err(refused(message)),
            "0.x matches the letters exactly"
        );
        assert_eq!(
            checked_identity(&strings(&["U", "UR"])),
            Err(refused(message))
        );
    }

    #[test]
    fn an_archive_needs_delete_typed() {
        assert_eq!(check_confirmation("DELETE"), Ok(()));
        assert_eq!(check_confirmation("  DELETE "), Ok(()), "0.x trims input");
        let message = "Type DELETE to confirm.";
        assert_eq!(check_confirmation("delete"), Err(refused(message)));
        assert_eq!(check_confirmation(""), Err(refused(message)));
        assert_eq!(check_confirmation("DELETE IT"), Err(refused(message)));
    }

    #[test]
    fn refused_edits_become_messages_for_the_user() {
        assert_eq!(edit_result(EditOutcome::Done), Ok(()));
        assert_eq!(
            edit_result(EditOutcome::Archived),
            Err(refused(store::decks::ARCHIVED_DECK))
        );
        assert_eq!(
            edit_result(EditOutcome::NotFound),
            Err(refused("This deck no longer exists."))
        );
        assert_eq!(
            edit_result(EditOutcome::InvalidName),
            Err(refused("A deck name needs 1 to 255 characters."))
        );
        assert_eq!(archive_result(EditOutcome::Done), Ok(()));
        assert_eq!(
            archive_result(EditOutcome::Archived),
            Ok(()),
            "already archived"
        );
        assert_eq!(
            archive_result(EditOutcome::NotFound),
            Err(refused("This deck no longer exists."))
        );
    }

    #[test]
    fn a_dek_file_name_replaces_characters_file_systems_refuse() {
        assert_eq!(dek_file_name("Qburn"), "Qburn.dek");
        assert_eq!(
            dek_file_name("Q: Burn/Red? <v2> \"x\" a\\b|c*"),
            "Q- Burn-Red- -v2- -x- a-b-c-.dek"
        );
        assert_eq!(dek_file_name("Qdeck\u{7}"), "Qdeck-.dek");
        assert_eq!(
            dek_file_name(" Qdeck. "),
            "Qdeck.dek",
            "Windows drops these"
        );
        assert_eq!(dek_file_name("..."), "deck.dek");
        assert_eq!(dek_file_name(""), "deck.dek");
    }

    #[test]
    fn a_dek_file_name_never_uses_a_windows_device_name() {
        for name in [
            "CON",
            "con",
            "Prn",
            "AUX",
            "nul",
            "COM1",
            "com9",
            "LPT1",
            "lpt9",
            "COM\u{b9}",
        ] {
            assert_eq!(dek_file_name(name), format!("_{name}.dek"), "{name}");
        }
        // Windows reserves the name before the first dot, trailing spaces ignored.
        assert_eq!(dek_file_name("nul.tar"), "_nul.tar.dek");
        assert_eq!(dek_file_name("CON .x"), "_CON .x.dek");
        assert_eq!(dek_file_name("aux."), "_aux.dek");
        // Names that only contain a device name are fine.
        assert_eq!(dek_file_name("Qcon"), "Qcon.dek");
        assert_eq!(dek_file_name("CONSOLE"), "CONSOLE.dek");
        assert_eq!(dek_file_name("COM0"), "COM0.dek");
        assert_eq!(dek_file_name("COM10"), "COM10.dek");
        assert_eq!(dek_file_name("LPT"), "LPT.dek");
    }

    #[test]
    fn a_dek_file_name_is_at_most_100_characters_before_the_extension() {
        let long = "Q".repeat(300);
        assert_eq!(dek_file_name(&long), format!("{}.dek", "Q".repeat(100)));
        // Counted in characters, not bytes.
        let wide = "\u{e9}".repeat(150);
        assert_eq!(
            dek_file_name(&wide),
            format!("{}.dek", "\u{e9}".repeat(100))
        );
        // A cut that ends in dots or spaces is trimmed again.
        let spaced = format!("{} .{}", "Q".repeat(98), "Q".repeat(50));
        assert_eq!(dek_file_name(&spaced), format!("{}.dek", "Q".repeat(98)));
        assert_eq!(dek_file_name(&" ".repeat(300)), "deck.dek");
    }

    fn query() -> DeckListQuery {
        DeckListQuery {
            format: None,
            recent: false,
            search: None,
            hide_archived: true,
            sort: DeckSortKey::LastPlayed,
            page: 1,
            per_page: 12,
            archetype: None,
        }
    }

    fn no_account() -> PageScope {
        PageScope {
            platform: store::Platform::Mtgo,
            account: None,
        }
    }

    #[test]
    fn a_list_query_becomes_the_store_filter() {
        let filter = list_filter(
            &DeckListQuery {
                format: Some("CMODERN".to_string()),
                recent: true,
                search: Some("  Qbu ".to_string()),
                hide_archived: false,
                sort: DeckSortKey::WinRate,
                page: 3,
                per_page: 48,
                archetype: Some("none".to_string()),
            },
            PageScope {
                platform: store::Platform::Mtgo,
                account: Some(7),
            },
        );
        assert_eq!(
            filter,
            DeckListFilter {
                account_id: Some(7),
                platform: store::Platform::Mtgo,
                format: Some("CMODERN".to_string()),
                recent: true,
                search: Some("Qbu".to_string()),
                hide_archived: false,
                sort: DeckSort::WinRate,
                page: 3,
                per_page: 48,
                archetype: Some(store::deck_pages::DeckArchetypeFilter::Unclassified),
            }
        );
        let blank = list_filter(
            &DeckListQuery {
                format: Some(" ".to_string()),
                search: Some("   ".to_string()),
                per_page: 30,
                ..query()
            },
            no_account(),
        );
        assert_eq!((blank.format, blank.search), (None, None));
        assert_eq!(blank.per_page, 12, "not a page size option");
        for (key, sort) in [
            (DeckSortKey::LastPlayed, DeckSort::LastPlayed),
            (DeckSortKey::MatchCount, DeckSort::MatchCount),
            (DeckSortKey::Name, DeckSort::Name),
        ] {
            assert_eq!(
                list_filter(
                    &DeckListQuery {
                        sort: key,
                        ..query()
                    },
                    no_account()
                )
                .sort,
                sort
            );
        }
    }

    #[test]
    fn the_ui_sends_and_receives_camel_case() {
        let parsed: DeckListQuery = serde_json::from_value(serde_json::json!({
            "format": null, "recent": false, "search": "Q", "hideArchived": true,
            "sort": "match_count", "page": 2, "perPage": 24
        }))
        .expect("parses");
        assert_eq!(parsed.sort, DeckSortKey::MatchCount);
        assert_eq!((parsed.page, parsed.per_page), (2, 24));
        assert_eq!(
            serde_json::to_value(DekExport::Saved {
                path: "C:/q.dek".to_string()
            })
            .expect("json"),
            serde_json::json!({ "kind": "saved", "path": "C:/q.dek" })
        );
        assert_eq!(
            serde_json::to_value(DekExport::Cancelled).expect("json"),
            serde_json::json!({ "kind": "cancelled" })
        );
        assert_eq!(
            serde_json::to_value(refused("no")).expect("json"),
            serde_json::json!({ "kind": "refused", "message": "no" })
        );
    }

    /// 2026-07-01T12:00:00Z
    const NOW: i64 = 1_782_907_200_000;
    const DAY: i64 = 86_400_000;

    #[test]
    fn a_page_without_a_timeframe_counts_the_default_two_months() {
        let tz = chrono_tz::UTC;
        assert_eq!(
            header_range(None, NOW, tz),
            stats::default_version_range(NOW)
        );
        assert_eq!(header_range(Some("alltime"), NOW, tz), Range::all_time());
        assert_eq!(header_range(Some("nonsense"), NOW, tz), Range::all_time());
        let week = stats::timeframe_range("week", NOW, tz).expect("week");
        assert_eq!(header_range(Some("week"), NOW, tz), week);

        assert_eq!(
            timeframe_ranges("alltime", NOW, tz),
            (Range::all_time(), None)
        );
        let (range, previous) = timeframe_ranges("week", NOW, tz);
        assert_eq!(range, week);
        assert_eq!(previous, stats::previous_range("week", week.from_ms, tz));
        assert!(previous.is_some());
    }

    fn cards(list: &[(u32, u32, bool)]) -> Vec<DeckCard> {
        list.iter()
            .map(|(catalog_id, quantity, sideboard)| DeckCard {
                catalog_id: *catalog_id,
                quantity: *quantity,
                sideboard: *sideboard,
            })
            .collect()
    }

    /// One account with one deck (two versions) and its cards, and a
    /// complete match won with the current version yesterday.
    fn seed(conn: &mut Connection) -> (i64, i64, i64) {
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, name, type, color_identity, cmc, set_name, set_code, image, art_crop) VALUES
                 (1001, 'Qbolt', 'Instant', 'R', 1.0, 'Qset', 'QS1', 'https://cards.scryfall.io/n/1.jpg', 'https://cards.scryfall.io/a/1.jpg'),
                 (1002, 'Qmountain', 'Basic Land - Mountain', '', 0.0, 'Qset', 'QS1', NULL, NULL),
                 (1003, 'Qgoblin', 'Creature - Goblin', 'R', 2.0, 'Qset', 'QS1', NULL, '');
             UPDATE cards SET local_image = '1001.jpg', local_art_crop = '1001_art.jpg'
                 WHERE catalog_id = 1001;
             INSERT INTO decks (id, mtgo_id, name, format, color_identity, cover_catalog_id,
                 created_at, updated_at, source)
             VALUES (5, 'q-deck', 'Qburn', 'CMODERN', 'U,R', 1001, 1, 1, 'file');",
        )
        .expect("deck");
        let old = decks::signature(&cards(&[(1001, 4, false), (1002, 20, false)]));
        let new = decks::signature(&cards(&[
            (1001, 4, false),
            (1003, 2, false),
            (1002, 18, false),
            (1001, 2, true),
            (4040, 1, true),
        ]));
        // Signatures are base64: safe inside the SQL text.
        conn.execute_batch(&format!(
            "INSERT INTO deck_versions (id, deck_id, signature, modified_at) VALUES
                 (10, 5, '{old}', {}), (11, 5, '{new}', {});",
            NOW - 30 * DAY,
            NOW - 10 * DAY
        ))
        .expect("versions");
        let token = "q-match-1";
        commit(
            conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(view(token, NOW - DAY, MatchState::Complete)),
                    revision: 1,
                }],
                decks: vec![DeckWrite::Link {
                    match_token: token.to_string(),
                    deck_version_id: 11,
                }],
                now_ms: NOW,
                ..WriteBatch::default()
            },
        )
        .expect("match");
        // The deck belongs to the match's account, the list's scope.
        conn.execute_batch(
            "UPDATE decks SET account_id = (SELECT id FROM accounts WHERE username = 'Qlocal');",
        )
        .expect("account");
        (5, 10, 11)
    }

    pub(crate) fn database(label: &str) -> (TestDir, Connection, (i64, i64, i64)) {
        let dir = TestDir::new(label);
        let mut conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        let ids = seed(&mut conn);
        (dir, conn, ids)
    }

    #[test]
    fn the_deck_list_maps_every_card_field() {
        let (_dir, conn, (deck_id, _, _)) = database("decks-list-view");
        let view = list(&conn, &query()).expect("list");
        assert_eq!(
            (view.total, view.page, view.per_page, view.recent),
            (1, 1, 12, false)
        );
        assert_eq!(
            view.format_options,
            vec![FormatOptionView {
                code: "CMODERN".to_string(),
                label: "Modern".to_string(),
                count: 1
            }]
        );
        assert_eq!(
            view.decks,
            vec![DeckView {
                id: deck_id,
                name: "Qburn".to_string(),
                original_name: None,
                format: "CMODERN".to_string(),
                format_label: "Modern".to_string(),
                colors: strings(&["U", "R"]),
                cover_art: Some("https://cards.scryfall.io/a/1.jpg".to_string()),
                local_cover_art: Some("1001_art.jpg".to_string()),
                custom_cover_art: None,
                archived: false,
                record: RecordView {
                    wins: 1,
                    losses: 0,
                    draws: 0,
                    total: 1,
                    win_rate: 100,
                    label: "1 - 0".to_string(),
                },
                kinds: Vec::new(),
                last_played_at_ms: Some(NOW - DAY),
                archetype: None,
                commander_format: false,
            }]
        );
        let json = serde_json::to_value(&view.decks[0]).expect("json");
        assert_eq!(json["lastPlayedAtMs"], NOW - DAY);
        assert_eq!(json["record"]["winRate"], 100);
    }

    /// Only the Arena Brawl codes are commander formats: an MTGO Brawl
    /// deck (`CBRAWL`) keeps its sideboard and its hand-set archetype.
    #[test]
    fn only_an_arena_brawl_deck_is_a_commander_format_deck() {
        let (_dir, conn, (deck_id, _, _)) = database("decks-commander-format");
        for (format, expected) in [
            ("ARENA_HISTORIC_BRAWL", true),
            ("ARENA_BRAWL", true),
            ("CBRAWL", false),
            ("CMODERN", false),
        ] {
            conn.execute("UPDATE decks SET format = ?1", [format])
                .expect("format");
            let view = header(&conn, deck_id, Some("alltime"), NOW, chrono_tz::UTC, None)
                .expect("header")
                .expect("deck");
            assert_eq!(view.deck.commander_format, expected, "{format}");
        }
        let json = serde_json::to_value(
            header(&conn, deck_id, Some("alltime"), NOW, chrono_tz::UTC, None)
                .expect("header")
                .expect("deck")
                .deck,
        )
        .expect("json");
        assert_eq!(json["commanderFormat"], false);
    }

    #[test]
    fn the_header_lists_all_versions_then_each_version() {
        let (_dir, conn, (deck_id, old, new)) = database("decks-header-view");
        let view = header(&conn, deck_id, Some("alltime"), NOW, chrono_tz::UTC, None)
            .expect("header")
            .expect("deck");
        assert_eq!(view.deck.name, "Qburn");
        assert_eq!(view.current_version_id, Some(new));
        let ids: Vec<Option<i64>> = view.versions.iter().map(|row| row.id).collect();
        assert_eq!(ids, vec![None, Some(old), Some(new)]);
        let all = &view.versions[0];
        assert_eq!(all.label, "All versions");
        assert_eq!((all.record.wins, all.record.total), (1, 1));
        assert_eq!(
            (all.games.won, all.games.lost, all.games.win_rate),
            (2, 1, 67)
        );
        assert_eq!(all.play_draw.play_draw_games, 3);
        assert!(view.versions[2].is_current);
        assert_eq!(view.versions[1].record.total, 0);
        assert!(
            header(&conn, 999, None, NOW, chrono_tz::UTC, None)
                .expect("header")
                .is_none()
        );
    }

    /// Moves the seeded deck and its match to an Arena account, the match a
    /// Bo1.
    fn move_seed_to_arena(conn: &Connection) {
        conn.execute_batch(
            "UPDATE accounts SET platform = 'arena', arena_user_id = 'Qarena-id'
                 WHERE username = 'Qlocal';
             UPDATE matches SET game_structure = 'bo1';",
        )
        .expect("arena");
    }

    #[test]
    fn an_arena_deck_header_has_kinds_and_no_trophies() {
        let (_dir, conn, (deck_id, _, _)) = database("decks-header-arena");
        move_seed_to_arena(&conn);
        let view = header(&conn, deck_id, Some("alltime"), NOW, chrono_tz::UTC, None)
            .expect("header")
            .expect("deck");
        assert_eq!(view.platform, PlatformView::Arena);
        assert_eq!(view.trophies, None);
        assert_eq!(
            view.kinds,
            Some(DeckKindsView {
                bo1: 1,
                bo3: 0,
                latest: Some(BestOfView::Bo1),
            })
        );
        let json = serde_json::to_value(&view).expect("json");
        assert_eq!(json["kinds"]["latest"], "bo1");
        assert_eq!(json["platform"], "arena");

        // The version rows follow the kind.
        let bo3 = header(
            &conn,
            deck_id,
            Some("alltime"),
            NOW,
            chrono_tz::UTC,
            Some(BestOfView::Bo3),
        )
        .expect("header")
        .expect("deck");
        assert_eq!(bo3.versions[0].record.total, 0);
        let dashboard = dashboard(
            &conn,
            deck_id,
            None,
            "alltime",
            NOW,
            chrono_tz::UTC,
            Some(BestOfView::Bo1),
        )
        .expect("dashboard");
        assert_eq!(dashboard.record.total, 1);
        let bo3_matches = matches(
            &conn,
            deck_id,
            None,
            "alltime",
            &first_page(),
            NOW,
            chrono_tz::UTC,
            Some(BestOfView::Bo3),
        )
        .expect("page");
        assert_eq!(bo3_matches.total, 0);
    }

    #[test]
    fn an_mtgo_deck_header_has_trophies_and_no_kinds() {
        let (_dir, conn, (deck_id, _, _)) = database("decks-header-mtgo");
        let view = header(&conn, deck_id, Some("alltime"), NOW, chrono_tz::UTC, None)
            .expect("header")
            .expect("deck");
        assert_eq!(view.platform, PlatformView::Mtgo);
        assert_eq!(view.trophies, Some(0));
        assert_eq!(view.kinds, None);
        // An MTGO deck is never filtered by kind.
        let bo1 = header(
            &conn,
            deck_id,
            Some("alltime"),
            NOW,
            chrono_tz::UTC,
            Some(BestOfView::Bo1),
        )
        .expect("header")
        .expect("deck");
        assert_eq!(bo1.versions[0].record.total, 1);
    }

    #[test]
    fn the_dashboard_maps_its_numbers_and_chart() {
        let (_dir, conn, (deck_id, old, _)) = database("decks-dashboard-view");
        let view =
            dashboard(&conn, deck_id, None, "week", NOW, chrono_tz::UTC, None).expect("dashboard");
        assert_eq!(view.record.label, "1 - 0");
        assert_eq!(
            view.delta,
            WinRateDeltaView {
                previous_rate: None,
                previous_total: 0,
                delta: None
            },
            "nothing in the previous week"
        );
        assert_eq!((view.games.won, view.games.lost), (2, 1));
        assert_eq!(view.boarding.games, 3);
        assert_eq!(
            (view.boarding.game_one_won, view.boarding.post_board_won),
            (1, 1)
        );
        assert_eq!(
            view.daily,
            vec![DayView {
                day: "2026-06-30".to_string(),
                wins: 1,
                losses: 0,
                draws: 0,
                win_rate: Some(100),
                trend: Some(100),
            }]
        );
        assert_eq!(view.sparkline, vec![100.0]);
        let json = serde_json::to_value(&view).expect("json");
        assert_eq!(json["playDraw"]["otpWon"], 2);

        let narrowed = dashboard(
            &conn,
            deck_id,
            Some(old),
            "alltime",
            NOW,
            chrono_tz::UTC,
            None,
        )
        .expect("dashboard");
        assert_eq!(narrowed.record.total, 0, "the old version was not played");
    }

    #[test]
    fn the_decklist_maps_groups_curve_and_colours() {
        let (_dir, conn, (deck_id, old, new)) = database("decks-decklist-view");
        let view = decklist(&conn, deck_id, None).expect("decklist");
        assert_eq!(view.version_id, Some(new));
        let groups: Vec<(&str, u32)> = view
            .main
            .iter()
            .map(|group| (group.type_name.as_str(), group.count))
            .collect();
        assert_eq!(groups, vec![("Creature", 2), ("Instant", 4), ("Land", 18)]);
        let bolt = &view.main[1].cards[0];
        assert_eq!(bolt.name, "Qbolt");
        assert_eq!(bolt.colors, strings(&["R"]));
        assert_eq!(bolt.cmc, Some(1.0));
        assert!(bolt.known);
        let side: Vec<(&str, u32, bool)> = view
            .side
            .iter()
            .map(|card| (card.name.as_str(), card.quantity, card.known))
            .collect();
        assert_eq!(side, vec![("Qbolt", 2, true), ("Card 4040", 1, false)]);
        assert!(view.curve.contains(&CurvePoint { cmc: 1, count: 4 }));
        assert!(view.curve.contains(&CurvePoint { cmc: 2, count: 2 }));
        assert!(view.colours.contains(&ColourCount {
            colour: "R".to_string(),
            count: 6
        }));
        assert_eq!(view.nonland_total, 6);

        let older = decklist(&conn, deck_id, Some(old)).expect("decklist");
        assert_eq!(older.version_id, Some(old));
    }

    fn first_page() -> MatchListQuery {
        MatchListQuery::newest_first(1)
    }

    #[test]
    fn the_deck_matches_page_has_the_match_list_rows() {
        let (_dir, conn, (deck_id, old, new)) = database("decks-matches-view");
        let page = matches(
            &conn,
            deck_id,
            None,
            "alltime",
            &first_page(),
            NOW,
            chrono_tz::UTC,
            None,
        )
        .expect("page");
        assert_eq!((page.total, page.page, page.per_page), (1, 1, PER_PAGE));
        assert_eq!(page.rows[0].token, "q-match-1");
        assert_eq!((page.rows[0].wins, page.rows[0].losses), (2, 1));
        let version = matches(
            &conn,
            deck_id,
            Some(new),
            "alltime",
            &first_page(),
            NOW,
            chrono_tz::UTC,
            None,
        )
        .expect("page");
        assert_eq!(version.total, 1);
        let none = matches(
            &conn,
            deck_id,
            Some(old),
            "alltime",
            &first_page(),
            NOW,
            chrono_tz::UTC,
            None,
        )
        .expect("page");
        assert_eq!(none.total, 0);
    }

    #[test]
    fn a_cover_must_be_a_printing_with_art() {
        let (_dir, conn, _) = database("decks-cover-view");
        assert_eq!(
            covers(&conn, "Qbolt").expect("covers"),
            vec![CoverOptionView {
                catalog_id: 1001,
                set_name: Some("Qset".to_string()),
                set_code: Some("QS1".to_string()),
                art_crop: "https://cards.scryfall.io/a/1.jpg".to_string(),
                local_art_crop: Some("1001_art.jpg".to_string()),
            }]
        );
        assert!(
            covers(&conn, "Qgoblin").expect("covers").is_empty(),
            "empty art"
        );
        assert_eq!(checked_cover(&conn, Some(1001)), Ok(Some(1001)));
        assert_eq!(checked_cover(&conn, None), Ok(None));
        let message = "That printing has no art to use as a cover.";
        assert_eq!(checked_cover(&conn, Some(1003)), Err(refused(message)));
        assert_eq!(checked_cover(&conn, Some(1002)), Err(refused(message)));
        assert_eq!(checked_cover(&conn, Some(9999)), Err(refused(message)));
    }

    #[test]
    fn the_cover_picker_offers_the_latest_version_names_that_have_art() {
        let (_dir, conn, (deck_id, _, _)) = database("decks-cover-settings");
        // Qgoblin's printing in the deck has no art, but another one does
        // (0.x looks for any printing of the name with art).
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, name, art_crop) VALUES
                 (1004, 'Qgoblin', 'https://cards.scryfall.io/a/4.jpg');",
        )
        .expect("card");
        assert_eq!(
            cover_settings(&conn, deck_id).expect("settings"),
            CoverSettingsView {
                card_names: strings(&["Qbolt", "Qgoblin"]),
                cover: Some(CoverCardView {
                    catalog_id: 1001,
                    name: "Qbolt".to_string(),
                }),
                custom_cover_art: None,
            }
        );

        // A cover with no card row, and a deck with no versions.
        conn.execute_batch(
            "UPDATE decks SET cover_catalog_id = 9999, cover_file = '1-2.png';
             UPDATE matches SET deck_version_id = NULL; DELETE FROM deck_versions;",
        )
        .expect("clear");
        assert_eq!(
            cover_settings(&conn, deck_id).expect("settings"),
            CoverSettingsView {
                card_names: Vec::new(),
                cover: None,
                custom_cover_art: Some("1-2.png".to_string()),
            }
        );
        assert_eq!(
            cover_settings(&conn, 404),
            Err(refused("This deck no longer exists."))
        );
    }

    #[test]
    fn a_dek_export_writes_the_version_cards_with_names() {
        let (_dir, conn, (deck_id, old, _)) = database("decks-dek-view");
        let (name, text) = dek_export(&conn, deck_id, None).expect("dek");
        assert_eq!(name, "Qburn.dek");
        // In signature order; card 4040 has no card data and is left out.
        let row = |catalog_id, quantity, sideboard, name: &str| DekCard {
            catalog_id,
            quantity,
            sideboard,
            name: name.to_string(),
        };
        let expected = dek_file(&[
            row(1001, 4, false, "Qbolt"),
            row(1001, 2, true, "Qbolt"),
            row(1002, 18, false, "Qmountain"),
            row(1003, 2, false, "Qgoblin"),
        ]);
        assert_eq!(
            text, expected,
            "the latest version; a card without data is left out"
        );
        let (_, older) = dek_export(&conn, deck_id, Some(old)).expect("dek");
        assert!(older.contains("Quantity=\"20\""));
        let (_, fallback) = dek_export(&conn, deck_id, Some(999)).expect("dek");
        assert_eq!(fallback, expected, "not this deck's version: the latest");
        assert_eq!(
            dek_export(&conn, 999, None),
            Err(refused("This deck no longer exists."))
        );
    }

    #[test]
    fn a_deck_without_versions_has_nothing_to_export() {
        let dir = TestDir::new("decks-dek-empty");
        let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        conn.execute_batch(
            "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source)
             VALUES (1, 'q-empty', 'Qempty', 'CMODERN', 1, 1, 'file');",
        )
        .expect("deck");
        assert_eq!(
            dek_export(&conn, 1, None),
            Err(refused("This deck has no cards to export."))
        );
    }

    fn saw_decks_changed(receiver: &std::sync::mpsc::Receiver<TrackerEvent>) -> bool {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if let Ok(TrackerEvent::DecksChanged) =
                receiver.recv_timeout(Duration::from_millis(100))
            {
                return true;
            }
        }
        false
    }

    #[test]
    fn edits_go_through_the_tracker_and_an_archived_deck_refuses_them() {
        let dir = TestDir::new("decks-edits");
        let (state, receiver) = ready_state(&dir);
        {
            let mut conn = store::open_writer(&state.db_path).expect("open");
            seed(&mut conn);
        }
        let stored = |column: &str| -> Option<String> {
            lock(&state.reader)
                .query_row(
                    &format!("SELECT {column} FROM decks WHERE id = 5"),
                    [],
                    |row| row.get(0),
                )
                .expect("read")
        };

        assert_eq!(rename(&state, 5, "  Qrenamed "), Ok(()));
        assert_eq!(stored("name").as_deref(), Some("Qrenamed"));
        assert_eq!(stored("original_name").as_deref(), Some("Qburn"));
        assert!(saw_decks_changed(&receiver), "the edit is announced");
        assert_eq!(
            rename(&state, 5, "  "),
            Err(refused("A deck name needs 1 to 255 characters."))
        );

        assert_eq!(set_identity(&state, 5, &strings(&["G", "U"])), Ok(()));
        assert_eq!(stored("color_identity").as_deref(), Some("U,G"));
        assert_eq!(set_cover(&state, 5, None), Ok(()));
        assert_eq!(stored("CAST(cover_catalog_id AS TEXT)"), None);
        assert_eq!(set_cover(&state, 5, Some(1001)), Ok(()));
        assert_eq!(
            set_cover(&state, 5, Some(1003)),
            Err(refused("That printing has no art to use as a cover."))
        );

        assert_eq!(
            archive(&state, 5, "delete", NOW),
            Err(refused("Type DELETE to confirm."))
        );
        assert_eq!(stored("CAST(archived_at AS TEXT)"), None);
        assert_eq!(archive(&state, 5, "DELETE", NOW), Ok(()));
        assert_eq!(stored("CAST(archived_at AS TEXT)"), Some(NOW.to_string()));
        assert_eq!(
            archive(&state, 5, "DELETE", NOW + 1),
            Ok(()),
            "already archived"
        );
        let read_only = refused(store::decks::ARCHIVED_DECK);
        assert_eq!(rename(&state, 5, "Qagain"), Err(read_only.clone()));
        assert_eq!(
            set_identity(&state, 5, &strings(&["W"])),
            Err(read_only.clone())
        );
        assert_eq!(set_cover(&state, 5, None), Err(read_only));
        assert_eq!(restore(&state, 5), Ok(()));
        assert_eq!(stored("CAST(archived_at AS TEXT)"), None);
        assert_eq!(
            restore(&state, 999),
            Err(refused("This deck no longer exists."))
        );

        assert_eq!(sync(&state), Ok(()), "a sync is a plain success");
        crate::commands::stop_tracker(&state);
        assert_eq!(
            rename(&state, 5, "Qlate"),
            Err(CommandError::Tracker("tracking is not running".to_string()))
        );
    }
}
