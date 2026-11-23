//! Reading 0.x drafts, picks with their notes, and limited registered decks
//! (limited spec section 7, import audit section 5.5). The tables are
//! optional: an old 0.x without `drafts` and `limited_deck_snapshots` has no
//! Limited data. Values are read loosely, as in `read`.
//!
//! A limited match's registered deck is its `limited_deck_snapshots` row;
//! a match of a limited league without one takes its limited deck version's
//! cards (0.x's `limited:` decks, which `read_decks` leaves out), captured at
//! the match's start. v1 keeps no limited deck rows (limited ruling 3).

use std::collections::{HashMap, HashSet};

use chrono::DateTime;
use limited::{Draft, Pick, RegisteredCard, Reservation, registered_cards_from_json};
use rusqlite::Connection;
use serde_json::Value;
use store::import_limited::{ImportedLimited, LegacyDraft, LegacyLimitedDeck};

use crate::parity_rules::{
    DEFAULT_PACK_SIZE, DEFAULT_PICKS_EXPECTED, DEFAULT_SEAT_COUNT, draft_state, large, small,
};
use crate::read::{ImportError, read_error};
use crate::sql::{
    as_integer, as_text, column_list, has_all, has_column, parse_id, parse_utc_ms, select_list,
    table_columns, value,
};

/// `drafts` columns without which a row means nothing.
const DRAFT_KEYS: [&str; 2] = ["id", "draft_token"];
/// `drafts` columns read when present, in the order `read_drafts` reads them.
const DRAFT_COLUMNS: [&str; 11] = [
    "league_id",
    "mtgo_draft_id",
    "pod_token",
    "seat_count",
    "seat_index",
    "booster_catalog_id",
    "state",
    "pack_size",
    "picks_expected",
    "started_at",
    "ended_at",
];
/// `draft_picks` columns without which a row means nothing.
const PICK_KEYS: [&str; 2] = ["draft_id", "ordinal"];
/// `draft_picks` columns read when present, in the order `read_picks` reads
/// them. `id` only orders rows.
const PICK_COLUMNS: [&str; 13] = [
    "pack_number",
    "pick_number",
    "pack_id",
    "direction",
    "cards_available",
    "picked_catalog_id",
    "picked_card_id",
    "reservations",
    "shown_at",
    "deadline_at",
    "picked_at",
    "note",
    "id",
];
/// `limited_deck_snapshots` columns without which a row means nothing.
const SNAPSHOT_KEYS: [&str; 3] = ["league_id", "cards", "captured_at"];
/// `limited_deck_snapshots` columns read when present. `id` only orders
/// rows.
const SNAPSHOT_COLUMNS: [&str; 5] = ["match_token", "match_id", "source", "signature", "id"];
/// `matches` columns of the limited matches, in the order `read_matches`
/// reads them.
const MATCH_KEYS: [&str; 3] = ["id", "token", "mtgo_id"];
const MATCH_COLUMNS: [&str; 3] = ["league_id", "deck_version_id", "started_at"];

/// Every `(table, columns)` this reader selects. The SQL is built from the
/// same lists, so the coverage ledger cannot drift from it.
pub const READ_COLUMNS: [(&str, &[&str]); 8] = [
    ("drafts", &DRAFT_KEYS),
    ("drafts", &DRAFT_COLUMNS),
    ("draft_picks", &PICK_KEYS),
    ("draft_picks", &PICK_COLUMNS),
    ("limited_deck_snapshots", &SNAPSHOT_KEYS),
    ("limited_deck_snapshots", &SNAPSHOT_COLUMNS),
    ("matches", &MATCH_KEYS),
    ("matches", &MATCH_COLUMNS),
];

/// Each limited deck version (of 0.x's `limited:` decks) by its 0.x id: its
/// signature in the current form.
pub(crate) type LimitedVersions = HashMap<i64, String>;

/// The 0.x Limited data v1 imports and what was left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitedRead {
    pub data: ImportedLimited,
    /// A blank draft token.
    pub drafts_skipped: u32,
    /// Drafts of a league left out because its matches are of a player
    /// that is not one of 0.x's accounts (issue #337).
    pub drafts_not_account: u32,
    /// A registered deck without a match token, readable cards or capture
    /// time, or a snapshot that is not a registered one.
    pub decks_skipped: u32,
}

/// The Limited data, or `None` when the 0.x database has neither a `drafts`
/// nor a `limited_deck_snapshots` table.
pub(crate) fn read_limited(
    conn: &Connection,
    versions: &LimitedVersions,
) -> Result<Option<LimitedRead>, ImportError> {
    let draft_columns = table_columns(conn, "drafts")?;
    let snapshot_columns = table_columns(conn, "limited_deck_snapshots")?;
    let has_drafts = has_all(&draft_columns, &DRAFT_KEYS);
    let has_snapshots = has_all(&snapshot_columns, &SNAPSHOT_KEYS);
    if !has_drafts && !has_snapshots {
        return Ok(None);
    }
    let mut read = LimitedRead {
        data: ImportedLimited::default(),
        drafts_skipped: 0,
        drafts_not_account: 0,
        decks_skipped: 0,
    };
    if has_drafts {
        read_drafts(conn, &draft_columns, &mut read)?;
    }
    let matches = read_matches(conn)?;
    if has_snapshots {
        read_snapshots(conn, &snapshot_columns, &matches, &mut read)?;
    }
    add_version_decks(&matches, versions, &mut read);
    Ok(Some(read))
}

/// A 0.x time: `YYYY-MM-DD HH:MM:SS` (UTC) or an ISO 8601 time with an
/// offset (`2026-08-22T11:12:05+00:00`), as UTC unix ms.
fn parse_time(text: &str) -> Option<i64> {
    match DateTime::parse_from_rfc3339(text.trim()) {
        Ok(at) => Some(at.timestamp_millis()),
        Err(_) => parse_utc_ms(text),
    }
}

/// One `drafts` row, loosely typed, in `DRAFT_KEYS` then `DRAFT_COLUMNS`
/// order.
struct DraftRow {
    id: i64,
    token: String,
    league_id: Option<i64>,
    mtgo_draft_id: Option<i64>,
    pod_token: Option<String>,
    seat_count: Option<i64>,
    seat_index: Option<i64>,
    booster_catalog_id: Option<i64>,
    state: Option<String>,
    pack_size: Option<i64>,
    picks_expected: Option<i64>,
    started_at: Option<String>,
    ended_at: Option<String>,
}

fn read_drafts(
    conn: &Connection,
    present: &[String],
    read: &mut LimitedRead,
) -> Result<(), ImportError> {
    let mut picks = read_picks(conn)?;
    let sql = format!(
        "SELECT {}, {} FROM drafts ORDER BY id",
        column_list("", &DRAFT_KEYS),
        select_list(present, &DRAFT_COLUMNS)
    );
    let mut stmt = conn.prepare(&sql).map_err(read_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(DraftRow {
                id: row.get(0)?,
                token: as_text(value(row, 1)?).unwrap_or_default(),
                league_id: as_integer(value(row, 2)?),
                mtgo_draft_id: as_integer(value(row, 3)?),
                pod_token: as_text(value(row, 4)?),
                seat_count: as_integer(value(row, 5)?),
                seat_index: as_integer(value(row, 6)?),
                booster_catalog_id: as_integer(value(row, 7)?),
                state: as_text(value(row, 8)?),
                pack_size: as_integer(value(row, 9)?),
                picks_expected: as_integer(value(row, 10)?),
                started_at: as_text(value(row, 11)?),
                ended_at: as_text(value(row, 12)?),
            })
        })
        .map_err(read_error)?;
    for row in rows {
        let DraftRow {
            id,
            token,
            league_id,
            mtgo_draft_id,
            pod_token,
            seat_count,
            seat_index,
            booster_catalog_id,
            state,
            pack_size,
            picks_expected,
            started_at,
            ended_at,
        } = row.map_err(read_error)?;
        let token = token.trim().to_string();
        // 0.x's default state is connecting.
        let state_text = state
            .as_deref()
            .unwrap_or("connecting")
            .trim()
            .to_lowercase();
        let (false, Some(state)) = (token.is_empty(), draft_state(&state_text)) else {
            read.drafts_skipped += 1;
            continue;
        };
        let (draft_picks, notes) = picks.remove(&id).unwrap_or_default();
        read.data.drafts.push(LegacyDraft {
            legacy_id: id,
            league_id,
            draft: Draft {
                token,
                league_event_id: None,
                course_id: None,
                draft_id: large(mtgo_draft_id),
                pod_token,
                seat_index: small(seat_index),
                seat_count: small(seat_count).unwrap_or(DEFAULT_SEAT_COUNT),
                booster_catalog_id: small(booster_catalog_id),
                state,
                pack_size: small(pack_size).unwrap_or(DEFAULT_PACK_SIZE),
                picks_expected: small(picks_expected).unwrap_or(DEFAULT_PICKS_EXPECTED),
                started_at_ms: started_at.as_deref().and_then(parse_utc_ms),
                ended_at_ms: ended_at.as_deref().and_then(parse_utc_ms),
                picks: draft_picks,
            },
            notes,
        });
    }
    Ok(())
}

/// Each draft's picks in ordinal order and its notes `(ordinal, note)`, by
/// 0.x draft id. A pick whose ordinal is not a positive number is left out.
type DraftPicks = HashMap<i64, (Vec<Pick>, Vec<(u32, String)>)>;

/// One `draft_picks` row, loosely typed, in `PICK_KEYS` then `PICK_COLUMNS`
/// order (`id` only orders rows, so it is not kept).
struct PickRow {
    draft_id: Option<i64>,
    ordinal: Option<i64>,
    pack_number: Option<i64>,
    pick_number: Option<i64>,
    pack_id: Option<i64>,
    direction: Option<i64>,
    cards_available: Option<String>,
    picked_catalog_id: Option<i64>,
    picked_card_id: Option<i64>,
    reservations: Option<String>,
    shown_at: Option<String>,
    deadline_at: Option<String>,
    picked_at: Option<String>,
    note: Option<String>,
}

fn read_picks(conn: &Connection) -> Result<DraftPicks, ImportError> {
    let mut picks: DraftPicks = HashMap::new();
    let present = table_columns(conn, "draft_picks")?;
    if !has_all(&present, &PICK_KEYS) {
        return Ok(picks);
    }
    let sql = format!(
        "SELECT {}, {} FROM draft_picks ORDER BY draft_id, ordinal, {}",
        column_list("", &PICK_KEYS),
        select_list(&present, &PICK_COLUMNS),
        if has_column(&present, "id") {
            "id"
        } else {
            "rowid"
        }
    );
    let mut stmt = conn.prepare(&sql).map_err(read_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(PickRow {
                draft_id: as_integer(value(row, 0)?),
                ordinal: as_integer(value(row, 1)?),
                pack_number: as_integer(value(row, 2)?),
                pick_number: as_integer(value(row, 3)?),
                pack_id: as_integer(value(row, 4)?),
                direction: as_integer(value(row, 5)?),
                cards_available: as_text(value(row, 6)?),
                picked_catalog_id: as_integer(value(row, 7)?),
                picked_card_id: as_integer(value(row, 8)?),
                reservations: as_text(value(row, 9)?),
                shown_at: as_text(value(row, 10)?),
                deadline_at: as_text(value(row, 11)?),
                picked_at: as_text(value(row, 12)?),
                note: as_text(value(row, 13)?),
            })
        })
        .map_err(read_error)?;
    for row in rows {
        let PickRow {
            draft_id,
            ordinal,
            pack_number,
            pick_number,
            pack_id,
            direction,
            cards_available,
            picked_catalog_id,
            picked_card_id,
            reservations,
            shown_at,
            deadline_at,
            picked_at,
            note,
        } = row.map_err(read_error)?;
        let (Some(draft_id), Some(ordinal)) = (draft_id, small(ordinal).filter(|o| *o > 0)) else {
            continue;
        };
        let entry = picks.entry(draft_id).or_default();
        // 0.x keeps one pick per ordinal: the first row wins.
        if entry.0.iter().any(|pick| pick.ordinal == ordinal) {
            continue;
        }
        entry.0.push(Pick {
            ordinal,
            pack_number: small(pack_number).unwrap_or(0),
            pick_number: small(pick_number).unwrap_or(0),
            pack_id: large(pack_id),
            direction: small(direction),
            cards_available: catalog_ids(cards_available.as_deref()),
            picked_catalog_id: small(picked_catalog_id),
            picked_card_id: large(picked_card_id),
            reservations: reservation_list(reservations.as_deref()),
            shown_at_ms: shown_at.as_deref().and_then(parse_utc_ms),
            deadline_at_ms: deadline_at.as_deref().and_then(parse_utc_ms),
            picked_at_ms: picked_at.as_deref().and_then(parse_utc_ms),
        });
        if let Some(note) = note.filter(|note| !note.trim().is_empty()) {
            entry.1.push((ordinal, note));
        }
    }
    Ok(picks)
}

/// 0.x `cards_available`: a JSON array of catalog ids (numbers or numeric
/// strings). Anything else reads as none.
fn catalog_ids(json: Option<&str>) -> Vec<u32> {
    let Some(Value::Array(items)) = json.and_then(|text| serde_json::from_str(text).ok()) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| limited::payload::as_u32(Some(item)))
        .collect()
}

/// 0.x `reservations`: `[{catalog_id, at}]`, `at` an ISO 8601 time. An entry
/// without a catalog id or a readable time is left out.
fn reservation_list(json: Option<&str>) -> Vec<Reservation> {
    let Some(Value::Array(items)) = json.and_then(|text| serde_json::from_str(text).ok()) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let catalog_id = limited::payload::as_u32(item.get("catalog_id"))?;
            let at_ms = parse_time(item.get("at")?.as_str()?)?;
            Some(Reservation { catalog_id, at_ms })
        })
        .collect()
}

/// One 0.x match of a limited league, or with a deck version.
struct MatchRow {
    token: String,
    mtgo_match_id: Option<i64>,
    league_id: Option<i64>,
    deck_version_id: Option<i64>,
    started_at_ms: Option<i64>,
}

/// Every 0.x match with a token, by its 0.x id.
fn read_matches(conn: &Connection) -> Result<HashMap<i64, MatchRow>, ImportError> {
    let present = table_columns(conn, "matches")?;
    let sql = format!(
        "SELECT {}, {} FROM matches ORDER BY id",
        column_list("", &MATCH_KEYS),
        select_list(&present, &MATCH_COLUMNS)
    );
    let mut stmt = conn.prepare(&sql).map_err(read_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                MatchRow {
                    token: as_text(value(row, 1)?)
                        .unwrap_or_default()
                        .trim()
                        .to_string(),
                    mtgo_match_id: as_text(value(row, 2)?).as_deref().and_then(parse_id),
                    league_id: as_integer(value(row, 3)?),
                    deck_version_id: as_integer(value(row, 4)?),
                    started_at_ms: as_text(value(row, 5)?).as_deref().and_then(parse_utc_ms),
                },
            ))
        })
        .map_err(read_error)?;
    let mut matches = HashMap::new();
    for row in rows {
        let (id, row) = row.map_err(read_error)?;
        if !row.token.is_empty() {
            matches.insert(id, row);
        }
    }
    Ok(matches)
}

/// A deck version signature's cards as registered rows.
fn signature_cards(signature: &str) -> Vec<RegisteredCard> {
    decks::decode_signature(signature)
        .into_iter()
        .map(|card| RegisteredCard {
            catalog_id: card.catalog_id,
            quantity: card.quantity,
            sideboard: card.sideboard,
        })
        .collect()
}

fn read_snapshots(
    conn: &Connection,
    present: &[String],
    matches: &HashMap<i64, MatchRow>,
    read: &mut LimitedRead,
) -> Result<(), ImportError> {
    let order = if has_column(present, "id") {
        "id"
    } else {
        "rowid"
    };
    let sql = format!(
        "SELECT {}, {} FROM limited_deck_snapshots ORDER BY {order}",
        column_list("", &SNAPSHOT_KEYS),
        select_list(present, &SNAPSHOT_COLUMNS)
    );
    let mut stmt = conn.prepare(&sql).map_err(read_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                as_integer(value(row, 0)?),
                as_text(value(row, 1)?),
                as_text(value(row, 2)?),
                as_text(value(row, 3)?),
                as_integer(value(row, 4)?),
                as_text(value(row, 5)?),
                as_text(value(row, 6)?),
            ))
        })
        .map_err(read_error)?;
    let by_token: HashMap<&str, &MatchRow> = matches
        .values()
        .map(|row| (row.token.as_str(), row))
        .collect();
    for row in rows {
        let (league_id, cards, captured_at, match_token, match_id, source, signature) =
            row.map_err(read_error)?;
        // 0.x only ever wrote registered snapshots.
        let registered = source
            .as_deref()
            .is_none_or(|source| source.trim().eq_ignore_ascii_case("registered"));
        let from_id = match_id.and_then(|id| matches.get(&id));
        let token = match_token
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty())
            .or_else(|| from_id.map(|row| row.token.clone()));
        let mut parsed = cards
            .as_deref()
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
            .map(|json| registered_cards_from_json(&json))
            .unwrap_or_default();
        if parsed.is_empty() {
            parsed = signature
                .as_deref()
                .map(signature_cards)
                .unwrap_or_default();
        }
        let captured_at_ms = captured_at.as_deref().and_then(parse_time);
        let (true, Some(league_id), Some(token), false, Some(captured_at_ms)) = (
            registered,
            league_id,
            token,
            parsed.is_empty(),
            captured_at_ms,
        ) else {
            read.decks_skipped += 1;
            continue;
        };
        let mtgo_match_id = from_id
            .or_else(|| by_token.get(token.as_str()).copied())
            .and_then(|row| row.mtgo_match_id);
        read.data.decks.push(LegacyLimitedDeck {
            league_id,
            match_token: token,
            mtgo_match_id,
            cards: parsed,
            captured_at_ms,
        });
    }
    Ok(())
}

/// A registered deck from the limited deck version of each match of a
/// limited league that has no snapshot (0.x linked it through
/// `deck_version_id`), captured at the match's start.
fn add_version_decks(
    matches: &HashMap<i64, MatchRow>,
    versions: &LimitedVersions,
    read: &mut LimitedRead,
) {
    let covered: HashSet<(i64, String)> = read
        .data
        .decks
        .iter()
        .map(|deck| (deck.league_id, deck.match_token.clone()))
        .collect();
    let mut ids: Vec<&i64> = matches.keys().collect();
    ids.sort();
    for id in ids {
        let row = &matches[id];
        let (Some(league_id), Some(signature)) = (
            row.league_id,
            row.deck_version_id
                .and_then(|version| versions.get(&version)),
        ) else {
            continue;
        };
        if covered.contains(&(league_id, row.token.clone())) {
            continue;
        }
        let cards = signature_cards(signature);
        let (false, Some(captured_at_ms)) = (cards.is_empty(), row.started_at_ms) else {
            read.decks_skipped += 1;
            continue;
        };
        read.data.decks.push(LegacyLimitedDeck {
            league_id,
            match_token: row.token.clone(),
            mtgo_match_id: row.mtgo_match_id,
            cards,
            captured_at_ms,
        });
    }
}
