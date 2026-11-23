//! The archetypes API calls about definitions (archetypes spec section 5):
//! the definitions check (T3's `check_definitions`, `POST
//! /api/archetypes/definitions/check`) for every held downloaded archetype,
//! and the same check asked about one uuid with version `null` to download
//! it ("Getting one"). Before the API has the endpoint (a 404), a definition
//! is the archetype's `/decklist` key list with no rules, and the check is
//! not asked again until the next start.
//!
//! The pacing of every archetype request lives here too: after any API
//! request the worker waits `API_PAUSE_MS` before the next (spec 6.3), so a
//! backfill never floods the API; detection the player asked for waits
//! `ASKED_API_PAUSE_MS`. Blocking: runs on the `archetypes` worker only.

use archetypes::{Definition, DefinitionCard, DefinitionsCheck, ObservedCard, colors_of};
use mymtgo_api::{ApiError, is_not_deployed};

use crate::api::reference::{check_definitions, fetch_decklist};
use crate::api::session::ApiState;

/// The pause after an API request before the next one (0.x staggered its
/// refresh 2 s apart).
pub const API_PAUSE_MS: i64 = 2_000;

/// The pause while detecting matches the player asked for ("Detect" on
/// rows): 0.x ran those at once, and the player is waiting. At most 80
/// estimates a minute, leaving headroom under the API's `archetype-estimates`
/// limit of 120 a minute per sender (shared with the sender's other
/// requests and devices).
pub const ASKED_API_PAUSE_MS: i64 = 750;

/// The time and sleeping the pacing needs, so tests can use a fake clock.
pub trait Clock {
    fn now_ms(&self) -> i64;
    fn sleep_ms(&self, ms: i64);
    /// Sleep like `sleep_ms`, but stop early when the worker has a signal
    /// the running job stops for (#300 F1). True when the whole pause
    /// passed.
    fn sleep_or_yield_ms(&self, ms: i64) -> bool {
        self.sleep_ms(ms);
        true
    }
}

/// The wall clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        crate::wall_clock_ms()
    }

    fn sleep_ms(&self, ms: i64) {
        if let Ok(ms) = u64::try_from(ms) {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
    }
}

/// Keeps archetype requests `API_PAUSE_MS` apart (or the pause set with
/// `set_pause_ms`), and remembers that the
/// definitions check answered 404 (not deployed): from then until the next
/// start the check is not asked again and definitions come from
/// `/decklist`. One per worker, so both hold across matches and jobs.
pub struct Pacer {
    clock: Box<dyn Clock>,
    pause_ms: i64,
    last_request_ms: Option<i64>,
    check_missing: bool,
}

impl Pacer {
    pub fn new(clock: Box<dyn Clock>) -> Pacer {
        Pacer {
            clock,
            pause_ms: API_PAUSE_MS,
            last_request_ms: None,
            check_missing: false,
        }
    }

    /// The pause from now on: `ASKED_API_PAUSE_MS` while the player waits,
    /// `API_PAUSE_MS` again after.
    pub fn set_pause_ms(&mut self, pause_ms: i64) {
        self.pause_ms = pause_ms;
    }

    /// Wait until the pause has passed since the last request ended.
    pub fn before_request(&mut self) {
        let wait = self.wait_ms();
        if wait > 0 {
            self.clock.sleep_ms(wait);
        }
    }

    /// The same wait, cut short when the worker has a signal the running
    /// job stops for (#300 F1). False when cut short: send nothing then.
    pub fn before_request_or_yield(&mut self) -> bool {
        let wait = self.wait_ms();
        wait <= 0 || self.clock.sleep_or_yield_ms(wait)
    }

    /// How long until the next request may go, in ms (0 or less: now).
    fn wait_ms(&self) -> i64 {
        match self.last_request_ms {
            Some(last) => last + self.pause_ms - self.clock.now_ms(),
            None => 0,
        }
    }

    /// A request just ended.
    pub fn after_request(&mut self) {
        self.last_request_ms = Some(self.clock.now_ms());
    }

    /// Now, by the pacing clock.
    pub fn now_ms(&self) -> i64 {
        self.clock.now_ms()
    }

    /// The check answered 404 since the start.
    pub fn check_missing(&self) -> bool {
        self.check_missing
    }
}

/// What the definitions check answered.
#[derive(Debug, Clone, PartialEq)]
pub enum CheckReply {
    Answer(DefinitionsCheck),
    /// A 404: the endpoint is not deployed (use the `/decklist` fallback).
    NotDeployed,
    /// Offline, no connection, another status, or an answer that does not
    /// read.
    Failed(ApiError),
}

/// One paced definitions check for these `(uuid, version)` pairs. After a
/// 404 nothing is sent until the next start.
pub fn check_versions(
    api: &ApiState,
    pacer: &mut Pacer,
    versions: &[(String, Option<String>)],
) -> CheckReply {
    if pacer.check_missing {
        return CheckReply::NotDeployed;
    }
    pacer.before_request();
    let sent = check_definitions(api, versions, pacer.now_ms());
    pacer.after_request();
    match sent {
        Ok(check) => CheckReply::Answer(check),
        Err(error) if is_not_deployed(&error) => {
            pacer.check_missing = true;
            CheckReply::NotDeployed
        }
        Err(error) => CheckReply::Failed(error),
    }
}

/// The definition the check gave for `uuid`: its own, or that of the
/// archetype it moved to. `None` when retired or unknown.
pub fn definition_for(check: &DefinitionsCheck, uuid: &str) -> Option<Definition> {
    let target = check
        .moved
        .iter()
        .find(|(from, _)| from == uuid)
        .map_or(uuid, |(_, to)| to.as_str());
    check
        .changed
        .iter()
        .find(|definition| definition.uuid == target)
        .cloned()
}

/// What asking for one archetype's definition came to.
#[derive(Debug, Clone, PartialEq)]
pub enum DefinitionReply {
    /// Boxed: a definition is large and the other answers are empty.
    Found(Box<Definition>),
    /// The server answered: retired, or unknown to it. Asking again gets
    /// the same.
    Gone,
    /// No answer (offline, a failed request, a cut-short wait): it may be
    /// had later.
    Failed,
}

impl DefinitionReply {
    /// The definition, if one was found.
    pub fn found(self) -> Option<Definition> {
        match self {
            DefinitionReply::Found(definition) => Some(*definition),
            DefinitionReply::Gone | DefinitionReply::Failed => None,
        }
    }
}

/// The definition of `uuid` from the API. The check is asked with version
/// `null`; once it answered 404, the `/decklist` key list is used instead:
/// a definition with only cards and the colours of its non-land cards.
pub fn fetch_definition(api: &ApiState, pacer: &mut Pacer, uuid: &str) -> DefinitionReply {
    if api.offline_mode().0 {
        return DefinitionReply::Failed;
    }
    match check_versions(api, pacer, &[(uuid.to_string(), None)]) {
        CheckReply::Answer(check) => match definition_for(&check, uuid) {
            Some(definition) => DefinitionReply::Found(Box::new(definition)),
            None => DefinitionReply::Gone,
        },
        CheckReply::NotDeployed => decklist_definition(api, pacer, uuid),
        CheckReply::Failed(error) => {
            tracing::warn!(%error, "the archetype definitions check failed");
            DefinitionReply::Failed
        }
    }
}

/// The `/decklist` fallback (spec section 5 "Before the API has the
/// endpoint").
fn decklist_definition(api: &ApiState, pacer: &mut Pacer, uuid: &str) -> DefinitionReply {
    pacer.before_request();
    let fetched = fetch_decklist(api, uuid, pacer.now_ms());
    pacer.after_request();
    let cards = match fetched {
        Ok(cards) => cards,
        // The server does not know this archetype.
        Err(ApiError::Status(404, _)) => return DefinitionReply::Gone,
        Err(error) => {
            tracing::warn!(%error, "an archetype decklist could not be fetched");
            return DefinitionReply::Failed;
        }
    };
    let observed: Vec<ObservedCard> = cards
        .iter()
        .map(|card| ObservedCard {
            catalog_id: i64::from(card.mtgo_id),
            quantity: card.quantity,
            sideboard: card.sideboard,
            oracle_id: Some(card.oracle_id.clone()),
            name: card.name.clone(),
            card_type: card.card_type.clone(),
            color_identity: card.color_identity.clone(),
            mana_cost: None,
        })
        .collect();
    DefinitionReply::Found(Box::new(Definition {
        uuid: uuid.to_string(),
        color_identity: colors_of(&observed),
        cards: cards
            .into_iter()
            .map(|card| DefinitionCard {
                mtgo_id: i64::from(card.mtgo_id),
                arena_id: None,
                oracle_id: Some(card.oracle_id),
                name: card.name,
                quantity: card.quantity,
                sideboard: card.sideboard,
            })
            .collect(),
        ..Definition::default()
    }))
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::api::session::testing::device_api;
    use crate::api::stub::{Reply, Stub};
    use mymtgo_api::archetypes::DEFINITIONS_CHECK_PATH;
    use std::sync::{Arc, Mutex};

    pub const NOW: i64 = 1_800_000_000_000;

    /// A clock that moves only when slept on, and records each sleep. While
    /// `signal_waits` is set, a sleep that may yield is cut short at once.
    #[derive(Clone, Default)]
    pub struct FakeClock {
        pub now: Arc<Mutex<i64>>,
        pub sleeps: Arc<Mutex<Vec<i64>>>,
        pub signal_waits: Arc<Mutex<bool>>,
    }

    impl Clock for FakeClock {
        fn now_ms(&self) -> i64 {
            *self.now.lock().expect("clock")
        }

        fn sleep_ms(&self, ms: i64) {
            self.sleeps.lock().expect("sleeps").push(ms);
            *self.now.lock().expect("clock") += ms;
        }

        fn sleep_or_yield_ms(&self, ms: i64) -> bool {
            if *self.signal_waits.lock().expect("signal") {
                return false;
            }
            self.sleep_ms(ms);
            true
        }
    }

    pub fn pacer() -> (Pacer, FakeClock) {
        let clock = FakeClock::default();
        *clock.now.lock().expect("clock") = NOW;
        (Pacer::new(Box::new(clock.clone())), clock)
    }

    pub const DEFINITION: &str = r#"{"uuid":"q-new","name":"Qnew Storm","format":"modern",
        "color_identity":"R","version":"q-v1","rank":[2,1,4,-17,0,0,0],
        "conditions":[{"type":"InMainboard","cards":["Qcard A"]}],"fallback":false,
        "common_cards":[],"min_overlap":0.1,"colour_fork":"41:Base",
        "cards":[{"mtgo_id":501,"oracle_id":"q-oracle-a","name":"Qcard A","quantity":4,"sideboard":false}]}"#;

    /// The download is T3's definitions check asked about one uuid with
    /// version `null` (no second builder for the same endpoint).
    #[test]
    fn the_check_for_one_uuid_asks_with_a_null_version() {
        let stub = Stub::start(|_| Reply::json(200, r#"{"changed":[],"moved":[],"retired":[]}"#));
        let api = device_api(&stub.base_url, false);
        let (mut pacer, _) = pacer();
        assert_eq!(
            fetch_definition(&api.state, &mut pacer, "q-new"),
            DefinitionReply::Gone
        );
        let seen = stub.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].target, DEFINITIONS_CHECK_PATH);
        assert_eq!(
            String::from_utf8(seen[0].body.clone()).expect("utf8"),
            r#"{"archetypes":[{"uuid":"q-new","version":null}]}"#
        );
    }

    #[test]
    fn a_changed_answer_gives_the_definition_and_a_move_its_target() {
        let stub = Stub::start(|_| {
            Reply::json(
                200,
                &format!(
                    r#"{{"changed":[{DEFINITION}],"moved":[{{"uuid":"q-old","to":"q-new"}}],"retired":[]}}"#
                ),
            )
        });
        let api = device_api(&stub.base_url, false);
        let (mut pacer, _) = pacer();
        let direct = fetch_definition(&api.state, &mut pacer, "q-new")
            .found()
            .expect("definition");
        assert_eq!(direct.name, "Qnew Storm");
        assert_eq!(direct.version.as_deref(), Some("q-v1"));
        let moved = fetch_definition(&api.state, &mut pacer, "q-old")
            .found()
            .expect("the target");
        assert_eq!(moved.uuid, "q-new");
    }

    // 0.x DetermineDeckArchetypeTest "returns null when the API archetype
    // stays unknown after a sync attempt": retired or unknown is gone; a
    // failure is told apart, so detection asks again later.
    #[test]
    fn a_retired_answer_is_gone_and_a_failure_failed() {
        let stub = Stub::start(|request| {
            if request.body.windows(6).any(|part| part == b"q-gone") {
                Reply::json(200, r#"{"changed":[],"moved":[],"retired":["q-gone"]}"#)
            } else {
                Reply::json(500, "{}")
            }
        });
        let api = device_api(&stub.base_url, false);
        let (mut pacer, _) = pacer();
        assert_eq!(
            fetch_definition(&api.state, &mut pacer, "q-gone"),
            DefinitionReply::Gone
        );
        assert_eq!(
            fetch_definition(&api.state, &mut pacer, "q-error"),
            DefinitionReply::Failed
        );
    }

    // Decision C3: before the API has the check, a 404 falls back to the
    // archetype's `/decklist`, a definition with only cards (T5 extends it).
    #[test]
    fn a_404_falls_back_to_the_decklist() {
        let stub = Stub::start(|request| {
            if request.target == DEFINITIONS_CHECK_PATH {
                Reply::json(404, r#"{"message":"Not Found"}"#)
            } else {
                Reply::json(
                    200,
                    r#"{"cards":[
                        {"mtgo_id":501,"oracle_id":"q-oracle-a","name":"Qcard A","type":"Creature",
                         "color_identity":"R","quantity":4,"sideboard":false},
                        {"mtgo_id":502,"oracle_id":"q-oracle-b","name":"Qland","type":"Land",
                         "color_identity":"G","quantity":2,"sideboard":true}]}"#,
                )
            }
        });
        let api = device_api(&stub.base_url, false);
        let (mut pacer, _) = pacer();
        let definition = fetch_definition(&api.state, &mut pacer, "q-new")
            .found()
            .expect("fallback");
        assert_eq!(definition.uuid, "q-new");
        assert!(definition.conditions.is_empty() && definition.version.is_none());
        assert_eq!(definition.color_identity.as_deref(), Some("R"), "lands out");
        assert_eq!(definition.cards.len(), 2);
        let targets: Vec<String> = stub.requests().into_iter().map(|r| r.target).collect();
        assert_eq!(
            targets,
            [
                DEFINITIONS_CHECK_PATH.to_string(),
                "/api/archetypes/q-new/decklist".to_string()
            ]
        );
        // Spec 5: after a 404 the check is not asked again until the next
        // start; definitions come straight from `/decklist`.
        assert!(pacer.check_missing());
        fetch_definition(&api.state, &mut pacer, "q-other")
            .found()
            .expect("fallback");
        assert_eq!(
            check_versions(&api.state, &mut pacer, &[("q-a".to_string(), None)]),
            CheckReply::NotDeployed
        );
        let targets: Vec<String> = stub.requests().into_iter().map(|r| r.target).collect();
        assert_eq!(targets.len(), 3, "{targets:?}");
        assert_eq!(targets[2], "/api/archetypes/q-other/decklist");
    }

    // Before the check is deployed: an archetype the server does not know
    // (a 404 from `/decklist`) is gone, a 500 failed.
    #[test]
    fn a_decklist_404_is_gone_and_a_500_failed() {
        let stub = Stub::start(|request| {
            if request.target == DEFINITIONS_CHECK_PATH || request.target.contains("q-unknown") {
                Reply::json(404, r#"{"message":"Not Found"}"#)
            } else {
                Reply::json(500, "{}")
            }
        });
        let api = device_api(&stub.base_url, false);
        let (mut pacer, _) = pacer();
        assert_eq!(
            fetch_definition(&api.state, &mut pacer, "q-unknown"),
            DefinitionReply::Gone
        );
        assert_eq!(
            fetch_definition(&api.state, &mut pacer, "q-error"),
            DefinitionReply::Failed
        );
    }

    #[test]
    fn offline_nothing_is_fetched() {
        let stub = Stub::start(|_| Reply::json(200, "{}"));
        let api = device_api(&stub.base_url, true);
        let (mut pacer, _) = pacer();
        assert_eq!(
            fetch_definition(&api.state, &mut pacer, "q-new"),
            DefinitionReply::Failed
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(stub.connections(), 0);
    }

    /// #300 F1: a pause that may yield ends at once for a waiting signal,
    /// and says so; the plain pause still waits.
    #[test]
    fn a_pause_that_may_yield_ends_for_a_waiting_signal() {
        let (mut pacer, clock) = pacer();
        assert!(pacer.before_request_or_yield(), "nothing to wait for");
        pacer.after_request();
        *clock.signal_waits.lock().expect("signal") = true;
        assert!(!pacer.before_request_or_yield());
        assert!(clock.sleeps.lock().expect("sleeps").is_empty());
        pacer.before_request();
        assert_eq!(*clock.sleeps.lock().expect("sleeps"), [2_000]);
    }

    /// Detection the player asked for waits `ASKED_API_PAUSE_MS`, well under
    /// the API's 120 estimates a minute; the plain pause comes back after it.
    #[test]
    fn asked_detection_is_paced_shorter_and_the_backfill_pause_returns() {
        let (mut pacer, clock) = pacer();
        pacer.set_pause_ms(ASKED_API_PAUSE_MS);
        pacer.before_request();
        pacer.after_request();
        pacer.before_request();
        pacer.after_request();
        pacer.set_pause_ms(API_PAUSE_MS);
        pacer.before_request();
        assert_eq!(*clock.sleeps.lock().expect("sleeps"), [750, 2_000]);
    }

    #[test]
    fn requests_are_kept_two_seconds_apart() {
        let (mut pacer, clock) = pacer();
        pacer.before_request();
        pacer.after_request();
        *clock.now.lock().expect("clock") += 500;
        pacer.before_request();
        pacer.after_request();
        *clock.now.lock().expect("clock") += 5_000;
        pacer.before_request();
        assert_eq!(*clock.sleeps.lock().expect("sleeps"), [1_500]);
    }
}
