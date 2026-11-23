//! Archetype lookups for #9's Metagame design (survey section 9, 07 spec
//! section 9, ruling 11): the estimate, one archetype's definition list and
//! its real decklists. Class `Lookup`: sent with the device key or bearer,
//! never offline. 0.x's whole-catalog download and the version probe are
//! not ported. The definitions check (archetypes spec sections 5 and 16.3)
//! replaces them: one call answers which held archetypes changed, moved or
//! retired. `GET /api/archetypes` is read again, but only for the picker,
//! one format at a time, and nothing of it is stored until picked (#489).

use archetypes::Definition;
use serde::Serialize;
use serde_json::Value;

use crate::error::ApiError;
use crate::gate::RequestClass;
use crate::loose;
use crate::php::urlencode;
use crate::request::{ApiRequest, ApiResponse, AuthMode, Body, Method};

pub const ESTIMATE_PATH: &str = "/api/archetypes/estimate";
pub const DEFINITIONS_CHECK_PATH: &str = "/api/archetypes/definitions/check";
/// The server's archetype list, which the picker offers (#489).
pub const CATALOG_PATH: &str = "/api/archetypes";
/// The most uuids one check may ask about (the API
/// `CheckArchetypeDefinitionsRequest`, `max:500`).
pub const DEFINITIONS_PER_CHECK: usize = 500;

/// A decklist download failed (0.x `DownloadArchetypeDecklist`).
pub const DECKLIST_FAILED: &str = "Failed to download decklist from API.";

/// `POST /api/archetypes/estimate` with `{"format": "<raw match format>",
/// "cards": [{"mtgo_id", "arena_id" or "name", "quantity"}]}`.
pub fn estimate_request(format: &str, cards: &[EstimateCard]) -> ApiRequest {
    let mut request = ApiRequest::new(
        Method::Post,
        ESTIMATE_PATH,
        RequestClass::Lookup,
        AuthMode::DeviceOrBearer,
    );
    let body = EstimateBody {
        format: format.to_string(),
        cards: cards
            .iter()
            .map(|card| match &card.id {
                EstimateCardId::Mtgo(mtgo_id) => EstimateWireCard::Mtgo {
                    mtgo_id: *mtgo_id,
                    quantity: card.quantity,
                },
                EstimateCardId::Arena(arena_id) => EstimateWireCard::Arena {
                    arena_id: *arena_id,
                    quantity: card.quantity,
                },
                EstimateCardId::Name(name) => EstimateWireCard::Name {
                    name: name.clone(),
                    quantity: card.quantity,
                },
            })
            .collect(),
    };
    request.body = Body::json(&body);
    request
}

/// The estimate body, its keys in 0.x's order.
#[derive(Serialize)]
struct EstimateBody {
    format: String,
    cards: Vec<EstimateWireCard>,
}

/// One card's id as the server knows it. An Endstep card has no id of its
/// own, so it goes by its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EstimateCardId {
    Mtgo(u32),
    Arena(u32),
    Name(String),
}

/// One card of an estimate request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EstimateCard {
    pub id: EstimateCardId,
    pub quantity: u32,
}

/// A card entry on the wire: the id key first, then the quantity.
#[derive(Serialize)]
#[serde(untagged)]
enum EstimateWireCard {
    Mtgo { mtgo_id: u32, quantity: u32 },
    Arena { arena_id: u32, quantity: u32 },
    Name { name: String, quantity: u32 },
}

/// The API's best archetype for a deck.
#[derive(Debug, Clone, PartialEq)]
pub struct ArchetypeEstimate {
    pub uuid: String,
    /// 1.0 exact, 0.5 partial (the API's `confidenceFloat`).
    pub confidence: f64,
    pub deck_version_uuid: Option<String>,
}

/// What an estimate request came to. Detection treats them apart: an
/// answer is written; "no archetype" and "refused" fall back to a confident
/// local answer or Homebrew; a failure writes nothing so the match is asked
/// about again later.
#[derive(Debug, Clone, PartialEq)]
pub enum EstimateOutcome {
    /// The server named an archetype (the first of its answers).
    Answered(ArchetypeEstimate),
    /// A 200 with an empty list: the server knows no archetype for it.
    NoArchetype,
    /// The server refused these cards (400, 413, 422) or answered without
    /// a uuid: asking again would get the same, so it counts as "no
    /// archetype". The reason is for the log.
    Refused(String),
    /// No answer: the request failed, the status was not 200 (401, 403,
    /// 429, 5xx, a firewall block) or the body was not an estimate list.
    /// The reason is for the log.
    Failed(String),
}

/// The statuses that refuse the request itself, the same on every try.
const REFUSED_STATUSES: [u16; 3] = [400, 413, 422];

/// The first answer of a 200 estimate (0.x `$response->ok()`), a 200 `[]`
/// as "no archetype", a refusal of the cards as refused, anything else as
/// failed.
pub fn parse_estimate(response: &ApiResponse) -> EstimateOutcome {
    if REFUSED_STATUSES.contains(&response.status) {
        return EstimateOutcome::Refused(format!("status {}", response.status));
    }
    if response.status != 200 {
        return EstimateOutcome::Failed(format!("status {}", response.status));
    }
    let not_a_list = || EstimateOutcome::Failed("the answer is not an estimate list".to_string());
    let Ok(body) = response.json::<Value>() else {
        return not_a_list();
    };
    let Some(answers) = body.as_array() else {
        return not_a_list();
    };
    let Some(first) = answers.first() else {
        return EstimateOutcome::NoArchetype;
    };
    match loose::text(first, "uuid") {
        Some(uuid) => EstimateOutcome::Answered(ArchetypeEstimate {
            uuid,
            confidence: loose::number(first, "confidence").unwrap_or(0.0),
            deck_version_uuid: loose::text(first, "deck_version_uuid"),
        }),
        None => EstimateOutcome::Refused("the first answer has no uuid".to_string()),
    }
}

/// `GET /api/archetypes/{uuid}/decklist`: one archetype's definition list.
pub fn decklist_request(uuid: &str) -> ApiRequest {
    archetype_get(uuid, "decklist")
}

/// `GET /api/archetypes/{uuid}/decklists`: its most-seen real decklists.
pub fn decklists_request(uuid: &str) -> ApiRequest {
    archetype_get(uuid, "decklists")
}

fn archetype_get(uuid: &str, what: &str) -> ApiRequest {
    let path = format!("/api/archetypes/{}/{what}", urlencode(uuid));
    ApiRequest::new(
        Method::Get,
        &path,
        RequestClass::Lookup,
        AuthMode::DeviceOrBearer,
    )
}

/// `GET /api/archetypes?format=<key>&active=1` (the API
/// `ListArchetypesController`): the format's active archetypes, which the
/// picker offers besides the held ones (catalog picker addendum, #489).
pub fn catalog_request(format_key: &str) -> ApiRequest {
    let mut request = ApiRequest::new(
        Method::Get,
        CATALOG_PATH,
        RequestClass::Lookup,
        AuthMode::DeviceOrBearer,
    );
    request.query = vec![
        ("format".to_string(), format_key.to_string()),
        ("active".to_string(), "1".to_string()),
    ];
    request
}

/// `GET /api/archetypes?active=1`: every format's active archetypes, for
/// the offline archetype download (the player's button, device key only).
pub fn offline_catalog_request() -> ApiRequest {
    let mut request = ApiRequest::new(
        Method::Get,
        CATALOG_PATH,
        RequestClass::OfflineDownload,
        AuthMode::Device,
    );
    request.query = vec![("active".to_string(), "1".to_string())];
    request
}

/// One archetype of the server's list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogArchetype {
    pub uuid: String,
    pub name: String,
    /// `U,B` as the server stores it; `None` when it has none.
    pub color_identity: Option<String>,
}

/// Reads the server's list. A non-2xx is `Status`; an answer that is not a
/// list is `Malformed`; an entry without a uuid or a name is dropped. The
/// colours are read from `colorIdentity` (laravel-data's name) or
/// `color_identity`.
pub fn parse_catalog(response: &ApiResponse) -> Result<Vec<CatalogArchetype>, ApiError> {
    if !response.is_success() {
        return Err(response.status_error());
    }
    let body: Value = response.json()?;
    let Value::Array(rows) = body else {
        return Err(ApiError::Malformed(
            "the archetype list is not a list".to_string(),
        ));
    };
    let mut catalog = Vec::new();
    for row in &rows {
        let Some(uuid) = loose::text(row, "uuid").filter(|uuid| !uuid.is_empty()) else {
            continue;
        };
        let Some(name) = loose::text(row, "name").filter(|name| !name.is_empty()) else {
            continue;
        };
        let color_identity = loose::text(row, "colorIdentity")
            .or_else(|| loose::text(row, "color_identity"))
            .filter(|colors| !colors.is_empty());
        catalog.push(CatalogArchetype {
            uuid,
            name,
            color_identity,
        });
    }
    Ok(catalog)
}

/// One card of a decklist. Only cards with an MTGO id and an oracle id are
/// kept (0.x skipped the rest; the API sends `mtgo_id` 0 for a card it has no
/// id for).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecklistCard {
    pub mtgo_id: u32,
    pub oracle_id: String,
    pub name: Option<String>,
    /// JSON `type`.
    pub card_type: Option<String>,
    pub color_identity: Option<String>,
    pub image: Option<String>,
    pub quantity: u32,
    pub sideboard: bool,
}

/// One real decklist of an archetype.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchetypeDeck {
    pub uuid: String,
    pub seen_count: u32,
    pub cards: Vec<DecklistCard>,
}

/// The `cards` of a 2xx `/decklist`. A non-2xx is `Status` with 0.x's
/// message.
pub fn parse_decklist(response: &ApiResponse) -> Result<Vec<DecklistCard>, ApiError> {
    let body = decklist_body(response)?;
    Ok(cards_of(&body))
}

/// The `decks` of a 2xx `/decklists` (a deck without a uuid is skipped). A
/// non-2xx is `Status` with 0.x's message.
pub fn parse_decklists(response: &ApiResponse) -> Result<Vec<ArchetypeDeck>, ApiError> {
    let body = decklist_body(response)?;
    let Some(Value::Array(decks)) = body.get("decks") else {
        return Ok(Vec::new());
    };
    let mut parsed = Vec::new();
    for deck in decks {
        let Some(uuid) = loose::text(deck, "uuid") else {
            continue;
        };
        let seen_count = loose::whole(deck, "seen_count")
            .and_then(|count| u32::try_from(count).ok())
            .unwrap_or(0);
        parsed.push(ArchetypeDeck {
            uuid,
            seen_count,
            cards: cards_of(deck),
        });
    }
    Ok(parsed)
}

fn decklist_body(response: &ApiResponse) -> Result<Value, ApiError> {
    if !response.is_success() {
        return Err(ApiError::Status(
            response.status,
            DECKLIST_FAILED.to_string(),
        ));
    }
    response.json()
}

/// The usable cards under `cards` (0.x's `empty()` on both ids).
fn cards_of(holder: &Value) -> Vec<DecklistCard> {
    let Some(Value::Array(cards)) = holder.get("cards") else {
        return Vec::new();
    };
    let mut parsed = Vec::new();
    for card in cards {
        let mtgo_id = loose::whole(card, "mtgo_id")
            .and_then(|id| u32::try_from(id).ok())
            .filter(|id| *id != 0);
        let oracle_id = loose::text(card, "oracle_id").filter(|id| !id.is_empty() && id != "0");
        let (Some(mtgo_id), Some(oracle_id)) = (mtgo_id, oracle_id) else {
            continue;
        };
        parsed.push(DecklistCard {
            mtgo_id,
            oracle_id,
            name: loose::text(card, "name"),
            card_type: loose::text(card, "type"),
            color_identity: loose::text(card, "color_identity"),
            image: loose::text(card, "image"),
            quantity: loose::whole(card, "quantity")
                .and_then(|quantity| u32::try_from(quantity).ok())
                .unwrap_or(0),
            sideboard: loose::truthy(card, "sideboard"),
        });
    }
    parsed
}

/// `POST /api/archetypes/definitions/check` for held archetypes as `(uuid,
/// version)`, the version `None` for one never downloaded. One request per
/// `DEFINITIONS_PER_CHECK` uuids, in the order given; none for an empty
/// list. `Lookup`, 30 s, 16 MiB answers.
pub fn definitions_check_request(versions: &[(String, Option<String>)]) -> Vec<ApiRequest> {
    check_requests(versions, RequestClass::Lookup, AuthMode::DeviceOrBearer)
}

/// The check for the offline archetype download: every uuid with version
/// `null`, so each answer is the full definition. Device key only.
pub fn offline_definitions_check_request(uuids: &[String]) -> Vec<ApiRequest> {
    let versions: Vec<(String, Option<String>)> =
        uuids.iter().map(|uuid| (uuid.clone(), None)).collect();
    check_requests(&versions, RequestClass::OfflineDownload, AuthMode::Device)
}

fn check_requests(
    versions: &[(String, Option<String>)],
    class: RequestClass,
    auth: AuthMode,
) -> Vec<ApiRequest> {
    let mut requests = Vec::new();
    for chunk in versions.chunks(DEFINITIONS_PER_CHECK) {
        let mut request = ApiRequest::new(Method::Post, DEFINITIONS_CHECK_PATH, class, auth);
        let body = CheckBody {
            archetypes: chunk
                .iter()
                .map(|(uuid, version)| CheckEntry {
                    uuid: uuid.clone(),
                    version: version.clone(),
                })
                .collect(),
        };
        request.body = Body::json(&body);
        requests.push(request);
    }
    requests
}

/// The check body, keys as spec 16.3.
#[derive(Serialize)]
struct CheckBody {
    archetypes: Vec<CheckEntry>,
}

#[derive(Serialize)]
struct CheckEntry {
    uuid: String,
    version: Option<String>,
}

/// The check's answer (spec 16.3), the `archetypes` crate's type so its
/// change plan reads it directly.
pub use archetypes::DefinitionsCheck;

/// Reads a 2xx check answer. A non-2xx is `Status` with the body (a 404 is
/// "not deployed", see `is_not_deployed`); an answer that is not an object
/// of lists is `Malformed`. Entries that cannot be used (a definition
/// without a uuid or of the wrong shape, a move without both uuids, a
/// retired uuid that is no text) are dropped and the rest still applies:
/// an archetype left out is simply asked about again at the next check.
pub fn parse_definitions_check(response: &ApiResponse) -> Result<DefinitionsCheck, ApiError> {
    parse_definitions_check_counted(response).map(|(check, _)| check)
}

/// `parse_definitions_check` with how many entries were dropped, so the
/// caller can say so (#300 F12).
pub fn parse_definitions_check_counted(
    response: &ApiResponse,
) -> Result<(DefinitionsCheck, u32), ApiError> {
    if !response.is_success() {
        return Err(response.status_error());
    }
    let body: Value = response.json()?;
    let Value::Object(_) = body else {
        return Err(ApiError::Malformed(
            "the definitions check answer is not an object".to_string(),
        ));
    };
    let mut check = DefinitionsCheck::default();
    let mut dropped = 0;
    for entry in list_of(&body, "changed")? {
        match definition_of(entry) {
            Some(definition) => check.changed.push(definition),
            None => dropped += 1,
        }
    }
    for entry in list_of(&body, "moved")? {
        let from = loose::text(entry, "uuid").filter(|uuid| !uuid.is_empty());
        let to = loose::text(entry, "to").filter(|uuid| !uuid.is_empty());
        match (from, to) {
            (Some(from), Some(to)) => check.moved.push((from, to)),
            _ => dropped += 1,
        }
    }
    for entry in list_of(&body, "retired")? {
        match entry {
            Value::String(uuid) if !uuid.is_empty() => check.retired.push(uuid.clone()),
            _ => dropped += 1,
        }
    }
    Ok((check, dropped))
}

/// The answers of every chunk joined in order, each definition (by uuid),
/// move (by its old uuid) and retirement once, the first kept (#300 F12).
pub fn join_definitions_checks(checks: Vec<DefinitionsCheck>) -> DefinitionsCheck {
    let mut joined = DefinitionsCheck::default();
    for check in checks {
        for definition in check.changed {
            if !joined
                .changed
                .iter()
                .any(|held| held.uuid == definition.uuid)
            {
                joined.changed.push(definition);
            }
        }
        for moved in check.moved {
            if !joined.moved.iter().any(|held| held.0 == moved.0) {
                joined.moved.push(moved);
            }
        }
        for uuid in check.retired {
            if !joined.retired.contains(&uuid) {
                joined.retired.push(uuid);
            }
        }
    }
    joined
}

/// Whether a check failed because the server does not have the endpoint
/// yet (spec 5: the worker falls back to `/decklist`).
pub fn is_not_deployed(error: &ApiError) -> bool {
    matches!(error, ApiError::Status(404, _))
}

/// The list under `key`: empty when missing or null, `Malformed` when it
/// is anything but a list.
fn list_of<'a>(body: &'a Value, key: &str) -> Result<&'a [Value], ApiError> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(items)) => Ok(items),
        Some(_) => Err(ApiError::Malformed(format!(
            "the definitions check's {key} is not a list"
        ))),
    }
}

/// One definition, or `None` when it has no uuid or the wrong shape.
fn definition_of(entry: &Value) -> Option<Definition> {
    let definition: Definition = serde_json::from_value(entry.clone()).ok()?;
    if definition.uuid.is_empty() {
        return None;
    }
    Some(definition)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::RequestClass;
    use crate::request::{AuthMode, Method};
    use crate::test_support::response;

    // The offline archetype download (2026-10-27 offline download design).
    #[test]
    fn the_offline_catalog_asks_every_format() {
        let request = offline_catalog_request();
        assert_eq!(request.method, Method::Get);
        assert_eq!(request.path, "/api/archetypes");
        assert_eq!(request.query, vec![("active".to_string(), "1".to_string())]);
        assert_eq!(request.class, RequestClass::OfflineDownload);
        assert_eq!(request.auth, AuthMode::Device);
    }

    #[test]
    fn the_offline_check_asks_full_definitions_in_chunks() {
        let uuids: Vec<String> = (0..501).map(|n| format!("q-{n}")).collect();
        let requests = offline_definitions_check_request(&uuids);
        assert_eq!(requests.len(), 2);
        for request in &requests {
            assert_eq!(request.path, "/api/archetypes/definitions/check");
            assert_eq!(request.class, RequestClass::OfflineDownload);
            assert_eq!(request.auth, AuthMode::Device);
            let body: Value = serde_json::from_slice(request.body.bytes()).expect("json");
            for entry in body["archetypes"].as_array().expect("list") {
                assert!(entry["version"].is_null());
            }
        }
    }

    // #489: the picker's server catalog.
    #[test]
    fn the_catalog_request_asks_for_one_formats_active_archetypes() {
        let request = catalog_request("modern");
        assert_eq!(request.method, Method::Get);
        assert_eq!(request.path, "/api/archetypes");
        assert_eq!(request.class, RequestClass::Lookup);
        assert_eq!(request.auth, AuthMode::DeviceOrBearer);
        assert_eq!(
            request.query,
            vec![
                ("format".to_string(), "modern".to_string()),
                ("active".to_string(), "1".to_string())
            ]
        );
    }

    #[test]
    fn the_catalog_reads_uuids_names_and_colours() {
        let body = r#"[
            {"uuid":"q-oculus","name":"Qoculus","format":"modern","colorIdentity":"U,B","super":null,"is_active":true},
            {"uuid":"q-reanimator","name":"Qreanimator","format":"modern","color_identity":"U,B,R"},
            {"uuid":"","name":"Qnouuid"},
            {"uuid":"q-noname"},
            "junk"
        ]"#;
        let catalog = parse_catalog(&response(200, body)).expect("catalog");
        assert_eq!(
            catalog,
            vec![
                CatalogArchetype {
                    uuid: "q-oculus".to_string(),
                    name: "Qoculus".to_string(),
                    color_identity: Some("U,B".to_string()),
                },
                CatalogArchetype {
                    uuid: "q-reanimator".to_string(),
                    name: "Qreanimator".to_string(),
                    color_identity: Some("U,B,R".to_string()),
                },
            ]
        );
    }

    #[test]
    fn a_catalog_that_is_not_a_list_or_not_2xx_is_an_error() {
        assert!(matches!(
            parse_catalog(&response(200, "{}")),
            Err(ApiError::Malformed(_))
        ));
        assert!(matches!(
            parse_catalog(&response(500, "[]")),
            Err(ApiError::Status(500, _))
        ));
    }

    fn card(mtgo_id: u32, oracle_id: &str, quantity: u32, sideboard: bool) -> DecklistCard {
        DecklistCard {
            mtgo_id,
            oracle_id: oracle_id.to_string(),
            name: Some("Qcard".to_string()),
            card_type: Some("Instant".to_string()),
            color_identity: Some("W".to_string()),
            image: Some("http://x".to_string()),
            quantity,
            sideboard,
        }
    }

    // 0.x ArchetypeFloorTest "estimateViaApi still calls the endpoint when
    // online": the request it sends.
    #[test]
    fn the_estimate_request_posts_the_format_and_cards() {
        let request = estimate_request(
            "CMODERN",
            &[
                EstimateCard {
                    id: EstimateCardId::Mtgo(12_345),
                    quantity: 4,
                },
                EstimateCard {
                    id: EstimateCardId::Mtgo(67_890),
                    quantity: 2,
                },
            ],
        );
        assert_eq!(request.method, Method::Post);
        assert_eq!(request.path, "/api/archetypes/estimate");
        assert_eq!(request.class, RequestClass::Lookup);
        assert_eq!(request.auth, AuthMode::DeviceOrBearer);
        assert_eq!(
            request.body.bytes(),
            br#"{"format":"CMODERN","cards":[{"mtgo_id":12345,"quantity":4},{"mtgo_id":67890,"quantity":2}]}"#
                .as_slice()
        );
    }

    #[test]
    fn the_estimate_request_posts_arena_ids() {
        let request = estimate_request(
            "CPIONEER",
            &[
                EstimateCard {
                    id: EstimateCardId::Mtgo(12_345),
                    quantity: 4,
                },
                EstimateCard {
                    id: EstimateCardId::Arena(87_654),
                    quantity: 2,
                },
            ],
        );
        assert_eq!(
            request.body.bytes(),
            br#"{"format":"CPIONEER","cards":[{"mtgo_id":12345,"quantity":4},{"arena_id":87654,"quantity":2}]}"#
                .as_slice()
        );
    }

    #[test]
    fn the_estimate_request_posts_endstep_cards_by_name() {
        let request = estimate_request(
            "Modern",
            &[EstimateCard {
                id: EstimateCardId::Name("Qend \"Card\"".to_string()),
                quantity: 4,
            }],
        );
        assert_eq!(
            request.body.bytes(),
            br#"{"format":"Modern","cards":[{"name":"Qend \"Card\"","quantity":4}]}"#.as_slice()
        );
    }

    #[test]
    fn the_estimate_is_the_first_answer_of_a_200() {
        let body = r#"[{"uuid":"q-arch","confidence":0.5,"deck_version_uuid":"q-deck"},
                       {"uuid":"q-other","confidence":1.0}]"#;
        assert_eq!(
            parse_estimate(&response(200, body)),
            EstimateOutcome::Answered(ArchetypeEstimate {
                uuid: "q-arch".to_string(),
                confidence: 0.5,
                deck_version_uuid: Some("q-deck".to_string()),
            })
        );
        let exact = r#"[{"uuid":"q-arch","confidence":1,"deck_version_uuid":null}]"#;
        assert_eq!(
            parse_estimate(&response(200, exact)),
            EstimateOutcome::Answered(ArchetypeEstimate {
                uuid: "q-arch".to_string(),
                confidence: 1.0,
                deck_version_uuid: None,
            })
        );
    }

    // A 200 `[]` is the server's "no archetype"; anything else that is not
    // an answer failed, so detection asks again later instead of taking a
    // weak local guess for good.
    #[test]
    fn only_an_empty_200_list_is_no_archetype() {
        assert_eq!(
            parse_estimate(&response(200, "[]")),
            EstimateOutcome::NoArchetype
        );
        let body = r#"[{"uuid":"q-arch","confidence":1}]"#;
        for (failed, reason) in [
            (response(200, "{}"), "the answer is not an estimate list"),
            (
                response(200, "not json"),
                "the answer is not an estimate list",
            ),
            // 0.x `$response->ok()`: only a 200 counts.
            (response(201, body), "status 201"),
            (response(401, body), "status 401"),
            (response(403, body), "status 403"),
            (response(429, body), "status 429"),
            (response(500, body), "status 500"),
        ] {
            assert_eq!(
                parse_estimate(&failed),
                EstimateOutcome::Failed(reason.to_string())
            );
        }
    }

    // The server refusing these cards, or answering without a uuid, would
    // say the same when asked again: "refused", which detection takes as
    // "no archetype" so one match cannot stay due for good.
    #[test]
    fn a_refused_estimate_is_not_asked_again() {
        let body = r#"{"message":"The cards field is required."}"#;
        for (refused, reason) in [
            (response(400, body), "status 400"),
            (response(413, body), "status 413"),
            (response(422, body), "status 422"),
            (
                response(200, r#"[{"confidence":1}]"#),
                "the first answer has no uuid",
            ),
        ] {
            assert_eq!(
                parse_estimate(&refused),
                EstimateOutcome::Refused(reason.to_string())
            );
        }
    }

    #[test]
    fn the_decklist_requests_get_the_archetype() {
        let one = decklist_request("q-arch-uuid");
        assert_eq!(one.method, Method::Get);
        assert_eq!(one.path, "/api/archetypes/q-arch-uuid/decklist");
        assert_eq!(one.class, RequestClass::Lookup);
        assert_eq!(one.auth, AuthMode::DeviceOrBearer);
        assert_eq!(one.body_limit, 16 * 1024 * 1024);
        let many = decklists_request("q-arch-uuid");
        assert_eq!(many.method, Method::Get);
        assert_eq!(many.path, "/api/archetypes/q-arch-uuid/decklists");
        assert_eq!(many.class, RequestClass::Lookup);
        let odd = decklists_request("q/../x");
        assert_eq!(odd.path, "/api/archetypes/q%2F..%2Fx/decklists");
    }

    // the API `ArchetypeDecklistController`: the latest deck version's cards,
    // `mtgo_id` cast to int (0 when the card has none), `sideboard` a bool.
    #[test]
    fn parse_decklist_reads_the_controller_shape() {
        let body = r#"{"cards":[
            {"mtgo_id":12345,"oracle_id":"oracle-1","name":"Qcard","type":"Instant",
             "color_identity":"W","image":"http://x","quantity":4,"sideboard":false},
            {"mtgo_id":12345,"oracle_id":"oracle-1","name":"Qcard","type":"Instant",
             "color_identity":"W","image":"http://x","quantity":1,"sideboard":true},
            {"mtgo_id":0,"oracle_id":"oracle-2","name":"Qnone","type":"Land",
             "color_identity":null,"image":null,"quantity":2,"sideboard":false}
        ]}"#;
        let cards = parse_decklist(&response(200, body)).expect("parsed");
        assert_eq!(
            cards,
            vec![
                card(12_345, "oracle-1", 4, false),
                card(12_345, "oracle-1", 1, true),
            ],
            "a card without an MTGO id is left out"
        );
        assert_eq!(
            parse_decklist(&response(200, r#"{"cards":[]}"#)).expect("empty"),
            Vec::new()
        );
    }

    // 0.x DownloadDecklistTest: "creates one archetype_deck per deck in API
    // response" (the parse) and "keeps maindeck and sideboard rows for the
    // same card".
    #[test]
    fn parse_decklists_reads_every_deck() {
        let body = r#"{"uuid":"arch-uuid","name":"Qarch","format":"modern","decks":[
            {"uuid":"deck-uuid-1","seen_count":5,"cards":[
                {"mtgo_id":"100","oracle_id":"oracle-1","name":"Qcard","type":"Instant",
                 "color_identity":"W","image":"http://x","quantity":4,"sideboard":false}]},
            {"uuid":"deck-uuid-2","seen_count":3,"cards":[
                {"mtgo_id":"100","oracle_id":"oracle-1","name":"Qcard","type":"Instant",
                 "color_identity":"W","image":"http://x","quantity":3,"sideboard":true},
                {"mtgo_id":"100","oracle_id":"oracle-1","name":"Qcard","type":"Instant",
                 "color_identity":"W","image":"http://x","quantity":1,"sideboard":false}]}
        ]}"#;
        let decks = parse_decklists(&response(200, body)).expect("parsed");
        assert_eq!(
            decks,
            vec![
                ArchetypeDeck {
                    uuid: "deck-uuid-1".to_string(),
                    seen_count: 5,
                    cards: vec![card(100, "oracle-1", 4, false)],
                },
                ArchetypeDeck {
                    uuid: "deck-uuid-2".to_string(),
                    seen_count: 3,
                    cards: vec![
                        card(100, "oracle-1", 3, true),
                        card(100, "oracle-1", 1, false)
                    ],
                },
            ]
        );
    }

    // 0.x DownloadDecklistTest: "skips cards with empty mtgo_id without
    // nulling existing card rows" (the parse leaves them out).
    #[test]
    fn cards_without_an_mtgo_or_oracle_id_are_left_out() {
        let body = r#"{"decks":[{"uuid":"deck-uuid","seen_count":1,"cards":[
            {"mtgo_id":null,"oracle_id":"oracle-keep","name":"Qkeep","type":"Instant",
             "color_identity":null,"image":null,"quantity":4,"sideboard":false},
            {"mtgo_id":"","oracle_id":"oracle-keep","quantity":4,"sideboard":false},
            {"mtgo_id":"5","oracle_id":null,"quantity":4,"sideboard":false},
            {"mtgo_id":"5","oracle_id":"","quantity":4,"sideboard":false}
        ]}]}"#;
        let decks = parse_decklists(&response(200, body)).expect("parsed");
        assert_eq!(decks.len(), 1);
        assert!(decks[0].cards.is_empty());
        let missing = r#"{"decks":[{"uuid":"q","cards":[
            {"mtgo_id":7,"oracle_id":"o-7","quantity":"2","sideboard":1}]}]}"#;
        let decks = parse_decklists(&response(200, missing)).expect("parsed");
        assert_eq!(decks[0].seen_count, 0);
        assert_eq!(decks[0].cards[0].quantity, 2);
        assert!(decks[0].cards[0].sideboard);
    }

    // 0.x DownloadDecklistTest: "throws when API returns non-2xx".
    #[test]
    fn a_failed_download_is_the_0x_error() {
        for parse in [
            |r: &ApiResponse| parse_decklists(r).map(|_| ()),
            |r: &ApiResponse| parse_decklist(r).map(|_| ()),
        ] {
            assert_eq!(
                parse(&response(500, "")),
                Err(ApiError::Status(500, DECKLIST_FAILED.to_string()))
            );
        }
        assert_eq!(DECKLIST_FAILED, "Failed to download decklist from API.");
        assert!(
            parse_decklists(&response(200, "{}"))
                .expect("none")
                .is_empty()
        );
        assert!(matches!(
            parse_decklists(&response(200, "[1")),
            Err(ApiError::Malformed(_))
        ));
    }

    fn versions(count: usize) -> Vec<(String, Option<String>)> {
        (0..count)
            .map(|index| {
                let version = if index % 2 == 0 {
                    Some(format!("q-version-{index}"))
                } else {
                    None
                };
                (format!("q-uuid-{index:04}"), version)
            })
            .collect()
    }

    // Spec 16.3: `{"archetypes": [{"uuid", "version"}]}`, a null version
    // for a row never downloaded.
    #[test]
    fn the_definitions_check_posts_every_uuid_and_version() {
        let requests = definitions_check_request(&[
            (
                "6f1c2e4a-0000-4000-8000-000000000001".to_string(),
                Some("3b5d".to_string()),
            ),
            ("6f1c2e4a-0000-4000-8000-000000000002".to_string(), None),
        ]);
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.method, Method::Post);
        assert_eq!(request.path, "/api/archetypes/definitions/check");
        assert!(request.query.is_empty());
        assert_eq!(request.class, RequestClass::Lookup);
        assert_eq!(request.auth, AuthMode::DeviceOrBearer);
        assert_eq!(request.timeout_ms, 30_000);
        assert_eq!(request.body_limit, 16 * 1024 * 1024);
        assert_eq!(
            request.body.bytes(),
            br#"{"archetypes":[{"uuid":"6f1c2e4a-0000-4000-8000-000000000001","version":"3b5d"},{"uuid":"6f1c2e4a-0000-4000-8000-000000000002","version":null}]}"#
                .as_slice()
        );
    }

    // the API `CheckArchetypeDefinitionsRequest`: at most 500 per request.
    #[test]
    fn the_definitions_check_is_sent_in_chunks_of_500() {
        assert!(definitions_check_request(&[]).is_empty(), "nothing to ask");
        assert_eq!(definitions_check_request(&versions(500)).len(), 1);
        let requests = definitions_check_request(&versions(1_001));
        assert_eq!(requests.len(), 3);
        let mut seen = Vec::new();
        for request in &requests {
            let body: Value = serde_json::from_slice(request.body.bytes()).expect("json");
            let entries = body["archetypes"].as_array().expect("a list");
            assert!(entries.len() <= DEFINITIONS_PER_CHECK);
            for entry in entries {
                seen.push(entry["uuid"].as_str().expect("uuid").to_string());
            }
        }
        assert_eq!(seen.len(), 1_001);
        assert_eq!(seen[0], "q-uuid-0000");
        assert_eq!(seen[500], "q-uuid-0500");
        assert_eq!(seen[1_000], "q-uuid-1000");
        let first: Value = serde_json::from_slice(requests[0].body.bytes()).expect("json");
        assert_eq!(first["archetypes"][0]["version"], "q-version-0");
        assert_eq!(first["archetypes"][1]["version"], Value::Null);
    }

    const RUBY_STORM: &str = r#"{
        "uuid": "6f1c2e4a-0000-4000-8000-000000000001",
        "name": "Ruby Storm",
        "format": "modern",
        "color_identity": "R",
        "version": "9c0f",
        "rank": [2, 1, 4, -17, 1, 2, 0],
        "conditions": [
            {"type": "InMainboard", "cards": ["Ral, Monsoon Mage"]},
            {"type": "OneOrMoreInMainboard", "cards": ["Ruby Medallion", "Past in Flames"]}
        ],
        "fallback": false,
        "common_cards": [],
        "min_overlap": 0.1,
        "colour_fork": "41:Base",
        "cards": [{"mtgo_id": 12345, "oracle_id": "q-oracle-1", "name": "Ral, Monsoon Mage",
                   "quantity": 4, "sideboard": false}]
    }"#;

    // Spec 16.3: a moved uuid whose target is in `changed` (rule 3 answers
    // the target as if asked with version null), and one whose target is
    // retired, so its definition is not in `changed`.
    #[test]
    fn parse_definitions_check_reads_every_answer_kind() {
        let body = format!(
            r#"{{"changed": [{RUBY_STORM}],
                "moved": [
                    {{"uuid": "6f1c2e4a-0000-4000-8000-000000000003",
                      "to": "6f1c2e4a-0000-4000-8000-000000000001"}},
                    {{"uuid": "6f1c2e4a-0000-4000-8000-000000000005",
                      "to": "6f1c2e4a-0000-4000-8000-000000000004"}}
                ],
                "retired": ["6f1c2e4a-0000-4000-8000-000000000004"]}}"#
        );
        let check = parse_definitions_check(&response(200, &body)).expect("parsed");
        assert_eq!(check.changed.len(), 1);
        let storm = &check.changed[0];
        assert_eq!(storm.uuid, "6f1c2e4a-0000-4000-8000-000000000001");
        assert_eq!(storm.name, "Ruby Storm");
        assert_eq!(storm.format.as_deref(), Some("modern"));
        assert_eq!(storm.color_identity.as_deref(), Some("R"));
        assert_eq!(storm.version.as_deref(), Some("9c0f"));
        assert_eq!(storm.rank, vec![2.0, 1.0, 4.0, -17.0, 1.0, 2.0, 0.0]);
        assert_eq!(storm.conditions.len(), 2);
        assert_eq!(
            storm.conditions[1].kind,
            archetypes::ConditionKind::OneOrMoreInMainboard
        );
        assert_eq!(storm.colour_fork.as_deref(), Some("41:Base"));
        assert_eq!(storm.cards.len(), 1);
        assert_eq!(storm.cards[0].mtgo_id, 12_345);
        assert_eq!(storm.cards[0].quantity, 4);
        assert_eq!(
            check.moved,
            vec![
                (
                    "6f1c2e4a-0000-4000-8000-000000000003".to_string(),
                    "6f1c2e4a-0000-4000-8000-000000000001".to_string()
                ),
                (
                    "6f1c2e4a-0000-4000-8000-000000000005".to_string(),
                    "6f1c2e4a-0000-4000-8000-000000000004".to_string()
                ),
            ]
        );
        let targets: Vec<&str> = check.moved.iter().map(|(_, to)| to.as_str()).collect();
        assert!(
            targets.contains(&storm.uuid.as_str()),
            "a target in changed"
        );
        assert_eq!(check.retired, vec!["6f1c2e4a-0000-4000-8000-000000000004"]);
        // the API: an empty list answers three empty lists.
        let empty = r#"{"changed": [], "moved": [], "retired": []}"#;
        assert_eq!(
            parse_definitions_check(&response(200, empty)).expect("empty"),
            DefinitionsCheck::default()
        );
    }

    // Spec 16.3: an archetype without a definition on the server, and the
    // `/decklist` fallback, give a definition with only `cards`.
    #[test]
    fn a_definition_with_only_cards_parses() {
        let body = r#"{"changed": [{"uuid": "q-arch", "cards": [
            {"mtgo_id": 7, "oracle_id": "q-oracle-7", "name": "Qcard", "quantity": 2,
             "sideboard": true}]}]}"#;
        let check = parse_definitions_check(&response(200, body)).expect("parsed");
        let only = &check.changed[0];
        assert_eq!(only.uuid, "q-arch");
        assert_eq!(only.version, None);
        assert!(only.rank.is_empty() && only.conditions.is_empty());
        assert!(!only.fallback);
        assert_eq!(only.min_overlap, 0.1);
        assert_eq!(only.cards[0].mtgo_id, 7);
        assert!(only.cards[0].sideboard);
        assert!(check.moved.is_empty() && check.retired.is_empty());
    }

    // A changed entry without a uuid, a moved entry without a target and a
    // retired entry that is no text are dropped; the rest still applies.
    #[test]
    fn unusable_entries_are_dropped() {
        let body = r#"{"changed": [{"name": "Qnameless"}, {"uuid": ""}, 7,
                                  {"uuid": "q-bad-rank", "rank": "high"},
                                  {"uuid": "q-good"}],
                       "moved": [{"uuid": "q-old"}, {"to": "q-new"}, "q",
                                 {"uuid": "q-old-2", "to": "q-new-2"}],
                       "retired": [null, 5, "q-gone"]}"#;
        let check = parse_definitions_check(&response(200, body)).expect("parsed");
        let uuids: Vec<&str> = check.changed.iter().map(|d| d.uuid.as_str()).collect();
        assert_eq!(uuids, vec!["q-good"]);
        assert_eq!(
            check.moved,
            vec![("q-old-2".to_string(), "q-new-2".to_string())]
        );
        assert_eq!(check.retired, vec!["q-gone"]);
    }

    /// #300 F12: the dropped entries are counted, so the caller can say so.
    #[test]
    fn dropped_entries_are_counted() {
        let body = r#"{"changed": [{"name": "Qnameless"}, {"uuid": "q-good"}],
                       "moved": [{"uuid": "q-old"}],
                       "retired": [null, "q-gone"]}"#;
        let (check, dropped) =
            parse_definitions_check_counted(&response(200, body)).expect("parsed");
        assert_eq!(dropped, 3);
        assert_eq!(check.changed.len(), 1);
        let (_, none) = parse_definitions_check_counted(&response(200, "{}")).expect("empty");
        assert_eq!(none, 0);
    }

    /// #300 F12: chunks joined name each definition, move and retirement
    /// once (the first), so one archetype is not applied twice.
    #[test]
    fn joined_chunks_name_each_entry_once() {
        let definition = |uuid: &str, name: &str| Definition {
            uuid: uuid.to_string(),
            name: name.to_string(),
            ..Definition::default()
        };
        let first = DefinitionsCheck {
            changed: vec![definition("q-target", "Qfirst")],
            moved: vec![("q-a".to_string(), "q-target".to_string())],
            retired: vec!["q-gone".to_string()],
        };
        let second = DefinitionsCheck {
            changed: vec![
                definition("q-target", "Qsecond"),
                definition("q-other", "Qother"),
            ],
            moved: vec![
                ("q-a".to_string(), "q-target".to_string()),
                ("q-b".to_string(), "q-target".to_string()),
            ],
            retired: vec!["q-gone".to_string()],
        };
        let joined = join_definitions_checks(vec![first, second]);
        let names: Vec<&str> = joined.changed.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["Qfirst", "Qother"]);
        assert_eq!(
            joined.moved,
            [
                ("q-a".to_string(), "q-target".to_string()),
                ("q-b".to_string(), "q-target".to_string())
            ]
        );
        assert_eq!(joined.retired, ["q-gone"]);
    }

    #[test]
    fn a_malformed_check_answer_is_a_typed_error() {
        for body in ["[1", "", "[]", r#""changed""#, r#"{"changed": {}}"#] {
            assert!(
                matches!(
                    parse_definitions_check(&response(200, body)),
                    Err(ApiError::Malformed(_))
                ),
                "{body:?}"
            );
        }
        // Missing lists are empty (the server always sends all three).
        assert_eq!(
            parse_definitions_check(&response(200, "{}")).expect("empty"),
            DefinitionsCheck::default()
        );
    }

    // Spec 5: a 404 means the endpoint is not deployed; the worker falls
    // back to `/decklist`. Any other failure is a `Status` too.
    #[test]
    fn a_404_is_read_as_not_deployed() {
        let missing = parse_definitions_check(&response(404, r#"{"message":"Not Found"}"#));
        assert!(matches!(missing, Err(ApiError::Status(404, _))));
        assert!(is_not_deployed(missing.as_ref().expect_err("an error")));
        let invalid = parse_definitions_check(&response(422, r#"{"message":"x"}"#));
        assert!(matches!(invalid, Err(ApiError::Status(422, _))));
        assert!(!is_not_deployed(invalid.as_ref().expect_err("an error")));
        assert!(!is_not_deployed(&ApiError::Offline));
    }
}
