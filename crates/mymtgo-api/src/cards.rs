//! Card identity lookups (survey section 8, 07 spec section 9): `POST
//! /api/cards` and `POST /api/cards/resolve`. Only the client half: #6
//! stores what comes back and decides when to ask.
//!
//! 0.x `PopulateMissingCardData` asked in four passes, each depending on
//! what the one before learned; here each pass is planned (`plan_card_lookups`)
//! and its answers matched (`match_answers`) as plain data, and
//! `run_card_passes` chains the four with any way of sending (the app's
//! client, or a fake in tests). Like a Laravel job split into a pure
//! "what to ask" and "what it means" around one `Http::post`.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value;

use crate::error::ApiError;
use crate::gate::RequestClass;
use crate::loose;
use crate::request::{ApiRequest, ApiResponse, AuthMode, Body, Method};

pub const CARDS_PATH: &str = "/api/cards";
pub const RESOLVE_PATH: &str = "/api/cards/resolve";

/// Catalog ids per id request (0.x `chunk(50)`).
pub const ID_CHUNK: usize = 50;
/// Endstep lookups per request (like the Arena id chunks).
pub const ENDSTEP_CHUNK: usize = 50;
/// Names per by-name request (0.x `chunk(100)`).
pub const NAME_CHUNK: usize = 100;
/// Catalog ids per import request (0.x `PopulateCardsInChunks`, 200).
pub const IMPORT_CHUNK: usize = 200;
/// How far below a back face its front face can sit, in the order tried
/// (0.x `FRONT_FACE_OFFSETS`): two where the set has foil ids, else one.
pub const FRONT_FACE_OFFSETS: [u32; 2] = [2, 1];
/// What a multi-face card's name holds; only such an answer is accepted
/// for a back face.
const FACE_SEPARATOR: &str = " // ";

/// `/api/cards/resolve` failed (0.x `ResolveCardsFromDek`).
pub const RESOLVE_FAILED: &str = "Failed to resolve cards from API.";

/// One Endstep card as the API is asked about it: the name exactly as
/// Endstep sends it and, when known, its printing (set code lower case).
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct EndstepLookup {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub set: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub collector_number: String,
}

/// One `/api/cards` body. `names` is sent only by the by-name pass.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CardsRequest {
    pub ids: Vec<u32>,
    pub tokens: Vec<String>,
    /// Arena ids (a catalog id minus `ARENA_ID_OFFSET`), sent by the id pass
    /// for Arena rows only.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub arena_ids: Vec<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub names: Option<Vec<String>>,
    /// Endstep cards by name and printing, sent by the id pass for Endstep
    /// rows only.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub endstep: Vec<EndstepLookup>,
}

/// `POST /api/cards`: class `Reference` (sent offline too), 15 s.
pub fn cards_request(cards: &CardsRequest) -> ApiRequest {
    let mut request = ApiRequest::new(
        Method::Post,
        CARDS_PATH,
        RequestClass::Reference,
        AuthMode::DeviceOrBearer,
    );
    request.body = Body::json(cards);
    request
}

/// One card the API answered with. Every field is optional and read
/// loosely (the API answers from SQL columns). `color_identity` is
/// normalised as 0.x stored it: a comma list whose empty entries are `C`,
/// `None` when empty.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CardAnswer {
    pub scryfall_id: Option<String>,
    pub oracle_id: Option<String>,
    pub name: Option<String>,
    /// JSON `type`.
    pub card_type: Option<String>,
    pub sub_type: Option<String>,
    pub rarity: Option<String>,
    pub color_identity: Option<String>,
    pub colors: Option<String>,
    pub cmc: Option<f64>,
    /// As the API stores it: `""` for a land, `None` for a card it holds
    /// no cost for (a modal or transforming card).
    pub mana_cost: Option<String>,
    /// Whether the answer had a `mana_cost` key at all, null included. The
    /// API only began sending it later, so this tells "no cost" apart from
    /// "an API that does not send costs yet".
    pub mana_cost_sent: bool,
    pub set_name: Option<String>,
    /// JSON `set`.
    pub set_code: Option<String>,
    pub art_crop: Option<String>,
    pub image: Option<String>,
    pub layout: Option<String>,
    /// The Arena id this answers (Arena answers carry `arena_id`).
    pub arena_id: Option<i64>,
    /// The catalog id this answers, as sent (a number or a string).
    pub value: Option<String>,
    /// The name this answers (by-name and Endstep answers only).
    pub query: Option<String>,
    /// The Endstep printing this answers, as sent: empty when the lookup
    /// had none (or the answer is no Endstep answer).
    pub query_set: String,
    pub query_collector_number: String,
}

impl CardAnswer {
    /// `value` as a whole number, so `"126517"` and `126517` compare equal
    /// (0.x compared with `==`).
    pub fn value_id(&self) -> Option<i64> {
        let value = self.value.as_ref()?;
        loose::whole_of(&Value::String(value.clone()))
    }

    fn answers_id(&self, mtgo_id: u32) -> bool {
        self.value_id() == Some(i64::from(mtgo_id))
    }

    /// Whether this answers an Endstep row: its echo is the row's name and
    /// printing (the set compared lower case, as the desktop stores it).
    fn answers_endstep(&self, card: &UnresolvedCard) -> bool {
        card.name.is_some()
            && self.query == card.name
            && self.query_set.to_lowercase() == card.set_code.to_lowercase()
            && self.query_collector_number == card.collector_number
    }

    fn from_json(object: &Value) -> CardAnswer {
        CardAnswer {
            scryfall_id: loose::text(object, "scryfall_id"),
            oracle_id: loose::text(object, "oracle_id"),
            name: loose::text(object, "name"),
            card_type: loose::text(object, "type"),
            sub_type: loose::text(object, "sub_type"),
            rarity: loose::text(object, "rarity"),
            color_identity: fill_colorless_gaps(loose::text(object, "color_identity").as_deref()),
            colors: loose::text(object, "colors"),
            cmc: loose::number(object, "cmc"),
            mana_cost: loose::text(object, "mana_cost"),
            mana_cost_sent: object.get("mana_cost").is_some(),
            set_name: loose::text(object, "set_name"),
            set_code: loose::text(object, "set"),
            art_crop: loose::text(object, "art_crop"),
            image: loose::text(object, "image"),
            layout: loose::text(object, "layout"),
            arena_id: loose::whole_of(object.get("arena_id").unwrap_or(&Value::Null)),
            value: loose::text(object, "value"),
            query: loose::text(object, "query"),
            query_set: loose::text(object, "query_set").unwrap_or_default(),
            query_collector_number: loose::text(object, "query_collector_number")
                .unwrap_or_default(),
        }
    }
}

/// 0.x's stored colour identity: an empty part is colourless, so `"W,,U"`
/// becomes `"W,C,U"`; nothing or `""` is `None`. The server's order is kept
/// as 0.x stored it: pages show the value through `cards::normalize_colors`
/// (WUBRG), so the stored order never reaches the screen.
pub fn fill_colorless_gaps(identity: Option<&str>) -> Option<String> {
    let identity = identity.filter(|identity| !identity.is_empty())?;
    let parts: Vec<&str> = identity
        .split(',')
        .map(|part| if part.is_empty() { "C" } else { part })
        .collect();
    Some(parts.join(","))
}

/// The answers of a 2xx `/api/cards`: a JSON list of objects (anything
/// else in the list is skipped). A non-2xx is `Status`, a body that is not
/// a list `Malformed`: either way the call answered nothing.
pub fn parse_cards(response: &ApiResponse) -> Result<Vec<CardAnswer>, ApiError> {
    if !response.is_success() {
        return Err(response.status_error());
    }
    let Value::Array(items) = response.json::<Value>()? else {
        return Err(ApiError::Malformed(
            "the cards answer is not a list".to_string(),
        ));
    };
    Ok(items
        .iter()
        .filter(|item| item.is_object())
        .map(CardAnswer::from_json)
        .collect())
}

/// A `cards` row without card data, as the lookup needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedCard {
    pub mtgo_id: u32,
    pub name: Option<String>,
    /// `rarity = 'token'`: asked by catalog id and by name in one call.
    pub is_token: bool,
    /// Which game's id `mtgo_id` is (an Arena row holds the offset id).
    pub platform: CardPlatform,
    /// An Endstep row's printing (`endstep_cards`, set code lower case);
    /// empty when unknown and for every other row.
    pub set_code: String,
    pub collector_number: String,
}

impl UnresolvedCard {
    /// A row whose platform follows from its catalog id.
    pub fn new(mtgo_id: u32, name: Option<String>, is_token: bool) -> UnresolvedCard {
        let id = i64::from(mtgo_id);
        let platform = if cards::grp_id_of(id).is_some() {
            CardPlatform::Arena
        } else if cards::endstep_row_of(id).is_some() {
            CardPlatform::Endstep
        } else {
            CardPlatform::Mtgo
        };
        UnresolvedCard {
            mtgo_id,
            name,
            is_token,
            platform,
            set_code: String::new(),
            collector_number: String::new(),
        }
    }

    /// The row with its Endstep printing.
    pub fn with_printing(mut self, set_code: String, collector_number: String) -> UnresolvedCard {
        self.set_code = set_code;
        self.collector_number = collector_number;
        self
    }

    /// What the API is asked about an Endstep row.
    fn endstep_lookup(&self) -> EndstepLookup {
        EndstepLookup {
            name: self.name.clone().unwrap_or_default(),
            set: self.set_code.clone(),
            collector_number: self.collector_number.clone(),
        }
    }

    /// The Arena id of an Arena row.
    fn arena_id(&self) -> u32 {
        self.mtgo_id.saturating_sub(cards::ARENA_ID_OFFSET)
    }

    fn is_arena(&self) -> bool {
        self.platform == CardPlatform::Arena
    }

    fn is_mtgo(&self) -> bool {
        self.platform == CardPlatform::Mtgo
    }
}

/// Which game a card row belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardPlatform {
    Mtgo,
    Arena,
    /// An Endstep row (`cards::ENDSTEP_ID_OFFSET` and up), asked about by
    /// name and printing in the id pass only.
    Endstep,
}

/// Which of 0.x's lookups a plan is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupPass {
    /// Rows without a Scryfall id: non-tokens by id in chunks of 50, named
    /// tokens in one call by id and name. Ids asked and not answered
    /// become `unknown`.
    Ids,
    /// Unknown ids above `offset`, asked as `id - offset` in chunks of 50;
    /// only a multi-face answer counts.
    BackFaces { offset: u32 },
    /// Unknown named rows, asked by name in chunks of 100.
    ByName,
    /// A 0.x import's nameless rows, by id in chunks of 200.
    Import,
}

/// What one pass looks up: the rows still without data and, for the back
/// face and by-name passes, the ids the id pass asked about and got no
/// answer for (only those may be guessed at).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolved {
    pub pass: LookupPass,
    pub cards: Vec<UnresolvedCard>,
    pub unknown: Vec<u32>,
}

/// A card matched to an answer. The row keeps its own `mtgo_id`; a token's
/// stored colour identity stays when the answer has none (0.x).
#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
    pub mtgo_id: u32,
    pub card: CardAnswer,
    pub is_token: bool,
}

/// What a pass (or every pass) learned: the rows matched, in catalog id
/// order, and the ids the id pass got no answer for that no later pass
/// resolved.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Resolutions {
    pub resolved: Vec<Resolution>,
    pub unknown: Vec<u32>,
}

/// One planned request and the rows it asks about.
struct Planned {
    request: CardsRequest,
    cards: Vec<UnresolvedCard>,
    kind: PlannedKind,
}

#[derive(Clone, Copy)]
enum PlannedKind {
    Regular,
    Tokens,
    Arena,
    Endstep,
    BackFace(u32),
    ByName,
    Import,
}

/// The requests of one pass, in the order they go out.
pub fn plan_card_lookups(unresolved: &Unresolved) -> Vec<CardsRequest> {
    plan(unresolved)
        .into_iter()
        .map(|planned| planned.request)
        .collect()
}

/// Match a pass's answers to its rows: `answers[n]` is what the pass's
/// `n`th planned request got back, `None` when it failed (a non-2xx or no
/// answer), which marks nothing unknown.
pub fn match_answers(unresolved: &Unresolved, answers: &[Option<Vec<CardAnswer>>]) -> Resolutions {
    let mut resolved: BTreeMap<u32, Resolution> = BTreeMap::new();
    let mut unknown: BTreeSet<u32> = BTreeSet::new();
    for (planned, answer) in plan(unresolved).iter().zip(answers) {
        let Some(answer) = answer else {
            continue;
        };
        match_one(planned, answer, &mut resolved, &mut unknown);
    }
    Resolutions {
        resolved: resolved.into_values().collect(),
        unknown: unknown.into_iter().collect(),
    }
}

/// Run 0.x `PopulateMissingCardData`'s passes over `cards`: ids, back faces
/// two then one below, then by name. `send` asks the API (`None` when the
/// call failed).
pub fn run_card_passes(
    cards: &[UnresolvedCard],
    send: &mut dyn FnMut(&CardsRequest) -> Option<Vec<CardAnswer>>,
) -> Resolutions {
    let mut all = Resolutions::default();
    let mut remaining: Vec<UnresolvedCard> = cards.to_vec();
    let ids = Unresolved {
        pass: LookupPass::Ids,
        cards: remaining.clone(),
        unknown: Vec::new(),
    };
    let learned = run_pass(&ids, send);
    let unknown = learned.unknown.clone();
    absorb(&mut all, &mut remaining, learned);
    for offset in FRONT_FACE_OFFSETS {
        let back_faces = Unresolved {
            pass: LookupPass::BackFaces { offset },
            cards: remaining.clone(),
            unknown: unknown.clone(),
        };
        // 0.x stopped the back-face passes at the first one with nothing
        // to ask.
        if plan(&back_faces).is_empty() {
            break;
        }
        let learned = run_pass(&back_faces, send);
        absorb(&mut all, &mut remaining, learned);
    }
    let by_name = Unresolved {
        pass: LookupPass::ByName,
        cards: remaining.clone(),
        unknown: unknown.clone(),
    };
    let learned = run_pass(&by_name, send);
    absorb(&mut all, &mut remaining, learned);
    all.resolved.sort_by_key(|resolution| resolution.mtgo_id);
    // What the id pass got no answer for and no later pass resolved.
    let resolved_ids: BTreeSet<u32> = all
        .resolved
        .iter()
        .map(|resolution| resolution.mtgo_id)
        .collect();
    all.unknown = unknown
        .into_iter()
        .filter(|id| !resolved_ids.contains(id))
        .collect();
    all
}

fn run_pass(
    unresolved: &Unresolved,
    send: &mut dyn FnMut(&CardsRequest) -> Option<Vec<CardAnswer>>,
) -> Resolutions {
    let answers: Vec<Option<Vec<CardAnswer>>> =
        plan_card_lookups(unresolved).iter().map(send).collect();
    match_answers(unresolved, &answers)
}

/// Add a pass's matches and drop the matched rows from the next passes.
fn absorb(all: &mut Resolutions, remaining: &mut Vec<UnresolvedCard>, learned: Resolutions) {
    let matched: BTreeSet<u32> = learned
        .resolved
        .iter()
        .map(|resolution| resolution.mtgo_id)
        .collect();
    remaining.retain(|card| !matched.contains(&card.mtgo_id));
    all.resolved.extend(learned.resolved);
}

fn plan(unresolved: &Unresolved) -> Vec<Planned> {
    let unknown: BTreeSet<u32> = unresolved.unknown.iter().copied().collect();
    match unresolved.pass {
        LookupPass::Ids => plan_ids(&unresolved.cards),
        LookupPass::BackFaces { offset } => {
            let cards: Vec<UnresolvedCard> = unresolved
                .cards
                .iter()
                .filter(|card| {
                    card.is_mtgo() && unknown.contains(&card.mtgo_id) && card.mtgo_id > offset
                })
                .cloned()
                .collect();
            chunked(&cards, ID_CHUNK, PlannedKind::BackFace(offset), |chunk| {
                CardsRequest {
                    ids: unique(chunk.iter().map(|card| card.mtgo_id - offset)),
                    tokens: Vec::new(),
                    arena_ids: Vec::new(),
                    names: None,
                    endstep: Vec::new(),
                }
            })
        }
        LookupPass::ByName => {
            let cards: Vec<UnresolvedCard> = unresolved
                .cards
                .iter()
                .filter(|card| {
                    card.is_mtgo() && unknown.contains(&card.mtgo_id) && card.name.is_some()
                })
                .cloned()
                .collect();
            chunked(&cards, NAME_CHUNK, PlannedKind::ByName, |chunk| {
                CardsRequest {
                    ids: Vec::new(),
                    tokens: Vec::new(),
                    arena_ids: Vec::new(),
                    names: Some(unique(chunk.iter().filter_map(|card| card.name.clone()))),
                    endstep: Vec::new(),
                }
            })
        }
        LookupPass::Import => {
            let cards: Vec<UnresolvedCard> = unresolved
                .cards
                .iter()
                .filter(|card| card.platform != CardPlatform::Endstep)
                .cloned()
                .collect();
            chunked(&cards, IMPORT_CHUNK, PlannedKind::Import, |chunk| {
                CardsRequest {
                    ids: unique(chunk.iter().map(|card| card.mtgo_id)),
                    tokens: Vec::new(),
                    arena_ids: Vec::new(),
                    names: None,
                    endstep: Vec::new(),
                }
            })
        }
    }
}

/// Non-tokens in chunks of 50; named tokens in one call with both their ids
/// and names (a token without a name is in neither, as 0.x). Arena rows go
/// by Arena id, except a named Arena token (an Arena replay's token with no
/// art), which goes with the tokens by name only. Named Endstep rows, tokens
/// too, go by name and printing in chunks of 50.
fn plan_ids(cards: &[UnresolvedCard]) -> Vec<Planned> {
    let regular: Vec<UnresolvedCard> = cards
        .iter()
        .filter(|c| !c.is_token && c.is_mtgo())
        .cloned()
        .collect();
    let tokens: Vec<UnresolvedCard> = cards
        .iter()
        .filter(|card| {
            card.is_token && card.name.is_some() && card.platform != CardPlatform::Endstep
        })
        .cloned()
        .collect();
    let arena: Vec<UnresolvedCard> = cards
        .iter()
        .filter(|c| c.is_arena() && !(c.is_token && c.name.is_some()))
        .cloned()
        .collect();
    let endstep: Vec<UnresolvedCard> = cards
        .iter()
        .filter(|card| card.platform == CardPlatform::Endstep && card.name.is_some())
        .cloned()
        .collect();
    let mut planned = chunked(&regular, ID_CHUNK, PlannedKind::Regular, |chunk| {
        CardsRequest {
            ids: unique(chunk.iter().map(|card| card.mtgo_id)),
            tokens: Vec::new(),
            arena_ids: Vec::new(),
            names: None,
            endstep: Vec::new(),
        }
    });
    planned.extend(chunked(&arena, ID_CHUNK, PlannedKind::Arena, |chunk| {
        CardsRequest {
            ids: Vec::new(),
            tokens: Vec::new(),
            arena_ids: unique(chunk.iter().map(UnresolvedCard::arena_id)),
            names: None,
            endstep: Vec::new(),
        }
    }));
    planned.extend(chunked(
        &endstep,
        ENDSTEP_CHUNK,
        PlannedKind::Endstep,
        |chunk| CardsRequest {
            endstep: unique(chunk.iter().map(UnresolvedCard::endstep_lookup)),
            ..CardsRequest::default()
        },
    ));
    if !tokens.is_empty() {
        planned.push(Planned {
            request: CardsRequest {
                ids: unique(
                    tokens
                        .iter()
                        .filter(|card| card.is_mtgo())
                        .map(|card| card.mtgo_id),
                ),
                tokens: unique(tokens.iter().filter_map(|card| card.name.clone())),
                arena_ids: Vec::new(),
                names: None,
                endstep: Vec::new(),
            },
            cards: tokens,
            kind: PlannedKind::Tokens,
        });
    }
    planned
}

fn chunked(
    cards: &[UnresolvedCard],
    size: usize,
    kind: PlannedKind,
    request: impl Fn(&[UnresolvedCard]) -> CardsRequest,
) -> Vec<Planned> {
    cards
        .chunks(size)
        .map(|chunk| Planned {
            request: request(chunk),
            cards: chunk.to_vec(),
            kind,
        })
        .collect()
}

/// The values in first-seen order without repeats (Laravel's
/// `->unique()->values()`).
fn unique<T: Ord + Clone>(values: impl Iterator<Item = T>) -> Vec<T> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for value in values {
        if seen.insert(value.clone()) {
            out.push(value);
        }
    }
    out
}

fn match_one(
    planned: &Planned,
    answers: &[CardAnswer],
    resolved: &mut BTreeMap<u32, Resolution>,
    unknown: &mut BTreeSet<u32>,
) {
    let mut found = |card: &UnresolvedCard, answer: &CardAnswer| {
        resolved.insert(
            card.mtgo_id,
            Resolution {
                mtgo_id: card.mtgo_id,
                card: answer.clone(),
                is_token: card.is_token,
            },
        );
    };
    match planned.kind {
        PlannedKind::Regular | PlannedKind::Import => {
            for card in &planned.cards {
                match answers
                    .iter()
                    .find(|answer| answer.answers_id(card.mtgo_id))
                {
                    Some(answer) => found(card, answer),
                    None if matches!(planned.kind, PlannedKind::Regular) => {
                        unknown.insert(card.mtgo_id);
                    }
                    None => {}
                }
            }
        }
        PlannedKind::Arena => {
            for card in &planned.cards {
                let wanted = i64::from(card.arena_id());
                match answers
                    .iter()
                    .find(|answer| answer.arena_id == Some(wanted))
                {
                    Some(answer) => found(card, answer),
                    None => {
                        unknown.insert(card.mtgo_id);
                    }
                }
            }
        }
        PlannedKind::Endstep => {
            // Every row with that name and printing gets the answer.
            for card in &planned.cards {
                match answers.iter().find(|answer| answer.answers_endstep(card)) {
                    Some(answer) => found(card, answer),
                    None => {
                        unknown.insert(card.mtgo_id);
                    }
                }
            }
        }
        PlannedKind::Tokens => {
            for card in &planned.cards {
                // An id answer names this exact printing; a name answer
                // (no value, a token layout) only stands in without one.
                let by_id = answers
                    .iter()
                    .find(|answer| answer.answers_id(card.mtgo_id));
                let by_name = || {
                    answers.iter().find(|answer| {
                        answer.value.is_none()
                            && answer.layout.as_deref() == Some("token")
                            && answer.name.is_some()
                            && answer.name == card.name
                    })
                };
                match by_id.or_else(by_name) {
                    Some(answer) => found(card, answer),
                    None => {
                        unknown.insert(card.mtgo_id);
                    }
                }
            }
        }
        PlannedKind::BackFace(offset) => {
            for answer in answers {
                let multi_face = answer
                    .name
                    .as_deref()
                    .is_some_and(|name| name.contains(FACE_SEPARATOR));
                if !multi_face {
                    continue;
                }
                let front = answer.value_id().unwrap_or(0);
                let card = planned
                    .cards
                    .iter()
                    .find(|card| i64::from(card.mtgo_id - offset) == front);
                if let Some(card) = card {
                    found(card, answer);
                }
            }
        }
        PlannedKind::ByName => {
            for answer in answers {
                let Some(query) = &answer.query else {
                    continue;
                };
                // One answer can serve several rows of the same name.
                for card in &planned.cards {
                    if card.name.as_ref() == Some(query) {
                        found(card, answer);
                    }
                }
            }
        }
    }
}

/// `POST /api/cards/resolve` with `{"mtgo_ids": [..]}`: class `Reference`.
pub fn resolve_request(mtgo_ids: &[u32]) -> ApiRequest {
    let mut request = ApiRequest::new(
        Method::Post,
        RESOLVE_PATH,
        RequestClass::Reference,
        AuthMode::DeviceOrBearer,
    );
    let body = serde_json::json!({ "mtgo_ids": mtgo_ids });
    request.body = Body::json(&body);
    request
}

/// One card of a `/api/cards/resolve` answer. Its colour identity is named
/// `identity` there.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedCard {
    pub mtgo_id: u32,
    pub oracle_id: Option<String>,
    pub name: Option<String>,
    /// JSON `type`.
    pub card_type: Option<String>,
    pub identity: Option<String>,
    pub image: Option<String>,
    pub art_crop: Option<String>,
    pub cmc: Option<f64>,
}

/// The `cards` of a resolve answer (a card without a whole `mtgo_id` is
/// skipped). A non-2xx is `Status` with 0.x's message, "Failed to resolve
/// cards from API.".
pub fn parse_resolved(response: &ApiResponse) -> Result<Vec<ResolvedCard>, ApiError> {
    if !response.is_success() {
        return Err(ApiError::Status(
            response.status,
            RESOLVE_FAILED.to_string(),
        ));
    }
    let body: Value = response.json()?;
    let Some(Value::Array(items)) = body.get("cards") else {
        return Ok(Vec::new());
    };
    let mut cards = Vec::new();
    for item in items {
        let Some(mtgo_id) = loose::whole(item, "mtgo_id").and_then(|id| u32::try_from(id).ok())
        else {
            continue;
        };
        cards.push(ResolvedCard {
            mtgo_id,
            oracle_id: loose::text(item, "oracle_id"),
            name: loose::text(item, "name"),
            card_type: loose::text(item, "type"),
            identity: loose::text(item, "identity"),
            image: loose::text(item, "image"),
            art_crop: loose::text(item, "art_crop"),
            cmc: loose::number(item, "cmc"),
        });
    }
    Ok(cards)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::RequestClass;
    use crate::request::{AuthMode, Body, Method};
    use crate::test_support::response;

    fn body_json(request: &ApiRequest) -> serde_json::Value {
        serde_json::from_slice(request.body.bytes()).expect("JSON body")
    }

    fn stub(mtgo_id: u32) -> UnresolvedCard {
        UnresolvedCard::new(mtgo_id, None, false)
    }

    fn named(mtgo_id: u32, name: &str) -> UnresolvedCard {
        UnresolvedCard::new(mtgo_id, Some(name.to_string()), false)
    }

    fn token(mtgo_id: u32, name: &str) -> UnresolvedCard {
        UnresolvedCard::new(mtgo_id, Some(name.to_string()), true)
    }

    /// An answer as the API gives it: `value` the catalog id it answers.
    fn answer(value: Option<&str>, name: &str, scryfall_id: &str) -> CardAnswer {
        CardAnswer {
            scryfall_id: Some(scryfall_id.to_string()),
            oracle_id: Some(format!("oracle-{scryfall_id}")),
            name: Some(name.to_string()),
            value: value.map(str::to_string),
            ..CardAnswer::default()
        }
    }

    /// Runs the passes against a fake API and records what was asked.
    fn run(
        cards: &[UnresolvedCard],
        api: impl Fn(&CardsRequest) -> Option<Vec<CardAnswer>>,
    ) -> (Resolutions, Vec<CardsRequest>) {
        let mut asked = Vec::new();
        let resolutions = run_card_passes(cards, &mut |request| {
            asked.push(request.clone());
            api(request)
        });
        (resolutions, asked)
    }

    fn resolved(resolutions: &Resolutions, mtgo_id: u32) -> Option<&CardAnswer> {
        resolutions
            .resolved
            .iter()
            .find(|resolution| resolution.mtgo_id == mtgo_id)
            .map(|resolution| &resolution.card)
    }

    fn arena_answer(arena_id: i64, name: &str) -> CardAnswer {
        CardAnswer {
            arena_id: Some(arena_id),
            name: Some(name.to_string()),
            scryfall_id: Some(format!("arena-{arena_id}")),
            ..CardAnswer::default()
        }
    }

    #[test]
    fn arena_rows_go_in_arena_ids() {
        let unresolved = Unresolved {
            pass: LookupPass::Ids,
            cards: vec![stub(123), stub(100_091_538)],
            unknown: Vec::new(),
        };
        let requests = plan_card_lookups(&unresolved);
        assert_eq!(requests.len(), 2);
        let json: Vec<String> = requests
            .iter()
            .map(|r| serde_json::to_string(r).expect("json"))
            .collect();
        assert_eq!(json[0], r#"{"ids":[123],"tokens":[]}"#);
        assert_eq!(json[1], r#"{"ids":[],"tokens":[],"arena_ids":[91538]}"#);
    }

    #[test]
    fn arena_answers_match_by_arena_id() {
        let unresolved = Unresolved {
            pass: LookupPass::Ids,
            cards: vec![stub(100_091_538), stub(100_000_007)],
            unknown: Vec::new(),
        };
        let answers = vec![Some(vec![arena_answer(91_538, "Qarena")])];
        let learned = match_answers(&unresolved, &answers);
        assert_eq!(learned.resolved.len(), 1);
        assert_eq!(learned.resolved[0].mtgo_id, 100_091_538);
        assert_eq!(learned.unknown, vec![100_000_007]);
    }

    /// An Arena replay's token with no art is a named token row: it is asked
    /// by name with MTGO's tokens (its Arena id is no MTGO id), and a token
    /// answer by that name resolves it.
    #[test]
    fn arena_tokens_are_asked_by_name_with_the_tokens() {
        let unresolved = Unresolved {
            pass: LookupPass::Ids,
            cards: vec![token(55, "Clue"), token(100_090_001, "Map")],
            unknown: Vec::new(),
        };
        let requests = plan_card_lookups(&unresolved);
        assert_eq!(requests.len(), 1, "{requests:?}");
        assert_eq!(requests[0].ids, vec![55]);
        assert_eq!(
            requests[0].tokens,
            vec!["Clue".to_string(), "Map".to_string()]
        );
        assert!(requests[0].arena_ids.is_empty());
        let map = CardAnswer {
            layout: Some("token".to_string()),
            ..answer(None, "Map", "q-map")
        };
        let learned = match_answers(&unresolved, &[Some(vec![map])]);
        assert_eq!(learned.resolved.len(), 1);
        assert_eq!(learned.resolved[0].mtgo_id, 100_090_001);
        assert!(learned.resolved[0].is_token);
        assert_eq!(learned.unknown, vec![55]);
    }

    #[test]
    fn arena_rows_skip_name_and_back_face_passes() {
        let cards = [named(100_091_538, "Qarena")];
        let mut asked = Vec::new();
        let all = run_card_passes(&cards, &mut |request| {
            asked.push(request.clone());
            Some(Vec::new())
        });
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].arena_ids, vec![91_538]);
        assert!(asked[0].ids.is_empty() && asked[0].names.is_none());
        // Unknown exactly like an unanswered MTGO id: reported once, no panic.
        assert_eq!(all.unknown, vec![100_091_538]);
    }

    /// An Endstep stub with its printing (`endstep_cards`).
    fn endstep(row: u32, name: &str, set_code: &str, number: &str) -> UnresolvedCard {
        named(cards::endstep_catalog_id(row), name)
            .with_printing(set_code.to_string(), number.to_string())
    }

    /// An Endstep answer: the card plus the lookup it answers, echoed.
    fn endstep_answer(name: &str, set: Option<&str>, number: Option<&str>, id: &str) -> CardAnswer {
        let body = serde_json::json!([{
            "scryfall_id": id,
            "name": name,
            "query": name,
            "query_set": set,
            "query_collector_number": number,
        }]);
        parse_cards(&response(200, &body.to_string()))
            .expect("parsed")
            .remove(0)
    }

    #[test]
    fn endstep_rows_are_sent_by_name_and_printing() {
        let printed = endstep(1, "Qend", "mh3", "12");
        assert_eq!(printed.platform, CardPlatform::Endstep);
        let unresolved = Unresolved {
            pass: LookupPass::Ids,
            cards: vec![printed, endstep(2, "Qopp", "", "")],
            unknown: Vec::new(),
        };
        let requests = plan_card_lookups(&unresolved);
        assert_eq!(requests.len(), 1, "{requests:?}");
        assert_eq!(
            serde_json::to_string(&requests[0]).expect("json"),
            r#"{"ids":[],"tokens":[],"endstep":[{"name":"Qend","set":"mh3","collector_number":"12"},{"name":"Qopp"}]}"#
        );
    }

    #[test]
    fn endstep_rows_go_in_chunks_of_fifty_and_skip_the_other_passes() {
        let cards: Vec<UnresolvedCard> = (1..=60)
            .map(|row| endstep(row, &format!("Qcard{row}"), "", ""))
            .chain([UnresolvedCard::new(
                cards::endstep_catalog_id(61),
                Some("Qtok".to_string()),
                true,
            )])
            .collect();
        let (_, asked) = run(&cards, |_| Some(Vec::new()));
        assert_eq!(asked.len(), 2, "no back face, token or by-name request");
        assert_eq!(asked[0].endstep.len(), 50);
        assert_eq!(asked[1].endstep.len(), 11);
        assert!(asked.iter().all(|request| request.ids.is_empty()
            && request.tokens.is_empty()
            && request.names.is_none()));
        let import = Unresolved {
            pass: LookupPass::Import,
            cards,
            unknown: Vec::new(),
        };
        assert!(plan_card_lookups(&import).is_empty());
    }

    #[test]
    fn two_printings_of_one_name_resolve_separately() {
        let cards = [endstep(1, "Qend", "mh3", "12"), endstep(2, "Qend", "", "")];
        let (all, _) = run(&cards, |_| {
            Some(vec![
                endstep_answer("Qend", Some("MH3"), Some("12"), "q-printed"),
                endstep_answer("Qend", None, None, "q-any"),
            ])
        });
        assert!(all.unknown.is_empty());
        assert_eq!(
            resolved(&all, cards[0].mtgo_id).and_then(|card| card.scryfall_id.as_deref()),
            Some("q-printed")
        );
        assert_eq!(
            resolved(&all, cards[1].mtgo_id).and_then(|card| card.scryfall_id.as_deref()),
            Some("q-any")
        );
        // Only its own printing answers a row.
        let (all, _) = run(&cards[1..], |_| {
            Some(vec![endstep_answer(
                "Qend",
                Some("mh3"),
                Some("12"),
                "q-printed",
            )])
        });
        assert!(all.resolved.is_empty());
        assert_eq!(all.unknown, vec![cards[1].mtgo_id]);
    }

    /// A desktop running before the API knows `endstep` gets no rows back:
    /// the stubs stay and are marked unknown like an unanswered MTGO id.
    #[test]
    fn an_api_without_endstep_leaves_stubs() {
        let cards = [endstep(1, "Qend", "mh3", "12"), endstep(2, "Qopp", "", "")];
        let (all, asked) = run(&cards, |_| Some(Vec::new()));
        assert_eq!(asked.len(), 1);
        assert!(all.resolved.is_empty());
        assert_eq!(all.unknown, vec![cards[0].mtgo_id, cards[1].mtgo_id]);
        let mtgo = run(&[stub(7)], |_| Some(Vec::new())).0;
        assert_eq!(mtgo.unknown, vec![7], "the same as an MTGO id");
        // A failed call marks nothing.
        let (all, _) = run(&cards, |_| None);
        assert!(all.resolved.is_empty() && all.unknown.is_empty());
    }

    #[test]
    fn the_cards_request_posts_ids_and_tokens_as_reference() {
        let request = cards_request(&CardsRequest {
            ids: vec![1, 2],
            tokens: vec!["Cat".to_string()],
            arena_ids: Vec::new(),
            names: None,
            endstep: Vec::new(),
        });
        assert_eq!(request.method, Method::Post);
        assert_eq!(request.path, "/api/cards");
        assert_eq!(request.class, RequestClass::Reference);
        assert_eq!(request.auth, AuthMode::DeviceOrBearer);
        assert_eq!(request.timeout_ms, 15_000);
        assert_eq!(request.body_limit, 16 * 1024 * 1024);
        assert!(matches!(request.body, Body::Json(_)));
        assert_eq!(
            request.body.bytes(),
            br#"{"ids":[1,2],"tokens":["Cat"]}"#.as_slice()
        );
        let by_name = cards_request(&CardsRequest {
            ids: Vec::new(),
            tokens: Vec::new(),
            arena_ids: Vec::new(),
            names: Some(vec!["Tear".to_string()]),
            endstep: Vec::new(),
        });
        assert_eq!(
            by_name.body.bytes(),
            br#"{"ids":[],"tokens":[],"names":["Tear"]}"#.as_slice()
        );
    }

    #[test]
    fn an_answer_records_whether_the_api_sent_a_mana_cost_at_all() {
        let body = r#"[
            {"value": 1, "mana_cost": "{R}"},
            {"value": 2, "mana_cost": ""},
            {"value": 3, "mana_cost": null},
            {"value": 4}
        ]"#;
        let answers = parse_cards(&response(200, body)).expect("parsed");
        let sent: Vec<bool> = answers.iter().map(|answer| answer.mana_cost_sent).collect();
        assert_eq!(sent, vec![true, true, true, false]);
        // A land's empty cost stays empty, apart from a cost never stored.
        assert_eq!(answers[1].mana_cost.as_deref(), Some(""));
        assert_eq!(answers[2].mana_cost, None);
    }

    #[test]
    fn answers_are_read_loosely() {
        let body = r#"[
            {"value": "100", "scryfall_id": "s-1", "oracle_id": "o-1", "name": "Qcard",
             "type": "Instant", "sub_type": null, "rarity": "common",
             "color_identity": "W,,U", "colors": "W,U", "cmc": "3.0",
             "mana_cost": "{1}{W}{U}", "set_name": "Qset", "set": "QST",
             "art_crop": "https://example.test/a.jpg", "image": "https://example.test/i.jpg",
             "layout": "normal"},
            {"value": 101, "color_identity": "", "cmc": 2},
            {"query": "Tear", "value": null, "color_identity": ","},
            "not an answer"
        ]"#;
        let answers = parse_cards(&response(200, body)).expect("parsed");
        assert_eq!(answers.len(), 3);
        let first = &answers[0];
        assert_eq!(first.value.as_deref(), Some("100"));
        assert_eq!(first.value_id(), Some(100));
        assert_eq!(first.card_type.as_deref(), Some("Instant"));
        assert_eq!(first.sub_type, None);
        assert_eq!(first.color_identity.as_deref(), Some("W,C,U"));
        assert_eq!(first.cmc, Some(3.0));
        assert_eq!(first.set_code.as_deref(), Some("QST"));
        assert_eq!(first.set_name.as_deref(), Some("Qset"));
        assert_eq!(first.mana_cost.as_deref(), Some("{1}{W}{U}"));
        assert_eq!(first.layout.as_deref(), Some("normal"));
        assert_eq!(first.image.as_deref(), Some("https://example.test/i.jpg"));
        assert_eq!(
            first.art_crop.as_deref(),
            Some("https://example.test/a.jpg")
        );
        assert_eq!(answers[1].value_id(), Some(101));
        assert_eq!(answers[1].color_identity, None, "empty is no identity");
        assert_eq!(answers[1].cmc, Some(2.0));
        assert_eq!(answers[2].value, None);
        assert_eq!(answers[2].query.as_deref(), Some("Tear"));
        assert_eq!(answers[2].query_set, "", "absent reads as empty");
        assert_eq!(answers[2].query_collector_number, "");
        assert_eq!(answers[2].color_identity.as_deref(), Some("C,C"));

        assert!(matches!(
            parse_cards(&response(200, r#"{"message":"Qno"}"#)),
            Err(ApiError::Malformed(_))
        ));
        assert!(matches!(
            parse_cards(&response(429, r#"{"message":"Too Many Attempts."}"#)),
            Err(ApiError::Status(429, _))
        ));
    }

    // 0.x PopulateMissingCardDataTest: "rebuilds the api client per chunk,
    // so a key that expires mid-run still resolves every chunk" (the
    // chunking; the key is the app's, see `reference.rs`).
    #[test]
    fn regular_cards_are_asked_in_chunks_of_fifty_with_no_tokens() {
        let cards: Vec<UnresolvedCard> = (1..=60).map(stub).collect();
        let (resolutions, asked) = run(&cards, |request| {
            Some(
                request
                    .ids
                    .iter()
                    .map(|id| answer(Some(&id.to_string()), &format!("Card {id}"), "s"))
                    .collect(),
            )
        });
        assert_eq!(asked.len(), 2);
        assert_eq!(asked[0].ids, (1..=50).collect::<Vec<u32>>());
        assert_eq!(asked[1].ids, (51..=60).collect::<Vec<u32>>());
        assert!(asked.iter().all(|request| request.tokens.is_empty()));
        assert!(asked.iter().all(|request| request.names.is_none()));
        assert_eq!(resolutions.resolved.len(), 60);
        assert!(resolutions.unknown.is_empty());
    }

    // 0.x PopulateMissingCardDataTest: "resolves a back face by retrying the
    // front face catalog id, which sits two below it".
    #[test]
    fn resolves_a_back_face_by_retrying_the_front_face_two_below() {
        let (resolutions, asked) = run(&[stub(126_519)], |request| {
            Some(if request.ids.contains(&126_517) {
                vec![answer(
                    Some("126517"),
                    "Boggart Trawler // Boggart Bog",
                    "scryfall-boggart",
                )]
            } else {
                Vec::new()
            })
        });
        let card = resolved(&resolutions, 126_519).expect("resolved");
        assert_eq!(card.scryfall_id.as_deref(), Some("scryfall-boggart"));
        assert_eq!(card.name.as_deref(), Some("Boggart Trawler // Boggart Bog"));
        assert_eq!(asked[1].ids, vec![126_517]);
        assert!(asked[1].tokens.is_empty());
    }

    // 0.x: "leaves a card alone when the id two below it is a different
    // single-faced card".
    #[test]
    fn leaves_a_card_alone_when_two_below_is_a_single_faced_card() {
        let (resolutions, _) = run(&[stub(16_156)], |request| {
            Some(if request.ids.contains(&16_154) {
                vec![answer(Some("16154"), "Qsingle", "scryfall-single")]
            } else {
                Vec::new()
            })
        });
        assert_eq!(resolved(&resolutions, 16_156), None);
    }

    // 0.x: "does not guess a neighbouring back face for a card whose own
    // lookup failed".
    #[test]
    fn does_not_guess_a_back_face_for_a_card_whose_own_lookup_failed() {
        let (resolutions, asked) = run(&[named(61_460, "Qevolution")], |request| {
            if request.ids.contains(&61_460) {
                return None;
            }
            Some(if request.ids.contains(&61_458) {
                vec![answer(Some("61458"), "Qfront // Qback", "scryfall-captive")]
            } else {
                Vec::new()
            })
        });
        assert_eq!(resolved(&resolutions, 61_460), None);
        assert!(resolutions.unknown.is_empty(), "unanswered, not unknown");
        assert_eq!(asked.len(), 1, "{asked:?}");
        assert!(asked.iter().all(|request| request.names.is_none()));
    }

    // 0.x: "fetches cards missing a scryfall id even when every card already
    // has a name".
    #[test]
    fn fetches_a_named_card_missing_its_scryfall_id() {
        let (resolutions, asked) = run(&[named(12_345, "Qbolt")], |_| {
            Some(vec![answer(Some("12345"), "Qbolt", "scryfall-bolt")])
        });
        assert_eq!(asked[0].ids, vec![12_345]);
        assert_eq!(
            resolved(&resolutions, 12_345).and_then(|card| card.scryfall_id.as_deref()),
            Some("scryfall-bolt")
        );
    }

    // 0.x: "resolves a card the api only knows by name, such as a split card
    // half".
    #[test]
    fn resolves_a_card_the_api_only_knows_by_name() {
        let (resolutions, asked) = run(&[named(48_556, "Tear")], |request| {
            let wanted = request
                .names
                .as_ref()
                .is_some_and(|names| names.contains(&"Tear".to_string()));
            Some(if wanted {
                let mut found = answer(None, "Wear // Tear", "scryfall-weartear");
                found.query = Some("Tear".to_string());
                vec![found]
            } else {
                Vec::new()
            })
        });
        let card = resolved(&resolutions, 48_556).expect("resolved");
        assert_eq!(card.name.as_deref(), Some("Wear // Tear"));
        let by_name = asked.last().expect("asked by name");
        assert!(by_name.ids.is_empty() && by_name.tokens.is_empty());
        assert_eq!(by_name.names, Some(vec!["Tear".to_string()]));
    }

    // 0.x: "does not ask by name for a card that has no name".
    #[test]
    fn does_not_ask_by_name_for_a_card_that_has_no_name() {
        let (_, asked) = run(&[stub(99_999)], |_| Some(Vec::new()));
        assert!(asked.iter().all(|request| request.names.is_none()));
        // The id, then two back-face guesses.
        assert_eq!(asked.len(), 3);
        assert_eq!(asked[1].ids, vec![99_997]);
        assert_eq!(asked[2].ids, vec![99_998]);
    }

    // 0.x: "finds a front face one below when the printing has no foil id
    // between them".
    #[test]
    fn finds_a_front_face_one_below() {
        let (resolutions, _) = run(&[stub(154_069)], |request| {
            Some(if request.ids.contains(&154_067) {
                vec![answer(Some("154067"), "Qsingle Riddler", "scryfall-riddle")]
            } else if request.ids.contains(&154_068) {
                vec![answer(
                    Some("154068"),
                    "Qslinker // Qmeal",
                    "scryfall-slinker",
                )]
            } else {
                Vec::new()
            })
        });
        let card = resolved(&resolutions, 154_069).expect("resolved");
        assert_eq!(card.name.as_deref(), Some("Qslinker // Qmeal"));
    }

    // 0.x: "prefers the front face two below when both neighbours are
    // multi-face".
    #[test]
    fn prefers_the_front_face_two_below() {
        let (resolutions, asked) = run(&[stub(126_519)], |request| {
            Some(if request.ids.contains(&126_517) {
                vec![answer(Some("126517"), "Qa // Qb", "scryfall-nonfoil")]
            } else if request.ids.contains(&126_518) {
                vec![answer(Some("126518"), "Qc // Qd", "scryfall-foil")]
            } else {
                Vec::new()
            })
        });
        assert_eq!(
            resolved(&resolutions, 126_519).and_then(|card| card.scryfall_id.as_deref()),
            Some("scryfall-nonfoil")
        );
        assert_eq!(asked.len(), 2, "one below is never asked");
    }

    // 0.x: "resolves a token by its catalog id ahead of its name".
    #[test]
    fn resolves_a_token_by_its_catalog_id_ahead_of_its_name() {
        let (resolutions, asked) = run(&[token(125_873, "Cat")], |request| {
            let mut answers = Vec::new();
            if request.ids.contains(&125_873) {
                let mut by_id = answer(Some("125873"), "Cat", "mh3-cat");
                by_id.layout = Some("token".to_string());
                answers.push(by_id);
            }
            if request.tokens.contains(&"Cat".to_string()) {
                let mut by_name = answer(None, "Cat", "green-cat");
                by_name.layout = Some("token".to_string());
                answers.push(by_name);
            }
            Some(answers)
        });
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].ids, vec![125_873]);
        assert_eq!(asked[0].tokens, vec!["Cat".to_string()]);
        let found = &resolutions.resolved[0];
        assert_eq!(found.card.scryfall_id.as_deref(), Some("mh3-cat"));
        assert!(found.is_token);
    }

    // `unknown` is what is still unknown after every pass: an id the id pass
    // got no answer for but a later pass resolved is not unknown.
    #[test]
    fn an_id_a_later_pass_resolved_is_not_left_unknown() {
        let (resolutions, _) = run(&[named(48_556, "Tear"), stub(7)], |request| {
            let wanted = request
                .names
                .as_ref()
                .is_some_and(|names| names.contains(&"Tear".to_string()));
            Some(if wanted {
                let mut found = answer(None, "Wear // Tear", "scryfall-weartear");
                found.query = Some("Tear".to_string());
                vec![found]
            } else {
                Vec::new()
            })
        });
        assert!(resolved(&resolutions, 48_556).is_some());
        assert_eq!(resolutions.unknown, vec![7], "only the one still unknown");
    }

    // 0.x: "still resolves a token by name when its catalog id is unknown".
    #[test]
    fn still_resolves_a_token_by_name_when_its_catalog_id_is_unknown() {
        let (resolutions, _) = run(&[token(999_001, "Cat")], |request| {
            Some(if request.tokens.contains(&"Cat".to_string()) {
                let mut by_name = answer(None, "Cat", "green-cat");
                by_name.layout = Some("token".to_string());
                vec![by_name]
            } else {
                Vec::new()
            })
        });
        assert_eq!(
            resolved(&resolutions, 999_001).and_then(|card| card.scryfall_id.as_deref()),
            Some("green-cat")
        );
    }

    #[test]
    fn a_token_name_answer_never_stands_in_for_another_ids_answer() {
        // An answer with a value is some card's id answer: it never matches
        // a token row by name. A token row without a name is not asked.
        let cards = [token(5, "Cat"), UnresolvedCard::new(6, None, true)];
        let (resolutions, asked) = run(&cards, |_| {
            let mut other = answer(Some("77"), "Cat", "other-cat");
            other.layout = Some("token".to_string());
            Some(vec![other])
        });
        assert_eq!(asked[0].ids, vec![5]);
        assert_eq!(resolved(&resolutions, 5), None);
        assert_eq!(resolutions.unknown, vec![5]);
    }

    #[test]
    fn the_plan_and_the_match_agree_pass_by_pass() {
        let unresolved = Unresolved {
            pass: LookupPass::Ids,
            cards: vec![stub(10), stub(10), token(20, "Qtoken"), named(30, "Qnamed")],
            unknown: Vec::new(),
        };
        let requests = plan_card_lookups(&unresolved);
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].ids, vec![10, 30], "unique, in order");
        assert_eq!(requests[1].ids, vec![20]);
        let answers = vec![Some(vec![answer(Some("10"), "Qten", "s-10")]), None];
        let matched = match_answers(&unresolved, &answers);
        assert_eq!(matched.resolved.len(), 1);
        assert_eq!(matched.resolved[0].mtgo_id, 10);
        assert_eq!(matched.unknown, vec![30], "a failed call marks nothing");

        let back_faces = Unresolved {
            pass: LookupPass::BackFaces { offset: 2 },
            cards: vec![stub(30), stub(2), stub(40)],
            unknown: vec![30, 2],
        };
        let requests = plan_card_lookups(&back_faces);
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].ids,
            vec![28],
            "only unknown ids above the offset"
        );
    }

    // 0.x PopulateCardsInChunksTest: "populates card data from the API".
    #[test]
    fn imports_ask_in_chunks_of_two_hundred_and_match_by_value() {
        let cards: Vec<UnresolvedCard> = (1..=201).map(stub).collect();
        let unresolved = Unresolved {
            pass: LookupPass::Import,
            cards,
            unknown: Vec::new(),
        };
        let requests = plan_card_lookups(&unresolved);
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].ids.len(), 200);
        assert_eq!(requests[1].ids, vec![201]);
        assert!(requests.iter().all(|request| request.tokens.is_empty()));
        let mut bolt = answer(Some("100"), "Qbolt", "scry-bolt");
        bolt.color_identity = Some("R".to_string());
        let matched = match_answers(&unresolved, &[Some(vec![bolt]), None]);
        assert_eq!(matched.resolved.len(), 1);
        assert_eq!(matched.resolved[0].mtgo_id, 100);
        assert_eq!(matched.resolved[0].card.name.as_deref(), Some("Qbolt"));
        assert!(matched.unknown.is_empty(), "imports mark nothing unknown");
    }

    // 0.x PopulateCardsInChunksTest: "skips when no unpopulated cards exist".
    #[test]
    fn nothing_unresolved_asks_nothing() {
        for pass in [
            LookupPass::Ids,
            LookupPass::BackFaces { offset: 2 },
            LookupPass::ByName,
            LookupPass::Import,
        ] {
            let unresolved = Unresolved {
                pass,
                cards: Vec::new(),
                unknown: Vec::new(),
            };
            assert!(plan_card_lookups(&unresolved).is_empty(), "{pass:?}");
        }
        let (resolutions, asked) = run(&[], |_| panic!("nothing to ask"));
        assert!(asked.is_empty());
        assert!(resolutions.resolved.is_empty());
    }

    // 0.x ResolveCardsFromDekTest: the request and its answer.
    #[test]
    fn the_resolve_request_posts_mtgo_ids_and_reads_identity() {
        let request = resolve_request(&[12_345, 67_890]);
        assert_eq!(request.method, Method::Post);
        assert_eq!(request.path, "/api/cards/resolve");
        assert_eq!(request.class, RequestClass::Reference);
        assert_eq!(request.auth, AuthMode::DeviceOrBearer);
        assert_eq!(
            request.body.bytes(),
            br#"{"mtgo_ids":[12345,67890]}"#.as_slice()
        );
        let body = r#"{"cards": [
            {"mtgo_id": 12345, "oracle_id": "oracle-bolt", "name": "Qbolt", "type": "Instant",
             "image": "https://example.com/bolt.jpg", "art_crop": "https://example.com/c.jpg",
             "cmc": 1, "identity": "R"},
            {"mtgo_id": "11111", "oracle_id": "oracle-land", "name": "Qland",
             "type": "Basic Land", "image": null, "art_crop": null, "cmc": 0.0, "identity": "R"}
        ]}"#;
        let cards = parse_resolved(&response(200, body)).expect("parsed");
        assert_eq!(
            cards[0],
            ResolvedCard {
                mtgo_id: 12_345,
                oracle_id: Some("oracle-bolt".to_string()),
                name: Some("Qbolt".to_string()),
                card_type: Some("Instant".to_string()),
                identity: Some("R".to_string()),
                image: Some("https://example.com/bolt.jpg".to_string()),
                art_crop: Some("https://example.com/c.jpg".to_string()),
                cmc: Some(1.0),
            }
        );
        assert_eq!(cards[1].mtgo_id, 11_111);
        assert_eq!(cards[1].image, None);
        assert_eq!(body_json(&request)["mtgo_ids"][1], 67_890);
    }

    // 0.x ResolveCardsFromDekTest: "throws on API failure".
    #[test]
    fn a_failed_resolve_is_the_0x_error() {
        let error = parse_resolved(&response(500, "[]")).expect_err("failed");
        assert_eq!(error, ApiError::Status(500, RESOLVE_FAILED.to_string()));
        assert_eq!(RESOLVE_FAILED, "Failed to resolve cards from API.");
        assert!(
            parse_resolved(&response(200, "{}"))
                .expect("no cards")
                .is_empty()
        );
    }
}
