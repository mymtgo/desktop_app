//! The Cards page commands (0.x `Cards\IndexController` and
//! `ArchetypesController`): the grid from `store::cards`, the archetype sheet
//! and "Fetch missing".

use serde::{Deserialize, Serialize};
use store::cards::{CardArchetype, CardTile, CardsPage, CardsPageQuery};
use store::{Connection, StoreError};
use tauri::{AppHandle, State};

use crate::bridge::lock;
use crate::cards::card_data::run_download;
use crate::commands::{CommandError, Startup, ready, storage};
use crate::logging::Timer;
use crate::platform::page_scope;
use store::platform::Platform;

/// What the page asks for (the query string's filters).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CardsQuery {
    pub search: Option<String>,
    pub format: Option<String>,
    /// Category names to hide (`Creature`, `Land`, ...).
    pub hidden_types: Vec<String>,
    pub group_printings: bool,
    /// 1-based.
    pub page: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CardTileView {
    pub catalog_id: u32,
    pub name: Option<String>,
    pub set_code: Option<String>,
    #[serde(rename = "type")]
    pub type_line: Option<String>,
    pub sub_type: Option<String>,
    pub oracle_id: Option<String>,
    /// The remote image URL.
    pub image: Option<String>,
    /// The downloaded file's name (`CardImage`'s `local`).
    pub local_image: Option<String>,
    pub popularity: u32,
    /// What the archetype sheet asks for: the oracle id, or `id:{catalog_id}`.
    pub group: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CardFormatView {
    pub key: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CardsPageView {
    pub cards: Vec<CardTileView>,
    pub page: u32,
    pub last_page: u32,
    pub total: u32,
    /// Cards on a full page (`store::cards::CARDS_PER_PAGE`).
    pub per_page: u32,
    pub formats: Vec<CardFormatView>,
    pub missing_count: u32,
    pub total_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CardArchetypeView {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    pub name: String,
    pub format: Option<String>,
    pub format_label: Option<String>,
    pub color_identity: Option<String>,
    pub maindeck: bool,
    pub sideboard: bool,
    pub deck_count: u32,
}

fn tile_view(tile: &CardTile) -> CardTileView {
    CardTileView {
        catalog_id: u32::try_from(tile.catalog_id).unwrap_or(0),
        name: tile.name.clone(),
        set_code: tile.set_code.clone(),
        type_line: tile.type_line.clone(),
        sub_type: tile.sub_type.clone(),
        oracle_id: tile.oracle_id.clone(),
        image: tile.image.clone(),
        local_image: tile.local_image.clone(),
        popularity: tile.popularity,
        group: tile.group_key(),
    }
}

pub fn page_view(page: &CardsPage) -> CardsPageView {
    CardsPageView {
        cards: page.cards.iter().map(tile_view).collect(),
        page: page.page,
        last_page: page.last_page,
        total: page.total,
        per_page: store::cards::CARDS_PER_PAGE,
        formats: page
            .formats
            .iter()
            .map(|(key, label)| CardFormatView {
                key: key.clone(),
                label: label.clone(),
            })
            .collect(),
        missing_count: page.missing_count,
        total_count: page.total_count,
    }
}

fn archetype_view(archetype: &CardArchetype) -> CardArchetypeView {
    CardArchetypeView {
        id: archetype.id,
        name: archetype.name.clone(),
        format: archetype.format.clone(),
        format_label: archetype.format_label.clone(),
        color_identity: archetype.color_identity.clone(),
        maindeck: archetype.maindeck,
        sideboard: archetype.sideboard,
        deck_count: archetype.deck_count,
    }
}

fn store_query(query: CardsQuery, platform: Platform) -> CardsPageQuery {
    CardsPageQuery {
        search: query.search.filter(|s| !s.trim().is_empty()),
        format: query.format.filter(|s| !s.is_empty()),
        hidden_types: query.hidden_types,
        group_printings: query.group_printings,
        page: query.page,
        platform,
    }
}

pub fn read_page(conn: &Connection, query: CardsQuery) -> Result<CardsPageView, StoreError> {
    let platform = page_scope(conn)?.platform;
    Ok(page_view(&store::cards::cards_page(
        conn,
        &store_query(query, platform),
    )?))
}

/// One page of the Cards page.
#[tauri::command]
#[specta::specta]
pub fn cards_page(
    query: CardsQuery,
    startup: State<'_, Startup>,
) -> Result<CardsPageView, CommandError> {
    let _timer = Timer::start("cards_page");
    let state = ready(&startup)?;
    read_page(&lock(&state.reader), query).map_err(storage)
}

/// The archetypes holding a card's group (`group` is an oracle id or
/// `id:{catalog_id}`), of `format` when given.
#[tauri::command]
#[specta::specta]
pub fn card_archetypes(
    group: String,
    format: Option<String>,
    startup: State<'_, Startup>,
) -> Result<Vec<CardArchetypeView>, CommandError> {
    let state = ready(&startup)?;
    let rows = store::cards::card_archetypes(&lock(&state.reader), &group, format.as_deref())
        .map_err(storage)?;
    Ok(rows.iter().map(archetype_view).collect())
}

/// "Fetch missing (N)": the same scan and population as Settings' "Download
/// card data", answered when done.
#[tauri::command]
#[specta::specta]
pub async fn fetch_missing_cards(app: AppHandle) -> Result<(), CommandError> {
    run_download(app).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_search_and_format_are_no_filter() {
        let query = store_query(
            CardsQuery {
                search: Some("  ".into()),
                format: Some(String::new()),
                hidden_types: vec!["Land".into()],
                group_printings: true,
                page: 2,
            },
            Platform::Mtgo,
        );
        assert_eq!(query.search, None);
        assert_eq!(query.format, None);
        assert_eq!((query.page, query.hidden_types.len()), (2, 1));
    }

    #[test]
    fn a_tile_carries_its_group_key() {
        let tile = CardTile {
            catalog_id: 7,
            name: None,
            set_code: None,
            type_line: None,
            sub_type: None,
            oracle_id: None,
            image: None,
            local_image: None,
            popularity: 0,
        };
        assert_eq!(tile_view(&tile).group, "id:7");
    }

    #[test]
    fn a_page_carries_its_page_size() {
        let page = CardsPage {
            cards: Vec::new(),
            page: 1,
            last_page: 1,
            total: 0,
            formats: Vec::new(),
            missing_count: 0,
            total_count: 0,
        };
        assert_eq!(page_view(&page).per_page, 80);
    }
}
