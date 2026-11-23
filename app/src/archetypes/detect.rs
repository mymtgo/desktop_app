//! Detection for one match and one deck (archetypes spec sections 6.1 to
//! 6.3, 0.x `DetermineMatchArchetypes`, `DetermineDeckArchetype` and
//! `SyncDecks::prefillArchetype`).
//!
//! The decision itself is the pure `archetypes` crate's; this module reads
//! its inputs, asks the API when the decision says so (`Lookups`), writes
//! an unheld API answer's definition before its row, and hands the results
//! to the tracker (`Writer`), in slices of `ARCHETYPE_DETECTIONS_PER_EDIT`.
//! Like a Laravel action that takes its HTTP client and repository as
//! arguments, so tests pass fakes.

use std::collections::{BTreeMap, HashMap};

use archetypes::{
    Candidate, Decision, HOMEBREW_UUID, ObservedCard, complete_maindeck, decide,
    decide_after_empty_estimate, format_key, is_limited, local_answers, plan_slices,
};
use mymtgo_api::{EstimateCard, EstimateCardId, EstimateOutcome, format_slug};
use store::Connection;
use store::archetype_edit::{ArchetypeEdit, ArchetypeEditOutcome, MatchDetection, SeatResult};
use store::archetypes::{
    CommanderSeats, CommanderSet, DeckFilter, DetectionInput, deck_detection_matches,
    deck_prefill_input, detection_input, downloaded_versions, due_log_matches, due_matches,
    held_candidates, held_definitions, held_uuid, merged_into, undefined_uuids,
};
use store::endstep_cards;

use crate::api::reference::estimate_archetype;
use crate::api::session::ApiState;
use crate::archetypes::definitions::{
    CheckReply, DefinitionReply, Pacer, check_versions, fetch_definition,
};

/// Matches per `SetDetections` edit, so no writer transaction exceeds
/// 100 ms (spec section 4 "Writers").
pub const ARCHETYPE_DETECTIONS_PER_EDIT: usize = 10;

/// Definitions per `ApplyDefinitions` edit (spec section 17: the apply of
/// 500 definitions runs in the background, slices under 100 ms).
pub const DEFINITIONS_PER_EDIT: usize = 50;

/// After an estimate fails the backfill asks the API nothing for this long:
/// a failure most often means the API is down or unreachable.
pub const FAILED_ESTIMATE_PAUSE_MS: i64 = 60_000;

/// How long the backfill passes over a match whose estimate failed, so one
/// match the API always fails on cannot hold up the older ones.
pub const FAILED_MATCH_SKIP_MS: i64 = 30 * 60_000;

/// The matches whose estimate failed and when, kept by the worker across
/// jobs. In memory only: after a restart they are asked about at once.
/// Like a Laravel job's `backoff`, without a failed jobs table.
#[derive(Debug, Default)]
pub struct FailedEstimates {
    failed_at: HashMap<i64, i64>,
    last_failure_ms: Option<i64>,
}

impl FailedEstimates {
    fn record(&mut self, match_id: i64, now_ms: i64) {
        self.failed_at.insert(match_id, now_ms);
        self.last_failure_ms = Some(now_ms);
    }

    fn clear(&mut self, match_id: i64) {
        self.failed_at.remove(&match_id);
    }

    /// The backfill waits after a recent failure.
    fn paused(&self, now_ms: i64) -> bool {
        self.last_failure_ms
            .is_some_and(|at| now_ms - at < FAILED_ESTIMATE_PAUSE_MS)
    }

    /// The matches still passed over at `now_ms`; older failures are
    /// forgotten, so those matches are asked about again.
    fn skipped(&mut self, now_ms: i64) -> Vec<i64> {
        self.failed_at
            .retain(|_, at| now_ms - *at < FAILED_MATCH_SKIP_MS);
        let mut ids: Vec<i64> = self.failed_at.keys().copied().collect();
        ids.sort_unstable();
        ids
    }
}

/// How a definitions check went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckRun {
    /// Answered and applied.
    Applied,
    /// Offline mode is on: nothing was asked.
    Offline,
    /// The endpoint is not deployed (404): rows without a definition took
    /// the `/decklist` fallback; no check until the next start.
    NotDeployed,
    /// MyMTGO could not be reached (`ApiError::is_transient`, as at launch
    /// or wake): tried again soon (`TransientRetry`).
    Unreachable,
}

/// What detection asks the API.
pub trait Lookups {
    /// The API may be asked (offline mode is off).
    fn api_allowed(&self) -> bool;
    /// The estimate for these cards (MTGO id, Arena id or name, copies) in this raw
    /// match format: an archetype, the server's "no archetype", or failed
    /// (no answer, so the match is asked about again later).
    fn estimate(&mut self, format: &str, cards: &[EstimateCard]) -> EstimateOutcome;
    /// The definition of a uuid v1 does not hold (or holds without one):
    /// found, gone (retired or unknown to the server) or failed.
    fn definition(&mut self, uuid: &str) -> DefinitionReply;
    /// The definitions check for these `(uuid, version)` pairs.
    fn check(&mut self, versions: &[(String, Option<String>)]) -> CheckReply;
    /// A request's paced wait was cut short by a signal the running job
    /// stops for (#300 F1): its "no answer" is not one, so the match it
    /// was for is not written.
    fn interrupted(&self) -> bool {
        false
    }
}

/// Where detection writes: the tracker's writer in the app.
pub trait Writer {
    fn edit(&mut self, edit: ArchetypeEdit) -> Result<ArchetypeEditOutcome, String>;
    /// Whether an edit can be taken now (tracking runs). Detection asks
    /// before it asks the API, so paced answers are not thrown away by a
    /// write that cannot happen (#300 F2).
    fn ready(&self) -> bool {
        true
    }
}

/// The API through `ApiState`, every request paced by the worker's `Pacer`.
pub struct ApiLookups<'a> {
    pub api: &'a ApiState,
    pub pacer: &'a mut Pacer,
    /// A paced wait was cut short (see `Lookups::interrupted`).
    pub interrupted: bool,
}

impl<'a> ApiLookups<'a> {
    pub fn new(api: &'a ApiState, pacer: &'a mut Pacer) -> ApiLookups<'a> {
        ApiLookups {
            api,
            pacer,
            interrupted: false,
        }
    }
}

impl Lookups for ApiLookups<'_> {
    fn api_allowed(&self) -> bool {
        !self.api.offline_mode().0
    }

    fn estimate(&mut self, format: &str, cards: &[EstimateCard]) -> EstimateOutcome {
        // Offline the estimate returns before any request (0.x's guard).
        if !self.api_allowed() {
            return EstimateOutcome::Failed("offline mode is on".to_string());
        }
        if !self.pacer.before_request_or_yield() {
            self.interrupted = true;
            return EstimateOutcome::Failed("cut short".to_string());
        }
        let outcome = estimate_archetype(self.api, format, cards, self.pacer.now_ms());
        self.pacer.after_request();
        outcome
    }

    fn definition(&mut self, uuid: &str) -> DefinitionReply {
        // The pause before the download may be cut short too; past it, the
        // download's own pacing has nothing left to wait.
        if !self.pacer.before_request_or_yield() {
            self.interrupted = true;
            return DefinitionReply::Failed;
        }
        fetch_definition(self.api, self.pacer, uuid)
    }

    fn check(&mut self, versions: &[(String, Option<String>)]) -> CheckReply {
        check_versions(self.api, self.pacer, versions)
    }

    fn interrupted(&self) -> bool {
        self.interrupted
    }
}

/// The archetype a decision accepts, `None` for Homebrew or nothing.
fn accepted(decision: Decision) -> Option<(i64, f64)> {
    match decision {
        Decision::Accept {
            archetype_id,
            confidence,
        }
        | Decision::AcceptOffline {
            archetype_id,
            confidence,
        } => Some((archetype_id, confidence)),
        Decision::AskApi | Decision::Homebrew | Decision::Nothing => None,
    }
}

/// One seat's detection.
enum Seat {
    /// Decided: the row to write, or none.
    Decided(Option<SeatResult>),
    /// The API was asked and gave no answer (the request failed or was cut
    /// short): the match is not written, so it stays due.
    Unanswered,
}

/// What the API's estimate came to for one seat.
enum ApiAnswer {
    /// A held archetype and the API's confidence.
    Held(i64, f64),
    /// No archetype: the server named none, refused the cards, or named
    /// one that is retired or gone.
    NoArchetype,
    /// No answer (see `Seat::Unanswered`).
    Unanswered,
}

/// The held archetype for an API answer.
enum Held {
    Archetype(i64),
    /// Retired, or gone from the server: no archetype.
    Unavailable,
    /// Not held and its definition could not be had now (see
    /// `Seat::Unanswered`).
    Unanswered,
}

/// One match's detection.
enum Detected {
    /// The result to write.
    Ready(MatchDetection),
    /// The match is not there or not ready (a log match before its game
    /// sync).
    NotReady,
    /// A seat's estimate gave no answer (see `Seat::Unanswered`).
    Unanswered,
}

/// Why `detect_until_yield` stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stopped {
    /// Every match was asked about.
    Done,
    /// A signal waits, or a request was cut short by one.
    Yielded,
    /// An estimate failed: the API is likely down, so the rest wait for
    /// the next wake instead of each failing in turn.
    Unanswered,
}

/// A store failure as the worker reports it.
fn failed(error: store::StoreError) -> String {
    error.to_string()
}

/// The estimate's cards for a seat. An Arena catalog id goes by its Arena
/// id, an Endstep one by its `endstep_cards` name (copies of one name
/// summed, so two printings are one entry), any other by its MTGO id. A
/// zero id of either kind, an Endstep id without a name and a card without
/// copies are left out. The named cards come after the others.
fn estimate_cards(
    reader: &Connection,
    cards: &[ObservedCard],
) -> Result<Vec<EstimateCard>, String> {
    let endstep_ids: Vec<u32> = cards
        .iter()
        .filter(|card| card.quantity > 0 && cards::endstep_row_of(card.catalog_id).is_some())
        .filter_map(|card| u32::try_from(card.catalog_id).ok())
        .collect();
    let names = if endstep_ids.is_empty() {
        BTreeMap::new()
    } else {
        endstep_cards::names_of(reader, &endstep_ids).map_err(failed)?
    };
    let mut by_id = Vec::new();
    let mut by_name: Vec<EstimateCard> = Vec::new();
    for card in cards {
        if card.quantity == 0 {
            continue;
        }
        if cards::endstep_row_of(card.catalog_id).is_some() {
            let name = u32::try_from(card.catalog_id)
                .ok()
                .and_then(|id| names.get(&id));
            if let Some(name) = name {
                add_by_name(&mut by_name, name, card.quantity);
            }
            continue;
        }
        if let Some(id) = server_id(card.catalog_id) {
            by_id.push(EstimateCard {
                id,
                quantity: card.quantity,
            });
        }
    }
    by_id.extend(by_name);
    Ok(by_id)
}

/// The id the server knows a non-Endstep catalog id by: its Arena id or
/// its MTGO id, none for a zero id.
fn server_id(catalog_id: i64) -> Option<EstimateCardId> {
    match cards::grp_id_of(catalog_id) {
        Some(grp) if grp.0 > 0 => Some(EstimateCardId::Arena(grp.0)),
        Some(_) => None,
        None => match u32::try_from(catalog_id).ok()? {
            0 => None,
            mtgo_id => Some(EstimateCardId::Mtgo(mtgo_id)),
        },
    }
}

/// Adds copies of `name`, to its entry when it has one.
fn add_by_name(by_name: &mut Vec<EstimateCard>, name: &str, quantity: u32) {
    for card in by_name.iter_mut() {
        if card.id == EstimateCardId::Name(name.to_string()) {
            card.quantity += quantity;
            return;
        }
    }
    by_name.push(EstimateCard {
        id: EstimateCardId::Name(name.to_string()),
        quantity,
    });
}

/// One detection run's inputs.
pub struct Detector<'a> {
    pub reader: &'a Connection,
    pub lookups: &'a mut dyn Lookups,
    pub writer: &'a mut dyn Writer,
    pub now_ms: &'a dyn Fn() -> i64,
    /// The matches whose estimate failed (the worker's, kept across jobs).
    pub failed: &'a mut FailedEstimates,
    /// A 0.x import is running: the backfill takes log matches only, so it
    /// never decides an imported match before the import's archetype rows
    /// land (archetypes spec section 15). "Detect" by hand still runs.
    pub logs_only: bool,
}

impl Detector<'_> {
    /// One match's detection (spec section 6.2), `None` when the match is
    /// not there or not ready (a log match before its game sync). A
    /// limited match is answered with no seats (marked detected without
    /// rows).
    pub fn detect_match(&mut self, match_id: i64) -> Result<Option<MatchDetection>, String> {
        match self.detect_one(match_id)? {
            Detected::Ready(result) => Ok(Some(result)),
            Detected::NotReady | Detected::Unanswered => Ok(None),
        }
    }

    /// `detect_match`, telling a match not ready from one whose estimate
    /// gave no answer.
    fn detect_one(&mut self, match_id: i64) -> Result<Detected, String> {
        let Some(input) = detection_input(self.reader, match_id).map_err(failed)? else {
            return Ok(Detected::NotReady);
        };
        let mut result = MatchDetection {
            match_id,
            deck_archetype_id: input.deck_archetype_id,
            deck_archetype: None,
            player: None,
            opponent: None,
        };
        if input.format.as_deref().is_some_and(is_limited) {
            return Ok(Detected::Ready(result));
        }
        if let Some(seats) = &input.commanders {
            return self.detect_commanders(&input, seats, result);
        }
        let format = input.format.clone();
        let candidates = match format.as_deref().and_then(format_key) {
            Some(key) => held_candidates(self.reader, &key).map_err(failed)?,
            None => Vec::new(),
        };
        if !input.player_manual {
            result.player = match input.deck_archetype_id {
                // 0.x: the deck's archetype, no estimate.
                Some(deck_archetype) => Some(SeatResult {
                    archetype_id: self.hop(deck_archetype)?,
                    confidence: 1.0,
                }),
                None => match self.decide_seat(
                    format.as_deref(),
                    &candidates,
                    &input.player_cards,
                    false,
                )? {
                    Seat::Decided(seat) => seat,
                    Seat::Unanswered => return Ok(Detected::Unanswered),
                },
            };
        }
        if !input.opponent_manual {
            result.opponent = match self.decide_seat(
                format.as_deref(),
                &candidates,
                &input.opponent_cards,
                true,
            )? {
                Seat::Decided(seat) => seat,
                Seat::Unanswered => return Ok(Detected::Unanswered),
            };
        }
        Ok(Detected::Ready(result))
    }

    /// A commander format match (Arena Brawl spec section 6): each seat is
    /// its commanders' archetype at confidence 1.0, no estimate and no
    /// scoring; the player's also becomes the deck's. A seat without
    /// commanders keeps no archetype (the opponent gets no Homebrew).
    fn detect_commanders(
        &mut self,
        input: &DetectionInput,
        seats: &CommanderSeats,
        mut result: MatchDetection,
    ) -> Result<Detected, String> {
        let format = input
            .format
            .as_deref()
            .and_then(format_key)
            .unwrap_or_default();
        let player = match &seats.player {
            Some(set) => Some(self.ensure_commander(&format, set)?),
            None => None,
        };
        // A hand-set or "none" opponent seat is left alone: no row for it.
        let opponent = match &seats.opponent {
            Some(set) if !input.opponent_manual => Some(self.ensure_commander(&format, set)?),
            _ => None,
        };
        let seat = |archetype_id: i64| SeatResult {
            archetype_id,
            confidence: 1.0,
        };
        if !input.player_manual {
            result.player = player.map(seat);
        }
        if !input.opponent_manual {
            result.opponent = opponent.map(seat);
        }
        result.deck_archetype = player;
        Ok(Detected::Ready(result))
    }

    /// The archetype row for a commander set, created when absent.
    fn ensure_commander(&mut self, format: &str, set: &CommanderSet) -> Result<i64, String> {
        let outcome = self.writer.edit(ArchetypeEdit::EnsureCommander {
            uuid: archetypes::commander::commander_uuid(format, &set.oracle_ids),
            name: archetypes::commander::commander_name(&set.names),
            format: format.to_string(),
            color_identity: archetypes::commander::commander_colors(&set.color_identities),
            now_ms: (self.now_ms)(),
        })?;
        match outcome {
            ArchetypeEditOutcome::Commander { archetype_id } => Ok(archetype_id),
            other => Err(format!("unexpected commander edit outcome: {other:?}")),
        }
    }

    /// One seat (spec section 6.1): no cards give no row; the opponent gets
    /// Homebrew when nothing answers, the player nothing. The answer
    /// follows one merge hop. When the API was asked and gave no answer
    /// the seat is `Unanswered`, never the offline guess: v1 holds only
    /// the archetypes it has met, so a weak local score written then would
    /// stay a wrong label for good.
    fn decide_seat(
        &mut self,
        format: Option<&str>,
        candidates: &[Candidate],
        cards: &[ObservedCard],
        opponent: bool,
    ) -> Result<Seat, String> {
        if cards.is_empty() {
            return Ok(Seat::Decided(None));
        }
        let api_allowed = self.lookups.api_allowed() && format.is_some();
        let local = local_answers(cards, candidates, opponent);
        let answer = match decide(local.clone(), api_allowed) {
            Decision::AskApi => match self.api_answer(format.unwrap_or_default(), cards)? {
                ApiAnswer::Held(archetype_id, confidence) => Some((archetype_id, confidence)),
                // Only a confident local answer stands (#489).
                ApiAnswer::NoArchetype => accepted(decide_after_empty_estimate(local)),
                // Failed or cut short (#300 F1): asked again later.
                ApiAnswer::Unanswered => return Ok(Seat::Unanswered),
            },
            decision => accepted(decision),
        };
        let answer = match answer {
            Some(answer) => Some(answer),
            None if opponent => self.homebrew()?.map(|id| (id, 0.0)),
            None => None,
        };
        match answer {
            Some((archetype_id, confidence)) => Ok(Seat::Decided(Some(SeatResult {
                archetype_id: self.hop(archetype_id)?,
                confidence: confidence.clamp(0.0, 1.0),
            }))),
            None => Ok(Seat::Decided(None)),
        }
    }

    /// The API's estimate as a held archetype (spec 6.1 step 4): an unheld
    /// uuid is downloaded first; a retired or gone one is no archetype, one
    /// whose download failed no answer.
    fn api_answer(&mut self, format: &str, cards: &[ObservedCard]) -> Result<ApiAnswer, String> {
        let pairs = estimate_cards(self.reader, cards)?;
        // No card the server could know: nothing to ask, and a request
        // without cards would only be refused.
        if pairs.is_empty() {
            return Ok(ApiAnswer::NoArchetype);
        }
        let estimate = match self.lookups.estimate(format, &pairs) {
            EstimateOutcome::Answered(estimate) => estimate,
            EstimateOutcome::NoArchetype => return Ok(ApiAnswer::NoArchetype),
            EstimateOutcome::Refused(reason) => {
                tracing::warn!(%reason, "the archetype estimate was refused");
                return Ok(ApiAnswer::NoArchetype);
            }
            EstimateOutcome::Failed(reason) => {
                if !self.lookups.interrupted() {
                    tracing::warn!(%reason, "the archetype estimate failed");
                }
                return Ok(ApiAnswer::Unanswered);
            }
        };
        match self.ensure_held(&estimate.uuid, format)? {
            Held::Archetype(id) => Ok(ApiAnswer::Held(id, estimate.confidence)),
            Held::Unavailable => Ok(ApiAnswer::NoArchetype),
            // Failed or cut short (#300 F1): asked again later.
            Held::Unanswered => Ok(ApiAnswer::Unanswered),
        }
    }

    /// The held, not retired archetype for an API uuid, downloading its
    /// definition first when v1 does not hold it or holds it only by uuid.
    fn ensure_held(&mut self, uuid: &str, format: &str) -> Result<Held, String> {
        let held = held_uuid(self.reader, uuid).map_err(failed)?;
        let needs_download = match held {
            None => true,
            Some(row) if row.retired => return Ok(Held::Unavailable),
            Some(row) => row.needs_definition,
        };
        if let Some(row) = held.filter(|_| !needs_download) {
            return Ok(Held::Archetype(row.id));
        }
        // Held only by uuid: still an answer without its definition.
        let definition = match self.lookups.definition(uuid) {
            DefinitionReply::Found(definition) => definition,
            DefinitionReply::Gone => {
                return Ok(held.map_or(Held::Unavailable, |row| Held::Archetype(row.id)));
            }
            DefinitionReply::Failed => {
                return Ok(held.map_or(Held::Unanswered, |row| Held::Archetype(row.id)));
            }
        };
        let target = definition.uuid.clone();
        let key = format_key(format).unwrap_or_default();
        self.writer.edit(ArchetypeEdit::EnsureDownloaded {
            definition,
            format: key,
            now_ms: (self.now_ms)(),
        })?;
        // A moved uuid answers with its target's definition.
        let row = match held_uuid(self.reader, &target).map_err(failed)? {
            Some(row) => Some(row),
            None => held_uuid(self.reader, uuid).map_err(failed)?,
        };
        Ok(row
            .filter(|row| !row.retired)
            .map_or(Held::Unavailable, |row| Held::Archetype(row.id)))
    }

    fn homebrew(&self) -> Result<Option<i64>, String> {
        Ok(held_uuid(self.reader, HOMEBREW_UUID)
            .map_err(failed)?
            .map(|row| row.id))
    }

    /// One merge hop (0.x `ResolveMergedArchetype`).
    fn hop(&self, archetype_id: i64) -> Result<i64, String> {
        let parent = merged_into(self.reader, archetype_id).map_err(failed)?;
        Ok(archetypes::merge_hop(archetype_id, |_| parent))
    }

    /// Write results in slices; returns how many matches were written (a
    /// result that went stale meanwhile is dropped by the edit).
    pub fn write(&mut self, results: Vec<MatchDetection>) -> Result<u32, String> {
        let mut written = 0;
        for slice in results.chunks(ARCHETYPE_DETECTIONS_PER_EDIT) {
            let edit = ArchetypeEdit::SetDetections {
                results: slice.to_vec(),
                now_ms: (self.now_ms)(),
            };
            if let ArchetypeEditOutcome::Done { matches, .. } = self.writer.edit(edit)? {
                written += u32::try_from(matches.len()).unwrap_or(u32::MAX);
            }
        }
        Ok(written)
    }

    /// "Detect" on matches (0.x's synchronous controller): the markers are
    /// cleared, each match is detected once, in the order given (the
    /// page's), and the count written is the answer. A match not ready yet
    /// stays queued.
    pub fn detect_matches(
        &mut self,
        match_ids: &[i64],
        should_yield: &dyn Fn() -> bool,
    ) -> Result<u32, String> {
        self.detect_matches_reporting(match_ids, should_yield, &mut |_| {})
    }

    /// `detect_matches`, writing each match as soon as it is decided and
    /// then telling `on_written` its id, so the pages show each answer
    /// while the rest are asked (and not all at the end).
    pub fn detect_matches_reporting(
        &mut self,
        match_ids: &[i64],
        should_yield: &dyn Fn() -> bool,
        on_written: &mut dyn FnMut(i64),
    ) -> Result<u32, String> {
        let mut ids: Vec<i64> = Vec::new();
        for id in match_ids {
            if !ids.contains(id) {
                ids.push(*id);
            }
        }
        if ids.is_empty() {
            return Ok(0);
        }
        self.writer_ready()?;
        // Cleared first: a result is written only while its marker is
        // clear (`SetDetections`). Stops for a waiting signal (#300 F1) or a
        // failed estimate: what is decided is written, the rest stays
        // queued (markers cleared) for the backfill.
        self.writer.edit(ArchetypeEdit::ClearDetected {
            match_ids: ids.clone(),
        })?;
        let mut written = 0;
        for id in &ids {
            let (results, stopped) = self.detect_until_yield(&[*id], should_yield)?;
            let count = self.write(results)?;
            if count > 0 {
                on_written(*id);
            }
            written += count;
            if stopped != Stopped::Done {
                break;
            }
        }
        Ok(written)
    }

    /// The tracker's writer can take edits; asked before any API request
    /// so a paced answer is never thrown away (#300 F2).
    fn writer_ready(&self) -> Result<(), String> {
        if self.writer.ready() {
            Ok(())
        } else {
            Err("tracking is restarting; try again".to_string())
        }
    }

    /// Detect `ids` in order until `should_yield` says a signal waits or
    /// an estimate gives no answer. A match whose request was cut short or
    /// failed is left out (its "no answer" is not one), so it stays due.
    /// Returns the results and why it stopped.
    fn detect_until_yield(
        &mut self,
        ids: &[i64],
        should_yield: &dyn Fn() -> bool,
    ) -> Result<(Vec<MatchDetection>, Stopped), String> {
        let mut results = Vec::new();
        for id in ids {
            if should_yield() {
                return Ok((results, Stopped::Yielded));
            }
            let detected = self.detect_one(*id)?;
            // Checked first: a cut-short wait never reached the API, so its
            // `Unanswered` is not recorded as a failure below.
            if self.lookups.interrupted() {
                return Ok((results, Stopped::Yielded));
            }
            match detected {
                Detected::Ready(result) => {
                    self.failed.clear(*id);
                    results.push(result);
                }
                Detected::NotReady => {}
                Detected::Unanswered => {
                    self.failed.record(*id, (self.now_ms)());
                    return Ok((results, Stopped::Unanswered));
                }
            }
        }
        Ok((results, Stopped::Done))
    }

    /// The deck "Detect archetypes" banner (0.x
    /// `QueueArchetypeDetectionForDeck`): the deck's complete matches under
    /// the filter are queued (their markers cleared); the count is the
    /// answer.
    pub fn queue_deck(&mut self, deck_id: i64, filter: DeckFilter) -> Result<u32, String> {
        let ids = deck_detection_matches(self.reader, deck_id, filter).map_err(failed)?;
        if !ids.is_empty() {
            self.writer.edit(ArchetypeEdit::ClearDetected {
                match_ids: ids.clone(),
            })?;
        }
        Ok(u32::try_from(ids.len()).unwrap_or(u32::MAX))
    }

    /// One backfill slice: up to `ARCHETYPE_DETECTIONS_PER_EDIT` due
    /// matches, newest first, detected one at a time and written together.
    /// Stops early when `should_yield` says so or an estimate fails.
    /// Returns whether more may be due now (a full slice, or one cut short);
    /// after a failure it is false. For `FAILED_ESTIMATE_PAUSE_MS` after a
    /// failure it asks nothing, and a match whose estimate failed is passed
    /// over for `FAILED_MATCH_SKIP_MS`. "Detect" by hand waits for neither.
    pub fn backfill(&mut self, should_yield: &dyn Fn() -> bool) -> Result<bool, String> {
        self.writer_ready()?;
        let now_ms = (self.now_ms)();
        if self.failed.paused(now_ms) {
            return Ok(false);
        }
        let skipped = self.failed.skipped(now_ms);
        let wanted = ARCHETYPE_DETECTIONS_PER_EDIT + skipped.len();
        let limit = u32::try_from(wanted).unwrap_or(u32::MAX);
        let mut due = if self.logs_only {
            due_log_matches(self.reader, limit).map_err(failed)?
        } else {
            due_matches(self.reader, limit).map_err(failed)?
        };
        due.retain(|id| !skipped.contains(id));
        due.truncate(ARCHETYPE_DETECTIONS_PER_EDIT);
        let (results, stopped) = self.detect_until_yield(&due, should_yield)?;
        self.write(results)?;
        Ok(match stopped {
            Stopped::Yielded => true,
            Stopped::Unanswered => false,
            Stopped::Done => due.len() == ARCHETYPE_DETECTIONS_PER_EDIT,
        })
    }

    /// The deck prefill (spec 6.3, 0.x `SyncDecks::prefillArchetype`): the
    /// decision over the latest maindeck, written only while the deck has no
    /// archetype. No Homebrew for decks, and nothing when the estimate gave
    /// no answer (asked again when the deck syncs again). A deck still being
    /// built, or in a format the server has no archetypes for (it reads
    /// those as Standard), is not asked about. Returns whether one was set.
    pub fn prefill_deck(&mut self, deck_id: i64) -> Result<bool, String> {
        let Some(input) = deck_prefill_input(self.reader, deck_id).map_err(failed)? else {
            return Ok(false);
        };
        if input.archetype_id.is_some()
            || format_slug(&input.format).is_none()
            || !complete_maindeck(&input.cards)
        {
            return Ok(false);
        }
        self.writer_ready()?;
        let candidates = match format_key(&input.format) {
            Some(key) => held_candidates(self.reader, &key).map_err(failed)?,
            None => Vec::new(),
        };
        let seat = self.decide_seat(Some(&input.format), &candidates, &input.cards, false)?;
        // Cut short or failed: asked again later, never written from a
        // half-made decision.
        let Seat::Decided(Some(seat)) = seat else {
            return Ok(false);
        };
        if self.lookups.interrupted() {
            return Ok(false);
        }
        let outcome = self.writer.edit(ArchetypeEdit::PrefillDeck {
            deck_id,
            archetype_id: seat.archetype_id,
        })?;
        Ok(matches!(outcome, ArchetypeEditOutcome::Done { decks, .. } if !decks.is_empty()))
    }

    /// The definitions of downloaded rows known only by uuid (a sync pull,
    /// the 0.x import): the check asked about them with version `null`, its
    /// whole answer applied, so a moved or retired uuid is settled once
    /// (T4 review M7).
    pub fn fetch_missing_definitions(
        &mut self,
        should_yield: &dyn Fn() -> bool,
    ) -> Result<CheckRun, String> {
        let versions: Vec<(String, Option<String>)> = undefined_uuids(self.reader)
            .map_err(failed)?
            .into_iter()
            .map(|uuid| (uuid, None))
            .collect();
        if versions.is_empty() {
            return Ok(CheckRun::Applied);
        }
        self.check(&versions, None, should_yield)
    }

    /// The change check (spec section 5): every downloaded row with its
    /// version, the answer applied as `plan_changes` says, the time
    /// recorded in `archetypes_checked_at`. Online only.
    pub fn check_definitions(
        &mut self,
        should_yield: &dyn Fn() -> bool,
    ) -> Result<CheckRun, String> {
        let versions = downloaded_versions(self.reader).map_err(failed)?;
        let checked_at = (self.now_ms)();
        self.check(&versions, Some(checked_at), should_yield)
    }

    fn check(
        &mut self,
        versions: &[(String, Option<String>)],
        checked_at: Option<i64>,
        should_yield: &dyn Fn() -> bool,
    ) -> Result<CheckRun, String> {
        if !self.lookups.api_allowed() {
            return Ok(CheckRun::Offline);
        }
        self.writer_ready()?;
        let check = match self.lookups.check(versions) {
            CheckReply::Answer(check) => check,
            CheckReply::NotDeployed => {
                self.decklist_fallback(should_yield)?;
                return Ok(CheckRun::NotDeployed);
            }
            CheckReply::Failed(error) if error.is_transient() => {
                tracing::warn!(%error, "the archetype definitions check could not reach MyMTGO");
                return Ok(CheckRun::Unreachable);
            }
            CheckReply::Failed(error) => return Err(error.to_string()),
        };
        let held = held_definitions(self.reader).map_err(failed)?;
        let slices = plan_slices(&held, &check, DEFINITIONS_PER_EDIT);
        let last = slices.len() - 1;
        for (index, plan) in slices.into_iter().enumerate() {
            self.writer.edit(ArchetypeEdit::ApplyDefinitions {
                plan,
                checked_at: if index == last { checked_at } else { None },
                now_ms: (self.now_ms)(),
            })?;
        }
        Ok(CheckRun::Applied)
    }

    /// Before the API has the check (spec section 5): rows without a
    /// definition take their `/decklist` key list (cards only, version
    /// null), one at a time while the API may be asked.
    fn decklist_fallback(&mut self, should_yield: &dyn Fn() -> bool) -> Result<(), String> {
        for uuid in undefined_uuids(self.reader).map_err(failed)? {
            if should_yield() || !self.lookups.api_allowed() {
                break;
            }
            let DefinitionReply::Found(definition) = self.lookups.definition(&uuid) else {
                continue;
            };
            let format = held_uuid(self.reader, &uuid)
                .map_err(failed)?
                .and_then(|row| {
                    store::archetypes::archetype(self.reader, row.id)
                        .ok()
                        .flatten()
                })
                .and_then(|row| row.format)
                .unwrap_or_default();
            self.writer.edit(ArchetypeEdit::EnsureDownloaded {
                definition,
                format,
                now_ms: (self.now_ms)(),
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
