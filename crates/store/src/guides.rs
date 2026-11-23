//! Sideboard guides and matchup notes, read (archetypes spec section 21.3,
//! survey section 13.3): a deck's guide list, one guide's editor and an
//! archetype's guides and notes block. The writes are `ArchetypeEdit`'s
//! guide and note variants (`archetype_edit::guides`).

use std::collections::{BTreeMap, HashMap};

use archetypes::guides::{
    ArchetypeNotes, CardMeta, Direction, GuideCard, GuideEditor, Note, VersionCard, Zones,
    editor_rows, notes_for, plan_summary, zone_quantities,
};
use decks::stats::Range;
use rusqlite::{Connection, OptionalExtension};

use crate::StoreError;
use crate::platform::{Platform, deck_of_platform};

/// A deck's all-time, all-version numbers against one opponent archetype
/// (0.x `GetArchetypeMatchupSpread::run($deck, null, null)`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Matchup {
    pub matches: u32,
    /// "W - L" (" - D" added when there are draws), `None` when never faced.
    pub match_record: Option<String>,
    /// Integers, `None` when never faced.
    pub match_winrate: Option<u32>,
    pub game_winrate: Option<u32>,
}

/// One row of a deck's guide list (0.x `SideboardGuideSummaryData`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideSummary {
    pub id: i64,
    pub deck_id: i64,
    pub archetype_id: i64,
    pub archetype_name: String,
    /// Canonical ("W,U,B"), `None` without colours.
    pub archetype_color_identity: Option<String>,
    pub archetype_retired: bool,
    pub archetype_fallback: bool,
    /// Planned copies summed per direction.
    pub cards_in: u32,
    pub cards_out: u32,
    /// Notes of the pair.
    pub notes_count: u32,
    pub updated_at: i64,
    pub matchup: Matchup,
}

/// The guide editor page (0.x `EditController`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideEditorPage {
    pub summary: GuideSummary,
    /// The deck has a version to plan with.
    pub has_version: bool,
    /// The plan as saved, in `(direction, oracle_id)` order.
    pub cards: Vec<GuideCard>,
    /// Copies in the latest version's sideboard (the balance chip).
    pub sideboard_size: u32,
    /// The columns over the latest version; `None` without a version.
    pub editor: Option<GuideEditor>,
    pub notes: ArchetypeNotes,
}

/// One planned card named for the archetype page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedCard {
    pub oracle_id: String,
    pub name: String,
    pub quantity: u32,
}

/// One deck's block on the archetype page (archetypes spec section 21.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchetypeGuideRow {
    pub deck_id: i64,
    pub deck_name: String,
    pub deck_archived: bool,
    /// The pair's guide, `None` when the deck has only notes.
    pub guide_id: Option<i64>,
    pub cards_in: u32,
    pub cards_out: u32,
    /// By name.
    pub planned_in: Vec<PlannedCard>,
    pub planned_out: Vec<PlannedCard>,
    pub matchup: Matchup,
    /// The deck's notes against the archetype, newest first.
    pub notes: Vec<Note>,
}

/// One guide row as stored, before its numbers.
struct GuideHead {
    id: i64,
    deck_id: i64,
    archetype_id: i64,
    archetype_name: String,
    archetype_color_identity: Option<String>,
    archetype_retired: bool,
    archetype_fallback: bool,
    updated_at: i64,
}

const GUIDE_HEAD: &str = "SELECT g.id, g.deck_id, g.archetype_id, a.name, a.color_identity,
         a.retired_at IS NOT NULL, a.source = 'fallback', g.updated_at
     FROM sideboard_guides g JOIN archetypes a ON a.id = g.archetype_id";

fn guide_head(row: &rusqlite::Row) -> rusqlite::Result<GuideHead> {
    Ok(GuideHead {
        id: row.get(0)?,
        deck_id: row.get(1)?,
        archetype_id: row.get(2)?,
        archetype_name: row.get(3)?,
        archetype_color_identity: row.get(4)?,
        archetype_retired: row.get(5)?,
        archetype_fallback: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

/// A guide's planned cards in `(direction, oracle_id)` order.
pub fn guide_cards(conn: &Connection, guide_id: i64) -> Result<Vec<GuideCard>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT oracle_id, direction, quantity FROM sideboard_guide_cards
         WHERE guide_id = ?1 ORDER BY direction, oracle_id",
    )?;
    let rows = stmt.query_map([guide_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, u32>(2)?,
        ))
    })?;
    let mut cards = Vec::new();
    for row in rows {
        let (oracle_id, direction, quantity) = row?;
        // The CHECK allows only `in` and `out`.
        if let Some(direction) = Direction::parse(&direction) {
            cards.push(GuideCard {
                oracle_id,
                direction,
                quantity,
            });
        }
    }
    Ok(cards)
}

/// Notes of one pair.
fn pair_note_count(conn: &Connection, deck_id: i64, archetype_id: i64) -> Result<u32, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT COUNT(*) FROM deck_archetype_notes WHERE deck_id = ?1 AND archetype_id = ?2",
        )?
        .query_row([deck_id, archetype_id], |row| row.get(0))?)
}

fn summarise(
    conn: &Connection,
    head: GuideHead,
    matchups: &HashMap<i64, Matchup>,
) -> Result<GuideSummary, StoreError> {
    let (cards_in, cards_out) = plan_summary(&guide_cards(conn, head.id)?);
    Ok(GuideSummary {
        id: head.id,
        deck_id: head.deck_id,
        archetype_id: head.archetype_id,
        archetype_name: head.archetype_name,
        archetype_color_identity: archetypes::normalize_colors(
            head.archetype_color_identity.as_deref(),
        ),
        archetype_retired: head.archetype_retired,
        archetype_fallback: head.archetype_fallback,
        cards_in,
        cards_out,
        notes_count: pair_note_count(conn, head.deck_id, head.archetype_id)?,
        updated_at: head.updated_at,
        matchup: matchups
            .get(&head.archetype_id)
            .cloned()
            .unwrap_or_default(),
    })
}

/// A deck's guides by archetype name, with plan totals, note counts and the
/// deck's all-time record against each (0.x `GetSideboardGuideSummaries`).
/// Archived decks included.
pub fn deck_guides(conn: &Connection, deck_id: i64) -> Result<Vec<GuideSummary>, StoreError> {
    let mut heads = Vec::new();
    {
        let mut stmt = conn.prepare_cached(&format!(
            "{GUIDE_HEAD} WHERE g.deck_id = ?1 ORDER BY a.name, g.id"
        ))?;
        let rows = stmt.query_map([deck_id], guide_head)?;
        for row in rows {
            heads.push(row?);
        }
    }
    if heads.is_empty() {
        return Ok(Vec::new());
    }
    let matchups = deck_matchups(conn, deck_id)?;
    let mut summaries = Vec::new();
    for head in heads {
        summaries.push(summarise(conn, head, &matchups)?);
    }
    Ok(summaries)
}

/// One guide's summary (0.x `GetSideboardGuideSummaries::forGuide`), `None`
/// when there is no such guide.
pub fn guide_summary(conn: &Connection, guide_id: i64) -> Result<Option<GuideSummary>, StoreError> {
    let head = conn
        .prepare_cached(&format!("{GUIDE_HEAD} WHERE g.id = ?1"))?
        .query_row([guide_id], guide_head)
        .optional()?;
    let Some(head) = head else {
        return Ok(None);
    };
    let matchups = deck_matchups(conn, head.deck_id)?;
    Ok(Some(summarise(conn, head, &matchups)?))
}

/// The guide editor (0.x `EditController`): the summary, the plan, the
/// columns over the deck's latest version with the numbers of every
/// version against the archetype, and the archetype's notes.
pub fn guide_editor(
    conn: &Connection,
    guide_id: i64,
) -> Result<Option<GuideEditorPage>, StoreError> {
    let Some(summary) = guide_summary(conn, guide_id)? else {
        return Ok(None);
    };
    let cards = guide_cards(conn, guide_id)?;
    let latest = latest_version_cards(conn, summary.deck_id)?;
    let has_version = latest.is_some();
    let (editor, sideboard_size) = match latest {
        Some(version_cards) => {
            let zones = zone_quantities(&version_cards);
            let (stats, postboard) =
                crate::card_stats::sided_stats(conn, summary.deck_id, summary.archetype_id)?;
            let meta = card_meta(conn, &zones, &cards, &version_cards)?;
            let editor = editor_rows(&zones, &cards, &stats, postboard, &meta);
            (Some(editor), zones.sideboard_size())
        }
        None => (None, 0),
    };
    let notes = archetype_notes(conn, summary.deck_id, summary.archetype_id)?;
    Ok(Some(GuideEditorPage {
        summary,
        has_version,
        cards,
        sideboard_size,
        editor,
        notes,
    }))
}

/// Every note against the archetype with its deck's name, split into this
/// deck's and the others' (0.x `GetArchetypeNotes`). Archived decks'
/// notes included.
pub fn archetype_notes(
    conn: &Connection,
    deck_id: i64,
    archetype_id: i64,
) -> Result<ArchetypeNotes, StoreError> {
    Ok(notes_for(deck_id, notes_of(conn, archetype_id)?))
}

fn notes_of(conn: &Connection, archetype_id: i64) -> Result<Vec<Note>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT n.id, n.deck_id, d.name, n.body, n.created_at
         FROM deck_archetype_notes n JOIN decks d ON d.id = n.deck_id
         WHERE n.archetype_id = ?1",
    )?;
    let rows = stmt.query_map([archetype_id], |row| {
        Ok(Note {
            id: row.get(0)?,
            deck_id: row.get(1)?,
            deck_name: row.get(2)?,
            body: row.get(3)?,
            created_at: row.get(4)?,
        })
    })?;
    let mut notes = Vec::new();
    for row in rows {
        notes.push(row?);
    }
    Ok(notes)
}

/// The archetype page's block (spec section 21.4): one row per deck that
/// has a guide or notes against the archetype, decks in use first, then by
/// deck name. Only decks of `platform` (an MTGO deck's guide never shows on
/// an Arena archetype page, and the other way round).
pub fn archetype_guides(
    conn: &Connection,
    platform: Platform,
    archetype_id: i64,
) -> Result<Vec<ArchetypeGuideRow>, StoreError> {
    let mut decks: Vec<(i64, String, bool, Option<i64>)> = Vec::new();
    {
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT d.id, d.name, d.archived_at IS NOT NULL,
                 (SELECT g.id FROM sideboard_guides g
                  WHERE g.deck_id = d.id AND g.archetype_id = ?1)
             FROM decks d
             WHERE (EXISTS (SELECT 1 FROM sideboard_guides g
                            WHERE g.deck_id = d.id AND g.archetype_id = ?1)
                 OR EXISTS (SELECT 1 FROM deck_archetype_notes n
                            WHERE n.deck_id = d.id AND n.archetype_id = ?1))
               AND {of_platform}
             ORDER BY d.archived_at IS NOT NULL, d.name, d.id",
            of_platform = deck_of_platform("d", 2)
        ))?;
        let rows = stmt.query_map(rusqlite::params![archetype_id, platform], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
        for row in rows {
            decks.push(row?);
        }
    }
    let notes = notes_of(conn, archetype_id)?;
    let mut out = Vec::new();
    for (deck_id, deck_name, deck_archived, guide_id) in decks {
        let cards = match guide_id {
            Some(id) => guide_cards(conn, id)?,
            None => Vec::new(),
        };
        let (cards_in, cards_out) = plan_summary(&cards);
        let mut planned_in = Vec::new();
        let mut planned_out = Vec::new();
        for card in &cards {
            let planned = PlannedCard {
                oracle_id: card.oracle_id.clone(),
                name: oracle_name(conn, &card.oracle_id)?,
                quantity: card.quantity,
            };
            match card.direction {
                Direction::In => planned_in.push(planned),
                Direction::Out => planned_out.push(planned),
            }
        }
        planned_in.sort_by(|a, b| (&a.name, &a.oracle_id).cmp(&(&b.name, &b.oracle_id)));
        planned_out.sort_by(|a, b| (&a.name, &a.oracle_id).cmp(&(&b.name, &b.oracle_id)));
        let deck_notes = notes_for(deck_id, notes.clone()).current;
        let matchup = deck_matchups(conn, deck_id)?
            .remove(&archetype_id)
            .unwrap_or_default();
        out.push(ArchetypeGuideRow {
            deck_id,
            deck_name,
            deck_archived,
            guide_id,
            cards_in,
            cards_out,
            planned_in,
            planned_out,
            matchup,
            notes: deck_notes,
        });
    }
    Ok(out)
}

/// A guide's `(deck_id, archetype_id)`, `None` when there is no such guide.
pub fn guide_pair(conn: &Connection, guide_id: i64) -> Result<Option<(i64, i64)>, StoreError> {
    Ok(conn
        .prepare_cached("SELECT deck_id, archetype_id FROM sideboard_guides WHERE id = ?1")?
        .query_row([guide_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?)
}

/// A note's `(deck_id, archetype_id)`, `None` when there is no such note.
pub fn note_pair(conn: &Connection, note_id: i64) -> Result<Option<(i64, i64)>, StoreError> {
    Ok(conn
        .prepare_cached("SELECT deck_id, archetype_id FROM deck_archetype_notes WHERE id = ?1")?
        .query_row([note_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?)
}

/// One deck the archetype page's "Add note" and "Create guide" offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideDeckOption {
    pub deck_id: i64,
    pub name: String,
    /// Complete listed matches of the deck against the archetype.
    pub matches: u32,
}

/// The decks the archetype page's block offers (spec section 21.4): the
/// account's decks in use (every one of the platform without a current
/// account) of the
/// archetype's format (any constructed format for a fallback), the deck
/// most played against it first, then by name. Nothing for an unknown
/// archetype.
pub fn guide_deck_options(
    conn: &Connection,
    account: Option<i64>,
    platform: Platform,
    archetype_id: i64,
) -> Result<Vec<GuideDeckOption>, StoreError> {
    let format: Option<Option<String>> = conn
        .prepare_cached("SELECT format FROM archetypes WHERE id = ?1")?
        .query_row([archetype_id], |row| row.get(0))
        .optional()?;
    let Some(format) = format else {
        return Ok(Vec::new());
    };
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT d.id, d.name, d.format,
             (SELECT COUNT(*) FROM deck_versions v
              JOIN listed_matches m ON m.deck_version_id = v.id
              JOIN match_archetypes x ON x.match_id = m.id AND x.is_opponent = 1
              WHERE v.deck_id = d.id AND m.state = 'complete' AND x.archetype_id = ?2)
         FROM decks d
         WHERE d.archived_at IS NULL AND (?1 IS NULL OR d.account_id = ?1)
           AND {of_platform}",
        of_platform = deck_of_platform("d", 3)
    ))?;
    let rows = stmt.query_map(rusqlite::params![account, archetype_id, platform], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, u32>(3)?,
        ))
    })?;
    let mut options = Vec::new();
    for row in rows {
        let (deck_id, name, deck_format, matches) = row?;
        let Some(key) = archetypes::format_key(&deck_format) else {
            continue;
        };
        if format.as_ref().is_some_and(|format| *format != key) {
            continue;
        }
        options.push(GuideDeckOption {
            deck_id,
            name,
            matches,
        });
    }
    options.sort_by(|a, b| {
        b.matches
            .cmp(&a.matches)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.deck_id.cmp(&b.deck_id))
    });
    Ok(options)
}

/// A card name by oracle id (the newest printing with a name), "Unknown
/// card" without one (0.x).
fn oracle_name(conn: &Connection, oracle_id: &str) -> Result<String, StoreError> {
    let name: Option<String> = conn
        .prepare_cached(
            "SELECT name FROM cards WHERE oracle_id = ?1 AND name IS NOT NULL
             ORDER BY catalog_id DESC LIMIT 1",
        )?
        .query_row([oracle_id], |row| row.get(0))
        .optional()?;
    Ok(name.unwrap_or_else(|| "Unknown card".to_string()))
}

/// The deck's latest version's cards with their oracle ids (Plan H's
/// signature decoding), `None` without a version.
pub fn latest_version_cards(
    conn: &Connection,
    deck_id: i64,
) -> Result<Option<Vec<VersionCard>>, StoreError> {
    let versions = crate::decks::versions_of(conn, deck_id)?;
    let Some(latest) = versions.last() else {
        return Ok(None);
    };
    let mut stmt = conn.prepare_cached("SELECT oracle_id FROM cards WHERE catalog_id = ?1")?;
    let mut cards = Vec::new();
    for card in decks::decode_signature(&latest.signature) {
        let oracle_id: Option<String> = stmt
            .query_row([card.catalog_id], |row| row.get(0))
            .optional()?
            .flatten();
        cards.push(VersionCard {
            catalog_id: card.catalog_id,
            oracle_id,
            quantity: card.quantity,
            sideboard: card.sideboard,
        });
    }
    Ok(Some(cards))
}

/// The latest version's zones, empty without a version (0.x).
pub fn latest_zones(conn: &Connection, deck_id: i64) -> Result<Zones, StoreError> {
    Ok(latest_version_cards(conn, deck_id)?
        .map(|cards| zone_quantities(&cards))
        .unwrap_or_default())
}

/// Card data per oracle id listed in the editor: the version's own
/// printing first, else the newest printing on file (0.x
/// `BuildSideboardGuide::cardMetadata`).
fn card_meta(
    conn: &Connection,
    zones: &Zones,
    planned: &[GuideCard],
    version_cards: &[VersionCard],
) -> Result<BTreeMap<String, CardMeta>, StoreError> {
    let mut oracles: Vec<&String> = zones
        .sideboard
        .keys()
        .chain(zones.maindeck.keys())
        .collect();
    oracles.extend(planned.iter().map(|card| &card.oracle_id));
    oracles.sort();
    oracles.dedup();
    let own: Vec<u32> = version_cards.iter().map(|card| card.catalog_id).collect();
    let mut stmt = conn.prepare_cached(
        "SELECT catalog_id, name, type, color_identity, image, local_image
         FROM cards WHERE oracle_id = ?1 ORDER BY catalog_id DESC",
    )?;
    let mut meta = BTreeMap::new();
    for oracle_id in oracles {
        let rows = stmt.query_map([oracle_id], |row| {
            Ok(CardMeta {
                catalog_id: Some(row.get(0)?),
                name: row.get(1)?,
                card_type: row.get(2)?,
                color_identity: row.get(3)?,
                image: row.get(4)?,
                local_image: row.get(5)?,
            })
        })?;
        let mut chosen: Option<CardMeta> = None;
        for row in rows {
            let row = row?;
            let is_own = row.catalog_id.is_some_and(|id| own.contains(&id));
            if is_own {
                chosen = Some(row);
                break;
            }
            if chosen.is_none() {
                chosen = Some(row);
            }
        }
        if let Some(mut chosen) = chosen {
            chosen.color_identity = cards::normalize_colors(chosen.color_identity.as_deref());
            meta.insert(oracle_id.clone(), chosen);
        }
    }
    Ok(meta)
}

/// The deck's numbers against every opponent archetype, all versions and
/// all time (0.x `GetArchetypeMatchupSpread::run($deck, null, null)`): the
/// deck Matchups tab's read (`matchups::matchup_spread`), so both pages
/// show the same numbers. A draw is neither a win nor a loss but counts as
/// played.
fn deck_matchups(conn: &Connection, deck_id: i64) -> Result<HashMap<i64, Matchup>, StoreError> {
    let versions = crate::matchups::deck_version_ids(conn, deck_id, None)?;
    let spread = crate::matchups::matchup_spread(conn, &versions, Range::all_time(), None)?;
    let mut out = HashMap::new();
    for row in spread {
        let record = row.record;
        let mut text = format!("{} - {}", record.wins, record.losses);
        if record.draws > 0 {
            text.push_str(&format!(" - {}", record.draws));
        }
        out.insert(
            row.archetype_id,
            Matchup {
                matches: record.total,
                match_record: Some(text),
                match_winrate: Some(record.win_rate),
                game_winrate: Some(row.games.win_rate),
            },
        );
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests;
