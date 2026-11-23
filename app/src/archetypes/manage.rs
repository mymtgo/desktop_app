//! The manage screen and the manual archetype commands (archetypes spec
//! sections 7 and 11): the manage list, the create and edit form (scan a
//! match, open a `.dek`, resolve cards), variants, reassign, merge,
//! unmerge, delete and `.dek` export. Every write goes through the
//! tracker's `ArchetypeEdit`; the strings are 0.x's.
//!
//! T8's "Create archetype from this match" opens the create form at
//! `/metagame/archetypes/new?source_match_id=<id>` (`createArchetypeRoute`
//! in `ui/src/lib/archetypes.ts`); the form asks `archetype_create_prefill`
//! for the match's format, colours and cards.

use std::collections::BTreeMap;
use std::path::Path;

use archetypes::ObservedCard;
use decks::DeckCard;
use mymtgo_api::{ApiError, ResolvedCard};
use serde::{Deserialize, Serialize};
use store::Connection;
use store::archetype_edit::{ArchetypeEdit, ArchetypeEditOutcome, ManualCard, NewManualArchetype};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

use crate::api::session::ApiState;
use crate::bridge::lock;
use crate::commands::{AppState, CommandError, Startup, off_async_workers, ready, storage};
use crate::decks::{DekExport, ListCardView, color_list};
use crate::logging::Timer;

/// 0.x `ScanMatchController`'s message.
pub const NO_OPPONENT_CARDS: &str = "No opponent cards found in this match.";
/// 0.x `ResolveCardsFromDek`'s message.
pub const RESOLVE_FAILED: &str = "Failed to resolve cards from API.";

// ---- Views -------------------------------------------------------------------

/// The manage screen's query (route query, remembered by the UI).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ManageQueryInput {
    pub search: Option<String>,
    pub format: Option<String>,
    /// `manual` (Custom) or `downloaded` (Server); none for every source.
    pub source: Option<String>,
    pub page: u32,
}

/// Where an archetype came from (`archetypes::Source`), as its stored text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum ArchetypeSourceView {
    Downloaded,
    Manual,
    Fallback,
    Commander,
}

impl From<archetypes::Source> for ArchetypeSourceView {
    fn from(source: archetypes::Source) -> Self {
        match source {
            archetypes::Source::Downloaded => ArchetypeSourceView::Downloaded,
            archetypes::Source::Manual => ArchetypeSourceView::Manual,
            archetypes::Source::Fallback => ArchetypeSourceView::Fallback,
            archetypes::Source::Commander => ArchetypeSourceView::Commander,
        }
    }
}

/// An archetype as a choice or a link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ArchetypeChoiceView {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    pub name: String,
    pub format: Option<String>,
    pub source: ArchetypeSourceView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ManageRowView {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    pub name: String,
    pub format: Option<String>,
    pub format_label: Option<String>,
    pub colors: Vec<String>,
    pub source: ArchetypeSourceView,
    pub retired: bool,
    /// A server archetype held as Custom by the offline download: no badge.
    pub offline_held: bool,
    pub incomplete: bool,
    pub merged_into: Option<ArchetypeChoiceView>,
    pub variants: u32,
    pub matches_faced: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ArchetypeFormatView {
    pub key: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ManageView {
    pub rows: Vec<ManageRowView>,
    pub total: u32,
    pub page: u32,
    pub per_page: u32,
    pub formats: Vec<ArchetypeFormatView>,
}

/// A match the create form offers as a source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct FormMatchView {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    pub token: String,
    pub opponent: String,
    #[specta(type = specta_typescript::Number)]
    pub started_at_ms: i64,
}

/// One resolved card of a scan or a `.dek` (0.x `ResolveCardsFromDek`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedCardView {
    pub catalog_id: u32,
    pub oracle_id: Option<String>,
    pub name: String,
    pub type_line: Option<String>,
    pub identity: Option<String>,
    pub quantity: u32,
    pub sideboard: bool,
}

/// Resolved cards and the colour identity of their non-land cards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedCardsView {
    pub cards: Vec<ResolvedCardView>,
    pub color_identity: Option<String>,
}

/// The create form's prefill from a source match (0.x
/// `CreateController::prefill`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PrefillView {
    #[specta(type = specta_typescript::Number)]
    pub source_match_id: i64,
    /// The archetypes format key of the match (`modern`).
    pub format: Option<String>,
    pub cards: ResolvedCardsView,
}

/// One card the form sends (0.x `cards.*`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct FormCardInput {
    pub catalog_id: u32,
    pub oracle_id: Option<String>,
    pub name: Option<String>,
    pub quantity: u32,
    pub sideboard: bool,
}

/// The create and edit form (0.x `StoreController`, `UpdateController`).
/// An edit sends no source match; its cards, when any, add a variant.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ArchetypeFormInput {
    pub name: String,
    pub format: String,
    /// Comma form ("U,B"); none for colourless.
    pub color_identity: Option<String>,
    pub cards: Vec<FormCardInput>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub source_match_id: Option<i64>,
    pub incomplete: bool,
}

/// After a create: the new archetype, which the form opens (0.x went to
/// the source match's deck Matches page instead).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CreatedArchetypeView {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
}

/// One list of the edit page.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct VariantView {
    pub list: u32,
    pub cards: Vec<ListCardView>,
}

/// What the edit page shows.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ArchetypeFormView {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    pub name: String,
    pub format: Option<String>,
    pub color_identity: Option<String>,
    pub source: ArchetypeSourceView,
    pub incomplete: bool,
    pub merged_into: Option<ArchetypeChoiceView>,
    pub variants: Vec<VariantView>,
}

/// One card of a given decklist to export.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ExportCard {
    pub catalog_id: u32,
    pub quantity: u32,
    pub sideboard: bool,
    pub name: String,
}

/// What `export_archetype_dek` writes: one of the archetype's lists (the
/// first without a number), or a live decklist's cards (the detail page).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ExportSource {
    List { list: Option<u32> },
    Decklist { cards: Vec<ExportCard> },
}

// ---- Rules -------------------------------------------------------------------

/// What resolution knows of one card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardFacts {
    pub oracle_id: Option<String>,
    pub name: Option<String>,
    pub card_type: Option<String>,
    pub identity: Option<String>,
}

/// The local card rows of these ids that are resolved (they have a type; a
/// stub has only a name), and the named Arena and Endstep stubs: the API
/// resolves MTGO ids only, so their name is all there is.
pub fn local_facts(
    conn: &Connection,
    ids: &[u32],
) -> Result<BTreeMap<u32, CardFacts>, store::StoreError> {
    let rows = store::decks::card_infos(conn, ids)?;
    let mut out = BTreeMap::new();
    for (id, row) in rows {
        let named_local = is_local_only(id) && row.name.is_some();
        if row.card_type.is_some() || named_local {
            out.insert(
                id,
                CardFacts {
                    oracle_id: row.oracle_id,
                    name: row.name,
                    card_type: row.card_type,
                    identity: row.color_identity,
                },
            );
        }
    }
    Ok(out)
}

/// Whether a catalog id is an Arena or Endstep card's, never sent to the
/// API: `/api/cards/resolve` takes MTGO ids only (#694).
fn is_local_only(catalog_id: u32) -> bool {
    let id = i64::from(catalog_id);
    cards::grp_id_of(id).is_some() || cards::endstep_row_of(id).is_some()
}

/// Resolve card entries (0.x `ResolveCardsFromDek`): the local rows first,
/// the rest through `ask_api` (`/api/cards/resolve`, asked only when some
/// are left; never for an Arena or Endstep card). A card nobody knows is left out. Entries keep their order and
/// their board; the colour identity is the non-land cards', WUBRG.
pub fn resolve_entries(
    entries: &[DeckCard],
    local: &BTreeMap<u32, CardFacts>,
    ask_api: impl FnOnce(&[u32]) -> Result<Vec<ResolvedCard>, ApiError>,
) -> Result<ResolvedCardsView, String> {
    let mut missing: Vec<u32> = Vec::new();
    for entry in entries {
        if entry.catalog_id > 0
            && !is_local_only(entry.catalog_id)
            && !local.contains_key(&entry.catalog_id)
            && !missing.contains(&entry.catalog_id)
        {
            missing.push(entry.catalog_id);
        }
    }
    let mut known = local.clone();
    if !missing.is_empty() {
        let answered = ask_api(&missing).map_err(|_| RESOLVE_FAILED.to_string())?;
        for card in answered {
            known.insert(
                card.mtgo_id,
                CardFacts {
                    oracle_id: card.oracle_id,
                    name: card.name,
                    card_type: card.card_type,
                    identity: card.identity,
                },
            );
        }
    }
    let mut cards = Vec::new();
    let mut observed = Vec::new();
    for entry in entries {
        let Some(facts) = known.get(&entry.catalog_id) else {
            continue;
        };
        cards.push(ResolvedCardView {
            catalog_id: entry.catalog_id,
            oracle_id: facts.oracle_id.clone(),
            name: facts
                .name
                .clone()
                .unwrap_or_else(|| format!("Card {}", entry.catalog_id)),
            type_line: facts.card_type.clone(),
            identity: facts.identity.clone(),
            quantity: entry.quantity,
            sideboard: entry.sideboard,
        });
        observed.push(ObservedCard {
            catalog_id: i64::from(entry.catalog_id),
            quantity: entry.quantity,
            sideboard: entry.sideboard,
            oracle_id: facts.oracle_id.clone(),
            name: facts.name.clone(),
            card_type: facts.card_type.clone(),
            color_identity: facts.identity.clone(),
            mana_cost: None,
        });
    }
    Ok(ResolvedCardsView {
        cards,
        color_identity: archetypes::colors_of(&observed),
    })
}

/// Resolve entries against the local cards, then `ask_api`.
fn resolve_with(
    conn: &Connection,
    entries: &[DeckCard],
    ask_api: impl FnOnce(&[u32]) -> Result<Vec<ResolvedCard>, ApiError>,
) -> Result<ResolvedCardsView, String> {
    let ids: Vec<u32> = entries.iter().map(|entry| entry.catalog_id).collect();
    let local = local_facts(conn, &ids).map_err(|error| error.to_string())?;
    resolve_entries(entries, &local, ask_api)
}

/// A `.dek` file's cards (0.x `UploadDekController`): the name must end
/// in `.dek`, the text must be XML.
pub fn read_dek_path(path: &Path) -> Result<Vec<DeckCard>, String> {
    const REQUIRED: &str = "The dek file field is required.";
    if path.as_os_str().is_empty() {
        return Err(REQUIRED.to_string());
    }
    let is_dek = path
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase().ends_with(".dek"))
        .unwrap_or(false);
    if !is_dek {
        return Err("The file must be a .dek file.".to_string());
    }
    // Checked before reading, so a huge file is never loaded.
    if std::fs::metadata(path).is_ok_and(|meta| meta.len() > decks::MAX_DEK_BYTES as u64) {
        return Err("The file is too large to be a .dek file.".to_string());
    }
    let Ok(bytes) = std::fs::read(path) else {
        return Err(REQUIRED.to_string());
    };
    decks::parse_dek(&String::from_utf8_lossy(&bytes))
}

/// The opponent's cards of a match, resolved ("Scan match").
pub fn scan_cards(
    conn: &Connection,
    match_id: i64,
    ask_api: impl FnOnce(&[u32]) -> Result<Vec<ResolvedCard>, ApiError>,
) -> Result<ResolvedCardsView, String> {
    let seen = store::archetypes::match_opponent_cards(conn, match_id)
        .map_err(|error| error.to_string())?;
    let mut entries = Vec::new();
    for card in &seen {
        if let Ok(catalog_id) = u32::try_from(card.catalog_id) {
            entries.push(DeckCard {
                catalog_id,
                quantity: card.quantity,
                sideboard: false,
            });
        }
    }
    if entries.is_empty() {
        return Err(NO_OPPONENT_CARDS.to_string());
    }
    resolve_with(conn, &entries, ask_api)
}

/// The create form's prefill: `None` without a source match or without
/// opponent cards (0.x).
pub fn create_prefill(
    conn: &Connection,
    source_match_id: Option<i64>,
    ask_api: impl FnOnce(&[u32]) -> Result<Vec<ResolvedCard>, ApiError>,
) -> Result<Option<PrefillView>, String> {
    let Some(match_id) = source_match_id else {
        return Ok(None);
    };
    let format =
        store::archetypes::match_format(conn, match_id).map_err(|error| error.to_string())?;
    let cards = match scan_cards(conn, match_id, ask_api) {
        Ok(cards) => cards,
        Err(message) if message == NO_OPPONENT_CARDS => return Ok(None),
        Err(message) => return Err(message),
    };
    Ok(Some(PrefillView {
        source_match_id: match_id,
        format: format.as_deref().and_then(archetypes::format_key),
        cards,
    }))
}

/// The form's validation, Laravel's messages. `cards_required` for a create
/// (`required|array|min:1`).
pub fn check_form(form: &ArchetypeFormInput, cards_required: bool) -> Result<(), String> {
    let name = form.name.trim();
    if name.is_empty() {
        return Err("The name field is required.".to_string());
    }
    if name.chars().count() > 255 {
        return Err("The name field must not be greater than 255 characters.".to_string());
    }
    if form.format.trim().is_empty() {
        return Err("The format field is required.".to_string());
    }
    if cards_required && form.cards.is_empty() {
        return Err("The cards field is required.".to_string());
    }
    for (index, card) in form.cards.iter().enumerate() {
        if card.quantity < 1 {
            return Err(format!(
                "The cards.{index}.quantity field must be at least 1."
            ));
        }
    }
    Ok(())
}

pub fn manual_cards(cards: &[FormCardInput]) -> Vec<ManualCard> {
    cards
        .iter()
        .map(|card| ManualCard {
            catalog_id: i64::from(card.catalog_id),
            oracle_id: card.oracle_id.clone(),
            name: card.name.clone(),
            quantity: card.quantity,
            sideboard: card.sideboard,
        })
        .collect()
}

/// Laravel's `Str::slug`: `Str::ascii` first (accented Latin letters
/// become plain ones, #300 F9), then `_` and `@` become `-` and `-at-`,
/// other marks go, whitespace and dashes become one `-`, lower case. A
/// letter outside the Latin table is dropped (0.x's `portable-ascii` knows
/// more scripts; MTGO names are Latin).
pub fn slug(text: &str) -> String {
    let mut spaced = String::new();
    for c in text.chars() {
        match c {
            '_' | '-' => spaced.push(' '),
            '@' => spaced.push_str(" at "),
            c if c.is_ascii_alphanumeric() || c.is_whitespace() => {
                spaced.push(c.to_ascii_lowercase());
            }
            c => spaced.push_str(ascii_of(c)),
        }
    }
    spaced.split_whitespace().collect::<Vec<_>>().join("-")
}

/// `Str::ascii` for one Latin letter, in lower case; empty for anything
/// else.
fn ascii_of(c: char) -> &'static str {
    let lower = c.to_lowercase().next().unwrap_or(c);
    match lower {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
        'ď' | 'đ' | 'ð' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'ĥ' | 'ħ' => "h",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'ĵ' => "j",
        'ķ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'œ' => "oe",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' => "s",
        'ß' => "ss",
        'ţ' | 'ť' | 'ŧ' => "t",
        'þ' => "th",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        _ => "",
    }
}

/// The `.dek` file name offered (0.x `{slug}.dek`).
pub fn dek_name(name: &str) -> String {
    let stem = slug(name);
    if stem.is_empty() {
        "archetype.dek".to_string()
    } else {
        format!("{stem}.dek")
    }
}

fn refused(message: &str) -> CommandError {
    CommandError::Refused(message.to_string())
}

/// The file name and `.dek` text of an export (0.x `ExportDekController`,
/// `GenerateDekFile`).
pub fn export_dek(
    conn: &Connection,
    id: i64,
    source: &ExportSource,
) -> Result<(String, String), CommandError> {
    let Some(archetype) = store::archetypes::archetype(conn, id).map_err(storage)? else {
        return Err(refused("The archetype was not found."));
    };
    if archetype.source == archetypes::Source::Fallback {
        return Err(refused("Fallback archetypes have no decklist to export."));
    }
    let mut rows = Vec::new();
    match source {
        ExportSource::List { list } => {
            let lists = store::archetypes::archetype_lists(conn, id).map_err(storage)?;
            let chosen = match list {
                Some(number) => lists.iter().find(|row| row.list == i64::from(*number)),
                None => lists.first(),
            };
            if let Some(chosen) = chosen {
                let ids: Vec<u32> = chosen
                    .cards
                    .iter()
                    .filter_map(|card| u32::try_from(card.catalog_id).ok())
                    .collect();
                let known = store::decks::card_infos(conn, &ids).map_err(storage)?;
                for card in &chosen.cards {
                    let Ok(catalog_id) = u32::try_from(card.catalog_id) else {
                        continue;
                    };
                    // As the deck export: a card without a card row is left out.
                    if let Some(info) = known.get(&catalog_id) {
                        rows.push(decks::DekCard {
                            catalog_id,
                            quantity: card.quantity,
                            sideboard: card.sideboard,
                            name: info.name.clone().unwrap_or_default(),
                        });
                    }
                }
            }
        }
        ExportSource::Decklist { cards } => {
            for card in cards {
                if card.quantity > 0 {
                    rows.push(decks::DekCard {
                        catalog_id: card.catalog_id,
                        quantity: card.quantity,
                        sideboard: card.sideboard,
                        name: card.name.clone(),
                    });
                }
            }
        }
    }
    if rows.is_empty() {
        return Err(refused("Archetype has no decklist to export."));
    }
    Ok((dek_name(&archetype.name), decks::dek_file(&rows)))
}

fn choice(option: &store::archetypes::ArchetypeOption) -> ArchetypeChoiceView {
    ArchetypeChoiceView {
        id: option.id,
        name: option.name.clone(),
        format: option.format.clone(),
        source: option.source.into(),
    }
}

pub fn manage_view(page: &store::archetypes::ManagePage) -> ManageView {
    let mut rows = Vec::new();
    for row in &page.rows {
        rows.push(ManageRowView {
            id: row.archetype.id,
            name: row.archetype.name.clone(),
            format: row.archetype.format.clone(),
            format_label: row
                .archetype
                .format
                .as_deref()
                .map(archetypes::format_label),
            colors: color_list(row.archetype.color_identity.as_deref()),
            source: row.archetype.source.into(),
            retired: row.retired,
            offline_held: row.offline_held,
            incomplete: row.incomplete,
            merged_into: row.merged_into.as_ref().map(choice),
            variants: row.variants,
            matches_faced: row.matches_faced,
        });
    }
    let formats = page
        .formats
        .iter()
        .map(|key| ArchetypeFormatView {
            key: key.clone(),
            label: archetypes::format_label(key),
        })
        .collect();
    ManageView {
        rows,
        total: page.total,
        page: page.page,
        per_page: page.per_page,
        formats,
    }
}

/// The edit page's data, `None` for an unknown id.
pub fn form_view(conn: &Connection, id: i64) -> Result<Option<ArchetypeFormView>, CommandError> {
    let Some(row) = store::archetypes::archetype(conn, id).map_err(storage)? else {
        return Ok(None);
    };
    let lists = store::archetypes::archetype_lists(conn, id).map_err(storage)?;
    let mut variants = Vec::new();
    for list in &lists {
        let ids: Vec<u32> = list
            .cards
            .iter()
            .filter_map(|card| u32::try_from(card.catalog_id).ok())
            .collect();
        let known = store::decks::card_infos(conn, &ids).map_err(storage)?;
        let mut cards = Vec::new();
        for card in &list.cards {
            let Ok(catalog_id) = u32::try_from(card.catalog_id) else {
                continue;
            };
            let info = known.get(&catalog_id);
            cards.push(ListCardView {
                catalog_id,
                name: info
                    .and_then(|info| info.name.clone())
                    .unwrap_or_else(|| format!("Card {catalog_id}")),
                type_line: info.and_then(|info| info.card_type.clone()),
                colors: color_list(info.and_then(|info| info.color_identity.as_deref())),
                image: info.and_then(|info| info.image.clone()),
                art_crop: info.and_then(|info| info.art_crop.clone()),
                local_image: info.and_then(|info| info.local_image.clone()),
                local_art_crop: info.and_then(|info| info.local_art_crop.clone()),
                cmc: info.and_then(|info| info.cmc),
                mana_cost: info.and_then(|info| info.mana_cost.clone()),
                quantity: card.quantity,
                sideboard: card.sideboard,
                known: info.is_some(),
            });
        }
        variants.push(VariantView {
            list: u32::try_from(list.list).unwrap_or(0),
            cards,
        });
    }
    Ok(Some(ArchetypeFormView {
        id: row.id,
        name: row.name,
        format: row.format,
        color_identity: row.color_identity,
        source: row.source.into(),
        incomplete: row.incomplete,
        merged_into: row.merged_into.as_ref().map(choice),
        variants,
    }))
}

// ---- Commands ----------------------------------------------------------------

/// Apply an edit through the tracker; a refusal is the user's message.
fn edit(state: &AppState, edit: ArchetypeEdit) -> Result<(), CommandError> {
    let editor = state.tracker_part(tracker::TrackerHandle::archetype_editor)?;
    let outcome = editor.edit(edit).map_err(CommandError::Tracker)?;
    match outcome {
        ArchetypeEditOutcome::Done { .. } => Ok(()),
        refusal => Err(refused(refusal.refusal().unwrap_or("Nothing changed."))),
    }
}

fn api_resolver(
    app: &AppHandle,
) -> impl FnOnce(&[u32]) -> Result<Vec<ResolvedCard>, ApiError> + '_ {
    move |ids| {
        crate::api::reference::resolve_cards(&app.state::<ApiState>(), ids, crate::wall_clock_ms())
    }
}

fn now_ms() -> i64 {
    crate::wall_clock_ms()
}

/// The manage screen's page (spec section 11).
#[tauri::command]
#[specta::specta]
pub async fn manage_archetypes(
    app: AppHandle,
    query: ManageQueryInput,
) -> Result<ManageView, CommandError> {
    let _timer = Timer::start("manage_archetypes");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let query = store::archetypes::ManageQuery {
            search: query.search,
            format: query.format,
            source: query.source,
            page: query.page,
        };
        let page =
            store::archetypes::manage_archetypes(&lock(&state.reader), &query).map_err(storage)?;
        Ok(manage_view(&page))
    })
    .await
}

/// What an archetype may be merged into, or a variant moved to (0.x
/// `MergeCandidatesController`).
#[tauri::command]
#[specta::specta]
pub async fn merge_candidates(
    app: AppHandle,
    id: u32,
) -> Result<Vec<ArchetypeChoiceView>, CommandError> {
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let options = store::archetypes::merge_candidates(&lock(&state.reader), i64::from(id))
            .map_err(storage)?;
        Ok(options.iter().map(choice).collect())
    })
    .await
}

/// Where "Move to archetype" may move a variant (T7 review I1): only the
/// targets `reassign_variant` accepts.
#[tauri::command]
#[specta::specta]
pub async fn reassign_candidates(
    app: AppHandle,
    id: u32,
) -> Result<Vec<ArchetypeChoiceView>, CommandError> {
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let options = store::archetypes::reassign_candidates(&lock(&state.reader), i64::from(id))
            .map_err(storage)?;
        Ok(options.iter().map(choice).collect())
    })
    .await
}

/// The edit page's data.
#[tauri::command]
#[specta::specta]
pub async fn archetype_form_data(
    app: AppHandle,
    id: u32,
) -> Result<Option<ArchetypeFormView>, CommandError> {
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        form_view(&lock(&state.reader), i64::from(id))
    })
    .await
}

/// The create form's match select (0.x `CreateController::matches`).
#[tauri::command]
#[specta::specta]
pub async fn create_form_matches(
    app: AppHandle,
    format: Option<String>,
    search: Option<String>,
) -> Result<Vec<FormMatchView>, CommandError> {
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let conn = lock(&state.reader);
        let scope = crate::platform::page_scope(&conn).map_err(storage)?;
        let rows = store::archetypes::form_matches(
            &conn,
            scope.platform,
            format.as_deref(),
            search.as_deref(),
        )
        .map_err(storage)?;
        Ok(rows
            .into_iter()
            .map(|row| FormMatchView {
                id: row.id,
                token: row.token,
                opponent: row.opponent,
                started_at_ms: row.started_at,
            })
            .collect())
    })
    .await
}

/// The create form's prefill from `?source_match_id=` (T8's "Create
/// archetype from this match"). `None` when the match has no opponent
/// cards.
#[tauri::command]
#[specta::specta]
pub async fn archetype_create_prefill(
    app: AppHandle,
    source_match_id: u32,
) -> Result<Option<PrefillView>, CommandError> {
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let conn = store::open_reader(&state.db_path).map_err(storage)?;
        create_prefill(&conn, Some(i64::from(source_match_id)), api_resolver(&app))
            .map_err(CommandError::Refused)
    })
    .await
}

/// "Scan match": the match's opponent cards, resolved.
#[tauri::command]
#[specta::specta]
pub async fn scan_match_cards(
    app: AppHandle,
    match_id: u32,
) -> Result<ResolvedCardsView, CommandError> {
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let conn = store::open_reader(&state.db_path).map_err(storage)?;
        scan_cards(&conn, i64::from(match_id), api_resolver(&app)).map_err(CommandError::Refused)
    })
    .await
}

/// "Upload .dek file": the user picks the file in the native dialog (the
/// UI never names a path, so no path from the page is ever opened); `None`
/// when it is cancelled.
#[tauri::command]
#[specta::specta]
pub async fn read_dek_file(app: AppHandle) -> Result<Option<ResolvedCardsView>, CommandError> {
    off_async_workers(move || {
        let mut dialog = app
            .dialog()
            .file()
            .set_title("Open Deck File")
            .add_filter("MTGO Deck", &["dek"]);
        if let Some(window) = app.get_webview_window("main") {
            dialog = dialog.set_parent(&window);
        }
        let Some(picked) = dialog.blocking_pick_file() else {
            return Ok(None);
        };
        let path = picked
            .into_path()
            .map_err(|error| CommandError::Refused(error.to_string()))?;
        let entries = read_dek_path(&path).map_err(CommandError::Refused)?;
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let conn = store::open_reader(&state.db_path).map_err(storage)?;
        resolve_with(&conn, &entries, api_resolver(&app))
            .map(Some)
            .map_err(CommandError::Refused)
    })
    .await
}

/// Create a manual archetype (0.x `StoreController`) and answer where to
/// go next.
#[tauri::command]
#[specta::specta]
pub async fn create_archetype(
    app: AppHandle,
    form: ArchetypeFormInput,
) -> Result<CreatedArchetypeView, CommandError> {
    let _timer = Timer::start("create_archetype");
    off_async_workers(move || {
        check_form(&form, true).map_err(CommandError::Refused)?;
        let device_id = app.state::<ApiState>().session().device_id.clone();
        let mut random = [0u8; 16];
        random.copy_from_slice(&crate::api::random::random_bytes(16));
        let uuid = archetypes::manual_uuid(device_id.as_deref(), random);
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        edit(
            state,
            ArchetypeEdit::CreateManual(Box::new(NewManualArchetype {
                uuid: uuid.clone(),
                name: form.name.trim().to_string(),
                format: form.format.clone(),
                color_identity: form.color_identity.clone(),
                incomplete: form.incomplete,
                source_match_id: form.source_match_id,
                cards: manual_cards(&form.cards),
                now_ms: now_ms(),
            })),
        )?;
        let conn = store::open_reader(&state.db_path).map_err(storage)?;
        let id = store::archetypes::held_uuid(&conn, &uuid)
            .map_err(storage)?
            .map(|held| held.id)
            .ok_or_else(|| CommandError::Storage("the new archetype was not found".to_string()))?;
        Ok(CreatedArchetypeView { id })
    })
    .await
}

/// Edit a manual archetype; cards add a variant (0.x `UpdateController`).
#[tauri::command]
#[specta::specta]
pub async fn update_archetype(
    app: AppHandle,
    id: u32,
    form: ArchetypeFormInput,
) -> Result<(), CommandError> {
    off_async_workers(move || {
        check_form(&form, false).map_err(CommandError::Refused)?;
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let variant = if form.cards.is_empty() {
            None
        } else {
            Some(manual_cards(&form.cards))
        };
        edit(
            state,
            ArchetypeEdit::UpdateManual {
                id: i64::from(id),
                name: form.name.trim().to_string(),
                format: form.format.clone(),
                color_identity: form.color_identity.clone(),
                variant,
                now_ms: now_ms(),
            },
        )
    })
    .await
}

/// Remove a variant (0.x `Variants/DestroyController`).
#[tauri::command]
#[specta::specta]
pub async fn remove_variant(app: AppHandle, id: u32, list: u32) -> Result<(), CommandError> {
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        edit(
            ready(&startup)?,
            ArchetypeEdit::RemoveVariant {
                id: i64::from(id),
                list: i64::from(list),
            },
        )
    })
    .await
}

/// Move a variant to another archetype; answers 0.x's flash.
#[tauri::command]
#[specta::specta]
pub async fn reassign_variant(
    app: AppHandle,
    id: u32,
    list: u32,
    target_id: u32,
) -> Result<String, CommandError> {
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        edit(
            state,
            ArchetypeEdit::ReassignVariant {
                id: i64::from(id),
                list: i64::from(list),
                target_id: i64::from(target_id),
                now_ms: now_ms(),
            },
        )?;
        let name = store::archetypes::archetype(&lock(&state.reader), i64::from(target_id))
            .map_err(storage)?
            .map(|row| row.name)
            .unwrap_or_default();
        Ok(format!("Variant reassigned to {name}."))
    })
    .await
}

/// Merge an archetype into a parent (0.x `MergeController`).
#[tauri::command]
#[specta::specta]
pub async fn merge_archetype(
    app: AppHandle,
    id: u32,
    parent_id: u32,
) -> Result<String, CommandError> {
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        edit(
            ready(&startup)?,
            ArchetypeEdit::Merge {
                source_id: i64::from(id),
                parent_id: i64::from(parent_id),
                now_ms: now_ms(),
            },
        )?;
        Ok("Archetype merged.".to_string())
    })
    .await
}

/// Undo a merge (0.x `UnmergeController`).
#[tauri::command]
#[specta::specta]
pub async fn unmerge_archetype(app: AppHandle, id: u32) -> Result<String, CommandError> {
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        edit(
            ready(&startup)?,
            ArchetypeEdit::Unmerge {
                source_id: i64::from(id),
                now_ms: now_ms(),
            },
        )?;
        Ok("Archetype unmerged.".to_string())
    })
    .await
}

/// Delete a manual archetype (0.x `DestroyController`).
#[tauri::command]
#[specta::specta]
pub async fn delete_archetype(app: AppHandle, id: u32) -> Result<(), CommandError> {
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        edit(
            ready(&startup)?,
            ArchetypeEdit::DeleteManual { id: i64::from(id) },
        )
    })
    .await
}

/// Save a `.dek` where the user picks (0.x `ExportDekController`).
#[tauri::command]
#[specta::specta]
pub async fn export_archetype_dek(
    app: AppHandle,
    id: u32,
    source: ExportSource,
) -> Result<DekExport, CommandError> {
    off_async_workers(move || {
        let (file_name, text) = {
            let startup = app.state::<Startup>();
            let state = ready(&startup)?;
            export_dek(&lock(&state.reader), i64::from(id), &source)?
        };
        let mut dialog = app
            .dialog()
            .file()
            .set_title("Save Deck File")
            .set_file_name(file_name)
            .add_filter("MTGO Deck", &["dek"]);
        if let Some(window) = app.get_webview_window("main") {
            dialog = dialog.set_parent(&window);
        }
        let Some(picked) = dialog.blocking_save_file() else {
            return Ok(DekExport::Cancelled);
        };
        let path = picked.into_path().map_err(|error| {
            CommandError::Storage(format!("could not save the deck file: {error}"))
        })?;
        std::fs::write(&path, text).map_err(|error| {
            CommandError::Storage(format!("could not save the deck file: {error}"))
        })?;
        Ok(DekExport::Saved {
            path: path.to_string_lossy().into_owned(),
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use fixture_tools::TestDir;
    use mymtgo_api::{ApiError, ResolvedCard};
    use store::archetype_edit::{ArchetypeEdit, NewManualArchetype, edit_archetypes};

    fn db(dir: &TestDir) -> store::Connection {
        let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        conn.execute_batch(
            "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);
             INSERT INTO opponents (id, username, first_seen_at) VALUES (1, 'Qopponent', 0);",
        )
        .expect("registries");
        conn
    }

    fn api_card(
        mtgo_id: u32,
        oracle: &str,
        name: &str,
        kind: &str,
        identity: &str,
    ) -> ResolvedCard {
        ResolvedCard {
            mtgo_id,
            oracle_id: Some(oracle.to_string()),
            name: Some(name.to_string()),
            card_type: Some(kind.to_string()),
            identity: Some(identity.to_string()),
            image: None,
            art_crop: None,
            cmc: None,
        }
    }

    fn entry(catalog_id: u32, quantity: u32, sideboard: bool) -> decks::DeckCard {
        decks::DeckCard {
            catalog_id,
            quantity,
            sideboard,
        }
    }

    fn never_asked(_: &[u32]) -> Result<Vec<ResolvedCard>, ApiError> {
        panic!("the API must not be asked")
    }

    // ---- 0.x ResolveCardsFromDekTest ----

    #[test]
    fn unknown_cards_are_resolved_by_the_api_and_give_the_colours() {
        let mut asked = Vec::new();
        let resolved = resolve_entries(
            &[entry(12_345, 4, false), entry(67_890, 2, true)],
            &BTreeMap::new(),
            |ids| {
                asked = ids.to_vec();
                Ok(vec![
                    api_card(12_345, "oracle-bolt", "Qbolt", "Instant", "R"),
                    api_card(67_890, "oracle-path", "Qpath", "Instant", "W"),
                ])
            },
        )
        .expect("resolved");
        assert_eq!(asked, [12_345, 67_890]);
        // WUBRG order, not the list's.
        assert_eq!(resolved.color_identity.as_deref(), Some("W,R"));
        assert_eq!(resolved.cards.len(), 2);
        let first = &resolved.cards[0];
        assert_eq!(
            (
                first.catalog_id,
                first.name.as_str(),
                first.quantity,
                first.sideboard
            ),
            (12_345, "Qbolt", 4, false)
        );
        assert_eq!(first.oracle_id.as_deref(), Some("oracle-bolt"));
    }

    #[test]
    fn lands_never_count_towards_the_colours() {
        let resolved = resolve_entries(
            &[entry(1, 8, false), entry(2, 4, false)],
            &BTreeMap::new(),
            |_| {
                Ok(vec![
                    api_card(1, "oracle-mountain", "Qmountain", "Basic Land", "R"),
                    api_card(2, "oracle-elf", "Qelf", "Creature", "G"),
                ])
            },
        )
        .expect("resolved");
        assert_eq!(resolved.color_identity.as_deref(), Some("G"));
    }

    #[test]
    fn an_api_failure_is_0xs_message() {
        let failed = resolve_entries(&[entry(12_345, 4, false)], &BTreeMap::new(), |_| {
            Err(ApiError::Status(500, "x".to_string()))
        });
        assert_eq!(failed, Err("Failed to resolve cards from API.".to_string()));
    }

    #[test]
    fn a_card_in_the_maindeck_and_the_sideboard_stays_two_rows() {
        let resolved = resolve_entries(
            &[entry(117_674, 2, false), entry(117_674, 1, true)],
            &BTreeMap::new(),
            |_| {
                Ok(vec![api_card(
                    117_674,
                    "oracle-lost",
                    "Qlost",
                    "Instant",
                    "W",
                )])
            },
        )
        .expect("resolved");
        let rows: Vec<(u32, bool)> = resolved
            .cards
            .iter()
            .map(|card| (card.quantity, card.sideboard))
            .collect();
        assert_eq!(rows, [(2, false), (1, true)]);
    }

    /// Spec section 7: cards are resolved through the local `cards` table
    /// first; only the rest go to the API, and a card nobody knows is left
    /// out (0.x keeps only what the API answered).
    #[test]
    fn known_cards_need_no_request_and_unknown_ones_are_left_out() {
        let mut local = BTreeMap::new();
        local.insert(
            5,
            CardFacts {
                oracle_id: Some("oracle-five".to_string()),
                name: Some("Qfive".to_string()),
                card_type: Some("Creature".to_string()),
                identity: Some("U".to_string()),
            },
        );
        let resolved = resolve_entries(&[entry(5, 4, false)], &local, never_asked).expect("local");
        assert_eq!(resolved.color_identity.as_deref(), Some("U"));
        assert_eq!(resolved.cards[0].name, "Qfive");

        let mut asked = Vec::new();
        let partly = resolve_entries(
            &[entry(5, 4, false), entry(6, 1, false), entry(0, 2, false)],
            &local,
            |ids| {
                asked = ids.to_vec();
                Ok(Vec::new())
            },
        )
        .expect("partly");
        assert_eq!(asked, [6]);
        assert_eq!(partly.cards.len(), 1);
    }

    #[test]
    fn the_local_facts_are_card_rows_with_a_type() {
        let dir = TestDir::new("manage-local-facts");
        let conn = db(&dir);
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, oracle_id, name, type, color_identity)
                 VALUES (5, 'oracle-five', 'Qfive', 'Creature', 'U');
             INSERT INTO cards (catalog_id, name) VALUES (6, 'Qstub');",
        )
        .expect("cards");
        let facts = local_facts(&conn, &[5, 6, 7]).expect("facts");
        assert_eq!(facts.keys().copied().collect::<Vec<_>>(), [5]);
    }

    // ---- 0.x UploadDekControllerTest ----

    const DEK: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Deck>\n  <Cards CatID=\"12345\" Quantity=\"4\" Sideboard=\"false\" Name=\"Qbolt\" Annotation=\"0\"/>\n</Deck>";

    #[test]
    fn an_opened_dek_file_is_parsed_and_resolved() {
        let dir = TestDir::new("manage-dek");
        dir.write("burn.DEK", DEK.as_bytes());
        let entries = read_dek_path(&dir.join("burn.DEK")).expect("read");
        let resolved = resolve_entries(&entries, &BTreeMap::new(), |_| {
            Ok(vec![api_card(
                12_345,
                "oracle-bolt",
                "Qbolt",
                "Instant",
                "R",
            )])
        })
        .expect("resolved");
        assert_eq!(resolved.cards.len(), 1);
        assert_eq!(resolved.color_identity.as_deref(), Some("R"));
    }

    #[test]
    fn a_file_that_is_not_a_dek_is_refused() {
        let dir = TestDir::new("manage-not-dek");
        dir.write("burn.txt", DEK.as_bytes());
        assert_eq!(
            read_dek_path(&dir.join("burn.txt")),
            Err("The file must be a .dek file.".to_string())
        );
        dir.write("broken.dek", b"not xml at all");
        assert_eq!(
            read_dek_path(&dir.join("broken.dek")),
            Err("Invalid .dek file: could not parse XML.".to_string())
        );
    }

    #[test]
    fn a_dek_file_over_the_size_limit_is_refused_unread() {
        let dir = TestDir::new("manage-huge-dek");
        dir.write("huge.dek", &vec![b' '; decks::MAX_DEK_BYTES + 1]);
        assert_eq!(
            read_dek_path(&dir.join("huge.dek")),
            Err("The file is too large to be a .dek file.".to_string())
        );
    }

    #[test]
    fn a_missing_file_is_refused() {
        assert_eq!(
            read_dek_path(std::path::Path::new("")),
            Err("The dek file field is required.".to_string())
        );
        let dir = TestDir::new("manage-missing-dek");
        assert_eq!(
            read_dek_path(&dir.join("gone.dek")),
            Err("The dek file field is required.".to_string())
        );
    }

    // ---- 0.x ScanMatchControllerTest and CreateControllerMatchesTest
    // (prefill) ----

    fn match_with_opponent_cards(conn: &store::Connection, id: i64, cards: &str) {
        conn.execute(
            "INSERT INTO matches (id, token, account_id, opponent_id, source, format,
                 match_type, state, outcome, started_at, last_activity_at, revision,
                 derived_version)
             VALUES (?1, 'Q-match-' || ?1, 1, 1, 'log', 'CMODERN', 'Modern', 'complete', 'win',
                 100, 100, 1, 0)",
            [id],
        )
        .expect("match");
        conn.execute(
            "INSERT INTO games (id, match_id, game_number, result, end_reason)
             VALUES (?1 * 10, ?1, 1, 'win', 'win')",
            [id],
        )
        .expect("game");
        if !cards.is_empty() {
            conn.execute_batch(&format!(
                "INSERT INTO game_cards (game_id, side, catalog_id, main_qty, side_qty, seen)
                 VALUES {cards};"
            ))
            .expect("cards");
        }
    }

    #[test]
    fn a_scan_resolves_the_opponents_cards() {
        let dir = TestDir::new("manage-scan");
        let conn = db(&dir);
        match_with_opponent_cards(&conn, 1, "(10, 'opponent', 12345, 0, 0, 6)");
        let scanned = scan_cards(&conn, 1, |_| {
            Ok(vec![api_card(
                12_345,
                "oracle-bolt",
                "Qbolt",
                "Instant",
                "R",
            )])
        })
        .expect("scan");
        assert_eq!(
            scanned
                .cards
                .iter()
                .map(|card| (card.quantity, card.sideboard))
                .collect::<Vec<_>>(),
            [(4, false)]
        );
    }

    #[test]
    fn a_scan_without_opponent_cards_is_0xs_message() {
        let dir = TestDir::new("manage-scan-empty");
        let conn = db(&dir);
        match_with_opponent_cards(&conn, 1, "");
        assert_eq!(
            scan_cards(&conn, 1, never_asked),
            Err("No opponent cards found in this match.".to_string())
        );
    }

    #[test]
    fn the_prefill_is_the_matchs_format_colours_and_cards() {
        let dir = TestDir::new("manage-prefill");
        let conn = db(&dir);
        match_with_opponent_cards(&conn, 1, "(10, 'opponent', 12345, 0, 0, 2)");
        let prefill = create_prefill(&conn, Some(1), |_| {
            Ok(vec![api_card(
                12_345,
                "oracle-bolt",
                "Qbolt",
                "Instant",
                "R",
            )])
        })
        .expect("prefill")
        .expect("some");
        assert_eq!(prefill.source_match_id, 1);
        assert_eq!(prefill.format.as_deref(), Some("modern"));
        assert_eq!(prefill.cards.color_identity.as_deref(), Some("R"));
        assert_eq!(prefill.cards.cards.len(), 1);
        assert_eq!(prefill.cards.cards[0].name, "Qbolt");
    }

    #[test]
    fn there_is_no_prefill_without_opponent_cards_or_a_source_match() {
        let dir = TestDir::new("manage-prefill-none");
        let conn = db(&dir);
        match_with_opponent_cards(&conn, 1, "");
        assert_eq!(create_prefill(&conn, Some(1), never_asked), Ok(None));
        assert_eq!(create_prefill(&conn, None, never_asked), Ok(None));
    }

    /// The opponent's Endstep cards of match 1: a named stub (row 1), an
    /// unnamed stub (row 2) and a card with no `cards` row (row 3).
    fn match_with_endstep_cards(conn: &store::Connection) {
        let named = cards::endstep_catalog_id(1);
        let unnamed = cards::endstep_catalog_id(2);
        let rowless = cards::endstep_catalog_id(3);
        conn.execute_batch(&format!(
            "INSERT INTO cards (catalog_id, name) VALUES ({named}, 'Qstub');
             INSERT INTO cards (catalog_id) VALUES ({unnamed});"
        ))
        .expect("cards");
        match_with_opponent_cards(
            conn,
            1,
            &format!(
                "(10, 'opponent', {named}, 0, 0, 3),
                 (10, 'opponent', {unnamed}, 0, 0, 1),
                 (10, 'opponent', {rowless}, 0, 0, 1)"
            ),
        );
    }

    #[test]
    fn archetype_scan_never_sends_endstep() {
        let dir = TestDir::new("manage-scan-endstep-outbound");
        let conn = db(&dir);
        match_with_endstep_cards(&conn);
        scan_cards(&conn, 1, never_asked).expect("scan");
        create_prefill(&conn, Some(1), never_asked).expect("prefill");
    }

    #[test]
    fn an_unresolved_endstep_card_is_scanned_by_its_name() {
        let dir = TestDir::new("manage-scan-endstep-named");
        let conn = db(&dir);
        match_with_endstep_cards(&conn);
        let named = cards::endstep_catalog_id(1);

        let scanned = scan_cards(&conn, 1, never_asked).expect("scan");
        let names: Vec<(u32, &str)> = scanned
            .cards
            .iter()
            .map(|card| (card.catalog_id, card.name.as_str()))
            .collect();
        assert_eq!(names, [(named, "Qstub")]);
        assert_eq!(scanned.cards[0].type_line, None);

        let prefill = create_prefill(&conn, Some(1), never_asked)
            .expect("prefill")
            .expect("some");
        assert_eq!(prefill.cards.cards.len(), 1);
        assert_eq!(prefill.cards.cards[0].name, "Qstub");
    }

    /// The opponent's Arena cards of match 1: a named stub (grpId 1), an
    /// unnamed stub (grpId 2) and a card with no `cards` row (grpId 3).
    fn match_with_arena_cards(conn: &store::Connection) {
        let named = cards::arena_catalog_id(cards::GrpId(1));
        let unnamed = cards::arena_catalog_id(cards::GrpId(2));
        let rowless = cards::arena_catalog_id(cards::GrpId(3));
        conn.execute_batch(&format!(
            "INSERT INTO cards (catalog_id, name) VALUES ({named}, 'Qarena');
             INSERT INTO cards (catalog_id) VALUES ({unnamed});"
        ))
        .expect("cards");
        match_with_opponent_cards(
            conn,
            1,
            &format!(
                "(10, 'opponent', {named}, 0, 0, 2),
                 (10, 'opponent', {unnamed}, 0, 0, 1),
                 (10, 'opponent', {rowless}, 0, 0, 1)"
            ),
        );
    }

    /// The API resolves MTGO ids only: an Arena id is never sent as one
    /// (#694).
    #[test]
    fn archetype_scan_never_sends_arena_ids() {
        let dir = TestDir::new("manage-scan-arena-outbound");
        let conn = db(&dir);
        match_with_arena_cards(&conn);
        scan_cards(&conn, 1, never_asked).expect("scan");
        create_prefill(&conn, Some(1), never_asked).expect("prefill");
    }

    #[test]
    fn an_unresolved_arena_card_is_scanned_by_its_name() {
        let dir = TestDir::new("manage-scan-arena-named");
        let conn = db(&dir);
        match_with_arena_cards(&conn);
        let named = cards::arena_catalog_id(cards::GrpId(1));

        let scanned = scan_cards(&conn, 1, never_asked).expect("scan");
        let names: Vec<(u32, &str)> = scanned
            .cards
            .iter()
            .map(|card| (card.catalog_id, card.name.as_str()))
            .collect();
        assert_eq!(names, [(named, "Qarena")]);
        assert_eq!(scanned.cards[0].type_line, None);
    }

    // ---- 0.x StoreController and UpdateController validation ----

    fn form(name: &str, format: &str, cards: usize) -> ArchetypeFormInput {
        ArchetypeFormInput {
            name: name.to_string(),
            format: format.to_string(),
            color_identity: None,
            cards: (0..cards)
                .map(|index| FormCardInput {
                    catalog_id: 100 + u32::try_from(index).expect("index"),
                    oracle_id: None,
                    name: None,
                    quantity: 1,
                    sideboard: false,
                })
                .collect(),
            source_match_id: None,
            incomplete: false,
        }
    }

    #[test]
    fn the_form_needs_a_name_a_format_and_cards() {
        assert_eq!(check_form(&form("Qburn", "modern", 1), true), Ok(()));
        assert_eq!(
            check_form(&form("  ", "modern", 1), true),
            Err("The name field is required.".to_string())
        );
        assert_eq!(
            check_form(&form(&"Q".repeat(256), "modern", 1), true),
            Err("The name field must not be greater than 255 characters.".to_string())
        );
        assert_eq!(
            check_form(&form(&"Q".repeat(255), "modern", 1), true),
            Ok(())
        );
        assert_eq!(
            check_form(&form("Qburn", "", 1), true),
            Err("The format field is required.".to_string())
        );
        assert_eq!(
            check_form(&form("Qburn", "modern", 0), true),
            Err("The cards field is required.".to_string())
        );
        // An edit may come without cards.
        assert_eq!(check_form(&form("Qburn", "modern", 0), false), Ok(()));
        let mut zero = form("Qburn", "modern", 1);
        zero.cards[0].quantity = 0;
        assert_eq!(
            check_form(&zero, true),
            Err("The cards.0.quantity field must be at least 1.".to_string())
        );
    }

    // ---- 0.x ExportDekTest, GenerateDekFileTest, FallbackControllerGuardsTest ----

    fn manual(conn: &mut store::Connection, lists: &[&[(u32, u32, bool)]]) -> i64 {
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, oracle_id, name, type) VALUES
                 (11111, 'oracle-shock', 'Qshock', 'Instant'),
                 (22222, 'oracle-counter', 'Qcounter', 'Instant');",
        )
        .expect("cards");
        let to_cards = |list: &[(u32, u32, bool)]| -> Vec<store::archetype_edit::ManualCard> {
            list.iter()
                .map(
                    |(catalog_id, quantity, sideboard)| store::archetype_edit::ManualCard {
                        catalog_id: i64::from(*catalog_id),
                        oracle_id: None,
                        name: None,
                        quantity: *quantity,
                        sideboard: *sideboard,
                    },
                )
                .collect()
        };
        edit_archetypes(
            conn,
            &ArchetypeEdit::CreateManual(Box::new(NewManualArchetype {
                uuid: "00000000-q-export".to_string(),
                name: "Mono Red: Burn!".to_string(),
                format: "modern".to_string(),
                color_identity: None,
                incomplete: false,
                source_match_id: None,
                cards: to_cards(lists[0]),
                now_ms: 1,
            })),
        )
        .expect("create");
        let id: i64 = conn
            .query_row(
                "SELECT id FROM archetypes WHERE uuid = '00000000-q-export'",
                [],
                |row| row.get(0),
            )
            .expect("id");
        for list in &lists[1..] {
            edit_archetypes(
                conn,
                &ArchetypeEdit::UpdateManual {
                    id,
                    name: "Mono Red: Burn!".to_string(),
                    format: "modern".to_string(),
                    color_identity: None,
                    variant: Some(to_cards(list)),
                    now_ms: 2,
                },
            )
            .expect("variant");
        }
        id
    }

    #[test]
    fn the_export_is_0xs_dek_xml_of_the_first_variant_by_default() {
        let dir = TestDir::new("manage-export-default");
        let mut conn = db(&dir);
        let id = manual(&mut conn, &[&[(11111, 4, false)], &[(22222, 4, false)]]);
        let (file_name, xml) =
            export_dek(&conn, id, &ExportSource::List { list: None }).expect("export");
        assert_eq!(file_name, "mono-red-burn.dek");
        assert_eq!(
            xml,
            [
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
                "<Deck xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">",
                "  <NetDeckID>0</NetDeckID>",
                "  <PreconstructedDeckID>0</PreconstructedDeckID>",
                "  <Cards CatID=\"11111\" Quantity=\"4\" Sideboard=\"false\" Name=\"Qshock\" Annotation=\"0\"/>",
                "</Deck>",
            ]
            .join("\n")
        );
    }

    #[test]
    fn the_export_takes_the_chosen_variant() {
        let dir = TestDir::new("manage-export-chosen");
        let mut conn = db(&dir);
        let id = manual(&mut conn, &[&[(11111, 4, false)], &[(22222, 2, true)]]);
        let (_, xml) =
            export_dek(&conn, id, &ExportSource::List { list: Some(2) }).expect("export");
        assert!(
            xml.contains("CatID=\"22222\" Quantity=\"2\" Sideboard=\"true\" Name=\"Qcounter\"")
        );
        assert!(!xml.contains("11111"));
    }

    /// A live decklist (the detail page, T10) is exported from its cards.
    #[test]
    fn the_export_writes_a_given_decklist() {
        let dir = TestDir::new("manage-export-decklist");
        let mut conn = db(&dir);
        let id = manual(&mut conn, &[&[(11111, 4, false)]]);
        let source = ExportSource::Decklist {
            cards: vec![ExportCard {
                catalog_id: 22222,
                quantity: 3,
                sideboard: false,
                name: "Qcounter".to_string(),
            }],
        };
        let (_, xml) = export_dek(&conn, id, &source).expect("export");
        assert!(xml.contains("CatID=\"22222\" Quantity=\"3\""));
    }

    #[test]
    fn an_archetype_without_a_list_or_a_fallback_has_nothing_to_export() {
        let dir = TestDir::new("manage-export-none");
        let conn = db(&dir);
        conn.execute(
            "INSERT INTO archetypes (id, uuid, name, format, source, created_at, updated_at)
             VALUES (50, 'q-empty', 'Qempty', 'modern', 'downloaded', 1, 1)",
            [],
        )
        .expect("archetype");
        assert_eq!(
            export_dek(&conn, 50, &ExportSource::List { list: None }),
            Err(CommandError::Refused(
                "Archetype has no decklist to export.".to_string()
            ))
        );
        assert_eq!(
            export_dek(&conn, 1, &ExportSource::List { list: None }),
            Err(CommandError::Refused(
                "Fallback archetypes have no decklist to export.".to_string()
            ))
        );
    }

    #[test]
    fn the_file_name_is_laravels_slug() {
        assert_eq!(slug("Mono Red: Burn!"), "mono-red-burn");
        assert_eq!(slug("  Izzet  Murktide  "), "izzet-murktide");
        assert_eq!(slug("Qa_b@c"), "qa-b-at-c");
        assert_eq!(slug("!!!"), "");
        assert_eq!(dek_name("!!!"), "archetype.dek");
    }

    /// #300 F9: Laravel's `Str::slug` runs `Str::ascii` first, so accented
    /// letters become plain ones and the file name is ASCII.
    #[test]
    fn the_file_name_transliterates_like_str_ascii() {
        assert_eq!(slug("Æther Déjà Vu"), "aether-deja-vu");
        assert_eq!(slug("Jötun Lim-Dûl Straße"), "jotun-lim-dul-strasse");
        assert_eq!(slug("Qœuvre Ñandú Ø"), "qoeuvre-nandu-o");
        assert_eq!(dek_name("Séance"), "seance.dek");
    }

    #[test]
    fn form_cards_become_manual_cards() {
        let input = vec![FormCardInput {
            catalog_id: 5,
            oracle_id: Some("oracle-five".to_string()),
            name: Some("Qfive".to_string()),
            quantity: 2,
            sideboard: true,
        }];
        let cards = manual_cards(&input);
        assert_eq!(cards[0].catalog_id, 5);
        assert_eq!(cards[0].name.as_deref(), Some("Qfive"));
        assert_eq!((cards[0].quantity, cards[0].sideboard), (2, true));
    }
}
