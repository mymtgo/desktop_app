//! The deck pages' archetype parts (archetypes spec sections 8 and 12):
//! the deck index's archetype rows, header and tabs, the deck archetype
//! set by hand or in bulk, the deck Matchups and Game Stats pages and the
//! dashboard's matchups and peer chart.

use chrono_tz::Tz;
use decks::stats::Range;
use serde::Serialize;
use store::Connection;
use store::archetype_edit::ArchetypeEdit;
use store::deck_index::{
    self, ArchetypeHeader, DeckArchetypeFilter, DeckArchetypeOption, DeckScope, HeaderArchetype,
};
use store::deck_pages::DeckArchetype;
use store::match_list::{Scope, match_list};
use store::matchups::{self, MatchupDetail, MatchupRow, PeerChart};
use tauri::{AppHandle, Manager, State};

use crate::bridge::lock;
use crate::cards::stats::{CardStatsPayload, CardStatsQuery};
use crate::commands::{AppState, CommandError, Startup, off_async_workers, ready, storage};
use crate::decks::{FormatOptionView, RecordView, deck_best_of, record_view, timeframe_ranges};
use crate::live::PER_PAGE;
use crate::logging::Timer;
use crate::matches::{MatchListQuery, match_filter, page_view};
use crate::platform::{BestOfView, PageScope, page_scope};
use crate::summary::{MatchPage, archetype_colors};

// ---- Views ------------------------------------------------------------------

/// A deck's archetype on its card and in its header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeckArchetypeView {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    pub name: String,
    /// Colour identity letters in WUBRG order.
    pub colors: Vec<String>,
    /// The player's own archetype (shown as "Custom").
    pub is_custom: bool,
}

pub fn deck_archetype_view(archetype: &DeckArchetype) -> DeckArchetypeView {
    DeckArchetypeView {
        id: archetype.id,
        name: archetype.name.clone(),
        colors: archetype_colors(archetype.color_identity.as_deref()),
        is_custom: archetype.custom,
    }
}

/// One archetype row of the deck index sidebar (0.x
/// `DeckArchetypeOptionData`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeckArchetypeOptionView {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    pub name: String,
    pub colors: Vec<String>,
    /// The archetypes format key (`modern`); `null` for a fallback.
    pub format: Option<String>,
    pub deck_count: u32,
    pub record: RecordView,
    /// The player's own archetype (shown as "Custom").
    pub is_custom: bool,
}

fn option_view(option: &DeckArchetypeOption) -> DeckArchetypeOptionView {
    DeckArchetypeOptionView {
        id: option.id,
        name: option.name.clone(),
        colors: archetype_colors(option.color_identity.as_deref()),
        format: option.format.clone(),
        deck_count: option.deck_count,
        record: record_view(&option.record),
        is_custom: option.custom,
    }
}

/// A matchup the header names (0.x `MatchupSummaryData`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchupSummaryView {
    #[specta(type = specta_typescript::Number)]
    pub archetype_id: i64,
    pub name: String,
    pub colors: Vec<String>,
    /// Match win rate.
    pub win_rate: u32,
    pub matches: u32,
}

fn summary_view(row: &MatchupRow) -> MatchupSummaryView {
    MatchupSummaryView {
        archetype_id: row.archetype_id,
        name: row.name.clone(),
        colors: archetype_colors(row.color_identity.as_deref()),
        win_rate: row.record.win_rate,
        matches: row.record.total,
    }
}

/// The archetype a header names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HeaderArchetypeView {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    pub name: String,
    pub colors: Vec<String>,
    /// The archetypes format key; `null` for a fallback.
    pub format: Option<String>,
    /// Homebrew or Rogue: the deck grid only, no tabs (0.x).
    pub is_fallback: bool,
}

fn header_archetype_view(archetype: &HeaderArchetype) -> HeaderArchetypeView {
    HeaderArchetypeView {
        id: archetype.id,
        name: archetype.name.clone(),
        colors: archetype_colors(archetype.color_identity.as_deref()),
        format: archetype.format.clone(),
        is_fallback: archetype.is_fallback,
    }
}

/// The deck index's header band (0.x `DeckArchetypeHeaderData`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ArchetypeHeaderView {
    /// `null` for Unclassified.
    pub archetype: Option<HeaderArchetypeView>,
    pub deck_count: u32,
    pub record: RecordView,
    /// At least 3 matches each, and two such archetypes (0.x).
    pub best: Option<MatchupSummaryView>,
    pub worst: Option<MatchupSummaryView>,
}

fn header_view(header: &ArchetypeHeader) -> ArchetypeHeaderView {
    ArchetypeHeaderView {
        archetype: header.archetype.as_ref().map(header_archetype_view),
        deck_count: header.deck_count,
        record: record_view(&header.record),
        best: header.best.as_ref().map(summary_view),
        worst: header.worst.as_ref().map(summary_view),
    }
}

/// What every deck index page shows around its body (0.x
/// `GetDeckIndexSharedProps`): the sidebar and the header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeckIndexShellView {
    pub format_options: Vec<FormatOptionView>,
    pub archetype_options: Vec<DeckArchetypeOptionView>,
    pub unclassified_count: u32,
    pub archetype_header: Option<ArchetypeHeaderView>,
}

/// One opponent archetype of a deck's Matchups (0.x
/// `GetArchetypeMatchupSpread` row).
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchupView {
    #[specta(type = specta_typescript::Number)]
    pub archetype_id: i64,
    pub name: String,
    pub colors: Vec<String>,
    pub match_win_rate: u32,
    pub game_win_rate: u32,
    pub matches: u32,
    /// `W - L` or `W - L - D`.
    pub match_record: String,
    /// `W - L` of games.
    pub game_record: String,
    pub match_wins: u32,
    pub match_losses: u32,
    pub match_draws: u32,
    pub games_won: u32,
    pub games_lost: u32,
    pub total_games: u32,
    pub otp_win_rate: u32,
    /// One decimal; `null` without turn counts.
    pub avg_turns: Option<f64>,
}

fn games_label(won: u32, lost: u32) -> String {
    format!("{won} - {lost}")
}

pub fn matchup_view(row: &MatchupRow) -> MatchupView {
    MatchupView {
        archetype_id: row.archetype_id,
        name: row.name.clone(),
        colors: archetype_colors(row.color_identity.as_deref()),
        match_win_rate: row.record.win_rate,
        game_win_rate: row.games.win_rate,
        matches: row.record.total,
        match_record: decks::stats::record_label(&row.record),
        game_record: games_label(row.games.won, row.games.lost),
        match_wins: row.record.wins,
        match_losses: row.record.losses,
        match_draws: row.record.draws,
        games_won: row.games.won,
        games_lost: row.games.lost,
        total_games: row.total_games,
        otp_win_rate: row.otp_win_rate,
        avg_turns: row.avg_turns,
    }
}

/// One game number's games (0.x `perGameWinrates` row).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PerGameView {
    pub game_number: u32,
    pub win_rate: u32,
    pub record: String,
    pub wins: u32,
    pub losses: u32,
}

/// One match of the drawer's history (0.x `matchHistory` row).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchHistoryView {
    pub token: String,
    #[specta(type = specta_typescript::Number)]
    pub started_at_ms: i64,
    /// `Mar 5` (0.x `dateFormatted`).
    pub date_label: String,
    pub is_league: bool,
    pub opponent: Option<String>,
    /// `2-1`.
    pub score: String,
    pub outcome: String,
    /// Each game: won, lost, or neither (`null`).
    pub game_results: Vec<Option<bool>>,
}

/// The Matchups drawer (0.x `GetArchetypeMatchupDetail`).
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchupDetailView {
    pub match_win_rate: u32,
    pub game_win_rate: u32,
    pub match_record: String,
    pub game_record: String,
    pub matches: u32,
    pub per_game: Vec<PerGameView>,
    pub otp_win_rate: u32,
    pub otp_record: String,
    pub otd_win_rate: u32,
    pub otd_record: String,
    pub avg_turns: Option<f64>,
    pub avg_mulligans: Option<f64>,
    pub on_play_rate: u32,
    pub history: Vec<MatchHistoryView>,
}

pub fn detail_view(detail: &MatchupDetail) -> MatchupDetailView {
    MatchupDetailView {
        match_win_rate: detail.record.win_rate,
        game_win_rate: detail.games.win_rate,
        match_record: decks::stats::record_label(&detail.record),
        game_record: games_label(detail.games.won, detail.games.lost),
        matches: detail.record.total,
        per_game: detail
            .per_game
            .iter()
            .map(|game| PerGameView {
                game_number: game.game_number,
                win_rate: game.games.win_rate,
                record: games_label(game.games.won, game.games.lost),
                wins: game.games.won,
                losses: game.games.lost,
            })
            .collect(),
        otp_win_rate: detail.otp.win_rate,
        otp_record: games_label(detail.otp.won, detail.otp.lost),
        otd_win_rate: detail.otd.win_rate,
        otd_record: games_label(detail.otd.won, detail.otd.lost),
        avg_turns: detail.avg_turns,
        avg_mulligans: detail.avg_mulligans,
        on_play_rate: detail.on_play_rate,
        history: detail
            .history
            .iter()
            .map(|found| MatchHistoryView {
                token: found.token.clone(),
                started_at_ms: found.started_at_ms,
                date_label: found.date_label.clone(),
                is_league: found.is_league,
                opponent: found.opponent.clone(),
                score: found.score.clone(),
                outcome: found.outcome.clone(),
                game_results: found.game_results.clone(),
            })
            .collect(),
    }
}

/// One row of the Game Stats table (0.x `AggregateGameStats` row).
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct GameStatsRowView {
    /// `all_games`, `game_1`, `game_2`, `game_3`.
    pub group: String,
    /// `overall`, `play`, `draw`.
    pub split: String,
    pub wins: u32,
    pub losses: u32,
    /// Percent, one decimal; `null` without games.
    pub win_rate: Option<f64>,
    /// Per game, two decimals.
    pub mulligans: Option<f64>,
    pub opponent_mulligans: Option<f64>,
    pub turns: Option<f64>,
}

/// An opponent archetype the Game Stats filter offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct GameStatsOpponentView {
    pub uuid: String,
    pub name: String,
    pub colors: Vec<String>,
}

/// The Game Stats page (0.x `stats`).
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct GameStatsView {
    pub rows: Vec<GameStatsRowView>,
    pub opponents: Vec<GameStatsOpponentView>,
}

/// One local day with a match: a line of the dashboard's peer archetype
/// chart or of the archetype Trends tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PeerDayView {
    /// `YYYY-MM-DD`.
    pub date: String,
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
    /// The line's win rate trend after the day's last match.
    pub trend: Option<u32>,
}

/// The dashboard's peer archetype line (0.x `peerChart`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PeerChartView {
    pub archetype_name: String,
    pub deck_count: u32,
    pub days: Vec<PeerDayView>,
}

/// One deck's line on the archetype Trends tab: its record in the range
/// and its days with a match, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeckTrendView {
    #[specta(type = specta_typescript::Number)]
    pub deck_id: i64,
    /// The deck's place among all the archetype's decks in scope (by id),
    /// so its line style does not change with the timeframe.
    pub slot: u32,
    pub name: String,
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
    pub days: Vec<PeerDayView>,
}

/// The archetype Trends tab: the Overall line and a line per deck.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ArchetypeTrendsView {
    /// Every deck's matches together, days with a match, oldest first.
    pub overall: Vec<PeerDayView>,
    pub decks: Vec<DeckTrendView>,
}

fn peer_view(chart: PeerChart) -> PeerChartView {
    PeerChartView {
        archetype_name: chart.archetype_name,
        deck_count: chart.deck_count,
        days: chart
            .days
            .into_iter()
            .map(|day| PeerDayView {
                date: day.date,
                wins: day.record.wins,
                losses: day.record.losses,
                draws: day.record.draws,
                trend: day.trend,
            })
            .collect(),
    }
}

/// A match's deck, for the archetype Matches tab's Deck column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RowDeckView {
    pub token: String,
    #[specta(type = specta_typescript::Number)]
    pub deck_id: i64,
    pub deck_name: String,
}

/// The archetype Matches tab (0.x `decks/archetypes/Matches`).
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ArchetypeMatchesView {
    pub page: MatchPage,
    /// Each row's deck.
    pub decks: Vec<RowDeckView>,
}

// ---- Reads ------------------------------------------------------------------

/// The deck index's archetype filter as 0.x reads it: `none` for
/// Unclassified, digits for one archetype, anything else for none.
pub fn archetype_filter(text: Option<&str>) -> Option<DeckArchetypeFilter> {
    let text = text?;
    if text == "none" {
        return Some(DeckArchetypeFilter::Unclassified);
    }
    if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) {
        return text.parse().ok().map(DeckArchetypeFilter::Is);
    }
    None
}

/// The filter's URL value (`none` or the id).
pub fn filter_key(filter: Option<DeckArchetypeFilter>) -> Option<String> {
    match filter? {
        DeckArchetypeFilter::Unclassified => Some("none".to_string()),
        DeckArchetypeFilter::Is(id) => Some(id.to_string()),
    }
}

/// The sidebar and header of the deck index over `scope` (0.x
/// `GetDeckIndexSharedProps`). The format options come from the deck list.
/// The header counts the matches in `range`; the sidebar stays all time.
pub fn index_shell(
    conn: &Connection,
    scope: &DeckScope,
    filter: Option<DeckArchetypeFilter>,
    range: Range,
) -> Result<DeckIndexShellView, CommandError> {
    let archetype_options = deck_index::archetype_options(conn, scope).map_err(storage)?;
    let unclassified_count = deck_index::unclassified_count(conn, scope).map_err(storage)?;
    let archetype_header = match filter {
        Some(filter) => deck_index::archetype_header(conn, scope, filter, range)
            .map_err(storage)?
            .as_ref()
            .map(header_view),
        None => None,
    };
    Ok(DeckIndexShellView {
        format_options: Vec::new(),
        archetype_options: archetype_options.iter().map(option_view).collect(),
        unclassified_count,
        archetype_header,
    })
}

/// The deck index scope of the current account.
fn scope_of(
    conn: &Connection,
    format: Option<&str>,
    hide_archived: bool,
) -> Result<DeckScope, CommandError> {
    let scope = page_scope(conn).map_err(storage)?;
    Ok(DeckScope {
        account_id: scope.account,
        platform: scope.platform,
        format: format
            .map(str::trim)
            .filter(|code| !code.is_empty())
            .map(str::to_string),
        hide_archived,
    })
}

/// The shell of an archetype tab: the sidebar (with the format options)
/// and the archetype's header; no header means no deck of it in scope, and
/// a fallback's header sends the page back to the grid (0.x
/// `ArchetypeTabController`). The header follows the tab's `timeframe`.
pub fn archetype_shell(
    conn: &Connection,
    archetype_id: i64,
    format: Option<&str>,
    hide_archived: bool,
    timeframe: &str,
    now_ms: i64,
    tz: Tz,
) -> Result<DeckIndexShellView, CommandError> {
    let scope = scope_of(conn, format, hide_archived)?;
    let (range, _previous) = timeframe_ranges(timeframe, now_ms, tz);
    let mut shell = index_shell(
        conn,
        &scope,
        Some(DeckArchetypeFilter::Is(archetype_id)),
        range,
    )?;
    let options = store::deck_pages::format_options(
        conn,
        scope.account_id,
        scope.platform,
        i64::from(hide_archived),
    )
    .map_err(storage)?;
    shell.format_options = options
        .into_iter()
        .map(|option| FormatOptionView {
            code: option.code,
            label: option.label,
            count: option.count,
        })
        .collect();
    Ok(shell)
}

/// The archetype Trends tab: each deck's results by local day in the
/// timeframe.
pub fn archetype_trends(
    conn: &Connection,
    archetype_id: i64,
    format: Option<&str>,
    hide_archived: bool,
    timeframe: &str,
    now_ms: i64,
    tz: Tz,
) -> Result<ArchetypeTrendsView, CommandError> {
    let scope = scope_of(conn, format, hide_archived)?;
    let (range, _previous) = timeframe_ranges(timeframe, now_ms, tz);
    let trends =
        deck_index::archetype_trends(conn, &scope, archetype_id, range, tz).map_err(storage)?;
    let day_view = |day: deck_index::TrendDay| PeerDayView {
        date: day.date,
        wins: day.record.wins,
        losses: day.record.losses,
        draws: day.record.draws,
        trend: day.trend,
    };
    let decks = trends
        .decks
        .into_iter()
        .map(|trend| DeckTrendView {
            deck_id: trend.deck_id,
            slot: trend.slot,
            name: trend.deck_name,
            wins: trend.record.wins,
            losses: trend.record.losses,
            draws: trend.record.draws,
            days: trend.days.into_iter().map(day_view).collect(),
        })
        .collect();
    Ok(ArchetypeTrendsView {
        overall: trends.overall.into_iter().map(day_view).collect(),
        decks,
    })
}

/// A deck's matchups in a timeframe (0.x `MatchupsController`).
pub fn matchups(
    conn: &Connection,
    deck_id: i64,
    version: Option<i64>,
    timeframe: &str,
    now_ms: i64,
    tz: Tz,
    best_of: Option<store::BestOf>,
) -> Result<Vec<MatchupView>, CommandError> {
    let (range, _previous) = timeframe_ranges(timeframe, now_ms, tz);
    spread_views(conn, deck_id, version, range, best_of)
}

/// A deck's matchups in `range`, over the matches of `best_of` (every match
/// when `None`).
pub fn spread_views(
    conn: &Connection,
    deck_id: i64,
    version: Option<i64>,
    range: Range,
    best_of: Option<store::BestOf>,
) -> Result<Vec<MatchupView>, CommandError> {
    let versions = matchups::deck_version_ids(conn, deck_id, version).map_err(storage)?;
    let rows = matchups::matchup_spread(conn, &versions, range, best_of).map_err(storage)?;
    Ok(rows.iter().map(matchup_view).collect())
}

/// One archetype's matchup in a timeframe (0.x `MatchupDetailController`).
#[allow(clippy::too_many_arguments)]
pub fn matchup_detail(
    conn: &Connection,
    deck_id: i64,
    archetype_id: i64,
    version: Option<i64>,
    timeframe: &str,
    now_ms: i64,
    tz: Tz,
    best_of: Option<store::BestOf>,
) -> Result<MatchupDetailView, CommandError> {
    let (range, _previous) = timeframe_ranges(timeframe, now_ms, tz);
    let versions = matchups::deck_version_ids(conn, deck_id, version).map_err(storage)?;
    let detail = matchups::matchup_detail(conn, &versions, archetype_id, range, tz, best_of)
        .map_err(storage)?;
    Ok(detail_view(&detail))
}

/// The Game Stats page (0.x `GameStatsController`): every version, the
/// timeframe, one opponent archetype by uuid or all.
pub fn game_stats(
    conn: &Connection,
    deck_id: i64,
    timeframe: &str,
    opponent: Option<&str>,
    now_ms: i64,
    tz: Tz,
) -> Result<GameStatsView, CommandError> {
    let (range, _previous) = timeframe_ranges(timeframe, now_ms, tz);
    let opponent = opponent.filter(|uuid| !uuid.is_empty());
    let rows = store::game_stats::game_stats(conn, deck_id, range, opponent).map_err(storage)?;
    let opponents = store::game_stats::game_stats_opponents(conn, deck_id).map_err(storage)?;
    Ok(GameStatsView {
        rows: rows
            .into_iter()
            .map(|row| GameStatsRowView {
                group: row.group.key().to_string(),
                split: row.split.key().to_string(),
                wins: row.wins,
                losses: row.losses,
                win_rate: row.win_rate,
                mulligans: row.mulligans,
                opponent_mulligans: row.opponent_mulligans,
                turns: row.turns,
            })
            .collect(),
        opponents: opponents
            .into_iter()
            .map(|option| GameStatsOpponentView {
                colors: archetype_colors(option.color_identity.as_deref()),
                uuid: option.uuid,
                name: option.name,
            })
            .collect(),
    })
}

/// The dashboard's peer chart (0.x `GetPeerArchetypeChartData`).
pub(crate) fn peer_chart_view(
    conn: &Connection,
    deck_id: i64,
    scope: PageScope,
    range: Range,
    tz: Tz,
    best_of: Option<store::BestOf>,
) -> Result<Option<PeerChartView>, CommandError> {
    let chart = matchups::peer_chart(
        conn,
        deck_id,
        scope.account,
        scope.platform,
        range,
        tz,
        best_of,
    )
    .map_err(storage)?;
    Ok(chart.map(peer_view))
}

/// The archetype's deck versions in scope.
fn tab_versions(
    conn: &Connection,
    archetype_id: i64,
    format: Option<&str>,
    hide_archived: bool,
) -> Result<Vec<i64>, CommandError> {
    let scope = scope_of(conn, format, hide_archived)?;
    deck_index::archetype_version_ids(conn, &scope, archetype_id).map_err(storage)
}

/// The archetype Matches tab: the complete matches of every deck of the
/// archetype in scope, through the list's filters (0.x
/// `Decks\Archetypes\MatchesController`), with each row's deck.
#[allow(clippy::too_many_arguments)]
pub fn archetype_matches(
    conn: &Connection,
    archetype_id: i64,
    format: Option<&str>,
    hide_archived: bool,
    timeframe: &str,
    query: &MatchListQuery,
    now_ms: i64,
    tz: Tz,
) -> Result<ArchetypeMatchesView, CommandError> {
    let versions = tab_versions(conn, archetype_id, format, hide_archived)?;
    let (range, _previous) = timeframe_ranges(timeframe, now_ms, tz);
    let filter = match_filter(query, range, tz)?;
    let scope = Scope::Versions(versions);
    let found = match_list(conn, scope.clone(), &filter, query.page, PER_PAGE).map_err(storage)?;
    let tokens: Vec<String> = found.rows.iter().map(|row| row.token.clone()).collect();
    let archetype_format = store::archetypes::archetype(conn, archetype_id)
        .map_err(storage)?
        .and_then(|row| row.format);
    let page = crate::matches::with_list_archetypes(
        conn,
        page_view(found),
        scope,
        range,
        archetype_format.as_deref(),
    )?;
    let decks = deck_index::decks_of_matches(conn, &tokens)
        .map_err(storage)?
        .into_iter()
        .map(|row| RowDeckView {
            token: row.token,
            deck_id: row.deck_id,
            deck_name: row.deck_name,
        })
        .collect();
    Ok(ArchetypeMatchesView { page, decks })
}

/// The archetype Matchups tab (0.x `Decks\Archetypes\MatchupsController`).
pub fn archetype_matchups(
    conn: &Connection,
    archetype_id: i64,
    format: Option<&str>,
    hide_archived: bool,
    timeframe: &str,
    now_ms: i64,
    tz: Tz,
) -> Result<Vec<MatchupView>, CommandError> {
    let versions = tab_versions(conn, archetype_id, format, hide_archived)?;
    let (range, _previous) = timeframe_ranges(timeframe, now_ms, tz);
    let rows = matchups::matchup_spread(conn, &versions, range, None).map_err(storage)?;
    Ok(rows.iter().map(matchup_view).collect())
}

/// The archetype Card stats tab: Plan S's card stats over every version of
/// the archetype's decks (0.x `Decks\Archetypes\CardStatsController`).
#[allow(clippy::too_many_arguments)]
pub fn archetype_card_stats(
    conn: &Connection,
    archetype_id: i64,
    format: Option<&str>,
    hide_archived: bool,
    query: &CardStatsQuery,
    trust: u32,
    now_ms: i64,
    tz: Tz,
) -> Result<CardStatsPayload, CommandError> {
    let versions = tab_versions(conn, archetype_id, format, hide_archived)?;
    crate::cards::stats::versions_payload(conn, &versions, query, trust, now_ms, tz)
}

// ---- Edits ------------------------------------------------------------------

/// Laravel's message for the `index`th deck id (`deck_ids.N`).
fn deck_id_message(index: usize, rule: &str) -> String {
    match rule {
        "distinct" => format!("The deck ids.{index} field has a duplicate value."),
        _ => format!("The selected deck ids.{index} is invalid."),
    }
}

/// Set or clear the archetype of several decks (0.x
/// `BulkUpdateDeckArchetypeController`): the ids are required, distinct and
/// known; decks of another account are left alone; archived decks are
/// included; the store refuses mixed formats and an archetype of another
/// format (a fallback fits any). Through the tracker
/// (`ArchetypeEdit::SetDeckArchetype`).
pub fn set_archetype_of_decks(
    state: &AppState,
    deck_ids: &[u32],
    archetype_id: Option<u32>,
) -> Result<(), CommandError> {
    let decks = checked_decks(&lock(&state.reader), deck_ids)?;
    let ids = decks.iter().map(|(id, _)| *id).collect();
    send(state, ids, archetype_id)
}

/// The bulk assign's decks as 0.x validates them (`deck_ids` required,
/// distinct, existing), with their formats; decks of another account than
/// the current one are dropped (0.x `forActiveAccount`).
pub fn checked_decks(
    conn: &Connection,
    deck_ids: &[u32],
) -> Result<Vec<(i64, String)>, CommandError> {
    if deck_ids.is_empty() {
        return Err(CommandError::Refused(
            "The deck ids field is required.".to_string(),
        ));
    }
    for (index, id) in deck_ids.iter().enumerate() {
        if deck_ids.iter().filter(|other| *other == id).count() > 1 {
            return Err(CommandError::Refused(deck_id_message(index, "distinct")));
        }
    }
    let account = page_scope(conn).map_err(storage)?.account;
    let mut mine = Vec::new();
    for (index, id) in deck_ids.iter().enumerate() {
        let deck = store::decks::deck_by_id(conn, i64::from(*id)).map_err(storage)?;
        let Some(deck) = deck else {
            return Err(CommandError::Refused(deck_id_message(index, "exists")));
        };
        if account.is_none() || deck.account_id == account {
            mine.push((deck.id, deck.format));
        }
    }
    Ok(mine)
}

/// Set or clear one deck's archetype from its Settings page (0.x
/// `UpdateDeckArchetypeController`): an archived deck is read-only.
pub fn set_archetype_of_deck(
    state: &AppState,
    deck_id: i64,
    archetype_id: Option<u32>,
) -> Result<(), CommandError> {
    let deck = store::decks::deck_by_id(&lock(&state.reader), deck_id).map_err(storage)?;
    let Some(deck) = deck else {
        return Err(CommandError::Refused(
            "This deck no longer exists.".to_string(),
        ));
    };
    if deck.archived_at.is_some() {
        return Err(CommandError::Refused(
            store::decks::ARCHIVED_DECK.to_string(),
        ));
    }
    send(state, vec![deck_id], archetype_id)
}

fn send(
    state: &AppState,
    deck_ids: Vec<i64>,
    archetype_id: Option<u32>,
) -> Result<(), CommandError> {
    let outcome = crate::matches::archetype_editor(state)?
        .edit(ArchetypeEdit::SetDeckArchetype {
            deck_ids,
            archetype_id: archetype_id.map(i64::from),
        })
        .map_err(CommandError::Tracker)?;
    match outcome.refusal() {
        Some(message) => Err(CommandError::Refused(message.to_string())),
        None => Ok(()),
    }
}

// ---- Commands ---------------------------------------------------------------

/// The deck index's bulk "Assign archetype" (`null` clears). Async so the
/// wait for the tracker never blocks the window.
#[tauri::command]
#[specta::specta]
pub async fn set_deck_archetype(
    app: AppHandle,
    deck_ids: Vec<u32>,
    archetype_id: Option<u32>,
) -> Result<(), CommandError> {
    let _timer = Timer::start("set_deck_archetype");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        set_archetype_of_decks(ready(&startup)?, &deck_ids, archetype_id)
    })
    .await
}

/// The deck Settings page's archetype picker (`null` clears).
#[tauri::command]
#[specta::specta]
pub async fn update_deck_archetype(
    app: AppHandle,
    id: u32,
    archetype_id: Option<u32>,
) -> Result<(), CommandError> {
    let _timer = Timer::start("update_deck_archetype");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        set_archetype_of_deck(ready(&startup)?, i64::from(id), archetype_id)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub fn deck_matchups(
    id: u32,
    version: Option<u32>,
    timeframe: String,
    best_of: Option<BestOfView>,
    startup: State<'_, Startup>,
) -> Result<Vec<MatchupView>, CommandError> {
    let _timer = Timer::start("deck_matchups");
    let state = ready(&startup)?;
    let conn = lock(&state.reader);
    let kind = deck_best_of(&conn, i64::from(id), best_of)?;
    matchups(
        &conn,
        i64::from(id),
        version.map(i64::from),
        &timeframe,
        crate::wall_clock_ms(),
        state.timezone,
        kind,
    )
}

#[tauri::command]
#[specta::specta]
pub fn deck_matchup_detail(
    id: u32,
    archetype_id: u32,
    version: Option<u32>,
    timeframe: String,
    best_of: Option<BestOfView>,
    startup: State<'_, Startup>,
) -> Result<MatchupDetailView, CommandError> {
    let _timer = Timer::start("deck_matchup_detail");
    let state = ready(&startup)?;
    let conn = lock(&state.reader);
    let kind = deck_best_of(&conn, i64::from(id), best_of)?;
    matchup_detail(
        &conn,
        i64::from(id),
        i64::from(archetype_id),
        version.map(i64::from),
        &timeframe,
        crate::wall_clock_ms(),
        state.timezone,
        kind,
    )
}

/// `opponent`: an archetype uuid, or `null` for every opponent.
#[tauri::command]
#[specta::specta]
pub fn deck_game_stats(
    id: u32,
    timeframe: String,
    opponent: Option<String>,
    startup: State<'_, Startup>,
) -> Result<GameStatsView, CommandError> {
    let _timer = Timer::start("deck_game_stats");
    let state = ready(&startup)?;
    game_stats(
        &lock(&state.reader),
        i64::from(id),
        &timeframe,
        opponent.as_deref(),
        crate::wall_clock_ms(),
        state.timezone,
    )
}

/// Whether the deck index hides archived decks (the saved setting).
fn hides_archived(state: &AppState) -> Result<bool, CommandError> {
    Ok(crate::commands::read_settings(state)?.hide_archived_decks)
}

/// The sidebar and header around an archetype tab. `format`: the deck
/// index's format filter; `timeframe`: the tab's range for the header.
#[tauri::command]
#[specta::specta]
pub fn deck_archetype_shell(
    archetype_id: u32,
    format: Option<String>,
    timeframe: String,
    startup: State<'_, Startup>,
) -> Result<DeckIndexShellView, CommandError> {
    let _timer = Timer::start("deck_archetype_shell");
    let state = ready(&startup)?;
    let hide = hides_archived(state)?;
    archetype_shell(
        &lock(&state.reader),
        i64::from(archetype_id),
        format.as_deref(),
        hide,
        &timeframe,
        crate::wall_clock_ms(),
        state.timezone,
    )
}

#[tauri::command]
#[specta::specta]
pub fn archetype_deck_matches(
    archetype_id: u32,
    format: Option<String>,
    timeframe: String,
    query: MatchListQuery,
    startup: State<'_, Startup>,
) -> Result<ArchetypeMatchesView, CommandError> {
    let _timer = Timer::start("archetype_deck_matches");
    let state = ready(&startup)?;
    let hide = hides_archived(state)?;
    archetype_matches(
        &lock(&state.reader),
        i64::from(archetype_id),
        format.as_deref(),
        hide,
        &timeframe,
        &query,
        crate::wall_clock_ms(),
        state.timezone,
    )
}

/// The decks of the listed matches, for the Matches page's Deck column.
#[tauri::command]
#[specta::specta]
pub fn listed_match_decks(
    tokens: Vec<String>,
    startup: State<'_, Startup>,
) -> Result<Vec<RowDeckView>, CommandError> {
    let _timer = Timer::start("listed_match_decks");
    let state = ready(&startup)?;
    let found = deck_index::decks_of_matches(&lock(&state.reader), &tokens).map_err(storage)?;
    Ok(found
        .into_iter()
        .map(|row| RowDeckView {
            token: row.token,
            deck_id: row.deck_id,
            deck_name: row.deck_name,
        })
        .collect())
}

#[tauri::command]
#[specta::specta]
pub fn archetype_deck_matchups(
    archetype_id: u32,
    format: Option<String>,
    timeframe: String,
    startup: State<'_, Startup>,
) -> Result<Vec<MatchupView>, CommandError> {
    let _timer = Timer::start("archetype_deck_matchups");
    let state = ready(&startup)?;
    let hide = hides_archived(state)?;
    archetype_matchups(
        &lock(&state.reader),
        i64::from(archetype_id),
        format.as_deref(),
        hide,
        &timeframe,
        crate::wall_clock_ms(),
        state.timezone,
    )
}

#[tauri::command]
#[specta::specta]
pub fn archetype_deck_trends(
    archetype_id: u32,
    format: Option<String>,
    timeframe: String,
    startup: State<'_, Startup>,
) -> Result<ArchetypeTrendsView, CommandError> {
    let _timer = Timer::start("archetype_deck_trends");
    let state = ready(&startup)?;
    let hide = hides_archived(state)?;
    archetype_trends(
        &lock(&state.reader),
        i64::from(archetype_id),
        format.as_deref(),
        hide,
        &timeframe,
        crate::wall_clock_ms(),
        state.timezone,
    )
}

#[tauri::command]
#[specta::specta]
pub fn archetype_deck_card_stats(
    archetype_id: u32,
    format: Option<String>,
    filters: CardStatsQuery,
    startup: State<'_, Startup>,
) -> Result<CardStatsPayload, CommandError> {
    let _timer = Timer::start("archetype_deck_card_stats");
    let state = ready(&startup)?;
    let settings = crate::commands::read_settings(state)?;
    archetype_card_stats(
        &lock(&state.reader),
        i64::from(archetype_id),
        format.as_deref(),
        settings.hide_archived_decks,
        &filters,
        settings.card_stats_trust,
        crate::wall_clock_ms(),
        state.timezone,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::stats::{CardStatsQuery, Perspective};
    use crate::commands::tests::ready_state;
    use crate::matches::MatchListQuery;
    use fixture_tools::TestDir;

    /// 2026-07-01T12:00:00Z.
    const NOW: i64 = 1_782_907_200_000;
    const DAY: i64 = 86_400_000;
    const UTC: chrono_tz::Tz = chrono_tz::UTC;

    struct Seeded {
        mine: i64,
        opponent: i64,
        legacy: i64,
        decks: [i64; 2],
        unclassified: i64,
        matches: Vec<i64>,
    }

    fn insert(conn: &Connection, sql: &str, values: impl store::Params) -> i64 {
        conn.query_row(sql, values, |row| row.get(0)).expect(sql)
    }

    /// Account Qlocal with two Modern decks of archetype Qeldrazi tron (a
    /// complete won match each against Qrakdos, one game on the play, five
    /// turns) and an unclassified deck; a Legacy archetype nobody plays.
    fn seed(conn: &Connection) -> Seeded {
        let account = insert(
            conn,
            "INSERT INTO accounts (username, first_seen_at) VALUES ('Qlocal', 0) RETURNING id",
            [],
        );
        let archetype = |name: &str, format: &str, colors: &str| {
            insert(
                conn,
                "INSERT INTO archetypes (uuid, name, format, color_identity, source, created_at,
                                         updated_at)
                 VALUES ('q-' || ?1, ?1, ?2, ?3, 'manual', 0, 0) RETURNING id",
                (name, format, colors),
            )
        };
        let mine = archetype("Qeldrazi tron", "modern", "C");
        let opponent = archetype("Qrakdos", "modern", "BR");
        let legacy = archetype("Qlegacy", "legacy", "U");
        let deck = |n: i64, archetype: Option<i64>| {
            insert(
                conn,
                "INSERT INTO decks (mtgo_id, account_id, name, format, created_at, updated_at,
                                    source, archetype_id)
                 VALUES ('q-deck-' || ?1, ?2, 'Qdeck ' || ?1, 'CMODERN', 0, 0, 'file', ?3)
                 RETURNING id",
                (n, account, archetype),
            )
        };
        let decks = [deck(1, Some(mine)), deck(2, Some(mine))];
        let unclassified = deck(3, None);
        let mut matches = Vec::new();
        for (n, deck_id) in decks.iter().enumerate() {
            let version = insert(
                conn,
                "INSERT INTO deck_versions (deck_id, signature, modified_at)
                 VALUES (?1, 'SIG-' || ?1, 0) RETURNING id",
                [deck_id],
            );
            let id = insert(
                conn,
                "INSERT INTO matches (token, account_id, source, format, state, outcome,
                     started_at, last_activity_at, revision, derived_version, deck_version_id)
                 VALUES ('q-match-' || ?1, ?2, 'log', 'CMODERN', 'complete', 'win', ?3, ?3, 1, 1,
                         ?4)
                 RETURNING id",
                (n as i64, account, NOW - DAY, version),
            );
            conn.execute(
                "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
                 VALUES (?1, 1, ?2, 1.0)",
                (id, opponent),
            )
            .expect("seat");
            conn.execute(
                "INSERT INTO games (match_id, game_number, result, end_reason, on_play,
                                    local_mulligans, opp_mulligans, turn_count, started_at)
                 VALUES (?1, 1, 'win', 'win', 1, 0, 1, 5, ?2)",
                (id, NOW - DAY),
            )
            .expect("game");
            matches.push(id);
        }
        Seeded {
            mine,
            opponent,
            legacy,
            decks,
            unclassified,
            matches,
        }
    }

    fn database(label: &str) -> (TestDir, Connection, Seeded) {
        let dir = TestDir::new(label);
        let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        let seeded = seed(&conn);
        (dir, conn, seeded)
    }

    fn id(value: i64) -> u32 {
        u32::try_from(value).expect("id")
    }

    fn refused(message: &str) -> CommandError {
        CommandError::Refused(message.to_string())
    }

    #[test]
    fn the_archetype_filter_is_none_or_digits() {
        assert_eq!(
            archetype_filter(Some("none")),
            Some(DeckArchetypeFilter::Unclassified)
        );
        assert_eq!(
            archetype_filter(Some("12")),
            Some(DeckArchetypeFilter::Is(12))
        );
        assert_eq!(archetype_filter(Some("abc")), None);
        assert_eq!(archetype_filter(Some("recent")), None);
        assert_eq!(archetype_filter(Some("")), None);
        assert_eq!(archetype_filter(None), None);
        assert_eq!(
            filter_key(Some(DeckArchetypeFilter::Is(7))).as_deref(),
            Some("7")
        );
        assert_eq!(
            filter_key(Some(DeckArchetypeFilter::Unclassified)).as_deref(),
            Some("none")
        );
        assert_eq!(filter_key(None), None);
    }

    // 0.x GetDeckIndexSharedProps: the sidebar's archetype rows and
    // Unclassified count, the header of the chosen archetype, each deck's
    // archetype badge.
    #[test]
    fn the_deck_index_has_archetype_rows_a_header_and_badges() {
        let (_dir, conn, seeded) = database("deck-index-archetypes");
        let mut query = crate::decks::DeckListQuery {
            format: None,
            recent: false,
            search: None,
            hide_archived: true,
            sort: crate::decks::DeckSortKey::Name,
            page: 1,
            per_page: 12,
            archetype: Some(seeded.mine.to_string()),
        };
        let view = crate::decks::list(&conn, &query).expect("list");
        assert_eq!(view.total, 2);
        assert_eq!(
            view.archetype.as_deref(),
            Some(seeded.mine.to_string().as_str())
        );
        assert_eq!(view.unclassified_count, 1);
        assert_eq!(view.archetype_options.len(), 1);
        let row = &view.archetype_options[0];
        assert_eq!(
            (row.id, row.name.as_str(), row.deck_count),
            (seeded.mine, "Qeldrazi tron", 2)
        );
        assert_eq!(row.colors, ["C"]);
        assert_eq!(row.format.as_deref(), Some("modern"));
        assert_eq!(row.record.label, "2 - 0");
        let header = view.archetype_header.as_ref().expect("header");
        assert_eq!(header.deck_count, 2);
        assert_eq!(header.record.total, 2);
        let named = header.archetype.as_ref().expect("archetype");
        assert_eq!((named.id, named.is_fallback), (seeded.mine, false));
        assert_eq!((header.best.as_ref(), header.worst.as_ref()), (None, None));
        let badge = view.decks[0].archetype.as_ref().expect("badge");
        assert_eq!(
            (badge.id, badge.name.as_str()),
            (seeded.mine, "Qeldrazi tron")
        );

        query.archetype = Some("none".to_string());
        let view = crate::decks::list(&conn, &query).expect("list");
        assert_eq!(view.decks.len(), 1);
        assert_eq!(view.decks[0].id, seeded.unclassified);
        assert_eq!(view.decks[0].archetype, None);
        assert_eq!(view.archetype_header.expect("header").archetype, None);

        query.archetype = None;
        let view = crate::decks::list(&conn, &query).expect("list");
        assert_eq!(view.archetype_header, None);
        assert_eq!(view.archetype, None);

        let json = serde_json::to_value(header_json_probe(&conn, &seeded)).expect("json");
        assert_eq!(json["archetypeHeader"]["deckCount"], 2);
        assert_eq!(json["unclassifiedCount"], 1);
        assert_eq!(json["archetypeOptions"][0]["deckCount"], 2);
    }

    fn header_json_probe(conn: &Connection, seeded: &Seeded) -> DeckIndexShellView {
        archetype_shell(conn, seeded.mine, None, true, "alltime", NOW, UTC).expect("shell")
    }

    /// 0.x `BulkUpdateDeckArchetypeTest` (thirteen), through the tracker.
    #[test]
    fn bulk_deck_archetypes_follow_0x() {
        let dir = TestDir::new("deck-archetype-bulk");
        let (state, _events) = ready_state(&dir);
        let (seeded, pauper_deck, pauper, other_account_deck, homebrew) = {
            let conn = store::open_writer(&state.db_path).expect("writer");
            let seeded = seed(&conn);
            let pauper = insert(
                &conn,
                "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
                 VALUES ('q-pauper', 'Qpauper', 'pauper', 'manual', 0, 0) RETURNING id",
                [],
            );
            let pauper_deck = insert(
                &conn,
                "INSERT INTO decks (mtgo_id, account_id, name, format, created_at, updated_at,
                                    source)
                 SELECT 'q-deck-p', id, 'Qpauper deck', 'CPAUPER', 0, 0, 'file'
                 FROM accounts WHERE username = 'Qlocal' RETURNING id",
                [],
            );
            let other = insert(
                &conn,
                "INSERT INTO accounts (username, first_seen_at) VALUES ('Qother', 0) RETURNING id",
                [],
            );
            let other_account_deck = insert(
                &conn,
                "INSERT INTO decks (mtgo_id, account_id, name, format, created_at, updated_at,
                                    source)
                 VALUES ('q-deck-o', ?1, 'Qtheirs', 'CMODERN', 0, 0, 'file') RETURNING id",
                [other],
            );
            let homebrew = insert(
                &conn,
                "SELECT id FROM archetypes WHERE name = 'Homebrew'",
                [],
            );
            (seeded, pauper_deck, pauper, other_account_deck, homebrew)
        };
        let stored = |deck: i64| -> Option<i64> {
            lock(&state.reader)
                .query_row(
                    "SELECT archetype_id FROM decks WHERE id = ?1",
                    [deck],
                    |row| row.get(0),
                )
                .expect("read")
        };
        let [one, two] = seeded.decks;
        let three = seeded.unclassified;

        // "rejects an empty deck list".
        assert_eq!(
            set_archetype_of_decks(&state, &[], Some(id(seeded.mine))),
            Err(refused("The deck ids field is required."))
        );
        // "rejects duplicate deck ids".
        assert_eq!(
            set_archetype_of_decks(&state, &[id(one), id(one)], None),
            Err(refused("The deck ids.0 field has a duplicate value."))
        );
        // "rejects unknown deck and archetype ids".
        assert_eq!(
            set_archetype_of_decks(&state, &[999_999], None),
            Err(refused("The selected deck ids.0 is invalid."))
        );
        assert_eq!(
            set_archetype_of_decks(&state, &[id(three)], Some(999_999)),
            Err(refused("The selected archetype id is invalid."))
        );
        // "rejects decks that do not share a format".
        assert_eq!(
            set_archetype_of_decks(&state, &[id(three), id(pauper_deck)], Some(id(seeded.mine))),
            Err(refused("Selected decks must all be the same format."))
        );
        // "rejects an archetype from another format".
        assert_eq!(
            set_archetype_of_decks(&state, &[id(three)], Some(id(pauper))),
            Err(refused("That archetype belongs to a different format."))
        );
        assert_eq!((stored(three), stored(pauper_deck)), (None, None));

        // "assigns an archetype to several decks at once" and "is
        // idempotent".
        for _ in 0..2 {
            set_archetype_of_decks(
                &state,
                &[id(one), id(two), id(three)],
                Some(id(seeded.opponent)),
            )
            .expect("assign");
        }
        assert_eq!(
            [stored(one), stored(two), stored(three)],
            [Some(seeded.opponent); 3]
        );

        // "reassigns trashed decks alongside live ones".
        {
            let writer = store::open_writer(&state.db_path).expect("writer");
            writer
                .execute("UPDATE decks SET archived_at = 5 WHERE id = ?1", [two])
                .expect("archive");
        }
        set_archetype_of_decks(&state, &[id(one), id(two)], Some(id(seeded.mine))).expect("assign");
        assert_eq!([stored(one), stored(two)], [Some(seeded.mine); 2]);

        // "accepts a fallback archetype for any format".
        set_archetype_of_decks(&state, &[id(pauper_deck)], Some(id(homebrew))).expect("fallback");
        assert_eq!(stored(pauper_deck), Some(homebrew));

        // "clears archetypes when archetype_id is null" (omitted is the same
        // null here) and "clears archetypes across formats since
        // unclassified is format-agnostic".
        assert_eq!(
            set_archetype_of_decks(&state, &[id(one), id(pauper_deck)], None),
            Ok(())
        );
        assert_eq!([stored(one), stored(pauper_deck)], [None, None]);

        let _ = (seeded.legacy, other_account_deck);
    }

    // BulkUpdateDeckArchetypeTest "ignores decks that belong to another
    // account": with Qlocal current (the latest log file's player), Qother's
    // deck is dropped from the set.
    #[test]
    fn a_bulk_assign_leaves_other_accounts_decks_alone() {
        let (_dir, conn, seeded) = database("deck-archetype-accounts");
        let other = insert(
            &conn,
            "INSERT INTO accounts (username, first_seen_at) VALUES ('Qother', 0) RETURNING id",
            [],
        );
        let theirs = insert(
            &conn,
            "INSERT INTO decks (mtgo_id, account_id, name, format, created_at, updated_at, source)
             VALUES ('q-deck-o', ?1, 'Qtheirs', 'CMODERN', 0, 0, 'file') RETURNING id",
            [other],
        );
        conn.execute_batch(
            "INSERT INTO log_files (path, identity_hash, head_hash, head_len, day_anchor, timezone,
                                    local_username, clock_date, first_seen_at, last_seen_at)
             VALUES ('q.log', '00', '00', 0, '2026-07-01', 'UTC', 'Qlocal', '2026-07-01', 0, 10);
             UPDATE accounts SET last_login_at = 10 WHERE username = 'Qlocal';",
        )
        .expect("log file and Qlocal's Login");
        let decks = checked_decks(&conn, &[id(seeded.unclassified), id(theirs)]).expect("decks");
        assert_eq!(decks, [(seeded.unclassified, "CMODERN".to_string())]);
    }

    /// 0.x `DeckArchetypeTest` PATCH cases and `MutationGuardTest` "blocks
    /// updating archetype on a trashed deck".
    #[test]
    fn one_decks_archetype_is_set_cleared_and_refused_when_archived() {
        let dir = TestDir::new("deck-archetype-single");
        let (state, _events) = ready_state(&dir);
        let seeded = {
            let conn = store::open_writer(&state.db_path).expect("writer");
            seed(&conn)
        };
        let deck = seeded.unclassified;
        let stored = || -> Option<i64> {
            lock(&state.reader)
                .query_row(
                    "SELECT archetype_id FROM decks WHERE id = ?1",
                    [deck],
                    |row| row.get(0),
                )
                .expect("read")
        };
        // "can update deck archetype via PATCH endpoint".
        set_archetype_of_deck(&state, deck, Some(id(seeded.mine))).expect("set");
        assert_eq!(stored(), Some(seeded.mine));
        // "can clear deck archetype via PATCH endpoint".
        set_archetype_of_deck(&state, deck, None).expect("clear");
        assert_eq!(stored(), None);
        assert_eq!(
            set_archetype_of_deck(&state, deck, Some(999_999)),
            Err(refused("The selected archetype id is invalid."))
        );
        assert_eq!(
            set_archetype_of_deck(&state, 999_999, None),
            Err(refused("This deck no longer exists."))
        );
        // The mutation guard: an archived deck keeps its archetype.
        set_archetype_of_deck(&state, deck, Some(id(seeded.mine))).expect("set");
        {
            let writer = store::open_writer(&state.db_path).expect("writer");
            writer
                .execute("UPDATE decks SET archived_at = 5 WHERE id = ?1", [deck])
                .expect("archive");
        }
        assert_eq!(
            set_archetype_of_deck(&state, deck, Some(id(seeded.opponent))),
            Err(refused(store::decks::ARCHIVED_DECK))
        );
        assert_eq!(stored(), Some(seeded.mine));
    }

    // 0.x MatchupsController and MatchupDetailControllerTest ("returns
    // matchup detail as json", "accepts timeframe query parameter").
    #[test]
    fn the_matchups_page_and_drawer_answer_in_camel_case() {
        let (_dir, conn, seeded) = database("deck-matchups");
        let deck = seeded.decks[0];
        let spread = matchups(&conn, deck, None, "alltime", NOW, UTC, None).expect("spread");
        assert_eq!(spread.len(), 1);
        let row = &spread[0];
        assert_eq!(
            (row.archetype_id, row.name.as_str()),
            (seeded.opponent, "Qrakdos")
        );
        assert_eq!(row.colors, ["B", "R"]);
        assert_eq!(
            (row.matches, row.match_win_rate, row.match_record.as_str()),
            (1, 100, "1 - 0")
        );
        assert_eq!(
            (
                row.game_record.as_str(),
                row.game_win_rate,
                row.otp_win_rate
            ),
            ("1 - 0", 100, 100)
        );
        assert_eq!(row.avg_turns, Some(5.0));
        let json = serde_json::to_value(row).expect("json");
        for key in [
            "archetypeId",
            "name",
            "colors",
            "matchWinRate",
            "gameWinRate",
            "matches",
            "matchRecord",
            "gameRecord",
            "matchWins",
            "matchLosses",
            "matchDraws",
            "gamesWon",
            "gamesLost",
            "totalGames",
            "otpWinRate",
            "avgTurns",
        ] {
            assert!(json.get(key).is_some(), "{key}");
        }

        let detail = matchup_detail(
            &conn,
            deck,
            seeded.opponent,
            None,
            "alltime",
            NOW,
            UTC,
            None,
        )
        .expect("detail");
        assert_eq!((detail.match_win_rate, detail.matches), (100, 1));
        assert_eq!(detail.history.len(), 1);
        assert_eq!(detail.history[0].score, "1-0");
        let json = serde_json::to_value(&detail).expect("json");
        for key in [
            "matchWinRate",
            "gameWinRate",
            "matchRecord",
            "gameRecord",
            "matches",
            "perGame",
            "otpWinRate",
            "otpRecord",
            "otdWinRate",
            "otdRecord",
            "avgTurns",
            "avgMulligans",
            "onPlayRate",
            "history",
        ] {
            assert!(json.get(key).is_some(), "{key}");
        }

        // A week ago is outside "today": nothing to show.
        let empty = matchup_detail(
            &conn,
            deck,
            seeded.opponent,
            None,
            "week",
            NOW + 30 * DAY,
            UTC,
            None,
        )
        .expect("detail");
        assert_eq!(empty.matches, 0);
        assert_eq!(empty.match_record, "0 - 0");
    }

    // 0.x GameStatsControllerTest, three cases.
    #[test]
    fn the_game_stats_page_has_twelve_rows_and_the_archetypes_faced() {
        let (_dir, conn, seeded) = database("deck-game-stats");
        let deck = seeded.decks[0];
        let view = game_stats(&conn, deck, "alltime", None, NOW, UTC).expect("stats");
        assert_eq!(view.rows.len(), 12);
        assert_eq!(view.opponents.len(), 1);
        assert_eq!(view.opponents[0].name, "Qrakdos");
        assert_eq!(
            (view.rows[0].group.as_str(), view.rows[0].split.as_str()),
            ("all_games", "overall")
        );
        assert_eq!((view.rows[0].wins, view.rows[0].losses), (1, 0));
        assert_eq!(view.rows[0].win_rate, Some(100.0));
        assert_eq!(view.rows[0].opponent_mulligans, Some(1.0));

        let later = game_stats(&conn, deck, "week", None, NOW + 180 * DAY, UTC).expect("stats");
        assert_eq!((later.rows[0].wins, later.rows[0].losses), (0, 0));

        let uuid = view.opponents[0].uuid.clone();
        let filtered = game_stats(&conn, deck, "alltime", Some(&uuid), NOW, UTC).expect("stats");
        assert_eq!(filtered.rows[0].wins, 1);
        let other = game_stats(&conn, deck, "alltime", Some("q-unknown"), NOW, UTC).expect("stats");
        assert_eq!(other.rows[0].wins, 0);
    }

    // 0.x ArchetypeTabsTest: the shell, the Matches, Matchups and Card stats
    // tabs over every deck of the archetype; a fallback and an archetype
    // without decks have no tabs.
    #[test]
    fn the_archetype_tabs_read_every_deck_of_the_archetype() {
        let (_dir, conn, seeded) = database("deck-archetype-tabs");
        let shell =
            archetype_shell(&conn, seeded.mine, None, true, "alltime", NOW, UTC).expect("shell");
        let header = shell.archetype_header.as_ref().expect("header");
        assert_eq!(header.deck_count, 2);
        assert_eq!(header.archetype.as_ref().map(|a| a.id), Some(seeded.mine));
        assert!(!shell.format_options.is_empty());
        assert_eq!(shell.unclassified_count, 1);

        let ranged = archetype_shell(
            &conn,
            seeded.mine,
            None,
            true,
            "2026-06-30..2026-06-30",
            NOW,
            UTC,
        )
        .expect("shell");
        assert_eq!(
            ranged.archetype_header.as_ref().map(|h| h.record.total),
            Some(2)
        );
        let elsewhen = archetype_shell(
            &conn,
            seeded.mine,
            None,
            true,
            "2026-06-01..2026-06-10",
            NOW,
            UTC,
        )
        .expect("shell");
        let header = elsewhen.archetype_header.as_ref().expect("header");
        assert_eq!((header.deck_count, header.record.total), (2, 0));
        assert_eq!(elsewhen.archetype_options[0].record.total, 2);

        let trends = archetype_trends(
            &conn,
            seeded.mine,
            None,
            true,
            "2026-06-30..2026-06-30",
            NOW,
            UTC,
        )
        .expect("trends");
        assert_eq!(trends.decks.len(), 2);
        assert_eq!(trends.decks[0].days.len(), 1);
        assert_eq!(trends.decks[0].days[0].date, "2026-06-30");
        assert_eq!(trends.decks[0].wins, 1);
        assert_eq!(trends.overall.len(), 1);
        assert_eq!(trends.overall[0].date, "2026-06-30");
        assert!(trends.overall[0].trend.is_some());
        assert!(trends.decks[0].days[0].trend.is_some());
        let none = archetype_trends(
            &conn,
            seeded.mine,
            None,
            true,
            "2026-06-01..2026-06-10",
            NOW,
            UTC,
        )
        .expect("trends");
        assert!(none.decks.is_empty());
        assert!(none.overall.is_empty());

        let page = archetype_matches(
            &conn,
            seeded.mine,
            None,
            true,
            "alltime",
            &MatchListQuery::newest_first(1),
            NOW,
            UTC,
        )
        .expect("matches");
        assert_eq!(page.page.rows.len(), 2);
        assert_eq!(page.page.unknown_archetype_count, 0);
        assert!(
            page.decks
                .iter()
                .all(|row| seeded.decks.contains(&row.deck_id))
        );
        assert_eq!(page.decks.len(), 2);

        let spread = archetype_matchups(&conn, seeded.mine, None, true, "alltime", NOW, UTC)
            .expect("spread");
        assert_eq!(spread.len(), 1);
        assert_eq!(
            (spread[0].archetype_id, spread[0].matches),
            (seeded.opponent, 2)
        );

        let stats = archetype_card_stats(
            &conn,
            seeded.mine,
            None,
            true,
            &CardStatsQuery {
                timeframe: "alltime".to_string(),
                board: None,
                play_draw: None,
                archetype: None,
                perspective: Perspective::Mine,
                best_of: None,
            },
            50,
            NOW,
            UTC,
        )
        .expect("card stats");
        assert_eq!(stats.perspective, Perspective::Mine);
        assert_eq!(stats.deck_winrate.games, 2);
        assert_eq!(stats.trust, 50);

        // "redirects to the listing when the archetype has no decks in scope".
        let none =
            archetype_shell(&conn, seeded.legacy, None, true, "alltime", NOW, UTC).expect("shell");
        assert_eq!(none.archetype_header, None);
        // A fallback's header says so: the page sends the player to the grid.
        let homebrew = insert(
            &conn,
            "SELECT id FROM archetypes WHERE name = 'Homebrew'",
            [],
        );
        conn.execute(
            "UPDATE decks SET archetype_id = ?1 WHERE id = ?2",
            (homebrew, seeded.unclassified),
        )
        .expect("homebrew");
        let fallback =
            archetype_shell(&conn, homebrew, None, true, "alltime", NOW, UTC).expect("shell");
        let header = fallback.archetype_header.expect("header");
        assert!(header.archetype.expect("archetype").is_fallback);
        let _ = &seeded.matches;
    }

    // 0.x DashboardController: the spread for the best and worst matchups
    // and the peer archetype chart.
    #[test]
    fn the_dashboard_has_the_matchup_spread_and_the_peer_chart() {
        let (_dir, conn, seeded) = database("deck-dashboard-archetypes");
        let view = crate::decks::dashboard(&conn, seeded.decks[0], None, "alltime", NOW, UTC, None)
            .expect("view");
        assert_eq!(view.matchup_spread.len(), 1);
        let peer = view.peer_chart.expect("peer");
        assert_eq!(
            (peer.archetype_name.as_str(), peer.deck_count),
            ("Qeldrazi tron", 1)
        );
        assert_eq!(peer.days.len(), 1);
        assert_eq!(
            (peer.days[0].wins, peer.days[0].losses, peer.days[0].draws),
            (1, 0, 0)
        );
        assert_eq!(peer.days[0].trend, Some(100));
        let lone =
            crate::decks::dashboard(&conn, seeded.unclassified, None, "alltime", NOW, UTC, None)
                .expect("view");
        assert_eq!(lone.peer_chart, None);
        assert!(lone.matchup_spread.is_empty());
    }

    #[test]
    fn the_deck_dashboard_matchups_read_one_kind() {
        let (_dir, conn, seeded) = database("deck-dashboard-kinds");
        conn.execute_batch(
            "UPDATE accounts SET platform = 'arena', arena_user_id = 'Qarena-id';
             UPDATE matches SET game_structure = 'bo1';",
        )
        .expect("arena");
        let read = |best_of| {
            crate::decks::dashboard(&conn, seeded.decks[0], None, "alltime", NOW, UTC, best_of)
                .expect("view")
                .matchup_spread
                .len()
        };
        assert_eq!(read(Some(BestOfView::Bo1)), 1);
        assert_eq!(read(Some(BestOfView::Bo3)), 0);
        assert_eq!(read(None), 1);
    }
}
