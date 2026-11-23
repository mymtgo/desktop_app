//! Limited storage and reads (limited spec sections 4, 6 and 7, target
//! schema section 9).
//!
//! Drafts, their picks and the registered decks of limited matches are
//! stored (`drafts`, `draft_picks`, `limited_deck_snapshots`): the league
//! sync writes them from the log (`draft_events` reads one account's draft
//! events, oldest first, for `limited::fold_drafts`; `parse_registered`
//! reads a `match_deck_registered` line), and `import_limited` writes 0.x's.
//! The pages read the rows (`stored_drafts`, `stored_registered_deck`).
//!
//! A pick note is the player's input on its pick (`draft_picks.note`), which
//! no sync writes; a note is saved through the tracker as a `DraftNoteEdit`,
//! like `MatchEdit`.

use std::collections::{BTreeMap, HashMap};

use limited::{
    Draft, DraftEventInput, DraftState, Pick, RegisteredCard, RegisteredDeck, Reservation,
};
use mtgo_protocol::EventKind;
use rusqlite::{OptionalExtension, Row, ToSql, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[cfg(test)]
use crate::compact::archived_events;
use crate::leagues::{logins_by_file, player_at};
use crate::write::decode_raw;
use crate::{Connection, StoreError};

/// The longest pick note, in characters (0.x `max:2000`).
pub const MAX_NOTE_CHARS: usize = 2000;

/// The most copies of one card a registered deck row keeps. MTGO never
/// registers more; a larger number is hostile input, and clamping it keeps
/// every pool sum in `limited` far from overflowing.
pub const MAX_CARD_QUANTITY: u32 = 250;

/// The draft event kinds the fold reads, as stored in `log_events.kind`.
pub const DRAFT_KINDS: [EventKind; 11] = [
    EventKind::DraftCreated,
    EventKind::DraftStateChanged,
    EventKind::DraftSelection,
    EventKind::DraftLeagueStanding,
    EventKind::DraftJoined,
    EventKind::DraftPodState,
    EventKind::DraftPackOpened,
    EventKind::DraftPendingPick,
    EventKind::DraftEnded,
    EventKind::DraftPickCommitted,
    EventKind::LeaguePoolGranted,
];

/// The same kinds as SQL, in the order of the `log_events_drafts` index, so
/// a read with this filter walks it.
const DRAFT_KINDS_SQL: &str = "('draft_created', 'draft_state_changed', 'draft_selection',
        'draft_league_standing', 'draft_joined', 'draft_pod_state', 'draft_pack_opened',
        'draft_pending_pick', 'draft_ended', 'draft_pick_committed', 'league_pool_granted')";

/// A user edit of a draft's pick notes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftNoteEdit {
    /// 0.x `UpdatePickNoteController` (`nullable|string|max:2000`): trimmed,
    /// as Laravel trims every string; empty deletes the note. The pick must
    /// exist: `ordinal` between 1 and the draft's expected picks, with a pick
    /// in the draft (live or imported from 0.x).
    Set {
        draft_token: String,
        ordinal: u32,
        note: String,
        now_ms: i64,
    },
}

/// What happened to a `DraftNoteEdit`. Only `Done` changed anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftNoteOutcome {
    Done,
    /// There is no such draft or pick (0.x answered 404).
    NotFound,
    NoteTooLong,
}

impl DraftNoteOutcome {
    /// The message to show for a refusal (0.x's where 0.x has one), `None`
    /// for `Done`.
    pub fn refusal(self) -> Option<&'static str> {
        match self {
            DraftNoteOutcome::Done => None,
            DraftNoteOutcome::NotFound => Some("This pick no longer exists."),
            DraftNoteOutcome::NoteTooLong => {
                Some("The note field must not be greater than 2000 characters.")
            }
        }
    }
}

/// Apply a user's pick note edit in its own transaction (on the writer
/// thread). A refused edit changes nothing.
pub fn edit_draft_note(
    conn: &mut Connection,
    edit: &DraftNoteEdit,
) -> Result<DraftNoteOutcome, StoreError> {
    let tx = conn.transaction()?;
    let outcome = match edit {
        DraftNoteEdit::Set {
            draft_token,
            ordinal,
            note,
            now_ms,
        } => set_note(&tx, draft_token, *ordinal, note, *now_ms)?,
    };
    // A refusal wrote nothing, so committing it changes nothing either.
    tx.commit()?;
    Ok(outcome)
}

fn set_note(
    tx: &Transaction,
    draft_token: &str,
    ordinal: u32,
    note: &str,
    now_ms: i64,
) -> Result<DraftNoteOutcome, StoreError> {
    let Some(draft_id) = pick_draft(tx, draft_token, ordinal)? else {
        return Ok(DraftNoteOutcome::NotFound);
    };
    let note = note.trim();
    if note.chars().count() > MAX_NOTE_CHARS {
        return Ok(DraftNoteOutcome::NoteTooLong);
    }
    let (note, saved_at) = if note.is_empty() {
        (None, None)
    } else {
        (Some(note), Some(now_ms))
    };
    tx.prepare_cached(
        "UPDATE draft_picks SET note = ?1, note_updated_at = ?2
         WHERE draft_id = ?3 AND ordinal = ?4",
    )?
    .execute(params![note, saved_at, draft_id, ordinal])?;
    Ok(DraftNoteOutcome::Done)
}

/// The id of the stored draft that has pick `ordinal` (0.x: `1 <= ordinal <=
/// picks_expected` and a pick row), from the log or imported from 0.x;
/// `None` when there is no such pick.
fn pick_draft(
    tx: &Transaction,
    draft_token: &str,
    ordinal: u32,
) -> Result<Option<i64>, StoreError> {
    Ok(tx
        .prepare_cached(
            "SELECT d.id FROM drafts d JOIN draft_picks p ON p.draft_id = d.id
             WHERE d.draft_token = ?1 AND d.detached_at IS NULL AND p.ordinal = ?2
                 AND p.detached_at IS NULL AND p.ordinal <= d.picks_expected",
        )?
        .query_row(params![draft_token, ordinal], |row| row.get(0))
        .optional()?)
}

/// A stored event as the fold takes it; `None` for a kind that is not a
/// draft kind.
fn draft_event(
    kind: &str,
    draft_token: Option<String>,
    logged_at_ms: i64,
    raw: String,
) -> Option<DraftEventInput> {
    let kind = *DRAFT_KINDS.iter().find(|known| known.as_str() == kind)?;
    Some(DraftEventInput {
        kind,
        draft_token,
        logged_at_ms,
        // The stored text keeps the line's `HH:MM:SS` prefix.
        line_time: raw.chars().take(8).collect(),
        raw,
    })
}

/// The account's draft events (every draft kind and `league_pool_granted`),
/// oldest first by `(log file, byte offset)`, for `limited::fold_drafts`.
/// An event's player is the latest Login in its file at or before it, else
/// the file's inherited player, as for league events.
pub fn draft_events(
    conn: &Connection,
    account_id: i64,
) -> Result<Vec<DraftEventInput>, StoreError> {
    let username: Option<String> = conn
        .query_row(
            "SELECT username FROM accounts WHERE id = ?1",
            [account_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(username) = username else {
        return Ok(Vec::new());
    };

    struct Row {
        file: i64,
        offset: i64,
        kind: String,
        token: Option<String>,
        logged_at: i64,
        raw: Vec<u8>,
        compressed: bool,
        inherited: Option<String>,
    }
    let mut read: Vec<Row> = Vec::new();
    {
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT e.log_file_id, e.byte_offset, e.kind, e.draft_token, e.logged_at, e.raw,
                 e.compressed, f.inherited_username
             FROM log_events e JOIN log_files f ON f.id = e.log_file_id
             WHERE e.kind IN {DRAFT_KINDS_SQL} AND e.quarantined = 0
             ORDER BY e.log_file_id, e.byte_offset"
        ))?;
        let rows = stmt.query_map([], |row| {
            Ok(Row {
                file: row.get(0)?,
                offset: row.get(1)?,
                kind: row.get(2)?,
                token: row.get(3)?,
                logged_at: row.get(4)?,
                raw: row.get(5)?,
                compressed: row.get(6)?,
                inherited: row.get(7)?,
            })
        })?;
        for row in rows {
            read.push(row?);
        }
    }
    if read.is_empty() {
        return Ok(Vec::new());
    }

    let logins = logins_by_file(conn)?;
    let mut events = Vec::new();
    for row in read {
        let player = logins
            .get(&row.file)
            .and_then(|file_logins| player_at(file_logins, row.offset))
            .or(row.inherited.as_deref());
        if player != Some(username.as_str()) {
            continue;
        }
        let raw = decode_raw(row.raw, row.compressed)?;
        if let Some(event) = draft_event(&row.kind, row.token, row.logged_at, raw) {
            events.push(event);
        }
    }
    Ok(events)
}

/// One draft's pick notes, by ordinal. A detached pick or draft keeps its
/// note but is not read.
pub fn pick_notes(
    conn: &Connection,
    draft_token: &str,
) -> Result<BTreeMap<u32, String>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT p.ordinal, p.note FROM draft_picks p JOIN drafts d ON d.id = p.draft_id
         WHERE d.draft_token = ?1 AND p.note IS NOT NULL
           AND p.detached_at IS NULL AND d.detached_at IS NULL
         ORDER BY p.ordinal",
    )?;
    let rows = stmt.query_map([draft_token], |row| {
        Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut notes = BTreeMap::new();
    for row in rows {
        let (ordinal, note) = row?;
        notes.insert(ordinal, note);
    }
    Ok(notes)
}

/// A pick note and when it was last saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickNote {
    pub note: String,
    pub updated_at_ms: i64,
}

/// One draft's pick notes with their save times, by ordinal (0.x
/// `draft_picks.note`; 0.x showed the pick's `updated_at`, v1 keeps the
/// note's own time). A detached pick or draft is not read.
pub fn pick_note_rows(
    conn: &Connection,
    draft_token: &str,
) -> Result<BTreeMap<u32, PickNote>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT p.ordinal, p.note, p.note_updated_at FROM draft_picks p
         JOIN drafts d ON d.id = p.draft_id
         WHERE d.draft_token = ?1 AND p.note IS NOT NULL
           AND p.detached_at IS NULL AND d.detached_at IS NULL
         ORDER BY p.ordinal",
    )?;
    let rows = stmt.query_map([draft_token], |row| {
        Ok((
            row.get::<_, u32>(0)?,
            PickNote {
                note: row.get(1)?,
                updated_at_ms: row.get(2)?,
            },
        ))
    })?;
    let mut notes = BTreeMap::new();
    for row in rows {
        let (ordinal, note) = row?;
        notes.insert(ordinal, note);
    }
    Ok(notes)
}

/// The name of a set, from any card of it that has one (0.x
/// `Card::where('set_code', ...)->whereNotNull('set_name')->value('set_name')`,
/// here the lowest catalog id). `None` when no card of the set has a name.
pub fn set_name(conn: &Connection, set_code: &str) -> Result<Option<String>, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT set_name FROM cards WHERE set_code = ?1 AND set_name IS NOT NULL
             ORDER BY catalog_id LIMIT 1",
        )?
        .query_row([set_code], |row| row.get(0))
        .optional()?)
}

/// Every pick note of an attached pick, by draft token, then ordinal
/// (tests).
#[cfg(test)]
pub fn all_pick_notes(
    conn: &Connection,
) -> Result<HashMap<String, BTreeMap<u32, String>>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT d.draft_token, p.ordinal, p.note FROM draft_picks p
             JOIN drafts d ON d.id = p.draft_id WHERE p.note IS NOT NULL
             AND p.detached_at IS NULL AND d.detached_at IS NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, u32>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut notes: HashMap<String, BTreeMap<u32, String>> = HashMap::new();
    for row in rows {
        let (token, ordinal, note) = row?;
        notes.entry(token).or_default().insert(ordinal, note);
    }
    Ok(notes)
}

/// A registered deck row clamped to sane numbers: at most
/// `MAX_CARD_QUANTITY` copies, and rows with no copies dropped (every count
/// ignores them, as 0.x did).
pub fn clamp_cards(cards: Vec<RegisteredCard>) -> Vec<RegisteredCard> {
    cards
        .into_iter()
        .filter(|card| card.quantity > 0)
        .map(|card| RegisteredCard {
            quantity: card.quantity.min(MAX_CARD_QUANTITY),
            ..card
        })
        .collect()
}

/// A registered deck from its cards (clamped), with 0.x's signature of them.
pub(crate) fn registered(
    match_token: &str,
    captured_at_ms: i64,
    cards: Vec<RegisteredCard>,
) -> RegisteredDeck {
    let cards = clamp_cards(cards);
    let deck_cards: Vec<decks::DeckCard> = cards
        .iter()
        .map(|card| decks::DeckCard {
            catalog_id: card.catalog_id,
            quantity: card.quantity,
            sideboard: card.sideboard,
        })
        .collect();
    RegisteredDeck {
        match_token: Some(match_token.to_string()),
        signature: decks::signature(&deck_cards),
        captured_at_ms,
        cards,
    }
}

/// The marker of a registered deck line (`FlsMatchDeckGetRespMessage`).
const REGISTERED_MARKER: &str = "FlsMatchDeckGetRespMessage";

/// The registered cards of a `match_deck_registered` text for this match
/// (0.x `ReadRegisteredDeck::fromEvent`), `None` when it is not one, names
/// another match or carries no cards.
///
/// Limited has its own reader rather than `decks::parse_registered_deck`
/// (which the live deck tracking relies on and keeps as it is), because 0.x
/// reads these lines more loosely:
/// - the payload is decoded with `RepairJson::firstObject` (up to three
///   missing closing brackets added back, `limited::payload::first_object`);
/// - a row without `InSideboard` is a main deck card (`?? false`);
/// - `CatalogID` and `Quantity` are cast as PHP's `(int)` does.
///
/// A row without a catalog id is left out (0.x could not show it either).
/// The payload's `MatchToken`, when present, must be this match's: it is
/// what finds the line among an archived match's events, so live and
/// archived reads agree.
pub(crate) fn parse_registered(raw: &str, match_token: &str) -> Option<Vec<RegisteredCard>> {
    let at = raw.find(REGISTERED_MARKER)?;
    let json = limited::payload::first_object(&raw[at..])?;
    if let Some(token) = json.get("MatchToken").and_then(Value::as_str)
        && token != match_token
    {
        return None;
    }
    let rows = json.get("Cards")?.as_array()?;
    if rows.is_empty() {
        return None;
    }
    let mut cards = Vec::new();
    for row in rows {
        let Some(catalog_id) = php_int(row.get("CatalogID")).and_then(|id| u32::try_from(id).ok())
        else {
            continue;
        };
        let quantity = php_int(row.get("Quantity"))
            .unwrap_or(0)
            .clamp(0, i64::from(u32::MAX));
        cards.push(RegisteredCard {
            catalog_id,
            // In range: clamped just above.
            quantity: u32::try_from(quantity).unwrap_or(0),
            sideboard: php_bool(row.get("InSideboard")),
        });
    }
    Some(cards)
}

/// PHP's `(int)` of a JSON value: a number truncated, a numeric string's
/// leading integer, a boolean as 0 or 1. `None` for a missing value, null or
/// anything else (the row is then of no use).
fn php_int(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|float| float as i64)),
        Value::String(text) => {
            let text = text.trim();
            let digits: String = text
                .chars()
                .enumerate()
                .take_while(|(index, ch)| ch.is_ascii_digit() || (*index == 0 && *ch == '-'))
                .map(|(_, ch)| ch)
                .collect();
            digits.parse().ok()
        }
        Value::Bool(flag) => Some(i64::from(*flag)),
        _ => None,
    }
}

/// PHP's `(bool)` of a JSON value: false for a missing value, null, false,
/// 0, `""`, `"0"` and an empty array, true otherwise.
fn php_bool(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|n| n != 0.0),
        Some(Value::String(text)) => !text.is_empty() && text != "0",
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(_)) => true,
    }
}

/// The deck MTGO registered for this match (0.x `ReadRegisteredDeck`), read
/// from its events: its newest readable `match_deck_registered` event in log
/// order, from `log_events` or the match's archive once compacted (the same
/// deck either way), captured at the event's time. Without one, the deck
/// imported from 0.x for the match. `None` when there is neither. The
/// reference the stored snapshots are checked against (the league sync
/// stores the same deck; the pages read `stored_registered_deck`).
///
/// Difference from 0.x (on purpose): 0.x took the newest event and answered
/// null when that one did not read; v1 skips an unreadable newest line (one
/// even `RepairJson` cannot mend, or one with no cards) and takes the newest
/// line that reads. See `docs/known_mtgo_lies_and_traps.md`.
#[cfg(test)]
pub(crate) fn registered_deck(
    conn: &Connection,
    match_token: &str,
) -> Result<Option<RegisteredDeck>, StoreError> {
    // (file, offset, logged at, raw), live and archived together.
    let mut found: Vec<(i64, u64, i64, String)> = Vec::new();
    {
        let mut stmt = conn.prepare_cached(
            "SELECT log_file_id, byte_offset, logged_at, raw, compressed FROM log_events
             WHERE match_token = ?1 AND kind = 'match_deck_registered' AND quarantined = 0",
        )?;
        let rows = stmt.query_map([match_token], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, bool>(4)?,
            ))
        })?;
        for row in rows {
            let (file, offset, at, raw, compressed) = row?;
            found.push((file, offset as u64, at, decode_raw(raw, compressed)?));
        }
    }
    for event in archived_events(conn, match_token)? {
        found.push((
            event.log_file_id,
            event.byte_offset,
            event.logged_at_ms,
            event.raw,
        ));
    }
    // Newest first; an event both live and archived reads the same.
    found.sort_by_key(|(file, offset, _, _)| std::cmp::Reverse((*file, *offset)));
    for (_, _, at, raw) in &found {
        if let Some(cards) = parse_registered(raw, match_token) {
            return Ok(Some(registered(match_token, *at, cards)));
        }
    }

    stored_snapshot(conn, match_token, "imported")
}

/// The deck MTGO registered for this match as stored: the league sync's
/// (`registered`), else the one imported from 0.x. `None` when the match
/// has neither.
pub fn stored_registered_deck(
    conn: &Connection,
    match_token: &str,
) -> Result<Option<RegisteredDeck>, StoreError> {
    match stored_snapshot(conn, match_token, "registered")? {
        Some(deck) => Ok(Some(deck)),
        None => stored_snapshot(conn, match_token, "imported"),
    }
}

/// The match's stored snapshot of one source.
fn stored_snapshot(
    conn: &Connection,
    match_token: &str,
    source: &str,
) -> Result<Option<RegisteredDeck>, StoreError> {
    let found: Option<(String, i64)> = conn
        .prepare_cached(
            "SELECT s.cards, s.captured_at FROM limited_deck_snapshots s
             JOIN matches m ON m.id = s.match_id WHERE m.token = ?1 AND s.source = ?2",
        )?
        .query_row(params![match_token, source], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()?;
    match found {
        Some((cards, captured_at)) => Ok(Some(registered(
            match_token,
            captured_at,
            stored_cards(&cards)?,
        ))),
        None => Ok(None),
    }
}

/// One stored registered card row (`limited_deck_snapshots.cards`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredCard {
    pub catalog_id: u32,
    pub quantity: u32,
    pub sideboard: bool,
}

/// The JSON stored for a registered deck.
pub(crate) fn cards_json(cards: &[RegisteredCard]) -> Result<String, StoreError> {
    let rows: Vec<StoredCard> = cards
        .iter()
        .map(|card| StoredCard {
            catalog_id: card.catalog_id,
            quantity: card.quantity,
            sideboard: card.sideboard,
        })
        .collect();
    Ok(serde_json::to_string(&rows)?)
}

pub(crate) fn stored_cards(json: &str) -> Result<Vec<RegisteredCard>, StoreError> {
    let rows: Vec<StoredCard> = serde_json::from_str(json)?;
    Ok(rows
        .into_iter()
        .map(|row| RegisteredCard {
            catalog_id: row.catalog_id,
            quantity: row.quantity,
            sideboard: row.sideboard,
        })
        .collect())
}

/// One stored reservation (`draft_picks.reservations`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredReservation {
    pub catalog_id: u32,
    pub at_ms: i64,
}

/// The JSON stored for a pick's reservations.
pub(crate) fn reservations_json(reservations: &[Reservation]) -> Result<String, StoreError> {
    let rows: Vec<StoredReservation> = reservations
        .iter()
        .map(|reservation| StoredReservation {
            catalog_id: reservation.catalog_id,
            at_ms: reservation.at_ms,
        })
        .collect();
    Ok(serde_json::to_string(&rows)?)
}

/// A stored draft (a `drafts` row that is not detached) with its picks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredDraft {
    pub id: i64,
    pub account_id: Option<i64>,
    /// The run that holds it (`drafts.league_id`).
    pub league_id: Option<i64>,
    pub draft: Draft,
}

/// The `drafts` columns (on a table aliased `d`) that `draft_at` reads, in
/// order: 13 columns.
pub(crate) const DRAFT_SELECT: &str = "d.draft_token, d.league_event_id, d.mtgo_course_id,
    d.mtgo_draft_id, d.pod_token, d.seat_count, d.seat_index, d.booster_catalog_id, d.state,
    d.pack_size, d.picks_expected, d.started_at, d.ended_at";

/// The draft (without picks) whose `DRAFT_SELECT` columns start at column
/// `first`, with its stored state text: the caller parses the state (with
/// `parse_draft_state`), as a corrupt state is a `StoreError`, not a row
/// error.
pub(crate) fn draft_at(row: &Row, first: usize) -> rusqlite::Result<(Draft, String)> {
    let draft = Draft {
        token: row.get(first)?,
        league_event_id: unsigned(row.get(first + 1)?),
        course_id: unsigned(row.get(first + 2)?),
        draft_id: unsigned(row.get(first + 3)?),
        pod_token: row.get(first + 4)?,
        seat_count: row.get(first + 5)?,
        seat_index: row.get(first + 6)?,
        booster_catalog_id: row.get(first + 7)?,
        state: DraftState::Connecting,
        pack_size: row.get(first + 9)?,
        picks_expected: row.get(first + 10)?,
        started_at_ms: row.get(first + 11)?,
        ended_at_ms: row.get(first + 12)?,
        picks: Vec::new(),
    };
    Ok((draft, row.get(first + 8)?))
}

/// The stored drafts `filter` takes (SQL on `drafts d`, binding `params`),
/// by start then id, each with its attached picks in ordinal order: two
/// queries, whatever the count.
pub(crate) fn stored_drafts(
    conn: &Connection,
    filter: &str,
    params: &[&dyn ToSql],
) -> Result<Vec<StoredDraft>, StoreError> {
    let mut drafts: Vec<StoredDraft> = Vec::new();
    {
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT d.id, d.account_id, d.league_id, {DRAFT_SELECT}
             FROM drafts d
             WHERE d.detached_at IS NULL AND ({filter}) ORDER BY d.started_at, d.id"
        ))?;
        let rows = stmt.query_map(params, |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, draft_at(row, 3)?))
        })?;
        for row in rows {
            let (id, account_id, league_id, (mut draft, state)) = row?;
            draft.state = parse_draft_state(&state)?;
            drafts.push(StoredDraft {
                id,
                account_id,
                league_id,
                draft,
            });
        }
    }
    if drafts.is_empty() {
        return Ok(drafts);
    }
    let mut picks = stored_picks(
        conn,
        &format!("d.detached_at IS NULL AND ({filter})"),
        params,
    )?;
    for found in &mut drafts {
        found.draft.picks = picks.remove(&found.id).unwrap_or_default();
    }
    Ok(drafts)
}

/// The attached picks of the drafts `filter` takes (SQL on `drafts d`), by
/// draft id, each in ordinal order.
pub(crate) fn stored_picks(
    conn: &Connection,
    filter: &str,
    params: &[&dyn ToSql],
) -> Result<HashMap<i64, Vec<Pick>>, StoreError> {
    let mut picks: HashMap<i64, Vec<Pick>> = HashMap::new();
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT p.draft_id, p.ordinal, p.pack_number, p.pick_number, p.pack_id, p.direction,
             p.cards_available, p.picked_catalog_id, p.picked_card_id, p.reservations,
             p.shown_at, p.deadline_at, p.picked_at
         FROM draft_picks p JOIN drafts d ON d.id = p.draft_id
         WHERE p.detached_at IS NULL AND ({filter}) ORDER BY p.draft_id, p.ordinal"
    ))?;
    let rows = stmt.query_map(params, |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, u32>(1)?,
            row.get::<_, u32>(2)?,
            row.get::<_, u32>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, Option<u32>>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, Option<u32>>(7)?,
            row.get::<_, Option<i64>>(8)?,
            row.get::<_, String>(9)?,
            row.get::<_, Option<i64>>(10)?,
            row.get::<_, Option<i64>>(11)?,
            row.get::<_, Option<i64>>(12)?,
        ))
    })?;
    for row in rows {
        let (
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
            shown_at_ms,
            deadline_at_ms,
            picked_at_ms,
        ) = row?;
        let reservations: Vec<StoredReservation> = serde_json::from_str(&reservations)?;
        picks.entry(draft_id).or_default().push(Pick {
            ordinal,
            pack_number,
            pick_number,
            pack_id: unsigned(pack_id),
            direction,
            cards_available: serde_json::from_str(&cards_available)?,
            picked_catalog_id,
            picked_card_id: unsigned(picked_card_id),
            reservations: reservations
                .into_iter()
                .map(|stored| Reservation {
                    catalog_id: stored.catalog_id,
                    at_ms: stored.at_ms,
                })
                .collect(),
            shown_at_ms,
            deadline_at_ms,
            picked_at_ms,
        });
    }
    Ok(picks)
}

/// The stored draft with this token (not detached), with its picks.
pub fn stored_draft(conn: &Connection, draft_token: &str) -> Result<Option<Draft>, StoreError> {
    Ok(stored_drafts(conn, "d.draft_token = ?1", &[&draft_token])?
        .into_iter()
        .next()
        .map(|found| found.draft))
}

/// A stored id as the fold's unsigned type; a negative one reads as none.
pub(crate) fn unsigned(value: Option<i64>) -> Option<u64> {
    value.and_then(|value| u64::try_from(value).ok())
}

/// A draft state as stored.
pub(crate) fn draft_state_str(state: DraftState) -> &'static str {
    match state {
        DraftState::Connecting => "connecting",
        DraftState::Picking => "picking",
        DraftState::Finished => "finished",
        DraftState::Abandoned => "abandoned",
    }
}

pub(crate) fn parse_draft_state(text: &str) -> Result<DraftState, StoreError> {
    match text {
        "connecting" => Ok(DraftState::Connecting),
        "picking" => Ok(DraftState::Picking),
        "finished" => Ok(DraftState::Finished),
        "abandoned" => Ok(DraftState::Abandoned),
        other => Err(StoreError::Corrupt(format!("draft state {other:?}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compact::compact_match;
    use crate::test_support::{db, insert_imported_run, link_imported_match, new_log_file};
    use crate::write::{NewEvent, WriteBatch, commit, insert_log_file};
    use fixture_tools::{load_fixture, prepared_events};
    use limited::fold_drafts;
    use mtgo_protocol::Subjects;

    const DRAFT: &str = "q-draft-0001";
    const MATCH: &str = "q-match-0001";

    fn account(conn: &Connection, username: &str) -> i64 {
        conn.query_row(
            "INSERT INTO accounts (username, first_seen_at) VALUES (?1, 0)
             ON CONFLICT (username) DO UPDATE SET username = excluded.username
             RETURNING id",
            [username],
            |row| row.get(0),
        )
        .expect("account")
    }

    /// A log file whose inherited player is `player`.
    fn file(conn: &Connection, path: &str, player: &str) -> i64 {
        let mut new = new_log_file(path);
        new.inherited_username = Some(player.to_string());
        insert_log_file(conn, &new).expect("file")
    }

    fn write(conn: &mut Connection, events: Vec<NewEvent>) {
        let batch = WriteBatch {
            events,
            ..WriteBatch::default()
        };
        commit(conn, &batch).expect("commit");
    }

    /// The hobbit fixture's events (every kind, so its Logins too), in file
    /// `file_id`, and its draft events as the fold takes them.
    fn hobbit(file_id: i64) -> (Vec<NewEvent>, Vec<DraftEventInput>) {
        let fixture = load_fixture("draft_hobbit");
        let mut stored = Vec::new();
        let mut inputs = Vec::new();
        for event in prepared_events(&fixture) {
            if let Some(input) = draft_event(
                event.kind.as_str(),
                event.subjects.draft_token.clone(),
                event.at_ms,
                event.text.clone(),
            ) {
                inputs.push(input);
            }
            stored.push(NewEvent {
                log_file_id: file_id,
                byte_offset: event.offset,
                kind: Some(event.kind),
                subjects: event.subjects,
                logged_at_ms: event.at_ms,
                raw: event.text,
                quarantined: false,
            });
        }
        (stored, inputs)
    }

    /// A 0.x draft `DRAFT` with picks 1 and 2 of 42, as `import_limited`
    /// stores it.
    fn imported_draft(conn: &Connection) {
        conn.execute_batch(&format!(
            "INSERT INTO drafts (id, draft_token, source, state, imported_from)
             VALUES (3, '{DRAFT}', 'imported', 'finished', 3);
             INSERT INTO draft_picks (draft_id, ordinal, pack_number, pick_number,
                 cards_available, reservations)
             VALUES (3, 1, 1, 1, '[1,2]', '[]'), (3, 2, 1, 2, '[3]', '[]');"
        ))
        .expect("imported draft");
    }

    fn set(token: &str, ordinal: u32, note: &str) -> DraftNoteEdit {
        DraftNoteEdit::Set {
            draft_token: token.to_string(),
            ordinal,
            note: note.to_string(),
            now_ms: 77,
        }
    }

    #[test]
    fn a_note_is_trimmed_saved_replaced_and_an_empty_one_deletes_it() {
        let (_dir, mut conn) = db("limited-note-rules");
        imported_draft(&conn);
        assert_eq!(
            edit_draft_note(&mut conn, &set(DRAFT, 1, "  Q took the bomb \n")).expect("edit"),
            DraftNoteOutcome::Done
        );
        assert_eq!(
            pick_notes(&conn, DRAFT).expect("notes"),
            BTreeMap::from([(1, "Q took the bomb".to_string())])
        );
        edit_draft_note(&mut conn, &set(DRAFT, 1, "Q second thoughts")).expect("edit");
        edit_draft_note(&mut conn, &set(DRAFT, 2, "Q wheel?")).expect("edit");
        assert_eq!(
            all_pick_notes(&conn).expect("all")[DRAFT],
            BTreeMap::from([
                (1, "Q second thoughts".to_string()),
                (2, "Q wheel?".to_string())
            ])
        );
        assert_eq!(
            edit_draft_note(&mut conn, &set(DRAFT, 1, "   ")).expect("edit"),
            DraftNoteOutcome::Done
        );
        assert_eq!(
            pick_notes(&conn, DRAFT).expect("notes"),
            BTreeMap::from([(2, "Q wheel?".to_string())]),
            "an empty note deletes the row"
        );
    }

    #[test]
    fn pick_note_rows_carry_when_each_note_was_saved() {
        let (_dir, mut conn) = db("limited-note-rows");
        imported_draft(&conn);
        edit_draft_note(&mut conn, &set(DRAFT, 1, " Q took the bomb ")).expect("edit");
        let rows = pick_note_rows(&conn, DRAFT).expect("rows");
        assert_eq!(
            rows,
            BTreeMap::from([(
                1,
                PickNote {
                    note: "Q took the bomb".to_string(),
                    updated_at_ms: 77,
                }
            )])
        );
        assert!(pick_note_rows(&conn, "q-other").expect("none").is_empty());
    }

    #[test]
    fn a_set_name_comes_from_any_card_of_the_set_that_has_one() {
        let (_dir, conn) = db("limited-set-name");
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, set_code, set_name) VALUES
                 (1, 'QNW', NULL), (2, 'QNW', 'Q New Set'), (3, 'QLD', 'Q Old Set');",
        )
        .expect("cards");
        assert_eq!(
            set_name(&conn, "QNW").expect("name").as_deref(),
            Some("Q New Set")
        );
        assert_eq!(set_name(&conn, "QZZ").expect("none"), None);
    }

    #[test]
    fn a_note_over_2000_characters_is_refused_with_0x_s_message() {
        let (_dir, mut conn) = db("limited-note-max");
        imported_draft(&conn);
        let longest = format!("  {}  ", "é".repeat(MAX_NOTE_CHARS));
        assert_eq!(
            edit_draft_note(&mut conn, &set(DRAFT, 1, &longest)).expect("edit"),
            DraftNoteOutcome::Done,
            "2000 characters after trimming, whatever their bytes"
        );
        let refused = edit_draft_note(&mut conn, &set(DRAFT, 2, &"q".repeat(2001))).expect("edit");
        assert_eq!(refused, DraftNoteOutcome::NoteTooLong);
        assert_eq!(
            refused.refusal(),
            Some("The note field must not be greater than 2000 characters.")
        );
        assert_eq!(DraftNoteOutcome::Done.refusal(), None);
        assert!(!pick_notes(&conn, DRAFT).expect("notes").contains_key(&2));
    }

    #[test]
    fn notes_of_detached_picks_and_drafts_are_not_read() {
        let (_dir, mut conn) = db("limited-note-detached");
        imported_draft(&conn);
        for ordinal in [1, 2] {
            edit_draft_note(&mut conn, &set(DRAFT, ordinal, "Q kept")).expect("edit");
        }
        conn.execute(
            "UPDATE draft_picks SET detached_at = 5 WHERE ordinal = 2",
            [],
        )
        .expect("a detached pick");
        assert_eq!(
            pick_notes(&conn, DRAFT)
                .expect("notes")
                .keys()
                .collect::<Vec<_>>(),
            vec![&1]
        );
        assert_eq!(
            pick_note_rows(&conn, DRAFT)
                .expect("rows")
                .keys()
                .collect::<Vec<_>>(),
            vec![&1]
        );
        conn.execute("UPDATE drafts SET detached_at = 5", [])
            .expect("a detached draft");
        assert!(pick_notes(&conn, DRAFT).expect("notes").is_empty());
        assert!(pick_note_rows(&conn, DRAFT).expect("rows").is_empty());
    }

    #[test]
    fn a_note_on_a_pick_that_does_not_exist_is_not_found() {
        let (_dir, mut conn) = db("limited-note-missing");
        imported_draft(&conn);
        for (token, ordinal) in [(DRAFT, 0), (DRAFT, 3), (DRAFT, 43), ("q-no-such-draft", 1)] {
            assert_eq!(
                edit_draft_note(&mut conn, &set(token, ordinal, "Q")).expect("edit"),
                DraftNoteOutcome::NotFound,
                "{token} {ordinal}"
            );
        }
        assert!(all_pick_notes(&conn).expect("all").is_empty());
    }

    #[test]
    fn a_note_on_a_live_pick_is_saved_and_survives_more_events() {
        let (_dir, mut conn) = db("limited-note-live");
        let file_id = file(&conn, "Qlogs/mtgo.log", "Qtixxxxxxx");
        let (events, inputs) = hobbit(file_id);
        let token = inputs
            .iter()
            .find_map(|event| event.draft_token.clone())
            .expect("a draft token");
        let half = events.len() / 2;
        let mut events = events;
        let later = events.split_off(half);
        let local = account(&conn, "Qtixxxxxxx");
        let sync = |conn: &mut Connection| {
            let tx = conn.transaction().expect("tx");
            crate::league_sync::sync_owner(
                &tx,
                crate::league_sync::LeagueOwner::Account(local),
                chrono_tz::UTC,
                1_900_000_000_000,
            )
            .expect("sync");
            tx.commit().expect("commit");
        };
        write(&mut conn, events);
        sync(&mut conn);
        assert_eq!(
            edit_draft_note(&mut conn, &set(&token, 1, "Q first pick")).expect("edit"),
            DraftNoteOutcome::Done
        );
        assert_eq!(
            edit_draft_note(&mut conn, &set(&token, 43, "Q")).expect("edit"),
            DraftNoteOutcome::NotFound,
            "past the expected picks"
        );
        write(&mut conn, later);
        sync(&mut conn);
        assert_eq!(
            edit_draft_note(&mut conn, &set(&token, 42, "Q last pick")).expect("edit"),
            DraftNoteOutcome::Done
        );
        assert_eq!(
            pick_notes(&conn, &token).expect("notes"),
            BTreeMap::from([
                (1, "Q first pick".to_string()),
                (42, "Q last pick".to_string())
            ])
        );
    }

    /// A pick note is input on the pick's row: a refold of the draft (more
    /// events, a full sync again) never touches it.
    #[test]
    fn a_pick_note_survives_a_refold() {
        let (_dir, mut conn) = db("limited-note-refold");
        let local = account(&conn, "Qtixxxxxxx");
        let file_id = file(&conn, "Qlogs/mtgo.log", "Qtixxxxxxx");
        let (events, inputs) = hobbit(file_id);
        let token = inputs
            .iter()
            .find_map(|event| event.draft_token.clone())
            .expect("a draft token");
        let mut events = events;
        let later = events.split_off(events.len() / 2);
        write(&mut conn, events);
        let sync = |conn: &mut Connection| {
            let tx = conn.transaction().expect("tx");
            crate::league_sync::sync_owner(
                &tx,
                crate::league_sync::LeagueOwner::Account(local),
                chrono_tz::UTC,
                1_900_000_000_000,
            )
            .expect("sync");
            tx.commit().expect("commit");
        };
        sync(&mut conn);
        assert_eq!(
            edit_draft_note(&mut conn, &set(&token, 1, "Q first pick")).expect("edit"),
            DraftNoteOutcome::Done
        );
        write(&mut conn, later);
        sync(&mut conn);
        sync(&mut conn);
        let stored: (String, i64) = conn
            .query_row(
                "SELECT p.note, p.note_updated_at FROM draft_picks p
                 JOIN drafts d ON d.id = p.draft_id
                 WHERE d.draft_token = ?1 AND p.ordinal = 1",
                [&token],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("the pick's note");
        assert_eq!(stored, ("Q first pick".to_string(), 77));
    }

    #[test]
    fn draft_events_are_the_account_s_oldest_first_and_fold_as_the_log_does() {
        let (_dir, mut conn) = db("limited-draft-events");
        let local = account(&conn, "Qtixxxxxxx");
        let other = account(&conn, "Qother");
        // The other player's file is created first, so its events come first
        // in log order.
        let other_file = file(&conn, "Qother/mtgo.log", "Qother");
        let file_id = file(&conn, "Qlogs/mtgo.log", "Qtixxxxxxx");
        let (mut events, inputs) = hobbit(file_id);
        let (other_events, _) = hobbit(other_file);
        // The other file has no Login of the local player: only the first
        // pending pick goes in, as the other player's.
        let stray = other_events
            .into_iter()
            .find(|event| event.kind == Some(EventKind::DraftPendingPick))
            .expect("a pending pick");
        events.push(stray);
        write(&mut conn, events);

        let read = draft_events(&conn, local).expect("events");
        assert_eq!(
            read, inputs,
            "every draft event, in log order, nothing else"
        );
        assert!(
            read.iter()
                .any(|event| event.raw.len() > crate::write::COMPRESS_OVER),
            "compressed rows read back as text"
        );
        assert_eq!(read[0].line_time.len(), 8);
        let tz = chrono_tz::Europe::London;
        let now = 1_900_000_000_000;
        let drafts = fold_drafts(&read, tz, now);
        assert_eq!(drafts, fold_drafts(&inputs, tz, now));
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].picks.len(), 42);

        let theirs = draft_events(&conn, other).expect("events");
        assert_eq!(theirs.len(), 1);
        assert_eq!(theirs[0].kind, EventKind::DraftPendingPick);
        assert!(draft_events(&conn, 999).expect("no account").is_empty());
    }

    #[test]
    fn a_login_switch_in_the_middle_of_a_draft_splits_its_events_by_account() {
        let (_dir, mut conn) = db("limited-draft-login-switch");
        let local = account(&conn, "Qtixxxxxxx");
        let other = account(&conn, "Qother");
        let file_id = file(&conn, "Qlogs/mtgo.log", "Qtixxxxxxx");
        let (mut events, inputs) = hobbit(file_id);
        // The other player logs in after the 20th draft event of the file.
        let switch_at = events
            .iter()
            .filter(|event| event.kind.is_some_and(|kind| DRAFT_KINDS.contains(&kind)))
            .nth(19)
            .expect("a 20th draft event")
            .byte_offset
            + 1;
        let mut login = events
            .iter()
            .find(|event| event.kind == Some(EventKind::Login))
            .expect("a login")
            .clone();
        login.byte_offset = switch_at;
        login.raw = "12:30:00 [INF] (Login|MtGO Login Last Success) Username: Qother".to_string();
        events.push(login);
        write(&mut conn, events);

        let mine = draft_events(&conn, local).expect("events");
        let theirs = draft_events(&conn, other).expect("events");
        assert_eq!(mine.len(), 20, "the local player keeps up to the switch");
        assert_eq!(mine[..], inputs[..20]);
        assert_eq!(
            theirs[..],
            inputs[20..],
            "the rest belongs to the new login"
        );
    }

    /// A `match_deck_registered` line for `MATCH` with these
    /// `(catalog id, quantity, sideboard)` rows.
    fn registered_line(rows: &[(u32, u64, bool)]) -> String {
        let cards: Vec<String> = rows
            .iter()
            .map(|(catalog_id, quantity, sideboard)| {
                format!(
                    "{{\"CatalogID\":{catalog_id},\"Annotation\":0,\"Quantity\":{quantity},\
                     \"InSideboard\":{sideboard}}}"
                )
            })
            .collect();
        format!(
            "12:07:09 [INF] (BaseClient|Inbound: FlsMatchDeckGetRespMessage) \
             {{\"MatchToken\":\"{MATCH}\",\"MatchID\":7,\"Cards\":[{}]}}",
            cards.join(",")
        )
    }

    fn registered_event(file_id: i64, offset: u64, raw: String) -> NewEvent {
        NewEvent {
            log_file_id: file_id,
            byte_offset: offset,
            kind: Some(EventKind::MatchDeckRegistered),
            subjects: Subjects {
                match_token: Some(MATCH.to_string()),
                match_id: Some(7),
                ..Subjects::default()
            },
            logged_at_ms: 5_000 + offset as i64,
            raw,
            quarantined: false,
        }
    }

    #[test]
    fn the_registered_deck_is_the_newest_and_reads_the_same_after_compaction() {
        let (_dir, mut conn) = db("limited-registered");
        let file_id = file(&conn, "Qlogs/mtgo.log", "Qlocal");
        assert_eq!(registered_deck(&conn, MATCH).expect("none"), None);
        write(
            &mut conn,
            vec![
                registered_event(file_id, 10, registered_line(&[(100, 2, false)])),
                registered_event(
                    file_id,
                    20,
                    registered_line(&[(100, 1, false), (200, 3, true)]),
                ),
            ],
        );
        let before = registered_deck(&conn, MATCH)
            .expect("read")
            .expect("a deck");
        assert_eq!(before.match_token.as_deref(), Some(MATCH));
        assert_eq!(before.captured_at_ms, 5_020, "the newest event's time");
        assert_eq!(
            before.cards,
            vec![
                RegisteredCard {
                    catalog_id: 100,
                    quantity: 1,
                    sideboard: false
                },
                RegisteredCard {
                    catalog_id: 200,
                    quantity: 3,
                    sideboard: true
                },
            ]
        );
        assert!(!before.signature.is_empty());

        assert_eq!(compact_match(&conn, MATCH, 9_000).expect("compact"), 2);
        let live: i64 = conn
            .query_row("SELECT COUNT(*) FROM log_events", [], |row| row.get(0))
            .expect("count");
        assert_eq!(live, 0, "the events moved into the archive");
        assert_eq!(
            registered_deck(&conn, MATCH).expect("read"),
            Some(before),
            "the same deck read from the archive"
        );
    }

    /// A `match_deck_registered` line for `MATCH` with this `Cards` JSON.
    fn registered_raw(cards: &str) -> String {
        format!(
            "12:37:19 [INF] (BaseClient|Inbound: FlsMatchDeckGetRespMessage) \
             {{\"MatchToken\":\"{MATCH}\",\"MatchID\":289328482,\"Cards\":{cards},\
             \"ResponseCode\":1}}"
        )
    }

    /// 0.x `ReadRegisteredDeckTest`: "reads the newest registered deck and
    /// treats a missing InSideboard as main deck".
    #[test]
    fn the_newest_registered_deck_reads_a_missing_in_sideboard_as_main_deck() {
        let (_dir, mut conn) = db("limited-registered-no-sideboard-flag");
        let file_id = file(&conn, "Qlogs/mtgo.log", "Qlocal");
        write(
            &mut conn,
            vec![
                registered_event(
                    file_id,
                    10,
                    registered_raw(r#"[{"CatalogID":111,"Quantity":1,"InSideboard":false}]"#),
                ),
                registered_event(
                    file_id,
                    20,
                    registered_raw(
                        r#"[{"CatalogID":154228,"Quantity":2,"InSideboard":false},
                            {"CatalogID":153896,"Quantity":1,"InSideboard":true},
                            {"CatalogID":153894,"Quantity":6}]"#,
                    ),
                ),
            ],
        );
        let deck = registered_deck(&conn, MATCH)
            .expect("read")
            .expect("a deck");
        assert_eq!(
            deck.cards,
            vec![
                RegisteredCard {
                    catalog_id: 154_228,
                    quantity: 2,
                    sideboard: false
                },
                RegisteredCard {
                    catalog_id: 153_896,
                    quantity: 1,
                    sideboard: true
                },
                RegisteredCard {
                    catalog_id: 153_894,
                    quantity: 6,
                    sideboard: false
                },
            ]
        );
    }

    /// MTGO can cut a long line short: 0.x's `RepairJson` adds back up to
    /// three missing closing brackets.
    #[test]
    fn a_registered_line_missing_its_closing_brackets_is_repaired() {
        let (_dir, mut conn) = db("limited-registered-truncated");
        let file_id = file(&conn, "Qlogs/mtgo.log", "Qlocal");
        let cut = format!(
            "12:37:19 [INF] (BaseClient|Inbound: FlsMatchDeckGetRespMessage) \
             {{\"MatchToken\":\"{MATCH}\",\"Cards\":[{{\"CatalogID\":7,\"Quantity\":3,\
             \"InSideboard\":false}}"
        );
        write(&mut conn, vec![registered_event(file_id, 10, cut)]);
        let deck = registered_deck(&conn, MATCH)
            .expect("read")
            .expect("a deck");
        assert_eq!(
            deck.cards,
            vec![RegisteredCard {
                catalog_id: 7,
                quantity: 3,
                sideboard: false
            }]
        );
    }

    /// 0.x: "returns null when the response carries no cards". With no other
    /// event and no import, there is no deck.
    #[test]
    fn a_registered_line_without_cards_is_no_deck() {
        let (_dir, mut conn) = db("limited-registered-empty");
        let file_id = file(&conn, "Qlogs/mtgo.log", "Qlocal");
        write(
            &mut conn,
            vec![registered_event(file_id, 10, registered_raw("[]"))],
        );
        assert_eq!(registered_deck(&conn, MATCH).expect("read"), None);
    }

    #[test]
    fn registered_quantities_are_clamped_and_empty_rows_dropped() {
        let (_dir, mut conn) = db("limited-registered-hostile");
        let file_id = file(&conn, "Qlogs/mtgo.log", "Qlocal");
        write(
            &mut conn,
            vec![registered_event(
                file_id,
                10,
                registered_line(&[(100, 4_294_967_295, false), (200, 0, false), (300, 2, true)]),
            )],
        );
        let deck = registered_deck(&conn, MATCH)
            .expect("read")
            .expect("a deck");
        assert_eq!(
            deck.cards,
            vec![
                RegisteredCard {
                    catalog_id: 100,
                    quantity: MAX_CARD_QUANTITY,
                    sideboard: false
                },
                RegisteredCard {
                    catalog_id: 300,
                    quantity: 2,
                    sideboard: true
                },
            ]
        );
    }

    #[test]
    fn without_events_the_registered_deck_is_the_imported_one() {
        let (_dir, conn) = db("limited-registered-imported");
        conn.execute_batch(&format!(
            "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);
             INSERT INTO matches (id, token, account_id, source, state, outcome, started_at,
                 last_activity_at, revision, derived_version)
             VALUES (8, '{MATCH}', 1, 'imported', 'complete', 'win', 1, 1, 1, 0);
             INSERT INTO limited_deck_snapshots (match_id, source, cards, signature, captured_at)
             VALUES (8, 'imported',
                 '[{{\"catalog_id\":100,\"quantity\":900,\"sideboard\":false}},
                   {{\"catalog_id\":200,\"quantity\":1,\"sideboard\":true}}]', '', 4000);"
        ))
        .expect("imported deck");
        let deck = registered_deck(&conn, MATCH)
            .expect("read")
            .expect("a deck");
        assert_eq!(deck.captured_at_ms, 4_000);
        assert_eq!(deck.cards[0].quantity, MAX_CARD_QUANTITY, "clamped on read");
        assert_eq!(
            stored_registered_deck(&conn, MATCH).expect("stored"),
            Some(deck)
        );
        assert_eq!(
            stored_registered_deck(&conn, "q-other").expect("none"),
            None
        );
    }

    #[test]
    fn limited_runs_stay_off_the_constructed_reads() {
        use crate::league_sync::ImportedSeed;
        use crate::league_sync::fold_inputs;
        use crate::league_sync::{LeagueOwner, sync_every_owner};
        use crate::leagues::{RunFilter, stored_limited_runs};
        use leagues::{LeagueInput, LeagueKind};
        let (_dir, conn) = db("limited-runs-apart");
        conn.execute_batch(
            "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);
             INSERT INTO matches (token, account_id, source, format, state, outcome, started_at,
                 last_activity_at, revision, derived_version)
             VALUES ('q-c', 1, 'imported', 'CMODERN', 'complete', 'win', 5, 5, 1, 0),
                    ('q-d', 1, 'imported', 'DHOBHOBHOB', 'complete', 'win', 6, 6, 1, 0);",
        )
        .expect("matches");
        let seed = |token: &str, event_id, name: &str, format: &str, started_at, kind: &str| {
            ImportedSeed {
                token: token.to_string(),
                event_id: Some(event_id),
                name: name.to_string(),
                format: Some(format.to_string()),
                state: "complete".to_string(),
                started_at,
                joined_at: None,
                completed_at: None,
                dropped_at: None,
                deck_version_id: None,
                kind: kind.to_string(),
                set_code: None,
                mtgo_course_id: None,
                legacy_id: None,
            }
        };
        insert_imported_run(
            &conn,
            42,
            Some(1),
            &seed("q-con", 1, "Q League", "CMODERN", 5, "constructed"),
        );
        insert_imported_run(
            &conn,
            29,
            Some(1),
            &ImportedSeed {
                set_code: Some("HOB".to_string()),
                mtgo_course_id: Some(35),
                ..seed("q-lim", 7001, "Q Draft", "Limited", 6, "draft")
            },
        );
        link_imported_match(&conn, 42, "q-c");
        link_imported_match(&conn, 29, "q-d");
        // Both runs seed the fold; the draft one is a limited seed, so the
        // fold makes it a draft run and only the constructed one stays
        // constructed.
        let inputs = fold_inputs(&conn, LeagueOwner::Account(1)).expect("inputs");
        let seeds = inputs
            .inputs
            .iter()
            .filter(|(_, _, _, input)| matches!(input, LeagueInput::ImportedRun(_)))
            .count();
        assert_eq!(seeds, 2, "every 0.x run seeds the fold");
        assert_eq!(
            inputs.facts.seeds.get("0x:29").map(|seed| seed.kind),
            Some(LeagueKind::Draft)
        );
        assert!(!inputs.facts.seeds.contains_key("0x:42"));
        let constructed = inputs.constructed_runs(chrono_tz::UTC);
        assert_eq!(constructed.len(), 1);
        assert_eq!(constructed[0].key, "0x:42");

        let tx = conn.unchecked_transaction().expect("tx");
        sync_every_owner(&tx, chrono_tz::UTC, 0).expect("sync");
        tx.commit().expect("commit");
        let limited = stored_limited_runs(
            &conn,
            &RunFilter {
                owners: vec![LeagueOwner::Account(1)],
                ..RunFilter::default()
            },
        )
        .expect("limited")
        .runs;
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].run.key, "0x:29");
        assert_eq!(
            (
                limited[0].run.kind.as_str(),
                limited[0].run.set_code.as_deref(),
                limited[0].run.course_id
            ),
            ("draft", Some("HOB"), Some(35))
        );
        let members: Vec<&str> = limited[0]
            .matches
            .iter()
            .map(|row| row.token.as_str())
            .collect();
        assert_eq!(members, vec!["q-d"]);
        assert!(
            stored_limited_runs(
                &conn,
                &RunFilter {
                    owners: vec![LeagueOwner::Nobody],
                    ..RunFilter::default()
                },
            )
            .expect("none")
            .runs
            .is_empty()
        );
    }
}
