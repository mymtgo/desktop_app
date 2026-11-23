//! What the game overlay asks the network (overlay spec 7.3 to 7.5): the
//! opponent's scouting, the API's estimate of the revealed cards, an
//! archetype's live decklists, and the stub of an archetype the API named
//! that v1 does not hold.
//!
//! The `overlay` thread never waits on any of them. `GameNet` keeps the
//! answers in memory (like Laravel's `Cache::remember` with the array
//! driver) and queues what is missing on the `overlay-net` thread through a
//! `GamePort`; the answers come back as `OverlayMsg::GameAnswered` and the
//! view is computed again. How long each answer is kept:
//!
//! - scouting: an hour per (username, format), a failure 5 minutes (v1
//!   addition, spec ruling 15), so an outage is not asked again every frame;
//! - the API estimate: 30 minutes per match and reveal set (0.x), failures
//!   too, so it is asked at most once per distinct reveal;
//! - the live local read: 30 minutes per match and reveal set (0.x);
//! - live decklists: Plan T's archetypes cache (10 minutes), asked again no
//!   sooner than that.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};

use archetypes::Candidate;
use mymtgo_api::{ArchetypeDeck, ArchetypeEstimate, MetagameShares, Scouting};
use overlay::LiveEstimate;
use store::StoreError;
use store::archetype_edit::ArchetypeEdit;
use tauri::{AppHandle, Manager};

use crate::api::session::ApiState;
use crate::archetypes::cache::{ArchetypeCaches, Cached, DECKLISTS_TTL_MS, metagame_key};
use crate::archetypes::worker::ArchetypeSignal;
use crate::bridge::lock;
use crate::overlay::net::{NetJob, NetSignal};
use crate::overlay::worker::OverlayMsg;

/// Scouting answers are kept an hour (0.x).
pub const SCOUTING_TTL_MS: i64 = 3_600_000;
/// A failed scouting lookup is asked again after 5 minutes.
pub const SCOUTING_FAILURE_TTL_MS: i64 = 300_000;
/// The API estimate is kept 30 minutes per reveal set (0.x).
pub const ESTIMATE_TTL_MS: i64 = 1_800_000;
/// A failed estimate is asked again after 5 minutes.
pub const ESTIMATE_FAILURE_TTL_MS: i64 = 300_000;
/// The live local read is kept 30 minutes per reveal set (0.x).
pub const LIVE_TTL_MS: i64 = 1_800_000;
/// The archetype picker's metagame window (owner answer 3).
pub const PICKER_DAYS: u32 = 90;

/// A request for the `overlay-net` thread.
#[derive(Debug, Clone, PartialEq)]
pub enum GameRequest {
    /// `POST /api/players` for the opponent in a format key.
    Scout {
        username: String,
        format_key: String,
    },
    /// `POST /api/archetypes/estimate` of the revealed cards (`key` is the
    /// match and reveal set; `format` the raw match format).
    Estimate {
        key: String,
        format: String,
        cards: Vec<(u32, u32)>,
    },
    /// An archetype's live decklists, fetched on the archetypes worker into
    /// Plan T's cache.
    Decklists { uuid: String },
    /// A format's 90-day metagame list, fetched on the archetypes worker into
    /// Plan T's cache (as the archetype picker asks it).
    Metagame { format_key: String },
    /// A `downloaded` row known only by its uuid (`ArchetypeEdit::EnsureStub`),
    /// then the archetypes worker's missing definitions download.
    Stub {
        uuid: String,
        name: String,
        format_key: String,
        color_identity: Option<String>,
    },
}

impl GameRequest {
    /// The request's waiting slot on the `overlay-net` thread.
    pub fn key(&self) -> String {
        match self {
            GameRequest::Scout {
                username,
                format_key,
            } => format!("overlay-scout:{username}:{format_key}"),
            GameRequest::Estimate { key, .. } => format!("overlay-estimate:{key}"),
            GameRequest::Decklists { uuid } => format!("overlay-decklists:{uuid}"),
            GameRequest::Metagame { format_key } => format!("overlay-metagame:{format_key}"),
            GameRequest::Stub { uuid, .. } => format!("overlay-stub:{uuid}"),
        }
    }
}

/// An answer from the `overlay-net` thread.
#[derive(Debug, Clone, PartialEq)]
pub enum GameAnswer {
    /// `failed` when the lookup did not get an answer (kept 5 minutes);
    /// otherwise `answer` is what the API said (`None`: no source).
    Scouted {
        username: String,
        format_key: String,
        answer: Option<Scouting>,
        failed: bool,
    },
    /// `failed` when the estimate did not get an answer (kept 5 minutes).
    Estimated {
        key: String,
        answer: Option<ArchetypeEstimate>,
        failed: bool,
    },
    /// The archetype's decklists are in Plan T's cache (or failed).
    Decklists { uuid: String },
}

/// What the game overlay needs from outside the `overlay` thread.
pub trait GamePort: Send {
    /// Lookups may be sent (not offline: the `Lookup` gate class).
    fn online(&self) -> bool;
    /// Queue a request on the `overlay-net` thread.
    fn submit(&mut self, request: GameRequest);
    /// An archetype's live decklists, when Plan T's cache holds fresh ones.
    fn decklists(&self, uuid: &str, now_ms: i64) -> Option<Vec<ArchetypeDeck>>;
    /// A format's 90-day metagame shares, when Plan T's cache holds them.
    fn metagame(&self, format_key: &str, now_ms: i64) -> Option<MetagameShares>;
}

/// What a lookup knows now.
#[derive(Debug, Clone, PartialEq)]
enum Lookup<T> {
    /// An answer (`None`: the API had none, or the lookup failed) still kept.
    Known(Option<T>),
    /// Asked, no answer yet.
    Waiting,
    /// Never asked, or the answer is too old.
    Unknown,
}

/// Answers by key, each kept until its own end, and the keys asked.
struct Remembered<T: Clone> {
    entries: BTreeMap<String, (i64, Option<T>)>,
    /// Asked at; a request with no answer is asked again after `retry_ms`.
    asked: BTreeMap<String, i64>,
    retry_ms: i64,
}

impl<T: Clone> Remembered<T> {
    fn new(retry_ms: i64) -> Remembered<T> {
        Remembered {
            entries: BTreeMap::new(),
            asked: BTreeMap::new(),
            retry_ms,
        }
    }

    fn get(&self, key: &str, now_ms: i64) -> Lookup<T> {
        if let Some((until_ms, answer)) = self.entries.get(key)
            && now_ms < *until_ms
        {
            return Lookup::Known(answer.clone());
        }
        match self.asked.get(key) {
            Some(at) if now_ms - at < self.retry_ms => Lookup::Waiting,
            _ => Lookup::Unknown,
        }
    }

    fn ask(&mut self, key: &str, now_ms: i64) {
        self.asked.insert(key.to_string(), now_ms);
    }

    fn store(&mut self, key: &str, answer: Option<T>, until_ms: i64) {
        self.asked.remove(key);
        self.entries.insert(key.to_string(), (until_ms, answer));
    }

    /// Forget answers past their end (memory stays bounded over a session).
    fn prune(&mut self, now_ms: i64) {
        self.entries.retain(|_, (until_ms, _)| now_ms < *until_ms);
    }
}

/// The game overlay's lookups as `game_view` asks them.
pub trait GameLookups {
    /// Lookups may be sent now.
    fn online(&self) -> bool;
    /// The live local read of a match's reveal set (`key`), computed with
    /// `compute` when not kept.
    fn live_read(
        &mut self,
        key: &str,
        now_ms: i64,
        compute: &mut dyn FnMut() -> Option<LiveEstimate>,
    ) -> Option<LiveEstimate>;
    /// The format's held candidates for the live read, `load`ed when the
    /// kept ones are of another format or `stamp`
    /// (`store::overlay::candidates_stamp`), or were forgotten.
    fn held_candidates(
        &mut self,
        format_key: &str,
        stamp: (i64, i64),
        load: &mut dyn FnMut() -> Result<Vec<Candidate>, StoreError>,
    ) -> Result<Arc<Vec<Candidate>>, StoreError>;
    /// The opponent's scouting, when known; asked when not.
    fn scouting(&mut self, username: &str, format_key: &str, now_ms: i64) -> Option<Scouting>;
    /// The API's estimate of a match's reveal set (`key`), when known;
    /// asked when not (only online, never twice for one set, and not until
    /// `overlay::estimate_due` says so after the match's last ask). While a
    /// newer set waits, the last answer the match had.
    fn estimate(
        &mut self,
        match_id: i64,
        key: &str,
        format: &str,
        cards: &[(u32, u32)],
        now_ms: i64,
    ) -> Option<ArchetypeEstimate>;
    /// An archetype's live decklists, when fresh; asked when not (online).
    fn decklists(&mut self, uuid: &str, now_ms: i64) -> Option<Vec<ArchetypeDeck>>;
    /// A name the format's metagame list gives a uuid (with its colours).
    /// When the list is not held, it is asked (online, once per format and
    /// while).
    fn metagame_name(
        &mut self,
        format_key: &str,
        uuid: &str,
        now_ms: i64,
    ) -> Option<(String, Option<String>)>;
    /// The latest scouting or estimate answer was a failure: MyMTGO could
    /// not be reached (#503).
    fn unreachable(&self) -> bool;
    /// Hand an unheld uuid to Plan T, once.
    fn ensure_stub(
        &mut self,
        uuid: &str,
        name: &str,
        format_key: &str,
        color_identity: Option<&str>,
    );
}

/// The game overlay's lookups and their answers, on the `overlay` thread.
pub struct GameNet {
    port: Option<Box<dyn GamePort>>,
    scouting: Remembered<Scouting>,
    estimates: Remembered<ArchetypeEstimate>,
    /// Each match's last estimate ask (#502), kept while its answer is.
    estimate_asks: BTreeMap<i64, AskedEstimate>,
    /// The latest scouting or estimate answer was a failure (#503).
    unreachable: bool,
    live: BTreeMap<String, (i64, Option<LiveEstimate>)>,
    decklists_asked: BTreeMap<String, i64>,
    /// The formats whose metagame list was asked for, and when.
    metagame_asked: BTreeMap<String, i64>,
    stubbed: BTreeSet<String>,
    /// The last format's held candidates.
    candidates: Option<KeptCandidates>,
    /// How many times the candidates were read (tests).
    candidate_reads: u32,
}

/// A match's last estimate ask: when, its reveal set and key, and the key
/// of an earlier set whose answer shows while this one waits.
struct AskedEstimate {
    at_ms: i64,
    cards: Vec<(u32, u32)>,
    key: String,
    shown: Option<String>,
    /// A newer set was held back: the overlay wakes when its wait ends.
    held_back: bool,
}

/// A format's held candidates and the stamp they were read at.
struct KeptCandidates {
    format_key: String,
    stamp: (i64, i64),
    list: Arc<Vec<Candidate>>,
}

impl Default for GameNet {
    fn default() -> GameNet {
        GameNet::new(None)
    }
}

impl GameNet {
    /// Lookups through `port`; without one nothing is ever asked.
    pub fn new(port: Option<Box<dyn GamePort>>) -> GameNet {
        GameNet {
            port,
            scouting: Remembered::new(SCOUTING_FAILURE_TTL_MS),
            estimates: Remembered::new(ESTIMATE_TTL_MS),
            estimate_asks: BTreeMap::new(),
            unreachable: false,
            live: BTreeMap::new(),
            decklists_asked: BTreeMap::new(),
            metagame_asked: BTreeMap::new(),
            stubbed: BTreeSet::new(),
            candidates: None,
            candidate_reads: 0,
        }
    }

    /// How many times the held candidates were read from the database.
    pub fn candidate_reads(&self) -> u32 {
        self.candidate_reads
    }

    /// Read the held candidates again at the next live read: on
    /// `ArchetypesChanged` (every archetype edit, some of which do not move
    /// the stamp) and `CardsChanged` (the card rows the lists join).
    pub fn forget_candidates(&mut self) {
        self.candidates = None;
    }

    /// Keep an answer from the `overlay-net` thread.
    pub fn answered(&mut self, answer: GameAnswer, now_ms: i64) {
        match answer {
            GameAnswer::Scouted {
                username,
                format_key,
                answer,
                failed,
            } => {
                let ttl = if failed {
                    SCOUTING_FAILURE_TTL_MS
                } else {
                    SCOUTING_TTL_MS
                };
                self.unreachable = failed;
                self.scouting
                    .store(&scouting_key(&username, &format_key), answer, now_ms + ttl);
            }
            GameAnswer::Estimated {
                key,
                answer,
                failed,
            } => {
                let ttl = if failed {
                    ESTIMATE_FAILURE_TTL_MS
                } else {
                    ESTIMATE_TTL_MS
                };
                self.unreachable = failed;
                self.estimates.store(&key, answer, now_ms + ttl);
            }
            // Plan T's cache holds them; the next view reads it.
            GameAnswer::Decklists { .. } => {}
        }
    }

    /// Forget answers past their end.
    pub fn prune(&mut self, now_ms: i64) {
        self.scouting.prune(now_ms);
        self.estimates.prune(now_ms);
        self.live.retain(|_, (until_ms, _)| now_ms < *until_ms);
        self.decklists_asked
            .retain(|_, at| now_ms - *at < DECKLISTS_TTL_MS);
        self.metagame_asked
            .retain(|_, at| now_ms - *at < DECKLISTS_TTL_MS);
    }

    fn submit(&mut self, request: GameRequest) {
        if let Some(port) = self.port.as_mut() {
            port.submit(request);
        }
    }
}

fn scouting_key(username: &str, format_key: &str) -> String {
    format!("{username}\u{1f}{format_key}")
}

impl GameNet {
    /// When a held-back reveal set's wait ends (#502), the earliest: the
    /// overlay wakes then to ask, as nothing else may change meanwhile.
    pub fn estimate_wake_at(&self) -> Option<i64> {
        self.estimate_asks
            .values()
            .filter(|asked| asked.held_back)
            .map(|asked| asked.at_ms + overlay::ESTIMATE_GAP_MS)
            .min()
    }

    /// Whether a held-back set's wait has ended at `now_ms`; each such wake
    /// is taken once (a recompute asks, or holds it back again).
    pub fn take_due_estimates(&mut self, now_ms: i64) -> bool {
        let mut due = false;
        for asked in self.estimate_asks.values_mut() {
            if asked.held_back && now_ms >= asked.at_ms + overlay::ESTIMATE_GAP_MS {
                asked.held_back = false;
                due = true;
            }
        }
        due
    }

    /// The matches with a kept ask, in id order (tests).
    #[cfg(test)]
    pub fn estimate_matches(&self) -> Vec<i64> {
        self.estimate_asks.keys().copied().collect()
    }

    /// The key of the match's newest set with a kept answer: its last ask,
    /// else the one shown while that waits.
    fn answered_key(&self, match_id: i64, now_ms: i64) -> Option<String> {
        let last = self.estimate_asks.get(&match_id)?;
        [Some(&last.key), last.shown.as_ref()]
            .into_iter()
            .flatten()
            .find(|key| matches!(self.estimates.get(key, now_ms), Lookup::Known(_)))
            .cloned()
    }

    /// The answer the match shows while a newer set waits.
    fn shown_estimate(&self, match_id: i64, now_ms: i64) -> Option<ArchetypeEstimate> {
        let key = self.answered_key(match_id, now_ms)?;
        match self.estimates.get(&key, now_ms) {
            Lookup::Known(answer) => answer,
            _ => None,
        }
    }
}

impl GameLookups for GameNet {
    fn online(&self) -> bool {
        self.port.as_ref().is_some_and(|port| port.online())
    }

    fn held_candidates(
        &mut self,
        format_key: &str,
        stamp: (i64, i64),
        load: &mut dyn FnMut() -> Result<Vec<Candidate>, StoreError>,
    ) -> Result<Arc<Vec<Candidate>>, StoreError> {
        if let Some(kept) = &self.candidates
            && kept.format_key == format_key
            && kept.stamp == stamp
        {
            return Ok(Arc::clone(&kept.list));
        }
        let list = Arc::new(load()?);
        self.candidate_reads += 1;
        self.candidates = Some(KeptCandidates {
            format_key: format_key.to_string(),
            stamp,
            list: Arc::clone(&list),
        });
        Ok(list)
    }

    fn live_read(
        &mut self,
        key: &str,
        now_ms: i64,
        compute: &mut dyn FnMut() -> Option<LiveEstimate>,
    ) -> Option<LiveEstimate> {
        if let Some((until_ms, estimate)) = self.live.get(key)
            && now_ms < *until_ms
        {
            return estimate.clone();
        }
        let estimate = compute();
        self.live
            .insert(key.to_string(), (now_ms + LIVE_TTL_MS, estimate.clone()));
        estimate
    }

    fn scouting(&mut self, username: &str, format_key: &str, now_ms: i64) -> Option<Scouting> {
        let key = scouting_key(username, format_key);
        match self.scouting.get(&key, now_ms) {
            Lookup::Known(answer) => answer,
            Lookup::Waiting => None,
            Lookup::Unknown => {
                if self.online() {
                    self.scouting.ask(&key, now_ms);
                    self.submit(GameRequest::Scout {
                        username: username.to_string(),
                        format_key: format_key.to_string(),
                    });
                }
                None
            }
        }
    }

    fn estimate(
        &mut self,
        match_id: i64,
        key: &str,
        format: &str,
        cards: &[(u32, u32)],
        now_ms: i64,
    ) -> Option<ArchetypeEstimate> {
        match self.estimates.get(key, now_ms) {
            Lookup::Known(answer) => answer,
            Lookup::Waiting => self.shown_estimate(match_id, now_ms),
            Lookup::Unknown => {
                // A set asked before whose answer expired asks again; a new
                // one waits until enough has been revealed (#502).
                if let Some(last) = self.estimate_asks.get_mut(&match_id)
                    && last.key != key
                    && !overlay::estimate_due(&last.cards, cards, now_ms - last.at_ms)
                {
                    last.held_back = overlay::new_cards(&last.cards, cards) > 0;
                    return self.shown_estimate(match_id, now_ms);
                }
                // Offline nothing is asked (0.x "never calls the API
                // estimator when stats sharing is switched off").
                if self.online() && !cards.is_empty() {
                    let shown = self.answered_key(match_id, now_ms);
                    // An ask whose answer has expired is done with (its
                    // match is over, or asks afresh).
                    self.estimate_asks
                        .retain(|_, asked| now_ms - asked.at_ms < ESTIMATE_TTL_MS);
                    self.estimates.ask(key, now_ms);
                    self.estimate_asks.insert(
                        match_id,
                        AskedEstimate {
                            at_ms: now_ms,
                            cards: cards.to_vec(),
                            key: key.to_string(),
                            shown,
                            held_back: false,
                        },
                    );
                    self.submit(GameRequest::Estimate {
                        key: key.to_string(),
                        format: format.to_string(),
                        cards: cards.to_vec(),
                    });
                }
                self.shown_estimate(match_id, now_ms)
            }
        }
    }

    fn decklists(&mut self, uuid: &str, now_ms: i64) -> Option<Vec<ArchetypeDeck>> {
        let port = self.port.as_ref()?;
        if let Some(decks) = port.decklists(uuid, now_ms) {
            return Some(decks);
        }
        let asked_recently = self
            .decklists_asked
            .get(uuid)
            .is_some_and(|at| now_ms - at < DECKLISTS_TTL_MS);
        if port.online() && !asked_recently {
            self.decklists_asked.insert(uuid.to_string(), now_ms);
            self.submit(GameRequest::Decklists {
                uuid: uuid.to_string(),
            });
        }
        None
    }

    fn metagame_name(
        &mut self,
        format_key: &str,
        uuid: &str,
        now_ms: i64,
    ) -> Option<(String, Option<String>)> {
        let port = self.port.as_ref()?;
        let Some(shares) = port.metagame(format_key, now_ms) else {
            let asked_recently = self
                .metagame_asked
                .get(format_key)
                .is_some_and(|at| now_ms - at < DECKLISTS_TTL_MS);
            if port.online() && !asked_recently {
                self.metagame_asked.insert(format_key.to_string(), now_ms);
                self.submit(GameRequest::Metagame {
                    format_key: format_key.to_string(),
                });
            }
            return None;
        };
        shares
            .archetypes
            .into_iter()
            .find(|share| share.uuid == uuid)
            .map(|share| (share.name, share.color_identity))
    }

    fn unreachable(&self) -> bool {
        // Offline nothing is asked, so a failure from before says nothing.
        self.unreachable && self.online()
    }

    fn ensure_stub(
        &mut self,
        uuid: &str,
        name: &str,
        format_key: &str,
        color_identity: Option<&str>,
    ) {
        if !self.stubbed.insert(uuid.to_string()) {
            return;
        }
        self.submit(GameRequest::Stub {
            uuid: uuid.to_string(),
            name: name.to_string(),
            format_key: format_key.to_string(),
            color_identity: color_identity.map(str::to_string),
        });
    }
}

/// The lookups of `GameNet` that never ask the network: the kept candidates
/// and live reads are shared, everything that would send a request, or only
/// holds an answer another source has to ask for, answers nothing. The
/// league card resolves the opponent with it when the game overlay has not
/// (spec 9, the Versus card's opponent name).
pub struct LocalOnly<'a>(pub &'a mut GameNet);

impl GameLookups for LocalOnly<'_> {
    fn online(&self) -> bool {
        false
    }

    fn live_read(
        &mut self,
        key: &str,
        now_ms: i64,
        compute: &mut dyn FnMut() -> Option<LiveEstimate>,
    ) -> Option<LiveEstimate> {
        self.0.live_read(key, now_ms, compute)
    }

    fn held_candidates(
        &mut self,
        format_key: &str,
        stamp: (i64, i64),
        load: &mut dyn FnMut() -> Result<Vec<Candidate>, StoreError>,
    ) -> Result<Arc<Vec<Candidate>>, StoreError> {
        self.0.held_candidates(format_key, stamp, load)
    }

    fn scouting(&mut self, _username: &str, _format_key: &str, _now_ms: i64) -> Option<Scouting> {
        None
    }

    fn estimate(
        &mut self,
        _match_id: i64,
        _key: &str,
        _format: &str,
        _cards: &[(u32, u32)],
        _now_ms: i64,
    ) -> Option<ArchetypeEstimate> {
        None
    }

    fn decklists(&mut self, _uuid: &str, _now_ms: i64) -> Option<Vec<ArchetypeDeck>> {
        None
    }

    fn metagame_name(
        &mut self,
        _format_key: &str,
        _uuid: &str,
        _now_ms: i64,
    ) -> Option<(String, Option<String>)> {
        None
    }

    fn unreachable(&self) -> bool {
        false
    }

    fn ensure_stub(
        &mut self,
        _uuid: &str,
        _name: &str,
        _format_key: &str,
        _color_identity: Option<&str>,
    ) {
    }
}

// ---- The requests ----------------------------------------------------------

/// The opponent's scouting (0.x `FetchOpponentScouting`): a 2xx or a 404 is
/// an answer (kept an hour); anything else, or no connection, a failure.
pub fn scout(api: &ApiState, username: &str, format_key: &str, now_ms: i64) -> GameAnswer {
    let request = mymtgo_api::players_request(username, format_key);
    let (answer, failed) = match api.send(&request, now_ms) {
        Ok(response) if response.is_success() || response.status == 404 => {
            (mymtgo_api::parse_players(&response), false)
        }
        Ok(response) => {
            tracing::debug!(status = response.status, "the opponent scouting failed");
            (None, true)
        }
        Err(error) => {
            tracing::debug!(%error, "the opponent scouting failed");
            (None, true)
        }
    };
    GameAnswer::Scouted {
        username: username.to_string(),
        format_key: format_key.to_string(),
        answer,
        failed,
    }
}

/// The API's estimate of the revealed cards (Q6's client; offline it sends
/// nothing). A failure is kept like an answer (0.x).
pub fn estimate(
    api: &ApiState,
    key: &str,
    format: &str,
    cards: &[(u32, u32)],
    now_ms: i64,
) -> GameAnswer {
    // The overlay's cards are MTGO ids (Arena has no overlay estimate).
    let cards: Vec<mymtgo_api::EstimateCard> = cards
        .iter()
        .map(|&(id, quantity)| mymtgo_api::EstimateCard {
            id: mymtgo_api::EstimateCardId::Mtgo(id),
            quantity,
        })
        .collect();
    let (answer, failed) =
        match crate::api::reference::estimate_archetype(api, format, &cards, now_ms) {
            mymtgo_api::EstimateOutcome::Answered(estimate) => (Some(estimate), false),
            mymtgo_api::EstimateOutcome::NoArchetype => (None, false),
            mymtgo_api::EstimateOutcome::Refused(reason) => {
                tracing::debug!(%reason, "the overlay's archetype estimate was refused");
                (None, false)
            }
            mymtgo_api::EstimateOutcome::Failed(reason) => {
                tracing::debug!(%reason, "the overlay's archetype estimate failed");
                (None, true)
            }
        };
    GameAnswer::Estimated {
        key: key.to_string(),
        answer,
        failed,
    }
}

/// The app's port: the gate from `ApiState`, Plan T's caches, requests on
/// the `overlay-net` thread.
pub struct AppGamePort {
    app: AppHandle,
    net: Sender<NetSignal>,
}

impl AppGamePort {
    pub fn new(app: AppHandle, net: Sender<NetSignal>) -> AppGamePort {
        AppGamePort { app, net }
    }
}

impl GamePort for AppGamePort {
    fn online(&self) -> bool {
        self.app
            .try_state::<ApiState>()
            .is_some_and(|api| !api.offline_mode().0)
    }

    fn submit(&mut self, request: GameRequest) {
        let app = self.app.clone();
        let job = NetJob {
            key: request.key(),
            run: Box::new(move || run_in_app(&app, request)),
        };
        if self.net.send(NetSignal::Job(job)).is_err() {
            tracing::warn!("the overlay network thread has stopped");
        }
    }

    fn decklists(&self, uuid: &str, now_ms: i64) -> Option<Vec<ArchetypeDeck>> {
        let caches = self.app.try_state::<ArchetypeCaches>()?;
        match lock(&caches.decklists).get(uuid, now_ms) {
            Cached::Fresh(decks) => Some(decks),
            _ => None,
        }
    }

    fn metagame(&self, format_key: &str, now_ms: i64) -> Option<MetagameShares> {
        let slug = mymtgo_api::format_slug(format_key)?;
        let caches = self.app.try_state::<ArchetypeCaches>()?;
        match lock(&caches.metagame).get(&metagame_key(slug, PICKER_DAYS), now_ms) {
            Cached::Fresh(shares) => Some(shares),
            _ => None,
        }
    }
}

/// One request in the running app (on the `overlay-net` thread).
fn run_in_app(app: &AppHandle, request: GameRequest) -> Option<OverlayMsg> {
    let now_ms = crate::wall_clock_ms();
    match request {
        GameRequest::Scout {
            username,
            format_key,
        } => {
            let api = app.try_state::<ApiState>()?;
            Some(OverlayMsg::GameAnswered(scout(
                &api,
                &username,
                &format_key,
                now_ms,
            )))
        }
        GameRequest::Estimate { key, format, cards } => {
            let api = app.try_state::<ApiState>()?;
            Some(OverlayMsg::GameAnswered(estimate(
                &api, &key, &format, &cards, now_ms,
            )))
        }
        GameRequest::Decklists { uuid } => {
            // Fetched (and cached) on the archetypes worker, paced with its
            // other requests; this thread waits for it.
            let worker = app.try_state::<crate::archetypes::worker::ArchetypesWorker>()?;
            let (reply, answer) = mpsc::channel();
            if !worker.signal(ArchetypeSignal::FetchDecklists {
                uuid: uuid.clone(),
                reply,
            }) {
                return None;
            }
            if let Ok(Err(error)) = answer.recv() {
                tracing::debug!(%error, "the overlay's decklists could not be fetched");
            }
            Some(OverlayMsg::GameAnswered(GameAnswer::Decklists { uuid }))
        }
        GameRequest::Metagame { format_key } => {
            // As the picker asks it: the cache's `begin` guards a second fetch.
            let slug = mymtgo_api::format_slug(&format_key)?;
            let caches = app.try_state::<ArchetypeCaches>()?;
            let cache_key = metagame_key(slug, PICKER_DAYS);
            {
                let mut cache = lock(&caches.metagame);
                if !matches!(cache.get(&cache_key, now_ms), Cached::Missing) {
                    return None;
                }
                cache.begin(&cache_key);
            }
            crate::archetypes::worker::signal(
                app,
                ArchetypeSignal::FetchMetagame {
                    slug: slug.to_string(),
                    days: PICKER_DAYS,
                },
            );
            None
        }
        GameRequest::Stub {
            uuid,
            name,
            format_key,
            color_identity,
        } => {
            let startup = app.state::<crate::commands::Startup>();
            let state = crate::commands::ready(&startup).ok()?;
            let edit = ArchetypeEdit::EnsureStub {
                uuid,
                name,
                format: format_key,
                color_identity,
                now_ms,
            };
            match crate::matches::archetype_editor(state).map(|editor| editor.edit(edit)) {
                Ok(Ok(_)) => {
                    crate::archetypes::worker::signal(app, ArchetypeSignal::FetchDefinitions)
                }
                Ok(Err(error)) => tracing::warn!(%error, "could not store the archetype stub"),
                Err(error) => tracing::warn!(?error, "could not store the archetype stub"),
            }
            // `ArchetypesChanged` follows from the tracker.
            None
        }
    }
}

/// A port for tests: what was asked, and what the caches hold.
#[cfg(test)]
pub mod testing {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// The port's state, shared with the test.
    #[derive(Debug, Default)]
    pub struct PortState {
        pub online: bool,
        pub requests: Vec<GameRequest>,
        pub decklists: BTreeMap<String, Vec<ArchetypeDeck>>,
        pub metagame: Option<MetagameShares>,
    }

    /// Records every request; answers from `PortState`.
    #[derive(Clone, Default)]
    pub struct FakePort(pub Arc<Mutex<PortState>>);

    impl FakePort {
        pub fn online() -> FakePort {
            let port = FakePort::default();
            lock(&port.0).online = true;
            port
        }

        pub fn requests(&self) -> Vec<GameRequest> {
            lock(&self.0).requests.clone()
        }

        /// A `GameNet` asking through this port.
        pub fn net(&self) -> GameNet {
            GameNet::new(Some(Box::new(self.clone())))
        }
    }

    impl GamePort for FakePort {
        fn online(&self) -> bool {
            lock(&self.0).online
        }

        fn submit(&mut self, request: GameRequest) {
            lock(&self.0).requests.push(request);
        }

        fn decklists(&self, uuid: &str, _: i64) -> Option<Vec<ArchetypeDeck>> {
            lock(&self.0).decklists.get(uuid).cloned()
        }

        fn metagame(&self, _: &str, _: i64) -> Option<MetagameShares> {
            lock(&self.0).metagame.clone()
        }
    }
}
