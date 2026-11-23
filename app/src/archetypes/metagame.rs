//! The Metagame page and the archetype detail commands (archetypes spec
//! sections 9 and 10).
//!
//! The player's numbers are read at once; the website's global shares and
//! an archetype's live decklists come from the API on the `archetypes`
//! worker (`ArchetypeSignal::FetchMetagame`, `FetchDecklists`) and are kept
//! in `cache::ArchetypeCaches`. While the shares are on their way the page
//! says `Loading` and the worker pushes `MetagameSharesReady` when they
//! arrive; offline, for a format the website does not list, or when the
//! request fails, the global column is `Unavailable` (blank).

use archetypes::metagame::{
    SeenCard, Tile, Timeframe, TypicalCard, local_key_cards, mark_typical, tiles,
};
use archetypes::{Source, format_label};
use arena_match::rank_band::RankBand;
use decks::stats::{Range, record_label};
use mymtgo_api::{ArchetypeDeck, GlobalCard, GlobalShare, MetagameShares, format_slug};
use serde::Serialize;
use std::collections::HashMap;
use store::Connection;
use store::decks::CardRow;
use store::metagame::{
    ArchetypeDetail, local_month, metagame_window, month_window, typical_of_lists, window_days,
};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

use crate::api::session::ApiState;
use crate::archetypes::cache::{ArchetypeCaches, Cached, TimedCache, metagame_key};
use crate::archetypes::definitions::Pacer;
use crate::archetypes::manage::{ArchetypeSourceView, VariantView, form_view};
use crate::archetypes::worker::{ArchetypeSignal, STOPPED, signal};
use crate::bridge::lock;
use crate::commands::{CommandError, Startup, off_async_workers, ready, storage};
use crate::decks::{ListCardView, color_list};
use crate::logging::Timer;

/// The archetype detail's answer for a missing row (0.x's 404).
pub const ARCHETYPE_NOT_FOUND: &str = "The archetype was not found.";

// ---- Views -------------------------------------------------------------------

/// Where the global column stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum GlobalState {
    Ready,
    /// Asked for; `MetagameSharesReady` follows.
    Loading,
    /// Offline, a format the website does not list, or a failed request:
    /// the column is blank and the page says "Global shares need a
    /// connection.".
    Unavailable,
}

/// One format the page offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MetagameFormatView {
    pub key: String,
    pub label: String,
    pub matches: u32,
}

/// One timeframe of the switch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct TimeframeView {
    pub key: String,
    pub label: String,
}

/// One Arena month of the month switch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MetagameMonthView {
    /// `2026-11`.
    pub key: String,
    /// `November 2026`.
    pub label: String,
}

/// One Arena rank band tab: the matches of the month played at that rank.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MetagameBandView {
    /// A `RankBand` key (`gold`).
    pub key: String,
    pub label: String,
    pub matches: u32,
}

/// One tile of the page.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MetagameTileView {
    #[specta(type = specta_typescript::Number)]
    pub archetype_id: i64,
    pub name: String,
    /// WUBRG letters.
    pub colors: Vec<String>,
    pub source: ArchetypeSourceView,
    pub retired: bool,
    /// A server archetype held as Custom by the offline download: no badge.
    pub offline_held: bool,
    pub matches: u32,
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
    /// Match win rate, draws counted (0.x `MatchRecord`).
    pub win_rate: u32,
    /// `W - L` or `W - L - D`.
    pub record: String,
    /// The player's share, percent, one decimal.
    pub share: f64,
    /// False for manual, fallback and retired archetypes: "-".
    pub comparable: bool,
    pub global_share: Option<f64>,
    pub difference: Option<f64>,
    /// The cards fanned out on the tile: the website tile's, else up to
    /// four from the archetype's stored list.
    pub cards: Vec<TileCardView>,
}

/// One card on a tile. A website card has only the remote images; a card
/// from the stored list also has the downloaded files when there are any.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct TileCardView {
    pub name: String,
    pub image: Option<String>,
    pub local_image: Option<String>,
    /// The art crop, faded behind the tile.
    pub art: Option<String>,
    pub local_art: Option<String>,
}

/// The Metagame page.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MetagamePage {
    /// The format shown (an archetypes key), `None` without any played.
    pub format: Option<String>,
    pub format_label: Option<String>,
    pub formats: Vec<MetagameFormatView>,
    pub timeframe: String,
    pub timeframe_label: String,
    pub timeframes: Vec<TimeframeView>,
    /// First and last local day (`YYYY-MM-DD`); `None` for all time.
    pub range_from: Option<String>,
    pub range_to: Option<String>,
    /// The current account's name, for the shared header.
    pub account: Option<String>,
    /// Every complete match of the format in the window.
    pub total_matches: u32,
    pub tiles: Vec<MetagameTileView>,
    /// Matches without an opponent archetype (the footer line).
    pub without_archetype: u32,
    pub global_state: GlobalState,
    /// Arena: every month with a match of the format, and the current one,
    /// newest first. Empty on MTGO.
    pub months: Vec<MetagameMonthView>,
    /// Arena: the month shown (`2026-11`). `None` on MTGO.
    pub month: Option<String>,
    /// Arena: every rank band (Spark to Mythic, then Unranked) with its
    /// matches in the month. Empty on MTGO.
    pub bands: Vec<MetagameBandView>,
    /// Arena: the band shown (a `RankBand` key); `None` is All.
    pub band: Option<String>,
}

/// An archetype named on the detail (the merge notice).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ArchetypeLinkView {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    pub name: String,
}

/// One of the player's decks against the archetype.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeckAgainstView {
    #[specta(type = specta_typescript::Number)]
    pub deck_id: i64,
    pub name: String,
    pub archived: bool,
    pub colors: Vec<String>,
    pub matches: u32,
    pub match_win_rate: u32,
    pub match_record: String,
    pub game_win_rate: u32,
    pub game_record: String,
    pub otp_win_rate: u32,
    pub otp_record: String,
    pub otd_win_rate: u32,
    pub otd_record: String,
}

/// One card seen against the archetype.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SeenCardView {
    pub key: String,
    pub catalog_id: u32,
    pub name: String,
    pub matches: u32,
    pub most_copies: u32,
    /// False: listed under "Not in typical lists".
    pub in_typical: bool,
    /// The card image and its downloaded file, from the card data.
    pub image: Option<String>,
    pub local_image: Option<String>,
}

/// The archetype detail page.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ArchetypeDetailView {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    pub uuid: String,
    pub name: String,
    pub format: Option<String>,
    pub format_label: Option<String>,
    pub colors: Vec<String>,
    pub source: ArchetypeSourceView,
    pub retired: bool,
    /// A server archetype held as Custom by the offline download: no badge.
    pub offline_held: bool,
    pub incomplete: bool,
    pub merged_into: Option<ArchetypeLinkView>,
    pub merged_from: Vec<ArchetypeLinkView>,
    pub timeframe: String,
    pub timeframe_label: String,
    pub timeframes: Vec<TimeframeView>,
    pub matches: u32,
    pub match_win_rate: u32,
    pub match_record: String,
    pub decks: Vec<DeckAgainstView>,
    pub cards_seen: Vec<SeenCardView>,
    /// The stored lists: a downloaded archetype's key list, a manual
    /// archetype's variants.
    pub lists: Vec<VariantView>,
}

/// Where the live decklists stand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum DecklistsState {
    Ready,
    /// "Decklists need a connection." with the stored key list.
    Offline,
    /// The request failed; shown as offline.
    Unavailable,
    /// A manual archetype: its variants are its lists.
    Manual,
    /// A fallback: 0.x's "System fallback ..." text.
    Fallback,
}

/// One live decklist.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct TypicalListView {
    pub uuid: String,
    pub seen_count: u32,
    /// "Variant {n} · seen {count}×" (0.x's picker).
    pub label: String,
    pub cards: Vec<ListCardView>,
}

/// The typical lists of an archetype.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ArchetypeDecklistsView {
    pub state: DecklistsState,
    pub lists: Vec<TypicalListView>,
}

// ---- Rules -------------------------------------------------------------------

/// The timeframes of the switch.
pub fn timeframe_views() -> Vec<TimeframeView> {
    Timeframe::ALL
        .iter()
        .map(|timeframe| TimeframeView {
            key: timeframe.key().to_string(),
            label: timeframe.label().to_string(),
        })
        .collect()
}

/// What the global column has for a format's window.
#[derive(Debug, Clone, PartialEq)]
pub enum GlobalAnswer {
    Ready(Vec<GlobalShare>),
    Loading,
    Unavailable,
}

/// The global column for `format_key` over `days`: from the cache, or a
/// fetch to ask the worker for (`Some((slug, days))`, marked pending).
/// Offline, nothing is asked and the column is unavailable.
pub fn global_answer(
    offline: bool,
    format_key: &str,
    days: u32,
    cache: &mut TimedCache<MetagameShares>,
    now_ms: i64,
) -> (GlobalAnswer, Option<(String, u32)>) {
    if offline {
        return (GlobalAnswer::Unavailable, None);
    }
    let Some(slug) = format_slug(format_key) else {
        return (GlobalAnswer::Unavailable, None);
    };
    let key = metagame_key(slug, days);
    match cache.get(&key, now_ms) {
        Cached::Fresh(shares) => (GlobalAnswer::Ready(shares.archetypes.clone()), None),
        Cached::Failed => (GlobalAnswer::Unavailable, None),
        Cached::Pending => (GlobalAnswer::Loading, None),
        Cached::Missing => {
            cache.begin(&key);
            (GlobalAnswer::Loading, Some((slug.to_string(), days)))
        }
    }
}

/// The account's name.
fn account_name(conn: &Connection, account: Option<i64>) -> Result<Option<String>, CommandError> {
    let Some(id) = account else {
        return Ok(None);
    };
    store::metagame::account_name(conn, id).map_err(storage)
}

/// The page's window: MTGO's timeframe, or an Arena month.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetagameWindow {
    Timeframe(Timeframe),
    Month { year: i32, month: u32 },
}

/// A month key (`2026-11`) as its year and month; `None` when it is not one.
pub fn parse_month(key: &str) -> Option<(i32, u32)> {
    let (year, month) = key.split_once('-')?;
    let year: i32 = year.parse().ok()?;
    let month: u32 = month.parse().ok()?;
    if !(1..=12).contains(&month) {
        return None;
    }
    Some((year, month))
}

/// `2026-11`.
pub fn month_key(year: i32, month: u32) -> String {
    format!("{year:04}-{month:02}")
}

/// `November 2026`.
pub fn month_label(year: i32, month: u32) -> String {
    match chrono::NaiveDate::from_ymd_opt(year, month, 1) {
        Some(first) => first.format("%B %Y").to_string(),
        None => month_key(year, month),
    }
}

/// The command's window: on Arena a `month` that parses, else (and always
/// on MTGO) the `timeframe`.
pub fn page_window(
    conn: &Connection,
    timeframe: &str,
    month: Option<&str>,
) -> Result<MetagameWindow, CommandError> {
    let platform = store::platform::current_platform(conn).map_err(storage)?;
    let asked = match platform {
        store::Platform::Arena => month.and_then(parse_month),
        store::Platform::Mtgo | store::Platform::Endstep => None,
    };
    Ok(match asked {
        Some((year, month)) => MetagameWindow::Month { year, month },
        None => MetagameWindow::Timeframe(Timeframe::parse(timeframe)),
    })
}

/// What an Arena page reads besides MTGO's: the months, the month shown and
/// its window, the band tabs and the tokens of the band shown.
struct ArenaMonth {
    months: Vec<MetagameMonthView>,
    month: String,
    range: Range,
    bands: Vec<MetagameBandView>,
    /// The band shown; `None` for All.
    band: Option<RankBand>,
    /// The tokens of the band shown's matches; `None` for All.
    tokens: Option<Vec<String>>,
}

/// The Arena month and band of the page for the format `key` (`None`
/// without a played format: only the current month). The month is `asked`
/// when the format has matches in it, else the current month in `tz`. A
/// band without matches that month is All, as an unknown month is the
/// current one.
#[allow(clippy::too_many_arguments)]
fn arena_month(
    conn: &Connection,
    account: Option<i64>,
    key: Option<&str>,
    asked: Option<(i32, u32)>,
    band: Option<RankBand>,
    now_ms: i64,
    tz: chrono_tz::Tz,
) -> Result<ArenaMonth, CommandError> {
    let platform = store::Platform::Arena;
    let current = local_month(now_ms, tz);
    let mut listed = vec![current];
    if let Some(key) = key {
        for started in
            store::metagame::match_starts(conn, account, platform, key).map_err(storage)?
        {
            let month = local_month(started, tz);
            if !listed.contains(&month) {
                listed.push(month);
            }
        }
    }
    listed.sort_by(|a, b| b.cmp(a));
    let (year, month) = match asked {
        Some(asked) if listed.contains(&asked) => asked,
        _ => current,
    };
    let range = month_window(year, month, tz).unwrap_or_else(Range::all_time);

    let tokens = match key {
        Some(key) => {
            store::metagame::match_tokens(conn, account, platform, key, range).map_err(storage)?
        }
        None => Vec::new(),
    };
    let held = crate::rank::match_bands_of(conn, account, tz).map_err(storage)?;
    let mut bands = Vec::new();
    let mut chosen = None;
    for each in RankBand::ALL {
        let bucket: Vec<String> = tokens
            .iter()
            .filter(|token| held.get(*token).copied().unwrap_or(RankBand::Unranked) == each)
            .cloned()
            .collect();
        bands.push(MetagameBandView {
            key: each.key().to_string(),
            label: each.label().to_string(),
            matches: bucket.len() as u32,
        });
        if band == Some(each) && !bucket.is_empty() {
            chosen = Some((each, bucket));
        }
    }
    Ok(ArenaMonth {
        months: listed
            .iter()
            .map(|&(year, month)| MetagameMonthView {
                key: month_key(year, month),
                label: month_label(year, month),
            })
            .collect(),
        month: month_key(year, month),
        range,
        bands,
        band: chosen.as_ref().map(|(each, _)| *each),
        tokens: chosen.map(|(_, bucket)| bucket),
    })
}

/// The Metagame page for `format` (the most played when not played). MTGO
/// reads the window's timeframe (a month window reads the default one) and
/// `global` answers the column for the chosen format key. Arena reads the
/// window's month (the current one in `tz` when none, or one without
/// matches of the format) and the rank band `band` (all when `None`); the
/// website's shares are MTGO data, so its global column is unavailable and
/// `global` is not called.
pub fn metagame_page(
    conn: &Connection,
    format: Option<&str>,
    window: MetagameWindow,
    band: Option<RankBand>,
    now_ms: i64,
    tz: chrono_tz::Tz,
    global: &mut dyn FnMut(&str, u32) -> GlobalAnswer,
) -> Result<MetagamePage, CommandError> {
    let scope = crate::platform::page_scope(conn).map_err(storage)?;
    let account = scope.account;
    let arena = scope.platform == store::Platform::Arena;
    let formats =
        store::metagame::metagame_formats(conn, account, scope.platform).map_err(storage)?;
    let chosen = match format {
        Some(asked) if formats.iter().any(|option| option.key == asked) => Some(asked.to_string()),
        _ => formats.first().map(|option| option.key.clone()),
    };
    let (timeframe, asked_month) = match window {
        MetagameWindow::Timeframe(timeframe) => (timeframe, None),
        MetagameWindow::Month { year, month } => (Timeframe::parse(""), Some((year, month))),
    };
    let arena_month = if arena {
        Some(arena_month(
            conn,
            account,
            chosen.as_deref(),
            asked_month,
            band,
            now_ms,
            tz,
        )?)
    } else {
        None
    };
    let range = match &arena_month {
        Some(arena) => arena.range,
        None => metagame_window(timeframe, now_ms, tz),
    };
    let (range_from, range_to) = match window_days(range, tz) {
        Some((from, to)) => (Some(from), Some(to)),
        None => (None, None),
    };
    let mut page = MetagamePage {
        format: chosen.clone(),
        format_label: chosen.as_deref().map(format_label),
        formats: formats
            .iter()
            .map(|option| MetagameFormatView {
                key: option.key.clone(),
                label: option.label.clone(),
                matches: option.matches,
            })
            .collect(),
        timeframe: timeframe.key().to_string(),
        timeframe_label: timeframe.label().to_string(),
        timeframes: timeframe_views(),
        range_from,
        range_to,
        account: account_name(conn, account)?,
        total_matches: 0,
        tiles: Vec::new(),
        without_archetype: 0,
        global_state: GlobalState::Unavailable,
        months: Vec::new(),
        month: None,
        bands: Vec::new(),
        band: None,
    };
    let mut tokens = None;
    if let Some(arena) = arena_month {
        page.months = arena.months;
        page.month = Some(arena.month);
        page.bands = arena.bands;
        page.band = arena.band.map(|band| band.key().to_string());
        tokens = arena.tokens;
    }
    let Some(key) = chosen else {
        return Ok(page);
    };
    let data = store::metagame::metagame_data(
        conn,
        account,
        scope.platform,
        &key,
        range,
        tokens.as_deref(),
    )
    .map_err(storage)?;
    let answer = if arena {
        GlobalAnswer::Unavailable
    } else {
        global(&key, timeframe.global_days())
    };
    let (state, rows) = match answer {
        GlobalAnswer::Ready(rows) => (GlobalState::Ready, rows),
        GlobalAnswer::Loading => (GlobalState::Loading, Vec::new()),
        GlobalAnswer::Unavailable => (GlobalState::Unavailable, Vec::new()),
    };
    let shares: Vec<(String, Option<f64>)> = rows
        .iter()
        .map(|row| (row.uuid.clone(), row.share))
        .collect();
    let global = match state {
        GlobalState::Ready => Some(shares.as_slice()),
        _ => None,
    };
    let faced = tiles(data.faced, data.total, global);
    let mut cards = tile_cards(conn, &faced, &rows)?;
    page.total_matches = data.total;
    page.without_archetype = data.without_archetype;
    page.global_state = state;
    page.tiles = faced
        .into_iter()
        .map(|tile| MetagameTileView {
            cards: cards.remove(&tile.archetype_id).unwrap_or_default(),
            archetype_id: tile.archetype_id,
            name: tile.name,
            colors: color_list(tile.color_identity.as_deref()),
            source: tile.source.into(),
            retired: tile.retired,
            offline_held: tile.offline_held,
            matches: tile.record.total,
            wins: tile.record.wins,
            losses: tile.record.losses,
            draws: tile.record.draws,
            win_rate: tile.record.win_rate,
            record: record_label(&tile.record),
            share: tile.share,
            comparable: tile.comparable,
            global_share: tile.global_share,
            difference: tile.difference,
        })
        .collect();
    Ok(page)
}

/// Each tile's cards by archetype id: the website tile's (`rows`, by
/// uuid), else `local_key_cards` of the archetype's stored list.
fn tile_cards(
    conn: &Connection,
    faced: &[Tile],
    rows: &[GlobalShare],
) -> Result<HashMap<i64, Vec<TileCardView>>, CommandError> {
    let mut out = HashMap::new();
    let mut without = Vec::new();
    for tile in faced {
        let website = rows
            .iter()
            .find(|row| row.uuid == tile.uuid && !row.cards.is_empty());
        match website {
            Some(row) => {
                out.insert(
                    tile.archetype_id,
                    row.cards.iter().map(website_card).collect(),
                );
            }
            None => without.push(tile.archetype_id),
        }
    }
    let lists = store::metagame::tile_list_cards(conn, &without).map_err(storage)?;
    let mut picks = Vec::new();
    let mut ids = Vec::new();
    for archetype_id in without {
        let Some(list) = lists.get(&archetype_id) else {
            continue;
        };
        let picked: Vec<u32> = local_key_cards(list)
            .into_iter()
            .filter_map(|catalog_id| u32::try_from(catalog_id).ok())
            .collect();
        ids.extend(picked.iter().copied());
        picks.push((archetype_id, picked));
    }
    let known = store::decks::card_infos(conn, &ids).map_err(storage)?;
    for (archetype_id, picked) in picks {
        let views: Vec<TileCardView> = picked
            .iter()
            .filter_map(|catalog_id| known.get(catalog_id))
            .map(local_card)
            .collect();
        out.insert(archetype_id, views);
    }
    Ok(out)
}

/// A website tile card.
fn website_card(card: &GlobalCard) -> TileCardView {
    TileCardView {
        name: card.name.clone(),
        image: Some(card.image.clone()),
        local_image: None,
        art: card.art.clone(),
        local_art: None,
    }
}

/// A stored list's card, from its card row.
fn local_card(card: &CardRow) -> TileCardView {
    TileCardView {
        name: card.name.clone().unwrap_or_default(),
        image: card.image.clone(),
        local_image: card.local_image.clone(),
        art: card.art_crop.clone(),
        local_art: card.local_art_crop.clone(),
    }
}

/// The typical cards of live decklists.
pub fn typical_of_decks(decks: &[ArchetypeDeck]) -> Vec<TypicalCard> {
    let mut out = Vec::new();
    for deck in decks {
        for card in &deck.cards {
            out.push(TypicalCard {
                catalog_id: i64::from(card.mtgo_id),
                oracle_id: Some(card.oracle_id.clone()).filter(|oracle| !oracle.is_empty()),
            });
        }
    }
    out
}

/// A card seen, with its images from `known` (the card data by catalog id).
fn seen_view(card: SeenCard, known: &HashMap<u32, CardRow>) -> SeenCardView {
    let catalog_id = u32::try_from(card.catalog_id).unwrap_or(0);
    let info = known.get(&catalog_id);
    SeenCardView {
        key: card.key,
        catalog_id,
        name: card.name,
        matches: card.matches,
        most_copies: card.most_copies,
        in_typical: card.in_typical,
        image: info.and_then(|info| info.image.clone()),
        local_image: info.and_then(|info| info.local_image.clone()),
    }
}

fn link(option: &store::archetypes::ArchetypeOption) -> ArchetypeLinkView {
    ArchetypeLinkView {
        id: option.id,
        name: option.name.clone(),
    }
}

/// The detail page: the store's detail, with cards seen marked against the
/// live decklists when they are cached (`live`), else the stored lists.
pub fn detail_view(
    conn: &Connection,
    detail: ArchetypeDetail,
    timeframe: Timeframe,
    live: Option<&[ArchetypeDeck]>,
) -> Result<ArchetypeDetailView, CommandError> {
    let ArchetypeDetail {
        archetype,
        record,
        decks,
        mut seen,
        lists,
    } = detail;
    if let Some(live) = live {
        let mut typical = typical_of_decks(live);
        typical.extend(typical_of_lists(&lists));
        mark_typical(&mut seen, &typical);
    }
    let seen_ids: Vec<u32> = seen
        .iter()
        .filter_map(|card| u32::try_from(card.catalog_id).ok())
        .collect();
    let known = store::decks::card_infos(conn, &seen_ids).map_err(storage)?;
    let stored = form_view(conn, archetype.id)?
        .map(|form| form.variants)
        .unwrap_or_default();
    let decks = decks
        .into_iter()
        .map(|deck| {
            let detail = deck.detail;
            DeckAgainstView {
                deck_id: deck.deck_id,
                name: deck.name,
                archived: deck.archived,
                colors: color_list(deck.color_identity.as_deref()),
                matches: detail.record.total,
                match_win_rate: detail.record.win_rate,
                match_record: record_label(&detail.record),
                game_win_rate: detail.games.win_rate,
                game_record: format!("{} - {}", detail.games.won, detail.games.lost),
                otp_win_rate: detail.otp.win_rate,
                otp_record: format!("{} - {}", detail.otp.won, detail.otp.lost),
                otd_win_rate: detail.otd.win_rate,
                otd_record: format!("{} - {}", detail.otd.won, detail.otd.lost),
            }
        })
        .collect();
    Ok(ArchetypeDetailView {
        id: archetype.id,
        uuid: archetype.uuid,
        name: archetype.name,
        format_label: archetype.format.as_deref().map(format_label),
        format: archetype.format,
        colors: color_list(archetype.color_identity.as_deref()),
        source: archetype.source.into(),
        retired: archetype.retired_at.is_some(),
        offline_held: archetype.offline_held,
        incomplete: archetype.incomplete,
        merged_into: archetype.merged_into.as_ref().map(link),
        merged_from: archetype.merged_from.iter().map(link).collect(),
        timeframe: timeframe.key().to_string(),
        timeframe_label: timeframe.label().to_string(),
        timeframes: timeframe_views(),
        matches: record.total,
        match_win_rate: record.win_rate,
        match_record: record_label(&record),
        decks,
        cards_seen: seen
            .into_iter()
            .map(|card| seen_view(card, &known))
            .collect(),
        lists: stored,
    })
}

/// The live decklists as the page shows them: most seen first (0.x
/// `ShowTest` "returns multiple decks ordered by seen_count desc").
pub fn typical_lists(decks: &[ArchetypeDeck]) -> Vec<TypicalListView> {
    let mut sorted: Vec<&ArchetypeDeck> = decks.iter().collect();
    sorted.sort_by(|a, b| {
        b.seen_count
            .cmp(&a.seen_count)
            .then_with(|| a.uuid.cmp(&b.uuid))
    });
    sorted
        .into_iter()
        .enumerate()
        .map(|(index, deck)| TypicalListView {
            uuid: deck.uuid.clone(),
            seen_count: deck.seen_count,
            label: format!("Variant {} · seen {}×", index + 1, deck.seen_count),
            cards: deck
                .cards
                .iter()
                .map(|card| ListCardView {
                    catalog_id: card.mtgo_id,
                    name: card
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("Card {}", card.mtgo_id)),
                    type_line: card.card_type.clone(),
                    colors: color_list(card.color_identity.as_deref()),
                    image: card.image.clone(),
                    art_crop: None,
                    local_image: None,
                    local_art_crop: None,
                    cmc: None,
                    mana_cost: None,
                    quantity: card.quantity,
                    sideboard: card.sideboard,
                    known: card.name.is_some(),
                })
                .collect(),
        })
        .collect()
}

/// Fill what the API's decklist cards lack from the local card data: the
/// mana cost (the decklist tab draws it), the mana value and the local
/// images, and for a card the API could not name its name, type and
/// colours. Cards without local card data keep what the API sent.
pub fn with_local_cards(lists: &mut [TypicalListView], known: &HashMap<u32, CardRow>) {
    for list in lists.iter_mut() {
        for card in list.cards.iter_mut() {
            let Some(info) = known.get(&card.catalog_id) else {
                continue;
            };
            if !card.known {
                if let Some(name) = &info.name {
                    card.name = name.clone();
                    card.known = true;
                }
                card.type_line = info.card_type.clone();
                card.colors = color_list(info.color_identity.as_deref());
            }
            card.mana_cost = info.mana_cost.clone();
            card.cmc = info.cmc;
            card.local_image = info.local_image.clone();
            card.local_art_crop = info.local_art_crop.clone();
            if card.art_crop.is_none() {
                card.art_crop = info.art_crop.clone();
            }
            if card.image.is_none() {
                card.image = info.image.clone();
            }
        }
    }
}

/// `with_local_cards` with the card data read from the database.
fn fill_local_cards(
    conn: &Connection,
    view: &mut ArchetypeDecklistsView,
) -> Result<(), CommandError> {
    let mut ids = Vec::new();
    for list in &view.lists {
        for card in &list.cards {
            ids.push(card.catalog_id);
        }
    }
    if ids.is_empty() {
        return Ok(());
    }
    let known = store::decks::card_infos(conn, &ids).map_err(storage)?;
    with_local_cards(&mut view.lists, &known);
    Ok(())
}

/// Hand `request` the decklist's cards that have no local card row at all
/// (issue #326): cards the user never met, so nothing shows their mana
/// cost. The request gives them stubs through the tracker, the cards worker
/// looks them up like every other stub (the `/api/cards` passes, class
/// `Reference`), and its `CardsChanged` refetches the page, which then
/// reads their costs. Nothing is asked when every card has a row (a row the
/// worker could not resolve yet is already queued). Zero and implausible ids
/// are never asked about.
pub fn queue_unknown_cards(
    conn: &Connection,
    view: &ArchetypeDecklistsView,
    request: &mut dyn FnMut(Vec<u32>),
) -> Result<(), CommandError> {
    let mut ids = Vec::new();
    for list in &view.lists {
        for card in &list.cards {
            ids.push(card.catalog_id);
        }
    }
    let unknown = store::cards::without_rows(conn, &ids).map_err(storage)?;
    if !unknown.is_empty() {
        request(unknown);
    }
    Ok(())
}

/// Where a decklists answer stands before any request: the source decides,
/// then offline mode, then the cache. `None`: ask the worker.
pub fn decklists_before_request(
    source: Source,
    offline: bool,
    cached: Cached<Vec<ArchetypeDeck>>,
) -> Option<ArchetypeDecklistsView> {
    let state = match source {
        // A commander archetype has no lists, as a manual one without variants.
        Source::Manual | Source::Commander => DecklistsState::Manual,
        Source::Fallback => DecklistsState::Fallback,
        Source::Downloaded if offline => DecklistsState::Offline,
        Source::Downloaded => match cached {
            Cached::Fresh(decks) => {
                return Some(ArchetypeDecklistsView {
                    state: DecklistsState::Ready,
                    lists: typical_lists(&decks),
                });
            }
            Cached::Failed => DecklistsState::Unavailable,
            Cached::Pending | Cached::Missing => return None,
        },
    };
    Some(ArchetypeDecklistsView {
        state,
        lists: Vec::new(),
    })
}

// ---- Worker jobs ---------------------------------------------------------------

/// The website's shares for a format's window, paced like every archetype
/// request. Offline it fails before any request.
pub fn fetch_global(
    api: &ApiState,
    pacer: &mut Pacer,
    slug: &str,
    days: u32,
) -> Result<MetagameShares, String> {
    if api.offline_mode().0 {
        return Err("offline".to_string());
    }
    pacer.before_request();
    let answer = crate::api::reference::fetch_metagame(api, slug, days, pacer.now_ms());
    pacer.after_request();
    answer.map_err(|error| error.to_string())
}

/// An archetype's live decklists, paced. Offline it fails before any
/// request.
pub fn fetch_live_decklists(
    api: &ApiState,
    pacer: &mut Pacer,
    uuid: &str,
) -> Result<Vec<ArchetypeDeck>, String> {
    if api.offline_mode().0 {
        return Err("offline".to_string());
    }
    pacer.before_request();
    let answer = crate::api::reference::fetch_decklists(api, uuid, pacer.now_ms());
    pacer.after_request();
    answer.map_err(|error| error.to_string())
}

// ---- Commands ----------------------------------------------------------------

fn offline(app: &AppHandle) -> bool {
    app.try_state::<ApiState>()
        .is_none_or(|api| api.offline_mode().0)
}

/// The Metagame page (`format` an archetypes key or `null` for the most
/// played; `timeframe` `last30`, `last7` or `alltime`). On Arena, `month`
/// (`2026-11`, the current month when `null` or unknown) replaces the
/// timeframe and `rank` (a rank band key, all when `null`) picks a band tab;
/// MTGO ignores both.
#[tauri::command]
#[specta::specta]
pub async fn metagame(
    app: AppHandle,
    format: Option<String>,
    timeframe: String,
    month: Option<String>,
    rank: Option<String>,
) -> Result<MetagamePage, CommandError> {
    let _timer = Timer::start("metagame");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let now = crate::wall_clock_ms();
        let is_offline = offline(&app);
        let mut fetch: Option<(String, u32)> = None;
        let mut global = |key: &str, days: u32| match app.try_state::<ArchetypeCaches>() {
            Some(caches) => {
                let (answer, asked) =
                    global_answer(is_offline, key, days, &mut lock(&caches.metagame), now);
                fetch = asked;
                answer
            }
            None => GlobalAnswer::Unavailable,
        };
        let conn = lock(&state.reader);
        let window = page_window(&conn, &timeframe, month.as_deref())?;
        let page = metagame_page(
            &conn,
            format.as_deref(),
            window,
            rank.as_deref().and_then(RankBand::from_key),
            now,
            state.timezone,
            &mut global,
        )?;
        drop(conn);
        if let Some((slug, days)) = fetch {
            signal(&app, ArchetypeSignal::FetchMetagame { slug, days });
        }
        Ok(page)
    })
    .await
}

/// One archetype's detail; `null` when there is no such archetype.
#[tauri::command]
#[specta::specta]
pub async fn archetype_detail(
    app: AppHandle,
    id: u32,
    timeframe: String,
) -> Result<Option<ArchetypeDetailView>, CommandError> {
    let _timer = Timer::start("archetype_detail");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let timeframe = Timeframe::parse(&timeframe);
        let now = crate::wall_clock_ms();
        let conn = lock(&state.reader);
        let scope = crate::platform::page_scope(&conn).map_err(storage)?;
        let range = metagame_window(timeframe, now, state.timezone);
        let Some(detail) = store::metagame::archetype_detail(
            &conn,
            scope.account,
            scope.platform,
            i64::from(id),
            range,
            state.timezone,
        )
        .map_err(storage)?
        else {
            return Ok(None);
        };
        let live = match app.try_state::<ArchetypeCaches>() {
            Some(caches) => match lock(&caches.decklists).get(&detail.archetype.uuid, now) {
                Cached::Fresh(decks) => Some(decks),
                _ => None,
            },
            None => None,
        };
        detail_view(&conn, detail, timeframe, live.as_deref()).map(Some)
    })
    .await
}

/// An archetype's typical lists: live from the API (on the worker, cached
/// 10 minutes), or why there are none.
#[tauri::command]
#[specta::specta]
pub async fn archetype_decklists(
    app: AppHandle,
    id: u32,
) -> Result<ArchetypeDecklistsView, CommandError> {
    let _timer = Timer::start("archetype_decklists");
    off_async_workers(move || {
        let (uuid, source) = {
            let startup = app.state::<Startup>();
            let state = ready(&startup)?;
            let row = store::archetypes::archetype(&lock(&state.reader), i64::from(id))
                .map_err(storage)?
                .ok_or_else(|| CommandError::Refused(ARCHETYPE_NOT_FOUND.to_string()))?;
            (row.uuid, row.source)
        };
        let now = crate::wall_clock_ms();
        let cached = match app.try_state::<ArchetypeCaches>() {
            Some(caches) => lock(&caches.decklists).get(&uuid, now),
            None => Cached::Failed,
        };
        let mut view = match decklists_before_request(source, offline(&app), cached) {
            Some(view) => view,
            None => decklists_after_request(ask_decklists(&app, uuid)),
        };
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let mut unknown = Vec::new();
        {
            let conn = lock(&state.reader);
            fill_local_cards(&conn, &mut view)?;
            queue_unknown_cards(&conn, &view, &mut |ids| unknown = ids)?;
        }
        // Never waits: the page shows what is stored and refetches on
        // `CardsChanged` once the lookup lands.
        if !unknown.is_empty()
            && let Some(tracker) = lock(&state.tracker).as_ref()
            && !tracker.card_stub_scanner().request(unknown)
        {
            tracing::warn!(
                "the tracker is busy; the decklist's unknown cards are asked about next time"
            );
        }
        Ok(view)
    })
    .await
}

/// The worker's answer as the page shows it.
fn decklists_after_request(answer: Result<Vec<ArchetypeDeck>, String>) -> ArchetypeDecklistsView {
    match answer {
        Ok(decks) => ArchetypeDecklistsView {
            state: DecklistsState::Ready,
            lists: typical_lists(&decks),
        },
        Err(error) => {
            tracing::warn!(%error, "the archetype decklists could not be fetched");
            ArchetypeDecklistsView {
                state: DecklistsState::Unavailable,
                lists: Vec::new(),
            }
        }
    }
}

/// Ask the worker for the decklists and wait for its answer.
fn ask_decklists(app: &AppHandle, uuid: String) -> Result<Vec<ArchetypeDeck>, String> {
    let worker = app
        .try_state::<crate::archetypes::worker::ArchetypesWorker>()
        .ok_or_else(|| "the archetype worker is not running".to_string())?;
    let (reply, answer) = std::sync::mpsc::channel();
    if !worker.signal(ArchetypeSignal::FetchDecklists { uuid, reply }) {
        return Err(STOPPED.to_string());
    }
    answer.recv().unwrap_or_else(|_| Err(STOPPED.to_string()))
}

/// "Save image": writes a PNG where the user picks (the fallback when the
/// clipboard refuses images, as WebKitGTK may). False when cancelled.
#[tauri::command]
#[specta::specta]
pub async fn save_image(
    app: AppHandle,
    png: Vec<u8>,
    file_name: String,
) -> Result<bool, CommandError> {
    off_async_workers(move || {
        let mut dialog = app
            .dialog()
            .file()
            .set_title("Save Image")
            .set_file_name(image_name(&file_name))
            .add_filter("PNG image", &["png"]);
        if let Some(window) = app.get_webview_window("main") {
            dialog = dialog.set_parent(&window);
        }
        let Some(picked) = dialog.blocking_save_file() else {
            return Ok(false);
        };
        let path = picked
            .into_path()
            .map_err(|error| CommandError::Storage(format!("could not save the image: {error}")))?;
        std::fs::write(&path, png)
            .map_err(|error| CommandError::Storage(format!("could not save the image: {error}")))?;
        Ok(true)
    })
    .await
}

/// The suggested file name: the page's, `.png` once.
pub fn image_name(name: &str) -> String {
    let trimmed = name.trim();
    let base = trimmed.strip_suffix(".png").unwrap_or(trimmed);
    if base.is_empty() {
        "metagame.png".to_string()
    } else {
        format!("{base}.png")
    }
}

#[cfg(test)]
mod tests;
