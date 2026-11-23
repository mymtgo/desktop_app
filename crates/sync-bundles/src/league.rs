//! The league bundle (0.x `LeagueBundleBuilder`, survey section 12), built
//! from v1's `leagues` row, its draft (`drafts`, `draft_picks` in ordinal
//! order) and its matches' registered decks (`limited_deck_snapshots`),
//! plus the carry (`deck_change_detected`, a column v1 dropped).

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};

use crate::carry::{Carry, LEAGUE_CARRY};
use crate::client_id::deck_client_id;
use crate::datetime::{bundle_datetime, parse_bundle_datetime};
use crate::deck::BundleError;
use crate::loose::{int, text};
use crate::platform::{BundlePlatform, add_platform, copy_platform};

/// One draft pick (`draft_picks`).
#[derive(Debug, Clone, PartialEq)]
pub struct PickInput {
    pub ordinal: i64,
    pub pack_number: i64,
    pub pick_number: i64,
    pub pack_id: Option<i64>,
    pub direction: Option<i64>,
    /// The JSON array of catalog ids.
    pub cards_available: Value,
    pub picked_catalog_id: Option<i64>,
    pub picked_card_id: Option<i64>,
    /// `(catalog_id, at_ms)` of each reservation.
    pub reservations: Vec<(u32, i64)>,
    pub shown_at_ms: Option<i64>,
    pub deadline_at_ms: Option<i64>,
    pub picked_at_ms: Option<i64>,
    pub note: Option<String>,
}

/// The run's draft (`drafts`).
#[derive(Debug, Clone, PartialEq)]
pub struct DraftInput {
    pub draft_token: String,
    pub mtgo_draft_id: Option<i64>,
    pub pod_token: Option<String>,
    pub seat_count: i64,
    pub seat_index: Option<i64>,
    pub booster_catalog_id: Option<i64>,
    /// `connecting`, `picking`, `finished` or `abandoned`.
    pub state: String,
    pub pack_size: i64,
    pub picks_expected: i64,
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
    pub picks: Vec<PickInput>,
}

/// One registered deck of a run's match (`limited_deck_snapshots`).
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotInput {
    /// The JSON array of `{catalog_id, quantity, sideboard}`.
    pub cards: Value,
    pub signature: String,
    pub captured_at_ms: i64,
    pub match_token: Option<String>,
}

/// What a league bundle is built from.
#[derive(Debug, Clone, PartialEq)]
pub struct LeagueBundleInput {
    /// The game the league is from (MTGO writes no `platform` key).
    pub platform: BundlePlatform,
    pub token: String,
    pub name: String,
    pub format: Option<String>,
    pub started_at_ms: i64,
    pub joined_at_ms: Option<i64>,
    /// The stored (derived) `dropped_at`.
    pub dropped_at_ms: Option<i64>,
    pub completed_at_ms: Option<i64>,
    /// `active`, `complete`, `partial` or `dropped`.
    pub state: String,
    pub notes: Option<String>,
    /// `source = 'manual'`.
    pub manual: bool,
    /// `constructed`, `draft` or `sealed`.
    pub kind: String,
    pub set_code: Option<String>,
    pub mtgo_course_id: Option<i64>,
    pub deck_mtgo_id: Option<String>,
    pub deck_version_signature: Option<String>,
    pub drafts: Vec<DraftInput>,
    pub snapshots: Vec<SnapshotInput>,
    /// `leagues.removed_at`: the player deleted the league (unix ms).
    pub removed_at_ms: Option<i64>,
    /// `leagues.user_state`: the player's status (`USER_STATES`).
    pub user_state: Option<String>,
    /// `leagues.user_dropped_at` (written as a bundle datetime).
    pub user_dropped_at_ms: Option<i64>,
    /// `leagues.edited_at`: the player's last edit (unix ms).
    pub edited_at_ms: Option<i64>,
}

fn datetime(ms: Option<i64>) -> Value {
    ms.map_or(Value::Null, |ms| Value::String(bundle_datetime(ms)))
}

/// 0.x's reservation time: Carbon's `toIso8601String()` in UTC.
fn reservation_time(ms: i64) -> String {
    let time = DateTime::from_timestamp_millis(ms.div_euclid(1_000) * 1_000).unwrap_or_default();
    time.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
}

fn pick_section(pick: &PickInput) -> Value {
    let reservations: Vec<Value> = pick
        .reservations
        .iter()
        .map(
            |(catalog_id, at_ms)| json!({"catalog_id": catalog_id, "at": reservation_time(*at_ms)}),
        )
        .collect();
    json!({
        "ordinal": pick.ordinal,
        "pack_number": pick.pack_number,
        "pick_number": pick.pick_number,
        "pack_id": pick.pack_id,
        "direction": pick.direction,
        "cards_available": pick.cards_available,
        "picked_catalog_id": pick.picked_catalog_id,
        "picked_card_id": pick.picked_card_id,
        "reservations": reservations,
        "shown_at": datetime(pick.shown_at_ms),
        "deadline_at": datetime(pick.deadline_at_ms),
        "picked_at": datetime(pick.picked_at_ms),
        "note": pick.note,
    })
}

fn draft_section(draft: &DraftInput) -> Value {
    let mut picks: Vec<&PickInput> = draft.picks.iter().collect();
    picks.sort_by_key(|pick| pick.ordinal);
    json!({
        "draft_token": draft.draft_token,
        "mtgo_draft_id": draft.mtgo_draft_id,
        "pod_token": draft.pod_token,
        "seat_count": draft.seat_count,
        "seat_index": draft.seat_index,
        "booster_catalog_id": draft.booster_catalog_id,
        "state": draft.state,
        "pack_size": draft.pack_size,
        "picks_expected": draft.picks_expected,
        "started_at": datetime(draft.started_at_ms),
        "ended_at": datetime(draft.ended_at_ms),
        "picks": picks.into_iter().map(pick_section).collect::<Vec<_>>(),
    })
}

/// A snapshot: 0.x only ever wrote `registered` ones, and v1's `imported`
/// rows are 0.x's registered decks.
fn snapshot_section(snapshot: &SnapshotInput) -> Value {
    json!({
        "source": "registered",
        "cards": snapshot.cards,
        "signature": snapshot.signature,
        "captured_at": bundle_datetime(snapshot.captured_at_ms),
        "match_token": snapshot.match_token,
    })
}

/// The statuses a player can set (`leagues.user_state`).
pub const USER_STATES: [&str; 4] = ["active", "complete", "partial", "dropped"];

/// The player's fields (league self-management spec 6.1), each only when
/// set. `removed_at` and `edited_at` are integer ms; `user_dropped_at` is a
/// bundle datetime, like `dropped_at`.
fn add_player_fields(league: &mut Value, input: &LeagueBundleInput) {
    let Some(fields) = league.as_object_mut() else {
        return;
    };
    if let Some(at) = input.removed_at_ms {
        fields.insert("removed_at".to_string(), json!(at));
    }
    if let Some(state) = &input.user_state {
        fields.insert("user_state".to_string(), Value::String(state.clone()));
    }
    if let Some(at) = input.user_dropped_at_ms {
        fields.insert(
            "user_dropped_at".to_string(),
            Value::String(bundle_datetime(at)),
        );
    }
    if let Some(at) = input.edited_at_ms {
        fields.insert("edited_at".to_string(), json!(at));
    }
}

/// A pulled league's player fields (spec 6.3), read loosely.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PulledLeagueFields {
    pub removed_at_ms: Option<i64>,
    pub user_state: Option<String>,
    pub user_dropped_at_ms: Option<i64>,
    pub edited_at_ms: Option<i64>,
}

/// The player fields of a pulled league bundle's `league` section. A
/// status v1 does not know, or a drop time that does not read, is not
/// recorded.
pub fn league_player_fields(bundle: &Value) -> PulledLeagueFields {
    let league = &bundle["league"];
    PulledLeagueFields {
        removed_at_ms: int(&league["removed_at"]),
        user_state: league["user_state"]
            .as_str()
            .filter(|state| USER_STATES.contains(state))
            .map(str::to_string),
        user_dropped_at_ms: league["user_dropped_at"]
            .as_str()
            .and_then(parse_bundle_datetime),
        edited_at_ms: int(&league["edited_at"]),
    }
}

/// The league bundle: `league`, `drafts` (at most one) and `snapshots`
/// sorted by `(captured_at, signature)`; `deck_change_detected` comes from
/// the carry, else `false`.
pub fn league_bundle(input: &LeagueBundleInput, carry: Option<&Carry>) -> Value {
    let mut snapshots: Vec<&SnapshotInput> = input.snapshots.iter().collect();
    snapshots.sort_by(|a, b| {
        (bundle_datetime(a.captured_at_ms), a.signature.as_bytes())
            .cmp(&(bundle_datetime(b.captured_at_ms), b.signature.as_bytes()))
    });
    let mut bundle = json!({
        "league": {
            "token": input.token,
            "name": input.name,
            "format": input.format,
            "started_at": bundle_datetime(input.started_at_ms),
            "joined_at": datetime(input.joined_at_ms),
            "dropped_at": datetime(input.dropped_at_ms),
            "completed_at": datetime(input.completed_at_ms),
            "state": input.state,
            "notes": input.notes,
            "manual": input.manual,
            "deck_change_detected": false,
            "kind": input.kind,
            "set_code": input.set_code,
            "mtgo_course_id": input.mtgo_course_id,
            "deck_mtgo_id": input.deck_mtgo_id,
            "deck_version_signature": input.deck_version_signature,
        },
        "drafts": input.drafts.iter().map(draft_section).collect::<Vec<_>>(),
        "snapshots": snapshots.into_iter().map(snapshot_section).collect::<Vec<_>>(),
    });
    add_player_fields(&mut bundle["league"], input);
    add_platform(&mut bundle, input.platform);
    if let Some(carry) = carry {
        carry.apply(&mut bundle);
    }
    bundle
}

/// The sidecar (0.x `LeagueBundleBuilder::sidecar`), nulls dropped. An
/// Arena course run's is `arena_run_sidecar`.
pub fn league_sidecar(bundle: &Value) -> Value {
    if is_arena_run_bundle(bundle) {
        return arena_run_sidecar(bundle);
    }
    let league = &bundle["league"];
    let deck_client_id = league["deck_mtgo_id"]
        .as_str()
        .map_or(Value::Null, |id| Value::String(deck_client_id(id)));
    let fields = [
        ("occurred_at", league["started_at"].clone()),
        ("name", league["name"].clone()),
        ("format", league["format"].clone()),
        ("state", league["state"].clone()),
        ("deck_client_id", deck_client_id),
    ];
    let mut sidecar = Map::new();
    for (key, value) in fields {
        if !value.is_null() {
            sidecar.insert(key.to_string(), value);
        }
    }
    let mut sidecar = Value::Object(sidecar);
    copy_platform(bundle, &mut sidecar);
    sidecar
}

// ---- Reading a pulled bundle -------------------------------------------------

fn missing(part: &str) -> BundleError {
    BundleError::Missing(part.to_string())
}

fn invalid(part: &str) -> BundleError {
    BundleError::Invalid(part.to_string())
}

/// An optional datetime: null is none; anything else must read.
fn time(value: &Value, part: &str) -> Result<Option<i64>, BundleError> {
    match value {
        Value::Null => Ok(None),
        Value::String(text) => parse_bundle_datetime(text)
            .map(Some)
            .ok_or_else(|| invalid(part)),
        _ => Err(invalid(part)),
    }
}

fn read_pick(pick: &Value) -> Result<PickInput, BundleError> {
    let mut reservations = Vec::new();
    if let Some(listed) = pick["reservations"].as_array() {
        for entry in listed {
            let catalog_id = int(&entry["catalog_id"]).and_then(|id| u32::try_from(id).ok());
            let at = entry["at"]
                .as_str()
                .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
                .map(|at| at.with_timezone(&Utc).timestamp_millis());
            if let (Some(catalog_id), Some(at)) = (catalog_id, at) {
                reservations.push((catalog_id, at));
            }
        }
    }
    let cards_available = match &pick["cards_available"] {
        Value::Array(cards) => Value::Array(cards.clone()),
        _ => Value::Array(Vec::new()),
    };
    Ok(PickInput {
        ordinal: int(&pick["ordinal"]).ok_or_else(|| missing("picks.ordinal"))?,
        pack_number: int(&pick["pack_number"]).unwrap_or(0),
        pick_number: int(&pick["pick_number"]).unwrap_or(0),
        pack_id: int(&pick["pack_id"]),
        direction: int(&pick["direction"]),
        cards_available,
        picked_catalog_id: int(&pick["picked_catalog_id"]),
        picked_card_id: int(&pick["picked_card_id"]),
        reservations,
        shown_at_ms: time(&pick["shown_at"], "picks.shown_at")?,
        deadline_at_ms: time(&pick["deadline_at"], "picks.deadline_at")?,
        picked_at_ms: time(&pick["picked_at"], "picks.picked_at")?,
        note: pick["note"].as_str().map(str::to_string),
    })
}

fn read_draft(draft: &Value) -> Result<DraftInput, BundleError> {
    let mut picks = Vec::new();
    for pick in draft["picks"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        picks.push(read_pick(pick)?);
    }
    Ok(DraftInput {
        draft_token: text(&draft["draft_token"]).ok_or_else(|| missing("drafts.draft_token"))?,
        mtgo_draft_id: int(&draft["mtgo_draft_id"]),
        pod_token: text(&draft["pod_token"]),
        seat_count: int(&draft["seat_count"]).unwrap_or(8),
        seat_index: int(&draft["seat_index"]),
        booster_catalog_id: int(&draft["booster_catalog_id"]),
        state: text(&draft["state"]).ok_or_else(|| missing("drafts.state"))?,
        pack_size: int(&draft["pack_size"]).unwrap_or(14),
        picks_expected: int(&draft["picks_expected"]).unwrap_or(42),
        started_at_ms: time(&draft["started_at"], "drafts.started_at")?,
        ended_at_ms: time(&draft["ended_at"], "drafts.ended_at")?,
        picks,
    })
}

/// A pulled league bundle as v1's columns and the carry of the rest. A
/// bundle written before 0.x's 2026-09-03 amendment has no `drafts` or
/// `snapshots`: none.
pub fn read_league_bundle(bundle: &Value) -> Result<(LeagueBundleInput, Carry), BundleError> {
    let league = bundle
        .get("league")
        .filter(|league| league.is_object())
        .ok_or_else(|| missing("league"))?;
    let started_at_ms = time(&league["started_at"], "league.started_at")?
        .ok_or_else(|| missing("league.started_at"))?;
    let mut drafts = Vec::new();
    for draft in bundle["drafts"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        drafts.push(read_draft(draft)?);
    }
    let mut snapshots = Vec::new();
    for snapshot in bundle["snapshots"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        snapshots.push(SnapshotInput {
            cards: snapshot["cards"].clone(),
            signature: text(&snapshot["signature"]).unwrap_or_default(),
            captured_at_ms: time(&snapshot["captured_at"], "snapshots.captured_at")?
                .ok_or_else(|| missing("snapshots.captured_at"))?,
            match_token: text(&snapshot["match_token"]),
        });
    }
    let player = league_player_fields(bundle);
    let input = LeagueBundleInput {
        platform: BundlePlatform::of_bundle(bundle)?,
        token: text(&league["token"]).ok_or_else(|| missing("league.token"))?,
        name: text(&league["name"]).unwrap_or_default(),
        format: text(&league["format"]),
        started_at_ms,
        joined_at_ms: time(&league["joined_at"], "league.joined_at")?,
        dropped_at_ms: time(&league["dropped_at"], "league.dropped_at")?,
        completed_at_ms: time(&league["completed_at"], "league.completed_at")?,
        state: text(&league["state"]).unwrap_or_else(|| "complete".to_string()),
        notes: text(&league["notes"]),
        manual: league["manual"].as_bool().unwrap_or(false),
        kind: text(&league["kind"]).unwrap_or_else(|| "constructed".to_string()),
        set_code: text(&league["set_code"]),
        mtgo_course_id: int(&league["mtgo_course_id"]),
        deck_mtgo_id: text(&league["deck_mtgo_id"]),
        deck_version_signature: text(&league["deck_version_signature"]),
        drafts,
        snapshots,
        removed_at_ms: player.removed_at_ms,
        user_state: player.user_state,
        user_dropped_at_ms: player.user_dropped_at_ms,
        edited_at_ms: player.edited_at_ms,
    };
    Ok((input, Carry::from_bundle(bundle, LEAGUE_CARRY)))
}

// ---- Arena course runs ---------------------------------------------------

/// The deck key prefix of an Arena course's limited deck: its `mtgo_id` is
/// `limited:arena_<course id>` (lead ruling R5).
pub const ARENA_COURSE_DECK_PREFIX: &str = "limited:arena_";

/// The newest snapshot of a course a device knows (cross-device sync spec
/// section 8): Arena's record of it and when it was observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSnapshot {
    pub wins: u32,
    pub losses: u32,
    /// `active` or `complete`.
    pub state: String,
    pub observed_at_ms: i64,
}

/// An Arena course run's bundle (a league bundle with `platform: "arena"`):
/// a pure function of state every device holding the course converges on,
/// so two devices build the same bytes. No start time, member list or deck
/// snapshots: those differ by device (matches carry their run's client id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArenaRunBundle {
    pub course_id: String,
    pub event_name: String,
    /// The run's name and format code, both from the event name.
    pub name: String,
    pub format: String,
    /// `draft`, `sealed` or `constructed`.
    pub kind: String,
    pub snapshot: RunSnapshot,
    pub notes: Option<String>,
    /// When the notes were last edited (`leagues.notes_edited_at`): the newer
    /// notes win on a pull.
    pub notes_edited_at_ms: Option<i64>,
    /// The player's fields (league self-management spec 6.1).
    pub player: PulledLeagueFields,
}

/// The kinds a run bundle may name.
const RUN_KINDS: [&str; 3] = ["constructed", "draft", "sealed"];

/// The states a run's snapshot may state.
const SNAPSHOT_STATES: [&str; 2] = ["active", "complete"];

/// The run's bundle: `league` holds the course, the snapshot's state (where
/// every league bundle has its state) and the rest of the snapshot, the
/// notes and the player's fields, each only when set.
pub fn arena_run_bundle(input: &ArenaRunBundle, carry: Option<&Carry>) -> Value {
    let mut league = json!({
        "course_id": input.course_id,
        "event_name": input.event_name,
        "name": input.name,
        "format": input.format,
        "kind": input.kind,
        "state": input.snapshot.state,
        "snapshot": {
            "wins": input.snapshot.wins,
            "losses": input.snapshot.losses,
            "observed_at": input.snapshot.observed_at_ms,
        },
        "notes": input.notes,
    });
    if let Some(fields) = league.as_object_mut() {
        if let Some(at) = input.notes_edited_at_ms {
            fields.insert("notes_edited_at".to_string(), json!(at));
        }
        if let Some(at) = input.player.removed_at_ms {
            fields.insert("removed_at".to_string(), json!(at));
        }
        if let Some(state) = &input.player.user_state {
            fields.insert("user_state".to_string(), Value::String(state.clone()));
        }
        if let Some(at) = input.player.user_dropped_at_ms {
            fields.insert(
                "user_dropped_at".to_string(),
                Value::String(bundle_datetime(at)),
            );
        }
        if let Some(at) = input.player.edited_at_ms {
            fields.insert("edited_at".to_string(), json!(at));
        }
    }
    let mut bundle = json!({ "league": league });
    add_platform(&mut bundle, BundlePlatform::Arena);
    if let Some(carry) = carry {
        carry.apply(&mut bundle);
    }
    bundle
}

/// When an Arena run bundle changed, for its upload's `updated_at`
/// (cross-device sync spec section 8): the newest of its own times (the
/// snapshot's observation, the notes edit and the player's fields), plus a
/// second per match in its record and one more when it is complete. Never
/// the clock: every device merges the same facts, and the server keeps the
/// newer `updated_at`, so a stale copy built late must not look newer. The
/// seconds (the server's times are whole seconds) make a merge newer than
/// a copy it took a part from: a run completes at its last listing's time
/// (`join_records`), and a newer record can come with older notes than
/// the copy it is merged with. `None` for any other bundle.
pub fn arena_run_changed_at(bundle: &Value) -> Option<i64> {
    if !is_arena_run_bundle(bundle) {
        return None;
    }
    let league = &bundle["league"];
    let times = [
        int(&league["snapshot"]["observed_at"]),
        int(&league["notes_edited_at"]),
        int(&league["edited_at"]),
        int(&league["removed_at"]),
        league["user_dropped_at"]
            .as_str()
            .and_then(parse_bundle_datetime),
    ];
    let newest = times.into_iter().flatten().max()?;
    let played = int(&league["snapshot"]["wins"]).unwrap_or(0)
        + int(&league["snapshot"]["losses"]).unwrap_or(0);
    let complete = i64::from(league["state"].as_str() == Some("complete"));
    Some(newest + 1_000 * (played.max(0) + complete))
}

/// Whether a league bundle is an Arena course run's.
pub fn is_arena_run_bundle(bundle: &Value) -> bool {
    BundlePlatform::of_bundle(bundle) == Ok(BundlePlatform::Arena)
}

/// An Arena run's sidecar: occurred at its snapshot, its name, format and
/// state, and the course deck whose slot gates it.
fn arena_run_sidecar(bundle: &Value) -> Value {
    let league = &bundle["league"];
    let occurred_at = int(&league["snapshot"]["observed_at"])
        .map_or(Value::Null, |ms| Value::String(bundle_datetime(ms)));
    let deck_client_id = league["course_id"].as_str().map_or(Value::Null, |course| {
        Value::String(deck_client_id(&format!(
            "{ARENA_COURSE_DECK_PREFIX}{course}"
        )))
    });
    let fields = [
        ("occurred_at", occurred_at),
        ("name", league["name"].clone()),
        ("format", league["format"].clone()),
        ("state", league["state"].clone()),
        ("deck_client_id", deck_client_id),
    ];
    let mut sidecar = Map::new();
    for (key, value) in fields {
        if !value.is_null() {
            sidecar.insert(key.to_string(), value);
        }
    }
    let mut sidecar = Value::Object(sidecar);
    copy_platform(bundle, &mut sidecar);
    sidecar
}

/// A count in a run snapshot: a whole number from 0.
fn count(value: &Value, part: &str) -> Result<u32, BundleError> {
    int(value)
        .and_then(|count| u32::try_from(count).ok())
        .ok_or_else(|| invalid(part))
}

/// A pulled Arena run bundle and the carry of the rest. A bundle that is not
/// an Arena run's, or whose course, kind or snapshot does not read, is
/// refused: a wrong snapshot would win over a right one by its time.
pub fn read_arena_run_bundle(bundle: &Value) -> Result<(ArenaRunBundle, Carry), BundleError> {
    if !is_arena_run_bundle(bundle) {
        return Err(invalid("platform"));
    }
    let league = bundle
        .get("league")
        .filter(|league| league.is_object())
        .ok_or_else(|| missing("league"))?;
    let course_id = text(&league["course_id"])
        .filter(|id| !id.is_empty())
        .ok_or_else(|| missing("league.course_id"))?;
    let event_name = text(&league["event_name"])
        .filter(|name| !name.is_empty())
        .ok_or_else(|| missing("league.event_name"))?;
    let kind = text(&league["kind"])
        .filter(|kind| RUN_KINDS.contains(&kind.as_str()))
        .ok_or_else(|| invalid("league.kind"))?;
    let state = text(&league["state"])
        .filter(|state| SNAPSHOT_STATES.contains(&state.as_str()))
        .ok_or_else(|| invalid("league.state"))?;
    let snapshot = &league["snapshot"];
    let input = ArenaRunBundle {
        course_id,
        event_name,
        name: text(&league["name"]).unwrap_or_default(),
        format: text(&league["format"]).unwrap_or_default(),
        kind,
        snapshot: RunSnapshot {
            wins: count(&snapshot["wins"], "league.snapshot.wins")?,
            losses: count(&snapshot["losses"], "league.snapshot.losses")?,
            state,
            observed_at_ms: int(&snapshot["observed_at"])
                .ok_or_else(|| invalid("league.snapshot.observed_at"))?,
        },
        notes: text(&league["notes"]),
        notes_edited_at_ms: int(&league["notes_edited_at"]),
        player: league_player_fields(bundle),
    };
    Ok((input, Carry::from_bundle(bundle, LEAGUE_CARRY)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::{bundle_hash, canonical_json};
    use crate::carry::{Carry, LEAGUE_CARRY};
    use crate::client_id::league_client_id;
    use serde_json::{Value, json};

    /// A league's client id: `{token}_{started_at}` (0.x `LeagueClientId`).
    fn league_bundle_client_id(input: &LeagueBundleInput) -> String {
        league_client_id(&input.token, input.started_at_ms)
    }

    // League self-management spec 6.1.
    #[test]
    fn a_league_without_player_fields_writes_none_of_their_keys() {
        let bundle = league_bundle(&sample(), None);
        for key in ["removed_at", "user_state", "user_dropped_at", "edited_at"] {
            assert!(bundle["league"].get(key).is_none(), "{key}");
        }
        assert_eq!(league_player_fields(&bundle), PulledLeagueFields::default());
    }

    #[test]
    fn writes_and_reads_back_the_league_player_fields() {
        let mut input = sample();
        input.removed_at_ms = Some(T + 1);
        input.user_state = Some("dropped".to_string());
        input.user_dropped_at_ms = Some(T + 5_000);
        input.edited_at_ms = Some(T + 2);
        let bundle = league_bundle(&input, None);
        assert_eq!(bundle["league"]["removed_at"], json!(T + 1));
        assert_eq!(bundle["league"]["user_state"], json!("dropped"));
        // A bundle datetime, like `dropped_at`.
        assert_eq!(
            bundle["league"]["user_dropped_at"],
            json!("2026-08-19T10:00:10Z")
        );
        assert_eq!(bundle["league"]["edited_at"], json!(T + 2));
        let (read, carry) = read_league_bundle(&bundle).expect("reads");
        assert_eq!(read, input);
        assert_eq!(
            canonical_json(&league_bundle(&read, Some(&carry))),
            canonical_json(&bundle)
        );
        assert_eq!(
            league_player_fields(&bundle),
            PulledLeagueFields {
                removed_at_ms: Some(T + 1),
                user_state: Some("dropped".to_string()),
                user_dropped_at_ms: Some(T + 5_000),
                edited_at_ms: Some(T + 2),
            }
        );
    }

    // A state v1 does not know and an unreadable drop time are not
    // recorded; they never refuse the bundle.
    #[test]
    fn reads_league_player_fields_loosely() {
        let mut bundle = league_bundle(&sample(), None);
        bundle["league"]["user_state"] = json!("paused");
        bundle["league"]["user_dropped_at"] = json!("yesterday");
        bundle["league"]["edited_at"] = json!("1787133605002");
        let fields = league_player_fields(&bundle);
        assert_eq!(fields.user_state, None);
        assert_eq!(fields.user_dropped_at_ms, None);
        assert_eq!(fields.edited_at_ms, Some(1_787_133_605_002));
        assert!(read_league_bundle(&bundle).is_ok());
    }

    const T: i64 = 1_787_133_605_000;

    fn pick(ordinal: i64, cards: Value, picked: i64) -> PickInput {
        PickInput {
            ordinal,
            pack_number: 1,
            pick_number: ordinal,
            pack_id: Some(77),
            direction: Some(1),
            cards_available: cards,
            picked_catalog_id: Some(picked),
            picked_card_id: None,
            reservations: vec![(picked as u32, T + 1_000)],
            shown_at_ms: Some(T),
            deadline_at_ms: Some(T + 40_000),
            picked_at_ms: Some(T + 2_000),
            note: None,
        }
    }

    fn sample() -> LeagueBundleInput {
        LeagueBundleInput {
            platform: BundlePlatform::Mtgo,
            token: "q-league-token".to_string(),
            name: "Qleague".to_string(),
            format: Some("CMODERN".to_string()),
            started_at_ms: T,
            joined_at_ms: Some(T + 1_000),
            dropped_at_ms: None,
            completed_at_ms: Some(T + 3_600_000),
            state: "complete".to_string(),
            notes: None,
            manual: false,
            kind: "constructed".to_string(),
            set_code: None,
            mtgo_course_id: None,
            deck_mtgo_id: Some("12345".to_string()),
            deck_version_signature: Some("q-signature".to_string()),
            drafts: Vec::new(),
            snapshots: Vec::new(),
            removed_at_ms: None,
            user_state: None,
            user_dropped_at_ms: None,
            edited_at_ms: None,
        }
    }

    fn keys(value: &Value) -> Vec<String> {
        let mut keys: Vec<String> = value.as_object().expect("a map").keys().cloned().collect();
        keys.sort();
        keys
    }

    // 0.x "gives two leagues sharing a token two distinct client ids".
    #[test]
    fn gives_two_leagues_sharing_a_token_two_distinct_client_ids() {
        let a = sample();
        let mut b = sample();
        b.started_at_ms = T + 2 * 86_400_000;
        assert_ne!(league_bundle_client_id(&a), league_bundle_client_id(&b));
        assert_eq!(
            league_bundle_client_id(&a),
            league_client_id("q-league-token", T)
        );
    }

    // 0.x "builds a league bundle with the limited fields and no local ids".
    #[test]
    fn builds_a_league_bundle_with_the_limited_fields_and_no_local_ids() {
        let bundle = league_bundle(&sample(), None);
        assert_eq!(keys(&bundle), ["drafts", "league", "snapshots"]);
        assert_eq!(
            keys(&bundle["league"]),
            [
                "completed_at",
                "deck_change_detected",
                "deck_mtgo_id",
                "deck_version_signature",
                "dropped_at",
                "format",
                "joined_at",
                "kind",
                "manual",
                "mtgo_course_id",
                "name",
                "notes",
                "set_code",
                "started_at",
                "state",
                "token",
            ]
        );
        assert_eq!(bundle["league"]["deck_change_detected"], json!(false));
        assert_eq!(bundle["league"]["manual"], json!(false));
    }

    // 0.x "carries the draft, its picks in ordinal order, and deck snapshots
    // in the league bundle".
    #[test]
    fn carries_the_draft_its_picks_in_ordinal_order_and_deck_snapshots() {
        let mut input = sample();
        input.kind = "draft".to_string();
        input.drafts = vec![DraftInput {
            draft_token: "q-draft".to_string(),
            mtgo_draft_id: Some(9),
            pod_token: Some("q-pod".to_string()),
            seat_count: 8,
            seat_index: Some(3),
            booster_catalog_id: Some(555),
            state: "finished".to_string(),
            pack_size: 14,
            picks_expected: 42,
            started_at_ms: Some(T),
            ended_at_ms: Some(T + 60_000),
            picks: vec![pick(2, json!([7, 8]), 8), pick(1, json!([5, 6]), 5)],
        }];
        input.snapshots = vec![SnapshotInput {
            cards: json!([{"catalog_id": 5, "quantity": 1, "sideboard": false}]),
            signature: "sig-1".to_string(),
            captured_at_ms: T,
            match_token: Some("q-match".to_string()),
        }];
        let bundle = league_bundle(&input, None);
        let picks = bundle["drafts"][0]["picks"].as_array().expect("picks");
        assert_eq!(bundle["drafts"].as_array().expect("drafts").len(), 1);
        assert_eq!(bundle["drafts"][0]["draft_token"], json!("q-draft"));
        assert_eq!(picks[0]["ordinal"], json!(1));
        assert_eq!(picks[1]["ordinal"], json!(2));
        assert_eq!(picks[0]["cards_available"], json!([5, 6]));
        assert_eq!(
            picks[0]["reservations"],
            json!([{"catalog_id": 5, "at": "2026-08-19T10:00:06+00:00"}])
        );
        assert_eq!(bundle["snapshots"][0]["signature"], json!("sig-1"));
        assert_eq!(bundle["snapshots"][0]["source"], json!("registered"));
        assert!(bundle["drafts"][0].get("id").is_none());
        assert!(bundle["snapshots"][0].get("match_id").is_none());
        assert_eq!(
            canonical_json(&bundle),
            canonical_json(&league_bundle(&input, None))
        );
        // What a pulled bundle reads back as builds the same bytes.
        let (read, carry) = read_league_bundle(&bundle).expect("reads");
        let mut expected = input.clone();
        expected.drafts[0].picks.sort_by_key(|pick| pick.ordinal);
        assert_eq!(read, expected);
        assert_eq!(
            canonical_json(&league_bundle(&read, Some(&carry))),
            canonical_json(&bundle)
        );
    }

    // 0.x "carries the deck client id on the league sidecar".
    #[test]
    fn mtgo_league_has_no_platform_key_and_an_endstep_one_carries_it() {
        let bundle = league_bundle(&sample(), None);
        assert!(bundle.get("platform").is_none());
        assert!(league_sidecar(&bundle).get("platform").is_none());
        let mut input = sample();
        input.platform = BundlePlatform::Endstep;
        let bundle = league_bundle(&input, None);
        assert_eq!(bundle["platform"], json!("endstep"));
        assert_eq!(league_sidecar(&bundle)["platform"], json!("endstep"));
    }

    #[test]
    fn carries_the_deck_client_id_on_the_league_sidecar() {
        let mut input = sample();
        input.deck_mtgo_id = Some("limited:abc".to_string());
        let sidecar = league_sidecar(&league_bundle(&input, None));
        assert_eq!(sidecar["deck_client_id"], json!("limited_abc"));
        assert_eq!(
            keys(&sidecar),
            ["deck_client_id", "format", "name", "occurred_at", "state"]
        );
    }

    #[test]
    fn deck_change_detected_comes_from_the_carry() {
        let carry = Carry::from_bundle(
            &json!({"league": {"deck_change_detected": true}}),
            LEAGUE_CARRY,
        );
        let bundle = league_bundle(&sample(), Some(&carry));
        assert_eq!(bundle["league"]["deck_change_detected"], json!(true));
    }

    // 0.x "imports a pre-amendment league bundle that carries no child keys".
    #[test]
    fn reads_a_pre_amendment_league_bundle_without_child_keys() {
        let mut bundle = league_bundle(&sample(), None);
        let map = bundle.as_object_mut().expect("map");
        map.remove("drafts");
        map.remove("snapshots");
        let (read, _) = read_league_bundle(&bundle).expect("reads");
        assert!(read.drafts.is_empty());
        assert!(read.snapshots.is_empty());
        assert_eq!(read.token, "q-league-token");
        assert!(read_league_bundle(&json!({"drafts": []})).is_err());
    }

    fn golden() -> Vec<Value> {
        let golden: Value = serde_json::from_str(include_str!(
            "../../../fixtures/parity/sync_league_bundles.golden.json"
        ))
        .expect("the league golden is JSON");
        golden["bundles"].as_array().expect("bundles").clone()
    }

    /// A golden's `input` (the v1 columns the golden's league is built from).
    fn input_from_json(input: &Value) -> Option<LeagueBundleInput> {
        let text = |value: &Value| value.as_str().map(str::to_string);
        let int = |value: &Value| value.as_i64();
        let mut drafts = Vec::new();
        for draft in input["drafts"].as_array()? {
            let mut picks = Vec::new();
            for pick in draft["picks"].as_array()? {
                let mut reservations = Vec::new();
                for row in pick["reservations"].as_array()? {
                    reservations.push((u32::try_from(row[0].as_u64()?).ok()?, row[1].as_i64()?));
                }
                picks.push(PickInput {
                    ordinal: int(&pick["ordinal"])?,
                    pack_number: int(&pick["pack_number"])?,
                    pick_number: int(&pick["pick_number"])?,
                    pack_id: int(&pick["pack_id"]),
                    direction: int(&pick["direction"]),
                    cards_available: pick["cards_available"].clone(),
                    picked_catalog_id: int(&pick["picked_catalog_id"]),
                    picked_card_id: int(&pick["picked_card_id"]),
                    reservations,
                    shown_at_ms: int(&pick["shown_at_ms"]),
                    deadline_at_ms: int(&pick["deadline_at_ms"]),
                    picked_at_ms: int(&pick["picked_at_ms"]),
                    note: text(&pick["note"]),
                });
            }
            drafts.push(DraftInput {
                draft_token: text(&draft["draft_token"])?,
                mtgo_draft_id: int(&draft["mtgo_draft_id"]),
                pod_token: text(&draft["pod_token"]),
                seat_count: int(&draft["seat_count"])?,
                seat_index: int(&draft["seat_index"]),
                booster_catalog_id: int(&draft["booster_catalog_id"]),
                state: text(&draft["state"])?,
                pack_size: int(&draft["pack_size"])?,
                picks_expected: int(&draft["picks_expected"])?,
                started_at_ms: int(&draft["started_at_ms"]),
                ended_at_ms: int(&draft["ended_at_ms"]),
                picks,
            });
        }
        let mut snapshots = Vec::new();
        for snapshot in input["snapshots"].as_array()? {
            snapshots.push(SnapshotInput {
                cards: snapshot["cards"].clone(),
                signature: text(&snapshot["signature"])?,
                captured_at_ms: int(&snapshot["captured_at_ms"])?,
                match_token: text(&snapshot["match_token"]),
            });
        }
        Some(LeagueBundleInput {
            platform: BundlePlatform::Mtgo,
            token: text(&input["token"])?,
            name: text(&input["name"])?,
            format: text(&input["format"]),
            started_at_ms: int(&input["started_at_ms"])?,
            joined_at_ms: int(&input["joined_at_ms"]),
            dropped_at_ms: int(&input["dropped_at_ms"]),
            completed_at_ms: int(&input["completed_at_ms"]),
            state: text(&input["state"])?,
            notes: text(&input["notes"]),
            manual: input["manual"].as_bool()?,
            kind: text(&input["kind"])?,
            set_code: text(&input["set_code"]),
            mtgo_course_id: int(&input["mtgo_course_id"]),
            deck_mtgo_id: text(&input["deck_mtgo_id"]),
            deck_version_signature: text(&input["deck_version_signature"]),
            drafts,
            snapshots,
            removed_at_ms: int(&input["removed_at_ms"]),
            user_state: text(&input["user_state"]),
            user_dropped_at_ms: int(&input["user_dropped_at_ms"]),
            edited_at_ms: int(&input["edited_at_ms"]),
        })
    }

    /// The golden's own `input` columns, not the bundle read back, build the
    /// golden bytes: a builder bug cannot hide behind a symmetric reader.
    #[test]
    fn every_golden_league_input_builds_the_golden_bundle() {
        for case in golden() {
            let input = input_from_json(&case["input"]).expect("golden input");
            let pulled: Value = serde_json::from_str(case["canonical"].as_str().expect("text"))
                .expect("canonical JSON");
            // The carry is the one part with no v1 column.
            let (_, carry) = read_league_bundle(&pulled).expect("reads");
            let bundle = league_bundle(&input, Some(&carry));
            let canonical = canonical_json(&bundle);
            assert_eq!(canonical, case["canonical"].as_str().expect("canonical"));
            assert_eq!(
                bundle_hash(&canonical),
                case["hash"].as_str().expect("hash")
            );
            assert_eq!(league_sidecar(&bundle), case["sidecar"]);
            assert_eq!(
                league_bundle_client_id(&input),
                case["client_id"].as_str().expect("id")
            );
            let (read, _) = read_league_bundle(&pulled).expect("reads");
            assert_eq!(read, input, "the pulled bundle reads into the same columns");
        }
    }

    #[test]
    fn every_golden_league_bundle_matches_0x_byte_for_byte() {
        let bundles = golden();
        assert!(!bundles.is_empty());
        for case in bundles {
            let pulled: Value = serde_json::from_str(case["canonical"].as_str().expect("text"))
                .expect("canonical JSON");
            // The pulled bundle reads into the v1 columns, which build the
            // same bytes.
            let (input, carry) = read_league_bundle(&pulled).expect("reads");
            let bundle = league_bundle(&input, Some(&carry));
            let canonical = canonical_json(&bundle);
            assert_eq!(canonical, case["canonical"].as_str().expect("canonical"));
            assert_eq!(
                bundle_hash(&canonical),
                case["hash"].as_str().expect("hash")
            );
            assert_eq!(league_sidecar(&bundle), case["sidecar"]);
            assert_eq!(
                league_bundle_client_id(&input),
                case["client_id"].as_str().expect("id")
            );
        }
    }

    /// Every field a v1 league bundle cannot write as 0.x does is justified
    /// in the parity allowlist (ruling 5).
    #[test]
    fn the_known_league_differences_are_allowlisted() {
        let allowlist: Value =
            serde_json::from_str(include_str!("../../../fixtures/parity/allowlist.json"))
                .expect("the allowlist is JSON");
        for field in [
            "snapshots[].source",
            "league.deck_change_detected",
            "drafts[].picks (detached)",
        ] {
            let listed =
                allowlist.as_array().expect("a list").iter().any(|entry| {
                    entry["fixture"] == "sync_league_bundles" && entry["field"] == field
                });
            assert!(listed, "{field}");
        }
    }

    // ---- Arena course runs (cross-device sync spec section 8) ------------

    fn arena_run() -> ArenaRunBundle {
        ArenaRunBundle {
            course_id: "aaaaaaaa-0000-4000-8000-00000000000a".to_string(),
            event_name: "PremierDraft_FRA_20260929".to_string(),
            name: "Premier Draft \u{b7} FRA".to_string(),
            format: "DFRAFRAFRA".to_string(),
            kind: "draft".to_string(),
            snapshot: RunSnapshot {
                wins: 5,
                losses: 1,
                state: "complete".to_string(),
                observed_at_ms: T,
            },
            notes: None,
            notes_edited_at_ms: None,
            player: PulledLeagueFields::default(),
        }
    }

    /// An Arena run's upload time is the bundle's own newest time, never the
    /// clock: a stale build never wins over a newer one by being built later.
    #[test]
    fn an_arena_run_changes_at_its_own_newest_time() {
        let mut input = arena_run();
        input.snapshot.state = "active".to_string();
        // A second per match of its 5-1 record.
        let games = 6_000;
        let at = |input: &ArenaRunBundle| arena_run_changed_at(&arena_run_bundle(input, None));
        assert_eq!(at(&input), Some(T + games));
        input.notes_edited_at_ms = Some(T + 60_000);
        assert_eq!(at(&input), Some(T + 60_000 + games));
        input.player.edited_at_ms = Some(T + 120_000);
        assert_eq!(at(&input), Some(T + 120_000 + games));
        input.player.user_dropped_at_ms = Some(T + 180_000);
        assert_eq!(at(&input), Some(T + 180_000 + games));
        input.player.removed_at_ms = Some(T + 240_000);
        assert_eq!(at(&input), Some(T + 240_000 + games));
        assert_eq!(
            arena_run_changed_at(&league_bundle(&sample(), None)),
            None,
            "an MTGO league keeps its own rule"
        );
    }

    /// A run completes at its last listing's time (`join_records`), so the
    /// complete copy is a second newer than the active one: it wins the
    /// server's last write wins, whose times are whole seconds.
    #[test]
    fn a_complete_arena_run_is_newer_than_the_active_one_it_closes() {
        let mut active = arena_run();
        active.snapshot.state = "active".to_string();
        let complete = arena_run();
        let at = |input: &ArenaRunBundle| {
            arena_run_changed_at(&arena_run_bundle(input, None)).expect("an Arena run")
        };
        assert_eq!(at(&complete), at(&active) + 1_000);
        assert!(bundle_datetime(at(&complete)) > bundle_datetime(at(&active)));
    }

    /// One device holds the newer record, the other newer notes: the merge
    /// of both is newer than either copy, so it replaces the server's.
    #[test]
    fn a_merged_arena_run_is_newer_than_each_copy_it_took_from() {
        let at = |input: &ArenaRunBundle| {
            arena_run_changed_at(&arena_run_bundle(input, None)).expect("an Arena run")
        };
        let mut newer_record = arena_run();
        newer_record.snapshot = RunSnapshot {
            wins: 4,
            losses: 0,
            state: "active".to_string(),
            observed_at_ms: T + 10_000,
        };
        newer_record.notes_edited_at_ms = Some(T + 5_000);
        let mut newer_notes = newer_record.clone();
        newer_notes.snapshot.wins = 3;
        newer_notes.snapshot.observed_at_ms = T + 8_000;
        newer_notes.notes_edited_at_ms = Some(T + 12_000);
        let mut merged = newer_record.clone();
        merged.notes_edited_at_ms = Some(T + 12_000);
        assert!(at(&merged) > at(&newer_record));
        assert!(at(&merged) > at(&newer_notes));
    }

    #[test]
    fn an_arena_run_bundle_carries_the_course_and_its_newest_snapshot_only() {
        let bundle = arena_run_bundle(&arena_run(), None);
        assert_eq!(bundle["platform"], json!("arena"));
        let league = &bundle["league"];
        assert_eq!(
            league["course_id"],
            json!("aaaaaaaa-0000-4000-8000-00000000000a")
        );
        assert_eq!(league["event_name"], json!("PremierDraft_FRA_20260929"));
        assert_eq!(league["kind"], json!("draft"));
        assert_eq!(league["state"], json!("complete"));
        assert_eq!(
            league["snapshot"],
            json!({"wins": 5, "losses": 1, "observed_at": T})
        );
        // No start time, member list or deck snapshots: they differ by device.
        for key in ["started_at", "token", "deck_mtgo_id", "notes_edited_at"] {
            assert!(league.get(key).is_none(), "{key}");
        }
        assert!(bundle.get("snapshots").is_none());
        assert!(bundle.get("drafts").is_none());
        assert!(bundle.get("matches").is_none());
    }

    #[test]
    fn an_arena_run_bundle_reads_back_to_the_same_bytes() {
        let mut input = arena_run();
        input.notes = Some("Qnote".to_string());
        input.notes_edited_at_ms = Some(T + 3);
        input.player = PulledLeagueFields {
            removed_at_ms: None,
            user_state: Some("dropped".to_string()),
            user_dropped_at_ms: Some(T + 5_000),
            edited_at_ms: Some(T + 2),
        };
        let bundle = arena_run_bundle(&input, None);
        assert!(is_arena_run_bundle(&bundle));
        let (read, carry) = read_arena_run_bundle(&bundle).expect("reads");
        assert_eq!(read, input);
        assert_eq!(
            canonical_json(&arena_run_bundle(&read, Some(&carry))),
            canonical_json(&bundle)
        );
        // An MTGO league bundle is not an Arena run.
        assert!(!is_arena_run_bundle(&league_bundle(&sample(), None)));
    }

    #[test]
    fn an_arena_run_bundle_that_does_not_read_is_refused() {
        let good = arena_run_bundle(&arena_run(), None);
        for (pointer, value) in [
            ("/league/course_id", json!("")),
            ("/league/kind", json!("cube")),
            ("/league/state", json!("dropped")),
            ("/league/snapshot/wins", json!(-1)),
            ("/league/snapshot/observed_at", json!("soon")),
        ] {
            let mut bundle = good.clone();
            *bundle.pointer_mut(pointer).expect("pointer") = value;
            assert!(read_arena_run_bundle(&bundle).is_err(), "{pointer}");
        }
        let mut no_platform = good.clone();
        no_platform
            .as_object_mut()
            .expect("object")
            .remove("platform");
        assert!(read_arena_run_bundle(&no_platform).is_err());
    }

    #[test]
    fn an_arena_run_sidecar_names_its_course_deck() {
        let bundle = arena_run_bundle(&arena_run(), None);
        assert_eq!(
            league_sidecar(&bundle),
            json!({
                "occurred_at": "2026-08-19T10:00:05Z",
                "name": "Premier Draft \u{b7} FRA",
                "format": "DFRAFRAFRA",
                "state": "complete",
                "deck_client_id": "limited_arena_aaaaaaaa-0000-4000-8000-00000000000a",
                "platform": "arena",
            })
        );
    }
}
