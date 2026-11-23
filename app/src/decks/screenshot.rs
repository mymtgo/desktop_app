//! The Decklist page's "Share Deck" screenshot data (release spec section
//! 11; 0.x `Decks/ScreenshotDataController` and `EncodeDeckScreenshotData`).
//!
//! Two steps, so the reader's lock is never held while an image is read or
//! fetched: `deck_sources` reads the deck, its record, its cover and its
//! latest version's cards; `screenshot_view` then turns every image into a
//! `data:` URL (once per printing, as 0.x).

use std::collections::HashMap;
use std::time::Instant;

use chrono_tz::Tz;
use serde::Serialize;
use store::Connection;
use store::cards::ImageSources;
use store::deck_pages;
use store::decks::{EditOutcome, card_infos, versions_of};
use tauri::{AppHandle, Manager};

use super::{RecordView, color_list, record_view, refused};
use crate::bridge::lock;
use crate::cards::data_url::{
    FetchBudget, SCREENSHOT_DEADLINE, current_folder, image_data_url_within,
};
use crate::cards::images::Fetcher;
use crate::commands::{CommandError, Startup, ready, storage};
use crate::logging::Timer;

/// One card of the screenshot (0.x `ScreenshotCard`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotCardView {
    pub name: String,
    /// The card's type, normalised (0.x `normalizeType`).
    #[serde(rename = "type")]
    pub card_type: String,
    pub quantity: u32,
    /// The card image as a `data:` URL, `None` when it could not be read.
    pub image_base64: Option<String>,
}

/// A mana value bucket: `0` to `6`, then `7+`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CmcBucketView {
    pub cmc: String,
    pub count: u32,
}

/// The main deck cards of one type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct TypeCountView {
    #[serde(rename = "type")]
    pub type_name: String,
    pub count: u32,
}

/// Everything the deck screenshot shows (0.x `ScreenshotDataController`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeckScreenshotView {
    pub name: String,
    /// The format for display (`Modern`).
    pub format: String,
    /// Colour identity letters, e.g. `["U", "R"]`.
    pub colors: Vec<String>,
    /// Every complete match of the deck, all versions, all time.
    pub match_record: RecordView,
    /// The cover's art crop as a `data:` URL.
    pub cover_art_base64: Option<String>,
    pub non_land_cards: Vec<ScreenshotCardView>,
    pub land_cards: Vec<ScreenshotCardView>,
    pub sideboard_cards: Vec<ScreenshotCardView>,
    /// Eight buckets (`0` to `6`, `7+`); empty when the deck has no version.
    pub cmc_distribution: Vec<CmcBucketView>,
    /// Canonical types then `Land`, only those with cards.
    pub type_distribution: Vec<TypeCountView>,
    /// An Arena Brawl deck: its sideboard rows are its commanders, shown
    /// first as "Commander".
    pub commander_format: bool,
}

/// One card of the deck before its image is read.
#[derive(Debug, Clone, PartialEq)]
pub struct ShotCard {
    pub catalog_id: u32,
    pub name: String,
    /// The type line as stored (`Creature - Goblin`, `Basic Land`).
    pub type_line: String,
    pub cmc: Option<f64>,
    pub quantity: u32,
    pub sideboard: bool,
    pub image: ImageSources,
}

/// A card placed in its list, its type normalised.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedCard {
    pub card: ShotCard,
    pub card_type: String,
}

/// The cards split, sorted and counted (0.x `EncodeDeckScreenshotData::run`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ShotLists {
    pub non_land: Vec<PlacedCard>,
    pub land: Vec<PlacedCard>,
    pub sideboard: Vec<PlacedCard>,
    pub cmc: Vec<(String, u32)>,
    pub types: Vec<(String, u32)>,
}

/// What the screenshot reads from the database.
#[derive(Debug, Clone, PartialEq)]
pub struct DeckSources {
    pub name: String,
    pub format: String,
    /// See `DeckScreenshotView::commander_format`.
    pub commander_format: bool,
    pub colors: Vec<String>,
    pub record: RecordView,
    /// The cover's art crop, only for a deck with a version (0.x).
    pub cover: Option<ImageSources>,
    /// The deck's own cover image (a file name in the deck covers folder),
    /// which replaces `cover` when its file can be read.
    pub custom_cover: Option<String>,
    /// `None` when the deck has no version.
    pub lists: Option<ShotLists>,
}

/// Types in the order a type line is matched (0.x `CANONICAL_TYPES`).
const CANONICAL_TYPES: [&str; 7] = [
    "Creature",
    "Planeswalker",
    "Battle",
    "Instant",
    "Sorcery",
    "Enchantment",
    "Artifact",
];

/// The sort rank of a non-land type (0.x `TYPE_ORDER`); others come last.
fn type_rank(card_type: &str) -> u32 {
    match card_type {
        "Creature" => 1,
        "Planeswalker" => 2,
        "Instant" => 3,
        "Sorcery" => 4,
        "Artifact" => 5,
        "Enchantment" => 6,
        _ => 99,
    }
}

/// The first canonical type the line contains, else `Land` when it has
/// one, else the line itself (0.x `normalizeType`).
pub fn normalize_type(line: &str) -> String {
    for canonical in CANONICAL_TYPES {
        if line.contains(canonical) {
            return canonical.to_string();
        }
    }
    if line.contains("Land") {
        return "Land".to_string();
    }
    line.to_string()
}

/// The cards in their three lists with the mana curve and type counts:
/// side cards keep list order; non-land cards sort by type rank then name,
/// lands by name; mana value is truncated, 7 and up share `7+` (0.x).
pub fn shot_lists(cards: Vec<ShotCard>) -> ShotLists {
    let mut lists = ShotLists::default();
    let mut buckets = [0u32; 8];
    let mut type_counts: HashMap<String, u32> = HashMap::new();

    for card in cards {
        let card_type = normalize_type(&card.type_line);
        if card.sideboard {
            lists.sideboard.push(PlacedCard { card, card_type });
        } else if card_type == "Land" {
            *type_counts.entry(card_type.clone()).or_default() += card.quantity;
            lists.land.push(PlacedCard { card, card_type });
        } else {
            let value = card.cmc.unwrap_or(0.0) as i64;
            let bucket = value.clamp(0, 7) as usize;
            buckets[bucket] += card.quantity;
            *type_counts.entry(card_type.clone()).or_default() += card.quantity;
            lists.non_land.push(PlacedCard { card, card_type });
        }
    }

    lists.non_land.sort_by(|a, b| {
        let rank_a = type_rank(&a.card_type);
        let rank_b = type_rank(&b.card_type);
        rank_a
            .cmp(&rank_b)
            .then_with(|| a.card.name.cmp(&b.card.name))
    });
    lists.land.sort_by(|a, b| a.card.name.cmp(&b.card.name));

    for (value, count) in buckets.iter().enumerate() {
        let label = if value == 7 {
            "7+".to_string()
        } else {
            value.to_string()
        };
        lists.cmc.push((label, *count));
    }
    for type_name in CANONICAL_TYPES.iter().chain(["Land"].iter()) {
        if let Some(count) = type_counts.get(*type_name) {
            lists.types.push((type_name.to_string(), *count));
        }
    }
    lists
}

/// The deck's latest version's cards that have card data, in list order
/// (0.x skips a card without a card row).
fn latest_cards(conn: &Connection, deck_id: i64) -> Result<Option<Vec<ShotCard>>, CommandError> {
    // Oldest first, so the last is the latest.
    let versions = versions_of(conn, deck_id).map_err(storage)?;
    let Some(latest) = versions.last() else {
        return Ok(None);
    };
    let rows = decks::decode_signature(&latest.signature);
    let mut ids = Vec::new();
    for row in &rows {
        ids.push(row.catalog_id);
    }
    let infos = card_infos(conn, &ids).map_err(storage)?;
    let mut cards = Vec::new();
    for row in rows {
        let Some(info) = infos.get(&row.catalog_id) else {
            continue;
        };
        cards.push(ShotCard {
            catalog_id: row.catalog_id,
            name: info.name.clone().unwrap_or_default(),
            type_line: info.card_type.clone().unwrap_or_default(),
            cmc: info.cmc,
            quantity: row.quantity,
            sideboard: row.sideboard,
            image: ImageSources {
                image: info.image.clone(),
                local_image: info.local_image.clone(),
            },
        });
    }
    Ok(Some(cards))
}

/// The deck's cover art crop sources (0.x `$deck->cover`), `None` when it
/// has no cover. A commander format deck without a chosen cover shows its
/// commander (`store::decks::commander_cover`).
pub fn cover_sources(
    conn: &Connection,
    deck_id: i64,
) -> Result<Option<ImageSources>, CommandError> {
    let Some(deck) = store::decks::deck_by_id(conn, deck_id).map_err(storage)? else {
        return Ok(None);
    };
    let catalog_id = match (deck.cover_catalog_id, &deck.cover_file) {
        (Some(chosen), _) => Some(chosen),
        (None, None) => {
            store::decks::commander_cover(conn, deck_id, &deck.format).map_err(storage)?
        }
        (None, Some(_)) => None,
    };
    let Some(catalog_id) = catalog_id else {
        return Ok(None);
    };
    let cards = card_infos(conn, &[catalog_id]).map_err(storage)?;
    Ok(cards.get(&catalog_id).map(|card| ImageSources {
        image: card.art_crop.clone(),
        local_image: card.local_art_crop.clone(),
    }))
}

/// What the screenshot needs from the database; refused when there is no
/// such deck (archived decks are found).
pub fn deck_sources(conn: &Connection, deck_id: i64, tz: Tz) -> Result<DeckSources, CommandError> {
    let header = deck_pages::deck_header(conn, deck_id, decks::stats::Range::all_time(), tz, None)
        .map_err(storage)?;
    let Some(header) = header else {
        return Err(refused(EditOutcome::NotFound));
    };
    let summary = header.summary;
    let lists = latest_cards(conn, deck_id)?.map(shot_lists);
    let (cover, custom_cover) = match lists {
        Some(_) => (cover_sources(conn, deck_id)?, summary.cover_file),
        None => (None, None),
    };
    Ok(DeckSources {
        name: summary.name,
        commander_format: store::match_commanders::is_commander_format(Some(&summary.format)),
        format: summary.format_label,
        colors: color_list(summary.color_identity.as_deref()),
        record: record_view(&summary.record),
        cover,
        custom_cover,
        lists,
    })
}

/// The screenshot with every image read through `image` (once per
/// printing).
pub fn screenshot_view(
    sources: DeckSources,
    image: &dyn Fn(&ImageSources) -> Option<String>,
) -> DeckScreenshotView {
    let cover_art_base64 = sources.cover.as_ref().and_then(image);
    let mut view = DeckScreenshotView {
        name: sources.name,
        format: sources.format,
        colors: sources.colors,
        match_record: sources.record,
        cover_art_base64,
        non_land_cards: Vec::new(),
        land_cards: Vec::new(),
        sideboard_cards: Vec::new(),
        cmc_distribution: Vec::new(),
        type_distribution: Vec::new(),
        commander_format: sources.commander_format,
    };
    let Some(lists) = sources.lists else {
        return view;
    };

    let mut cache: HashMap<u32, Option<String>> = HashMap::new();
    let mut card_view = |placed: PlacedCard| {
        let url = cache
            .entry(placed.card.catalog_id)
            .or_insert_with(|| image(&placed.card.image))
            .clone();
        ScreenshotCardView {
            name: placed.card.name,
            card_type: placed.card_type,
            quantity: placed.card.quantity,
            image_base64: url,
        }
    };
    // 0.x encodes in list order; the cache makes the order irrelevant.
    for placed in lists.non_land {
        view.non_land_cards.push(card_view(placed));
    }
    for placed in lists.land {
        view.land_cards.push(card_view(placed));
    }
    for placed in lists.sideboard {
        view.sideboard_cards.push(card_view(placed));
    }
    for (cmc, count) in lists.cmc {
        view.cmc_distribution.push(CmcBucketView { cmc, count });
    }
    for (type_name, count) in lists.types {
        view.type_distribution
            .push(TypeCountView { type_name, count });
    }
    view
}

/// "Share Deck" (0.x `decks/{deck}/screenshot-data`). Off the IPC thread:
/// images are read from disk or Scryfall (5 s each at most, and no fetch
/// after `SCREENSHOT_DEADLINE` in total).
#[tauri::command]
#[specta::specta]
pub async fn deck_screenshot_data(
    app: AppHandle,
    id: u32,
) -> Result<DeckScreenshotView, CommandError> {
    let _timer = Timer::start("deck_screenshot_data");
    crate::commands::off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let mut sources = deck_sources(&lock(&state.reader), i64::from(id), state.timezone)?;
        let covers = super::cover::folder(&state.data_dir);
        let custom = sources
            .custom_cover
            .take()
            .and_then(|name| super::cover::data_url(&covers, &name));
        if custom.is_some() {
            sources.cover = None;
        }
        let folder = current_folder(&app);
        let fetcher = Fetcher::standard();
        let budget = FetchBudget::starting(Instant::now(), SCREENSHOT_DEADLINE);
        let mut view = screenshot_view(sources, &|image| {
            let timeout = budget.next_timeout(Instant::now());
            image_data_url_within(image, folder.as_deref(), &fetcher, timeout)
        });
        if custom.is_some() {
            view.cover_art_base64 = custom;
        }
        Ok(view)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decks::tests::database;
    use std::cell::RefCell;

    fn card(
        catalog_id: u32,
        name: &str,
        line: &str,
        cmc: f64,
        quantity: u32,
        sideboard: bool,
    ) -> ShotCard {
        ShotCard {
            catalog_id,
            name: name.to_string(),
            type_line: line.to_string(),
            cmc: Some(cmc),
            quantity,
            sideboard,
            image: ImageSources::default(),
        }
    }

    fn names(cards: &[PlacedCard]) -> Vec<(String, String)> {
        cards
            .iter()
            .map(|placed| (placed.card.name.clone(), placed.card_type.clone()))
            .collect()
    }

    fn pairs(list: &[(&str, u32)]) -> Vec<(String, u32)> {
        list.iter()
            .map(|(key, count)| (key.to_string(), *count))
            .collect()
    }

    #[test]
    fn types_normalise_as_0x() {
        assert_eq!(normalize_type("Artifact Creature - Golem"), "Creature");
        assert_eq!(normalize_type("Legendary Planeswalker - Q"), "Planeswalker");
        assert_eq!(normalize_type("Enchantment Land"), "Enchantment");
        assert_eq!(normalize_type("Basic Land - Mountain"), "Land");
        assert_eq!(normalize_type("Kindred Tribal"), "Kindred Tribal");
        assert_eq!(normalize_type(""), "");
    }

    #[test]
    fn cards_split_sort_and_count_as_0x() {
        let lists = shot_lists(vec![
            card(1, "Qzap", "Instant", 1.0, 4, false),
            card(2, "Qwall", "Battle - Siege", 3.0, 1, false),
            card(3, "Qbeast", "Creature - Beast", 9.0, 2, false),
            card(4, "Qaxe", "Artifact - Equipment", 2.5, 1, false),
            card(5, "Qswamp", "Basic Land - Swamp", 0.0, 10, false),
            card(6, "Qcave", "Land", 0.0, 4, false),
            card(7, "Qodd", "Kindred Tribal", 0.0, 1, false),
            card(8, "Qzz", "Sorcery", 1.0, 2, true),
            card(9, "Qaa", "Instant", 1.0, 1, true),
            card(10, "Qant", "Creature - Insect", 1.0, 3, false),
        ]);
        let pair = |name: &str, kind: &str| (name.to_string(), kind.to_string());
        assert_eq!(
            names(&lists.non_land),
            vec![
                pair("Qant", "Creature"),
                pair("Qbeast", "Creature"),
                pair("Qzap", "Instant"),
                pair("Qaxe", "Artifact"),
                pair("Qodd", "Kindred Tribal"),
                pair("Qwall", "Battle"),
            ],
            "type rank then name; Battle and other types rank 99, by name"
        );
        assert_eq!(
            names(&lists.land),
            vec![pair("Qcave", "Land"), pair("Qswamp", "Land")]
        );
        assert_eq!(
            names(&lists.sideboard),
            vec![pair("Qzz", "Sorcery"), pair("Qaa", "Instant")],
            "list order"
        );
        assert_eq!(
            lists.cmc,
            pairs(&[
                ("0", 1),
                ("1", 7),
                ("2", 1),
                ("3", 1),
                ("4", 0),
                ("5", 0),
                ("6", 0),
                ("7+", 2)
            ])
        );
        assert_eq!(
            lists.types,
            pairs(&[
                ("Creature", 5),
                ("Battle", 1),
                ("Instant", 4),
                ("Artifact", 1),
                ("Land", 14)
            ])
        );
    }

    // 0.x Decks/ScreenshotDataTest: "returns screenshot data with base64
    // card images".
    #[test]
    fn the_deck_screenshot_has_0x_fields_and_inlines_each_printing_once() {
        let (_dir, conn, (deck_id, _, _)) = database("decks-screenshot");
        let sources = deck_sources(&conn, deck_id, chrono_tz::UTC).expect("sources");
        let asked = RefCell::new(Vec::new());
        let view = screenshot_view(sources, &|image| {
            asked.borrow_mut().push(image.clone());
            image
                .local_image
                .as_ref()
                .map(|name| format!("data:image/jpeg;base64,{name}"))
        });

        assert_eq!(view.name, "Qburn");
        assert_eq!(view.format, "Modern");
        assert_eq!(view.colors, vec!["U".to_string(), "R".to_string()]);
        assert_eq!((view.match_record.wins, view.match_record.total), (1, 1));
        assert_eq!(view.match_record.label, "1 - 0");
        assert_eq!(
            view.cover_art_base64.as_deref(),
            Some("data:image/jpeg;base64,1001_art.jpg")
        );
        let shown = |cards: &[ScreenshotCardView]| -> Vec<(String, String, u32, Option<String>)> {
            cards
                .iter()
                .map(|card| {
                    (
                        card.name.clone(),
                        card.card_type.clone(),
                        card.quantity,
                        card.image_base64.clone(),
                    )
                })
                .collect()
        };
        let bolt = Some("data:image/jpeg;base64,1001.jpg".to_string());
        assert_eq!(
            shown(&view.non_land_cards),
            vec![
                ("Qgoblin".to_string(), "Creature".to_string(), 2, None),
                ("Qbolt".to_string(), "Instant".to_string(), 4, bolt.clone()),
            ]
        );
        assert_eq!(
            shown(&view.land_cards),
            vec![("Qmountain".to_string(), "Land".to_string(), 18, None)]
        );
        assert_eq!(
            shown(&view.sideboard_cards),
            vec![("Qbolt".to_string(), "Instant".to_string(), 2, bolt)],
            "the card without card data is left out"
        );
        assert_eq!(asked.borrow().len(), 4, "cover, then one per printing");
        let cmc: Vec<(String, u32)> = view
            .cmc_distribution
            .iter()
            .map(|bucket| (bucket.cmc.clone(), bucket.count))
            .collect();
        assert_eq!(
            cmc,
            pairs(&[
                ("0", 0),
                ("1", 4),
                ("2", 2),
                ("3", 0),
                ("4", 0),
                ("5", 0),
                ("6", 0),
                ("7+", 0)
            ])
        );
        let json = serde_json::to_value(&view).expect("json");
        for key in [
            "name",
            "format",
            "colors",
            "matchRecord",
            "coverArtBase64",
            "nonLandCards",
            "landCards",
            "sideboardCards",
            "cmcDistribution",
            "typeDistribution",
            "commanderFormat",
        ] {
            assert!(json.get(key).is_some(), "{key}");
        }
        assert_eq!(json["nonLandCards"][0]["type"], "Creature");
        assert_eq!(
            json["nonLandCards"][1]["imageBase64"],
            "data:image/jpeg;base64,1001.jpg"
        );
        assert_eq!(
            json["typeDistribution"],
            serde_json::json!([
                {"type": "Creature", "count": 2},
                {"type": "Instant", "count": 4},
                {"type": "Land", "count": 18}
            ])
        );
    }

    // 0.x "returns empty arrays when deck has no version".
    #[test]
    fn a_deck_without_a_version_has_empty_lists_and_no_cover() {
        let (_dir, conn, _) = database("decks-screenshot-empty");
        conn.execute_batch(
            "INSERT INTO decks (id, mtgo_id, name, format, cover_catalog_id, created_at, updated_at, source)
             VALUES (6, 'q-empty', 'Qempty', 'CPAUPER', 1001, 1, 1, 'file');",
        )
        .expect("deck");
        let sources = deck_sources(&conn, 6, chrono_tz::UTC).expect("sources");
        let view = screenshot_view(sources, &|_| Some("data:x".to_string()));
        assert_eq!(view.name, "Qempty");
        assert_eq!(view.cover_art_base64, None);
        assert!(view.non_land_cards.is_empty());
        assert!(view.land_cards.is_empty());
        assert!(view.sideboard_cards.is_empty());
        assert!(view.cmc_distribution.is_empty());
        assert!(view.type_distribution.is_empty());
        assert_eq!(view.match_record.total, 0);
        assert_eq!(
            deck_sources(&conn, 999, chrono_tz::UTC),
            Err(CommandError::Refused(
                "This deck no longer exists.".to_string()
            ))
        );
    }

    /// The Commander block comes first for the Arena Brawl codes only.
    #[test]
    fn only_an_arena_brawl_deck_screenshot_is_a_commander_format() {
        let (_dir, conn, (deck_id, _, _)) = database("decks-screenshot-commander");
        for (format, expected) in [
            ("ARENA_HISTORIC_BRAWL", true),
            ("ARENA_BRAWL", true),
            ("CBRAWL", false),
            ("CMODERN", false),
        ] {
            conn.execute("UPDATE decks SET format = ?1", [format])
                .expect("format");
            let sources = deck_sources(&conn, deck_id, chrono_tz::UTC).expect("sources");
            let view = screenshot_view(sources, &|_| None);
            assert_eq!(view.commander_format, expected, "{format}");
        }
    }

    #[test]
    fn the_sources_carry_the_decks_own_cover_file_beside_the_printing() {
        let (_dir, conn, (deck_id, _, _)) = database("decks-screenshot-custom-cover");
        let sources = deck_sources(&conn, deck_id, chrono_tz::UTC).expect("sources");
        assert_eq!(sources.custom_cover, None);
        conn.execute_batch("UPDATE decks SET cover_file = '1-5.png'")
            .expect("custom cover");
        let sources = deck_sources(&conn, deck_id, chrono_tz::UTC).expect("sources");
        assert_eq!(sources.custom_cover.as_deref(), Some("1-5.png"));
        assert!(sources.cover.is_some(), "the printing is still read");
    }

    #[test]
    fn the_cover_is_the_art_crop_of_the_cover_printing() {
        let (_dir, conn, (deck_id, _, _)) = database("decks-screenshot-cover");
        assert_eq!(
            cover_sources(&conn, deck_id).expect("cover"),
            Some(ImageSources {
                image: Some("https://cards.scryfall.io/a/1.jpg".to_string()),
                local_image: Some("1001_art.jpg".to_string()),
            })
        );
        conn.execute_batch("UPDATE decks SET cover_catalog_id = NULL")
            .expect("no cover");
        assert_eq!(cover_sources(&conn, deck_id).expect("cover"), None);
        assert_eq!(cover_sources(&conn, 999).expect("cover"), None);
    }

    /// A Brawl deck without a chosen cover shows its commander, the latest
    /// version's first sideboard row (Qbolt here).
    #[test]
    fn a_brawl_decks_default_cover_is_its_commander() {
        let (_dir, conn, (deck_id, _, _)) = database("decks-screenshot-commander-cover");
        conn.execute_batch(
            "UPDATE decks SET cover_catalog_id = NULL, format = 'ARENA_HISTORIC_BRAWL'",
        )
        .expect("brawl");
        assert_eq!(
            cover_sources(&conn, deck_id).expect("cover"),
            Some(ImageSources {
                image: Some("https://cards.scryfall.io/a/1.jpg".to_string()),
                local_image: Some("1001_art.jpg".to_string()),
            })
        );
        conn.execute_batch("UPDATE decks SET cover_file = '1-5.png'")
            .expect("own cover");
        assert_eq!(cover_sources(&conn, deck_id).expect("cover"), None);
    }
}
