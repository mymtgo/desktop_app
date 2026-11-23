//! The draft fold (0.x `ApplyDraftEvent`, `ReconcileDraftFromEnded` and
//! `AbandonStaleDrafts`; limited spec section 4).
//!
//! 0.x applied each draft event to `drafts` and `draft_picks` rows. Here the
//! same branches run over plain structs: `DraftFold::apply` takes one event at
//! a time in log order `(file, offset)`, and `DraftFold::drafts` returns the
//! drafts as they stand at `now`. `fold_drafts` does both for a whole list, so
//! folding events one by one or all at once gives the same drafts.
//!
//! Pick notes are not part of the fold: they live in their own table, keyed by
//! draft token and ordinal, so refolding never touches them.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use chrono::{DateTime, Duration, NaiveTime};
use chrono_tz::Tz;
use mtgo_protocol::{DayClock, EventKind};
use regex::Regex;
use serde_json::Value;

use crate::payload::{as_string, as_u32, as_u64, first_object, payload_time_ms};

/// A draft with no activity for longer than this is abandoned (0.x
/// `AbandonStaleDrafts`, 30 minutes).
pub const ABANDON_AFTER_MS: i64 = 30 * 60 * 1000;

/// 0.x column defaults for a new draft row.
const DEFAULT_SEAT_COUNT: u32 = 8;
const DEFAULT_PACK_SIZE: u32 = 14;
const DEFAULT_PICKS_EXPECTED: u32 = 42;
/// Packs in a draft: `picks_expected` is the pack size times this.
const PACKS: u32 = 3;
/// How far a line time may sit after its deadline's date before it belongs to
/// the day before (0.x `ConvertMtgoTimestamp`).
const ROLLOVER_HOURS: i64 = 12;

static SELECTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Reserved - (?<reserved>[\d,]*) ~ Committed - (?<committed>[\d,]*)")
        .expect("valid regex")
});
static POOL_LEAGUE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"for league: (?<league>\d+)").expect("valid regex"));

/// One stored draft event, as the app reads it from `log_events`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftEventInput {
    pub kind: EventKind,
    /// `log_events.draft_token`; `None` for `draft_selection` and
    /// `league_pool_granted`, which carry no token.
    pub draft_token: Option<String>,
    /// The event's UTC time.
    pub logged_at_ms: i64,
    /// The line's local `HH:MM:SS` (the first 8 characters of the raw text).
    pub line_time: String,
    /// The event's full text.
    pub raw: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftState {
    Connecting,
    Picking,
    Finished,
    Abandoned,
}

/// A card the player hovered (reserved) before committing, with when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    pub catalog_id: u32,
    pub at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    /// 1-based position in the draft: `(pack_number - 1) * pack_size + pick_number`.
    pub ordinal: u32,
    pub pack_number: u32,
    pub pick_number: u32,
    pub pack_id: Option<u64>,
    pub direction: Option<u32>,
    pub cards_available: Vec<u32>,
    pub picked_catalog_id: Option<u32>,
    /// Set by the server's commit ack: the "committed" marker.
    pub picked_card_id: Option<u64>,
    pub reservations: Vec<Reservation>,
    pub shown_at_ms: Option<i64>,
    pub deadline_at_ms: Option<i64>,
    pub picked_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub token: String,
    /// MTGO's league id (0.x `leagues.event_id`).
    pub league_event_id: Option<u64>,
    /// MTGO's course id (0.x `leagues.mtgo_course_id`).
    pub course_id: Option<u64>,
    /// MTGO's draft group id (0.x `drafts.mtgo_draft_id`).
    pub draft_id: Option<u64>,
    pub pod_token: Option<String>,
    pub seat_index: Option<u32>,
    pub seat_count: u32,
    pub booster_catalog_id: Option<u32>,
    pub state: DraftState,
    pub pack_size: u32,
    pub picks_expected: u32,
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
    /// Ordered by ordinal.
    pub picks: Vec<Pick>,
}

/// A draft being folded, with what the fold needs beyond the public fields.
#[derive(Debug, Clone)]
struct DraftRecord {
    draft: Draft,
    /// The first event's time: 0.x's `created_at`, the last activity fallback.
    first_event_ms: i64,
    /// The local LoginID from this draft's first standing that has one.
    local_login_id: Option<u64>,
    /// The latest pod state's players' LoginIDs, in seat order.
    pod_login_ids: Option<Vec<Option<u64>>>,
}

impl DraftRecord {
    fn new(token: &str, first_event_ms: i64) -> Self {
        Self {
            draft: Draft {
                token: token.to_string(),
                league_event_id: None,
                course_id: None,
                draft_id: None,
                pod_token: None,
                seat_index: None,
                seat_count: DEFAULT_SEAT_COUNT,
                booster_catalog_id: None,
                state: DraftState::Connecting,
                pack_size: DEFAULT_PACK_SIZE,
                picks_expected: DEFAULT_PICKS_EXPECTED,
                started_at_ms: None,
                ended_at_ms: None,
                picks: Vec::new(),
            },
            first_event_ms,
            local_login_id: None,
            pod_login_ids: None,
        }
    }

    /// The newest shown or picked time over the picks, else the start, else
    /// the first event (0.x `AbandonStaleDrafts::lastActivity`).
    fn last_activity_ms(&self) -> i64 {
        let mut latest: Option<i64> = None;
        for pick in &self.draft.picks {
            latest = latest.max(pick.shown_at_ms).max(pick.picked_at_ms);
        }
        latest
            .or(self.draft.started_at_ms)
            .unwrap_or(self.first_event_ms)
    }

    /// Picking or connecting with no activity for more than 30 minutes before `at_ms`.
    fn is_stale(&self, at_ms: i64) -> bool {
        let open = matches!(
            self.draft.state,
            DraftState::Connecting | DraftState::Picking
        );
        open && self.last_activity_ms() < at_ms - ABANDON_AFTER_MS
    }

    /// The newest `shown_at` over the picks, for choosing the active draft.
    fn latest_shown_ms(&self) -> Option<i64> {
        self.draft
            .picks
            .iter()
            .filter_map(|pick| pick.shown_at_ms)
            .max()
    }

    /// The draft as it stands at `now_ms`: seat worked out, stale drafts abandoned.
    fn snapshot(&self, now_ms: i64) -> Draft {
        let mut draft = self.draft.clone();
        draft.seat_index = self.seat_index();
        if self.is_stale(now_ms) {
            draft.state = DraftState::Abandoned;
        }
        draft
    }

    /// The local player's seat in the latest pod: the last player whose
    /// LoginID matches the standing's (0.x `podState`).
    fn seat_index(&self) -> Option<u32> {
        let local = self.local_login_id?;
        let players = self.pod_login_ids.as_ref()?;
        let mut seat = None;
        for (index, login_id) in players.iter().enumerate() {
            if *login_id == Some(local) {
                seat = u32::try_from(index).ok();
            }
        }
        seat
    }

    /// The pick with this ordinal, created empty (0.x `firstOrNew`) when missing.
    fn pick_mut(&mut self, ordinal: u32, new_pick: impl FnOnce() -> Pick) -> &mut Pick {
        let picks = &mut self.draft.picks;
        let index = match picks.iter().position(|pick| pick.ordinal >= ordinal) {
            Some(index) if picks[index].ordinal == ordinal => index,
            Some(index) => {
                picks.insert(index, new_pick());
                index
            }
            None => {
                picks.push(new_pick());
                picks.len() - 1
            }
        };
        &mut picks[index]
    }
}

/// An empty pick row, as 0.x's `DraftPick::firstOrNew` before its fill.
fn empty_pick(ordinal: u32) -> Pick {
    Pick {
        ordinal,
        pack_number: 0,
        pick_number: 0,
        pack_id: None,
        direction: None,
        cards_available: Vec::new(),
        picked_catalog_id: None,
        picked_card_id: None,
        reservations: Vec::new(),
        shown_at_ms: None,
        deadline_at_ms: None,
        picked_at_ms: None,
    }
}

/// Folds draft events, one at a time, in log order.
#[derive(Debug, Clone)]
pub struct DraftFold {
    tz: Tz,
    /// In creation order (0.x draft ids).
    records: Vec<DraftRecord>,
}

impl DraftFold {
    /// `tz` is the OS timezone the log's line times are in.
    pub fn new(tz: Tz) -> Self {
        Self {
            tz,
            records: Vec::new(),
        }
    }

    /// Apply one event. Events must arrive in log order `(file, offset)`.
    pub fn apply(&mut self, event: &DraftEventInput) {
        match event.kind {
            EventKind::DraftCreated => {
                self.draft_for(event);
            }
            EventKind::DraftLeagueStanding => self.standing(event),
            EventKind::DraftJoined => self.joined(event),
            EventKind::DraftPodState => self.pod_state(event),
            EventKind::DraftPendingPick => self.pending_pick(event),
            EventKind::DraftSelection => self.selection(event),
            EventKind::DraftPickCommitted => self.committed(event),
            EventKind::DraftEnded => self.ended(event),
            EventKind::DraftStateChanged => self.state_changed(event),
            EventKind::LeaguePoolGranted => self.pool_granted(event),
            // Pack opened is a no-op in 0.x; other kinds are not draft events.
            _ => {}
        }
    }

    /// The drafts as they stand at `now_ms`, in the order they were first seen.
    /// A connecting or picking draft with no activity for more than 30 minutes
    /// before `now_ms` is abandoned.
    pub fn drafts(&self, now_ms: i64) -> Vec<Draft> {
        self.records
            .iter()
            .map(|record| record.snapshot(now_ms))
            .collect()
    }

    /// The draft for the event's token, created (state connecting) when new.
    /// `None` for token-less events.
    fn draft_for(&mut self, event: &DraftEventInput) -> Option<usize> {
        let token = event.draft_token.as_deref()?;
        if let Some(index) = self
            .records
            .iter()
            .position(|record| record.draft.token == token)
        {
            return Some(index);
        }
        self.records
            .push(DraftRecord::new(token, event.logged_at_ms));
        Some(self.records.len() - 1)
    }

    /// The draft token-less lines belong to: the picking draft with the newest
    /// shown pick (0.x `activeDraft`). A draft that has gone quiet for more
    /// than 30 minutes before the line is skipped, as 0.x had abandoned it by
    /// then.
    fn active_draft(&self, at_ms: i64) -> Option<usize> {
        let mut best: Option<(usize, Option<i64>)> = None;
        for (index, record) in self.records.iter().enumerate() {
            if record.draft.state != DraftState::Picking || record.is_stale(at_ms) {
                continue;
            }
            let shown = record.latest_shown_ms();
            // Ties go to the newer draft.
            let better = match best {
                None => true,
                Some((_, best_shown)) => shown >= best_shown,
            };
            if better {
                best = Some((index, shown));
            }
        }
        best.map(|(index, _)| index)
    }

    fn standing(&mut self, event: &DraftEventInput) {
        let json = first_object(&event.raw);
        let Some(index) = self.draft_for(event) else {
            return;
        };
        let Some(json) = json else {
            return;
        };
        let record = &mut self.records[index];
        if let Some(league) = as_u64(json.get("LeagueID")) {
            record.draft.league_event_id = Some(league);
            record.draft.course_id = as_u64(json.get("CourseID"));
        }
        if record.draft.draft_id.is_none() {
            record.draft.draft_id = as_u64(json.get("DraftGroupID"));
        }
        if record.draft.started_at_ms.is_none() {
            record.draft.started_at_ms = payload_time_ms(json.get("StartDate"));
        }
        if record.local_login_id.is_none() {
            record.local_login_id = as_u64(json.get("LoginID")).filter(|id| *id != 0);
        }
    }

    fn joined(&mut self, event: &DraftEventInput) {
        let json = first_object(&event.raw);
        let Some(index) = self.draft_for(event) else {
            return;
        };
        let Some(json) = json else {
            return;
        };
        let draft = &mut self.records[index].draft;
        if draft.league_event_id.is_none() {
            draft.league_event_id = as_u64(json.get("LeagueID"));
        }
        if draft.draft_id.is_none() {
            draft.draft_id = as_u64(json.get("DraftGroupID"));
        }
    }

    fn pod_state(&mut self, event: &DraftEventInput) {
        let json = first_object(&event.raw);
        let Some(index) = self.draft_for(event) else {
            return;
        };
        let Some(json) = json else {
            return;
        };
        let pod = json.get("Pods").and_then(|pods| pods.get(0));
        let players: Vec<Value> = pod
            .and_then(|pod| pod.get("Players"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let record = &mut self.records[index];
        record.pod_login_ids = Some(
            players
                .iter()
                .map(|player| as_u64(player.get("LoginID")))
                .collect(),
        );
        let draft = &mut record.draft;
        if draft.draft_id.is_none() {
            draft.draft_id = as_u64(json.get("DraftID"));
        }
        draft.pod_token = as_string(pod.and_then(|pod| pod.get("Token")));
        draft.seat_count = u32::try_from(players.len()).unwrap_or(u32::MAX).max(1);
        draft.booster_catalog_id = as_u32(
            players
                .first()
                .and_then(|player| player.get("BoosterInfo"))
                .and_then(|boosters| boosters.get(0))
                .and_then(|booster| booster.get("CatalogID")),
        );
    }

    fn pending_pick(&mut self, event: &DraftEventInput) {
        let json = first_object(&event.raw);
        let Some(index) = self.draft_for(event) else {
            return;
        };
        let Some(json) = json else {
            return;
        };
        let Some(pending) = json.get("PendingPick").filter(|value| !is_empty(value)) else {
            return;
        };
        let cards: Vec<u32> = pending
            .get("CardsAvailable")
            .and_then(Value::as_array)
            .map(|cards| {
                cards
                    .iter()
                    .map(|card| as_u32(Some(card)).unwrap_or(0))
                    .collect()
            })
            .unwrap_or_default();
        let Ok(count) = u32::try_from(cards.len()) else {
            return;
        };
        if count == 0 {
            return;
        }

        let record = &mut self.records[index];
        let pack_size = record.draft.pack_size.max(count);
        let Some(booster_index) = booster_index(pending) else {
            return;
        };
        let Some((pack_number, pick_number, ordinal)) = numbering(booster_index, pack_size, count)
        else {
            return;
        };

        let deadline = payload_time_ms(pending.get("EndTime"));
        let shown = match deadline {
            Some(deadline) => line_time_on_deadline_date(deadline, &event.line_time, self.tz)
                .unwrap_or(event.logged_at_ms),
            None => event.logged_at_ms,
        };

        let pick = record.pick_mut(ordinal, || empty_pick(ordinal));
        pick.pack_number = pack_number;
        pick.pick_number = pick_number;
        if let Some(pack_id) = as_u64(pending.get("PackID")) {
            pick.pack_id = Some(pack_id);
        }
        if let Some(direction) = as_u32(pending.get("Direction")) {
            pick.direction = Some(direction);
        }
        pick.cards_available = cards;
        if pick.shown_at_ms.is_none() {
            pick.shown_at_ms = Some(shown);
        }
        if deadline.is_some() {
            pick.deadline_at_ms = deadline;
        }

        let draft = &mut record.draft;
        if draft.state != DraftState::Finished {
            draft.state = DraftState::Picking;
        }
        draft.pack_size = pack_size;
        draft.picks_expected = pack_size.saturating_mul(PACKS);
        if draft.draft_id.is_none() {
            draft.draft_id = as_u64(json.get("DraftID"));
        }
    }

    fn selection(&mut self, event: &DraftEventInput) {
        let Some(caps) = SELECTION.captures(&event.raw) else {
            return;
        };
        let Some(index) = self.active_draft(event.logged_at_ms) else {
            return;
        };
        let Some(pick) = self.records[index]
            .draft
            .picks
            .iter_mut()
            .rev()
            .find(|pick| pick.picked_catalog_id.is_none())
        else {
            return;
        };

        let reserved = &caps["reserved"];
        if !reserved.is_empty() {
            let entry = Reservation {
                catalog_id: first_id(reserved),
                at_ms: event.logged_at_ms,
            };
            // The same line applied twice must not double the trail.
            if !pick.reservations.contains(&entry) {
                pick.reservations.push(entry);
            }
        }

        let committed = &caps["committed"];
        if !committed.is_empty() {
            pick.picked_catalog_id = Some(first_id(committed));
            if pick.picked_at_ms.is_none() {
                pick.picked_at_ms = Some(event.logged_at_ms);
            }
        }
    }

    fn committed(&mut self, event: &DraftEventInput) {
        let json = first_object(&event.raw);
        let Some(index) = self.draft_for(event) else {
            return;
        };
        let Some(json) = json else {
            return;
        };
        let Some(last) = json
            .get("PicksMade")
            .and_then(Value::as_array)
            .and_then(|made| made.last())
        else {
            return;
        };
        let selection = last.get("Selections").and_then(|list| list.get(0));
        let catalog_id = as_u32(selection.and_then(|s| s.get("CatalogID"))).unwrap_or(0);
        let card_id = as_u64(selection.and_then(|s| s.get("CardID"))).unwrap_or(0);
        let pack_id = as_u64(last.get("PackID"));
        let time = payload_time_ms(last.get("Time"));

        let picks = &mut self.records[index].draft.picks;
        if commit_already_applied(picks, pack_id, catalog_id, card_id) {
            return;
        }
        let target = picks
            .iter()
            .position(|pick| pick.pack_id == pack_id && pick.picked_card_id.is_none())
            .or_else(|| picks.iter().position(|pick| pick.picked_card_id.is_none()));
        let Some(target) = target else {
            return;
        };

        let pick = &mut picks[target];
        if catalog_id != 0 {
            pick.picked_catalog_id = Some(catalog_id);
        }
        pick.picked_card_id = (card_id != 0).then_some(card_id);
        if time.is_some() {
            pick.picked_at_ms = time;
        }
    }

    fn ended(&mut self, event: &DraftEventInput) {
        let json = first_object(&event.raw);
        let Some(index) = self.draft_for(event) else {
            return;
        };
        let Some(json) = json else {
            return;
        };
        let picks: Vec<Value> = json
            .get("Picks")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        reconcile_from_ended(&mut self.records[index], &picks, event.logged_at_ms);
    }

    fn state_changed(&mut self, event: &DraftEventInput) {
        if !event.raw.contains("to DraftFinishedState") {
            return;
        }
        let Some(index) = self.draft_for(event) else {
            return;
        };
        let draft = &mut self.records[index].draft;
        if draft.state != DraftState::Finished {
            draft.state = DraftState::Finished;
            if draft.ended_at_ms.is_none() {
                draft.ended_at_ms = Some(event.logged_at_ms);
            }
        }
    }

    /// The pool grant names the league: the newest draft already in that
    /// league, else the active draft, which joins the league when it has none
    /// (0.x `poolGranted` and `LinkUnlinkedDrafts::linkByLeagueId`). 0.x only
    /// logged a warning when the granted cards differed from the picks.
    fn pool_granted(&mut self, event: &DraftEventInput) {
        let Some(league) = POOL_LEAGUE
            .captures(&event.raw)
            .and_then(|caps| caps["league"].parse::<u64>().ok())
        else {
            return;
        };
        let in_league = self
            .records
            .iter()
            .rposition(|record| record.draft.league_event_id == Some(league));
        let Some(index) = in_league.or_else(|| self.active_draft(event.logged_at_ms)) else {
            return;
        };
        let draft = &mut self.records[index].draft;
        if draft.league_event_id.is_none() {
            draft.league_event_id = Some(league);
        }
    }
}

/// Fold one account's draft events (in log order) and return the drafts as
/// they stand at `now_ms`. `local_tz` is the OS timezone of the log's line times.
pub fn fold_drafts(events: &[DraftEventInput], local_tz: Tz, now_ms: i64) -> Vec<Draft> {
    let mut fold = DraftFold::new(local_tz);
    for event in events {
        fold.apply(event);
    }
    fold.drafts(now_ms)
}

/// How many of each card the draft picked (0.x `Draft::poolCounts`).
pub fn pool_counts(draft: &Draft) -> BTreeMap<u32, u32> {
    let mut counts = BTreeMap::new();
    for pick in &draft.picks {
        if let Some(catalog_id) = pick.picked_catalog_id {
            *counts.entry(catalog_id).or_insert(0) += 1;
        }
    }
    counts
}

/// The pending pick's `BoosterInfo.BoosterIndex`: 0 when absent (0.x `?? 0`),
/// `None` when present but not a whole number that fits (negative, huge or
/// junk), so the line is ignored rather than numbered onto a real pick.
fn booster_index(pending: &Value) -> Option<u32> {
    let value = pending
        .get("BoosterInfo")
        .and_then(|info| info.get("BoosterIndex"));
    match value {
        None | Some(Value::Null) => Some(0),
        Some(value) => as_u32(Some(value)),
    }
}

/// Pack number, pick number and ordinal of a pending pick (0.x `pendingPick`):
/// pack `BoosterIndex + 1`, pick `pack_size + 1 - count`, ordinal
/// `(pack - 1) * pack_size + pick`. `None` when the numbers do not fit, so a
/// hostile payload can neither panic nor wrap onto a real pick.
fn numbering(booster_index: u32, pack_size: u32, count: u32) -> Option<(u32, u32, u32)> {
    let pack_number = booster_index.checked_add(1)?;
    let pick_number = pack_size.checked_add(1)?.checked_sub(count)?;
    let ordinal = booster_index
        .checked_mul(pack_size)?
        .checked_add(pick_number)?;
    Some((pack_number, pick_number, ordinal))
}

/// PHP's `empty()` for a decoded JSON value.
fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(flag) => !flag,
        Value::Array(items) => items.is_empty(),
        Value::Object(fields) => fields.is_empty(),
        Value::String(text) => text.is_empty() || text == "0",
        Value::Number(number) => number.as_f64() == Some(0.0),
    }
}

/// The first id of a `154538,154228` list, 0 when it does not parse (PHP `(int)`).
fn first_id(list: &str) -> u32 {
    list.split(',')
        .next()
        .and_then(|id| id.parse().ok())
        .unwrap_or(0)
}

/// True when this commit has already been projected onto its own pick (0.x
/// `commitAlreadyApplied`). `picked_card_id` is the marker: the selection line
/// writes `picked_catalog_id` first, before the server ack.
fn commit_already_applied(
    picks: &[Pick],
    pack_id: Option<u64>,
    catalog_id: u32,
    card_id: u64,
) -> bool {
    if catalog_id == 0 {
        return false;
    }
    picks.iter().any(|pick| {
        let same_card = pick.picked_catalog_id == Some(catalog_id);
        let same_pack = match pack_id {
            Some(pack_id) => pick.pack_id == Some(pack_id),
            None => pick.picked_at_ms.is_some(),
        };
        let same_copy = if card_id != 0 {
            pick.picked_card_id == Some(card_id)
        } else {
            pick.picked_at_ms.is_some()
        };
        same_card && same_pack && same_copy
    })
}

/// The ended message's `Picks` is the authoritative ordered list of the local
/// player's picks: `Picks[i]` is ordinal `i + 1`. Fill whatever the live stream
/// missed without overwriting it, then finish the draft (0.x
/// `ReconcileDraftFromEnded`).
fn reconcile_from_ended(record: &mut DraftRecord, picks: &[Value], ended_at_ms: i64) {
    let pack_size = record.draft.pack_size.max(1);
    for (index, entry) in picks.iter().enumerate() {
        let Ok(ordinal) = u32::try_from(index + 1) else {
            break;
        };
        let selection = entry.get("Selections").and_then(|list| list.get(0));
        let catalog_id = as_u32(selection.and_then(|s| s.get("CatalogID"))).unwrap_or(0);
        let card_id = as_u64(selection.and_then(|s| s.get("CardID"))).unwrap_or(0);

        let pick = record.pick_mut(ordinal, || Pick {
            pack_number: (ordinal - 1) / pack_size + 1,
            pick_number: (ordinal - 1) % pack_size + 1,
            cards_available: vec![catalog_id],
            ..empty_pick(ordinal)
        });
        if pick.pack_id.is_none() {
            pick.pack_id = as_u64(entry.get("PackID"));
        }
        if catalog_id != 0 {
            pick.picked_catalog_id = Some(catalog_id);
        }
        if card_id != 0 {
            pick.picked_card_id = Some(card_id);
        }
        if pick.picked_at_ms.is_none() {
            pick.picked_at_ms = payload_time_ms(entry.get("Time"));
        }
    }

    let draft = &mut record.draft;
    draft.state = DraftState::Finished;
    if draft.ended_at_ms.is_none() {
        draft.ended_at_ms = Some(ended_at_ms);
    }
    let count = u32::try_from(picks.len()).unwrap_or(u32::MAX);
    draft.picks_expected = draft.picks_expected.max(count);
}

/// The line's local `HH:MM:SS` on the deadline's local date, as UTC
/// milliseconds; a day earlier when that lands more than 12 hours after the
/// deadline (0.x `ConvertMtgoTimestamp`). `None` when the line time does not parse.
fn line_time_on_deadline_date(deadline_ms: i64, line_time: &str, tz: Tz) -> Option<i64> {
    let time = NaiveTime::parse_from_str(line_time, "%H:%M:%S").ok()?;
    let deadline = DateTime::from_timestamp_millis(deadline_ms)?.with_timezone(&tz);
    let date = deadline.date_naive();
    let at = DayClock::new(date, tz).resolve(time);
    let limit = deadline_ms + Duration::hours(ROLLOVER_HOURS).num_milliseconds();
    if at > limit {
        return Some(DayClock::new(date.pred_opt()?, tz).resolve(time));
    }
    Some(at)
}
