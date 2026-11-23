//! Card and archetype lookups as IO (07 spec section 9): the client half
//! #6 and #9 call. Nothing here stores anything or runs on a timer; each
//! function sends through `ApiState::send` (the gate, the device key or
//! bearer) and returns what the pure `mymtgo-api` parsers make of the
//! answer. Blocking: call from the `api` worker or `spawn_blocking`.

use mymtgo_api::{
    ApiError, ApiRequest, ApiResponse, ArchetypeDeck, CardAnswer, CardsRequest, CatalogArchetype,
    DecklistCard, DefinitionsCheck, EstimateCard, EstimateOutcome, LookupPass, MetagameShares,
    Resolutions, ResolvedCard, Unresolved, UnresolvedCard, cards_request, catalog_request,
    decklist_request, decklists_request, definitions_check_request, estimate_request,
    join_definitions_checks, match_answers, metagame_request, parse_cards, parse_catalog,
    parse_decklist, parse_decklists, parse_definitions_check_counted, parse_estimate,
    parse_metagame, parse_resolved, plan_card_lookups, resolve_request, run_card_passes,
};

use crate::api::session::ApiState;
use crate::api::tokens::recover_unauthorized;

/// One `/api/cards` call. Each call builds its auth headers afresh, so a
/// key that expires mid-run is registered again before the next chunk (0.x
/// built a client per chunk for this). A failure is logged and returned;
/// the caller treats it as "answered nothing".
pub fn fetch_cards(
    state: &ApiState,
    cards: &CardsRequest,
    now_ms: i64,
) -> Result<Vec<CardAnswer>, ApiError> {
    let result = state
        .send(&cards_request(cards), now_ms)
        .and_then(|response| parse_cards(&response));
    if let Err(error) = &result {
        tracing::warn!(error = %error, "a card lookup failed");
    }
    result
}

/// What `look_up_cards` learned.
#[derive(Debug, Default)]
pub struct CardsLookup {
    pub resolutions: Resolutions,
    /// A request failed. `resolutions.unknown` is then empty: an id only
    /// counts as unknown to the API when every pass asked about it was
    /// answered (#501).
    pub failed: bool,
}

/// 0.x `PopulateMissingCardData`'s passes over these rows: ids, back faces,
/// then names.
pub fn look_up_cards(state: &ApiState, cards: &[UnresolvedCard], now_ms: i64) -> CardsLookup {
    let mut failed = false;
    let mut resolutions = run_card_passes(cards, &mut |request| {
        let answer = fetch_cards(state, request, now_ms).ok();
        failed |= answer.is_none();
        answer
    });
    if failed {
        resolutions.unknown.clear();
    }
    CardsLookup {
        resolutions,
        failed,
    }
}

/// What looking resolved rows up again for their mana costs learned.
#[derive(Debug, Default)]
pub struct CostLookup {
    pub resolutions: Resolutions,
    /// Whether the API's answers carry a `mana_cost` key (null included):
    /// `None` until an answer with a card in it was seen.
    pub costs_sent: Option<bool>,
    /// A request failed, so the rest of the rows were not asked.
    pub failed: bool,
}

/// `look_up_cards` for rows stored before the API sent mana costs.
/// `costs_sent` is what earlier batches of the run learned; the first
/// answer with a card in it decides it otherwise. Nothing more is asked once
/// the API is seen not to send costs (one request a run until it does), or
/// once a request fails (a throttled or failing API is not asked again until
/// the next run).
pub fn look_up_mana_costs(
    state: &ApiState,
    cards: &[UnresolvedCard],
    costs_sent: Option<bool>,
    now_ms: i64,
) -> CostLookup {
    let mut costs_sent = costs_sent;
    let mut failed = false;
    let resolutions = run_card_passes(cards, &mut |request| {
        if failed || costs_sent == Some(false) {
            return None;
        }
        match fetch_cards(state, request, now_ms) {
            Ok(answers) => {
                if costs_sent.is_none() && !answers.is_empty() {
                    costs_sent = Some(answers.iter().any(|answer| answer.mana_cost_sent));
                }
                Some(answers)
            }
            Err(_) => {
                failed = true;
                None
            }
        }
    });
    CostLookup {
        resolutions,
        costs_sent,
        failed,
    }
}

/// 0.x `PopulateCardsInChunks`: an import's nameless rows by id, 200 a call.
pub fn look_up_imported_cards(state: &ApiState, mtgo_ids: &[u32], now_ms: i64) -> Resolutions {
    let unresolved = Unresolved {
        pass: LookupPass::Import,
        cards: mtgo_ids
            .iter()
            .map(|&mtgo_id| UnresolvedCard::new(mtgo_id, None, false))
            .collect(),
        unknown: Vec::new(),
    };
    let answers: Vec<Option<Vec<CardAnswer>>> = plan_card_lookups(&unresolved)
        .iter()
        .map(|request| fetch_cards(state, request, now_ms).ok())
        .collect();
    match_answers(&unresolved, &answers)
}

/// `/api/cards/resolve` for a .dek upload (0.x `ResolveCardsFromDek`). A
/// non-2xx is `Status` with "Failed to resolve cards from API.".
pub fn resolve_cards(
    state: &ApiState,
    mtgo_ids: &[u32],
    now_ms: i64,
) -> Result<Vec<ResolvedCard>, ApiError> {
    let response = state.send(&resolve_request(mtgo_ids), now_ms)?;
    parse_resolved(&response)
}

/// The API's archetype for a deck (0.x `estimateViaApi`). Offline it is
/// `Failed` without a request (no answer, asked again once online). A 401
/// recovers and retries once. Nothing is logged here: detection warns
/// about a failure, the overlay (which asks often during a game) only
/// notes it at debug.
pub fn estimate_archetype(
    state: &ApiState,
    format: &str,
    cards: &[EstimateCard],
    now_ms: i64,
) -> EstimateOutcome {
    if state.offline_mode().0 {
        return EstimateOutcome::Failed(ApiError::Offline.to_string());
    }
    match send_retrying_401(state, &estimate_request(format, cards), now_ms) {
        Ok(response) => parse_estimate(&response),
        Err(error) => EstimateOutcome::Failed(error.to_string()),
    }
}

/// One archetype's definition list. A 401 recovers (refresh or register
/// again) and retries once.
pub fn fetch_decklist(
    state: &ApiState,
    uuid: &str,
    now_ms: i64,
) -> Result<Vec<DecklistCard>, ApiError> {
    let response = send_retrying_401(state, &decklist_request(uuid), now_ms)?;
    parse_decklist(&response)
}

/// The server's active archetypes of one format key (the picker's
/// catalog, #489). A 401 recovers and retries once.
pub fn fetch_catalog(
    state: &ApiState,
    format_key: &str,
    now_ms: i64,
) -> Result<Vec<CatalogArchetype>, ApiError> {
    let response = send_retrying_401(state, &catalog_request(format_key), now_ms)?;
    parse_catalog(&response)
}

/// An archetype's real decklists (0.x `DownloadArchetypeDecklist`). A 401
/// recovers and retries once; a non-2xx is `Status` with "Failed to
/// download decklist from API.".
pub fn fetch_decklists(
    state: &ApiState,
    uuid: &str,
    now_ms: i64,
) -> Result<Vec<ArchetypeDeck>, ApiError> {
    let response = send_retrying_401(state, &decklists_request(uuid), now_ms)?;
    parse_decklists(&response)
}

/// The definitions check (archetypes spec sections 5 and 16.3) for held
/// archetypes as `(uuid, version)`: one request per 500 uuids, the answers
/// joined in order. Nothing is sent for an empty list. The first failed
/// chunk ends the check with its error: `Offline` before any socket opens,
/// `Status(404, ..)` while the API has no endpoint (`is_not_deployed`). A
/// 401 recovers and retries once per chunk.
pub fn check_definitions(
    state: &ApiState,
    versions: &[(String, Option<String>)],
    now_ms: i64,
) -> Result<DefinitionsCheck, ApiError> {
    let mut checks = Vec::new();
    let mut dropped = 0;
    for request in definitions_check_request(versions) {
        let response = send_retrying_401(state, &request, now_ms)?;
        let (check, left_out) = parse_definitions_check_counted(&response)?;
        dropped += left_out;
        checks.push(check);
    }
    if dropped > 0 {
        // Those archetypes are asked about again at the next check.
        tracing::warn!(
            dropped,
            "the archetype definitions check sent entries that could not be used"
        );
    }
    Ok(join_definitions_checks(checks))
}

/// The field's archetype shares for a format slug (`format_slug`) over
/// `days` (spec 16.4). A 401 recovers and retries once; offline it is
/// `Offline` without a request; a non-2xx is `Status`.
pub fn fetch_metagame(
    state: &ApiState,
    slug: &str,
    days: u32,
    now_ms: i64,
) -> Result<MetagameShares, ApiError> {
    let response = send_retrying_401(state, &metagame_request(slug, days), now_ms)?;
    parse_metagame(&response)
}

/// Send; on a 401 run the recovery (0.x `RecoverFromUnauthorized`) and send
/// once more.
fn send_retrying_401(
    state: &ApiState,
    request: &ApiRequest,
    now_ms: i64,
) -> Result<ApiResponse, ApiError> {
    // The bearer the request carried, not the one stored once the 401 is
    // back: another caller may have refreshed meanwhile (#264).
    let (response, used) = state.send_with_bearer(request, now_ms)?;
    if response.status != 401 {
        return Ok(response);
    }
    recover_unauthorized(state, used.as_deref(), now_ms);
    state.send(request, now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::session::testing::device_api;
    use crate::api::stub::{Reply, Stub};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const NOW: i64 = 1_800_000_000_000;

    fn mtgo_card(id: u32, quantity: u32) -> EstimateCard {
        EstimateCard {
            id: mymtgo_api::EstimateCardId::Mtgo(id),
            quantity,
        }
    }

    fn requested_ids(body: &[u8]) -> Vec<u64> {
        let body: serde_json::Value = serde_json::from_slice(body).expect("json");
        body["ids"]
            .as_array()
            .map(|ids| ids.iter().filter_map(|id| id.as_u64()).collect())
            .unwrap_or_default()
    }

    fn stubs(range: std::ops::RangeInclusive<u32>) -> Vec<UnresolvedCard> {
        range
            .map(|mtgo_id| UnresolvedCard::new(mtgo_id, None, false))
            .collect()
    }

    /// Answers every id it is asked about.
    fn answer_every_id(request: &crate::api::stub::Recorded) -> Reply {
        if request.target == "/api/devices/register" {
            return Reply::json(200, r#"{"api_key":"key-two"}"#);
        }
        let answers: Vec<serde_json::Value> = requested_ids(&request.body)
            .into_iter()
            .map(|id| {
                serde_json::json!({
                    "value": id, "scryfall_id": format!("scryfall-{id}"),
                    "oracle_id": format!("oracle-{id}"), "name": format!("Card {id}"),
                    "image": "https://example.test/card.png",
                })
            })
            .collect();
        Reply::json(200, &serde_json::Value::Array(answers).to_string())
    }

    // 0.x PopulateMissingCardDataTest: "rebuilds the api client per chunk,
    // so a key that expires mid-run still resolves every chunk".
    #[test]
    fn a_key_that_expires_mid_run_still_resolves_every_chunk() {
        let stub = Stub::start(answer_every_id);
        let api = device_api(&stub.base_url, false);
        let mut calls = 0;
        let resolutions = run_card_passes(&stubs(1..=60), &mut |request| {
            let answer = fetch_cards(&api.state, request, NOW).ok();
            calls += 1;
            if calls == 1 {
                // The key expires between the first and second chunk.
                api.state.session().api_key_expires_at = Some(NOW - 3_600_000);
            }
            answer
        });
        assert_eq!(calls, 2);
        assert_eq!(resolutions.resolved.len(), 60);
        let seen = stub.requests();
        let keys: Vec<(String, Option<String>)> = seen
            .iter()
            .map(|request| {
                (
                    request.target.clone(),
                    request.header("x-api-key").map(str::to_string),
                )
            })
            .collect();
        assert_eq!(
            keys,
            vec![
                ("/api/cards".to_string(), Some("key-one".to_string())),
                ("/api/devices/register".to_string(), None),
                ("/api/cards".to_string(), Some("key-two".to_string())),
            ]
        );
    }

    #[test]
    fn look_up_cards_runs_the_passes_against_the_api() {
        let stub = Stub::start(|request| {
            if requested_ids(&request.body).contains(&126_517) {
                Reply::json(
                    200,
                    r#"[{"value":"126517","scryfall_id":"scryfall-boggart","name":"Qtrawler // Qbog"}]"#,
                )
            } else {
                Reply::json(200, "[]")
            }
        });
        let api = device_api(&stub.base_url, false);
        let resolutions = look_up_cards(&api.state, &stubs(126_519..=126_519), NOW).resolutions;
        assert_eq!(resolutions.resolved.len(), 1);
        assert_eq!(resolutions.resolved[0].mtgo_id, 126_519);
        assert_eq!(
            resolutions.resolved[0].card.scryfall_id.as_deref(),
            Some("scryfall-boggart")
        );
        assert_eq!(stub.requests().len(), 2, "the id, then two below");
    }

    // 0.x PopulateCardsInChunksTest: "populates card data from the API".
    #[test]
    fn an_import_populates_card_data_from_the_api() {
        let stub = Stub::start(|_| {
            Reply::json(
                200,
                r#"[{"value":100,"name":"Qbolt","oracle_id":"oracle-bolt","scryfall_id":"scry-bolt",
                    "type":"Instant","sub_type":null,"rarity":"Common","color_identity":"R",
                    "image":"https://example.com/bolt.jpg"}]"#,
            )
        });
        let api = device_api(&stub.base_url, false);
        let resolutions = look_up_imported_cards(&api.state, &[100], NOW);
        assert_eq!(resolutions.resolved[0].card.name.as_deref(), Some("Qbolt"));
        assert_eq!(
            resolutions.resolved[0].card.oracle_id.as_deref(),
            Some("oracle-bolt")
        );
        assert_eq!(stub.requests()[0].body, br#"{"ids":[100],"tokens":[]}"#);
        // "skips when no unpopulated cards exist".
        let empty = Stub::start(|_| Reply::json(200, "[]"));
        let api = self::device_api(&empty.base_url, false);
        assert!(
            look_up_imported_cards(&api.state, &[], NOW)
                .resolved
                .is_empty()
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(empty.connections(), 0);
    }

    #[test]
    fn card_lookups_go_out_offline() {
        let stub = Stub::start(|_| Reply::json(200, r#"{"cards":[]}"#));
        let api = device_api(&stub.base_url, true);
        assert_eq!(resolve_cards(&api.state, &[1], NOW), Ok(Vec::new()));
        assert!(
            fetch_cards(
                &api.state,
                &CardsRequest {
                    ids: vec![1],
                    ..CardsRequest::default()
                },
                NOW
            )
            .is_err(),
            "an object is no list of answers"
        );
        assert_eq!(stub.requests().len(), 2);
    }

    // 0.x ResolveCardsFromDekTest: "resolves card data from parsed dek
    // entries" (the request) and "throws on API failure".
    #[test]
    fn resolve_cards_posts_the_ids_and_fails_as_0x() {
        let stub = Stub::start(|_| {
            Reply::json(
                200,
                r#"{"cards":[{"mtgo_id":12345,"oracle_id":"oracle-bolt","name":"Qbolt",
                    "type":"Instant","image":null,"art_crop":null,"cmc":1,"identity":"R"}]}"#,
            )
        });
        let api = device_api(&stub.base_url, false);
        let cards = resolve_cards(&api.state, &[12_345, 67_890], NOW).expect("resolved");
        assert_eq!(cards[0].identity.as_deref(), Some("R"));
        let seen = stub.requests();
        assert_eq!(seen[0].target, "/api/cards/resolve");
        assert_eq!(seen[0].body, br#"{"mtgo_ids":[12345,67890]}"#);
        let failing = Stub::start(|_| Reply::json(500, "[]"));
        let api = self::device_api(&failing.base_url, false);
        assert_eq!(
            resolve_cards(&api.state, &[12_345], NOW),
            Err(ApiError::Status(
                500,
                "Failed to resolve cards from API.".to_string()
            ))
        );
    }

    // 0.x ArchetypeFloorTest: "estimateViaApi returns null offline without
    // sending a request".
    #[test]
    fn estimate_returns_none_offline_without_sending_a_request() {
        let stub = Stub::start(|_| Reply::json(200, r#"[{"uuid":"q","confidence":1}]"#));
        let api = device_api(&stub.base_url, true);
        assert_eq!(
            estimate_archetype(&api.state, "CMODERN", &[mtgo_card(1, 4)], NOW),
            EstimateOutcome::Failed(ApiError::Offline.to_string())
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(stub.connections(), 0);
    }

    // 0.x ArchetypeFloorTest: "estimateViaApi still calls the endpoint when
    // online".
    #[test]
    fn estimate_still_calls_the_endpoint_when_online() {
        let stub = Stub::start(|_| {
            Reply::json(
                200,
                r#"[{"uuid":"q-arch","confidence":0.5,"deck_version_uuid":"q-deck"}]"#,
            )
        });
        let api = device_api(&stub.base_url, false);
        let EstimateOutcome::Answered(estimate) =
            estimate_archetype(&api.state, "CMODERN", &[mtgo_card(1, 4)], NOW)
        else {
            panic!("an estimate");
        };
        assert_eq!(estimate.uuid, "q-arch");
        assert_eq!(estimate.deck_version_uuid.as_deref(), Some("q-deck"));
        let seen = stub.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].target, "/api/archetypes/estimate");
        assert_eq!(
            seen[0].body,
            br#"{"format":"CMODERN","cards":[{"mtgo_id":1,"quantity":4}]}"#
        );
    }

    #[test]
    fn an_empty_estimate_is_no_archetype_and_a_500_failed() {
        let stub = Stub::start(|_| Reply::json(200, "[]"));
        let api = device_api(&stub.base_url, false);
        assert_eq!(
            estimate_archetype(&api.state, "CMODERN", &[mtgo_card(1, 4)], NOW),
            EstimateOutcome::NoArchetype
        );
        let stub = Stub::start(|_| Reply::json(500, r#"{"message":"Server Error"}"#));
        let api = device_api(&stub.base_url, false);
        assert_eq!(
            estimate_archetype(&api.state, "CMODERN", &[mtgo_card(1, 4)], NOW),
            EstimateOutcome::Failed("status 500".to_string())
        );
    }

    // A dead key or bearer used to fail every estimate until another call
    // recovered it: the estimate recovers and retries once, like the other
    // archetype calls.
    #[test]
    fn the_estimate_retries_on_401_after_re_registration() {
        let estimate_calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&estimate_calls);
        let stub = Stub::start(move |request| {
            if request.target == "/api/devices/register" {
                return Reply::json(200, r#"{"api_key":"new-key"}"#);
            }
            if counted.fetch_add(1, Ordering::SeqCst) == 0 {
                Reply::json(401, "[]")
            } else {
                Reply::json(200, r#"[{"uuid":"q-arch","confidence":1}]"#)
            }
        });
        let api = device_api(&stub.base_url, false);
        let outcome = estimate_archetype(&api.state, "CMODERN", &[mtgo_card(1, 4)], NOW);
        assert!(
            matches!(&outcome, EstimateOutcome::Answered(estimate) if estimate.uuid == "q-arch"),
            "{outcome:?}"
        );
        let seen = stub.requests();
        let targets: Vec<&str> = seen.iter().map(|r| r.target.as_str()).collect();
        assert_eq!(
            targets,
            vec![
                "/api/archetypes/estimate",
                "/api/devices/register",
                "/api/archetypes/estimate",
            ]
        );
        assert_eq!(seen[2].header("x-api-key"), Some("new-key"));
    }

    const DECKLISTS: &str = r#"{"uuid":"arch-uuid","name":"Qarch","format":"modern","decks":[
        {"uuid":"retry-deck-uuid","seen_count":4,"cards":[
            {"mtgo_id":"11111","oracle_id":"oracle-retry","name":"Qcard","type":"Instant",
             "color_identity":"G","image":null,"quantity":4,"sideboard":false}]}]}"#;

    // 0.x DownloadDecklistTest: "retries on 401 after re-registration".
    #[test]
    fn retries_on_401_after_re_registration() {
        let decklist_calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&decklist_calls);
        let stub = Stub::start(move |request| {
            if request.target == "/api/devices/register" {
                return Reply::json(200, r#"{"api_key":"new-key"}"#);
            }
            if counted.fetch_add(1, Ordering::SeqCst) == 0 {
                Reply::json(401, "[]")
            } else {
                Reply::json(200, DECKLISTS)
            }
        });
        let api = device_api(&stub.base_url, false);
        let decks = fetch_decklists(&api.state, "arch-uuid", NOW).expect("downloaded");
        assert_eq!(decks.len(), 1);
        assert_eq!(decks[0].uuid, "retry-deck-uuid");
        let seen = stub.requests();
        let targets: Vec<&str> = seen.iter().map(|r| r.target.as_str()).collect();
        assert_eq!(
            targets,
            vec![
                "/api/archetypes/arch-uuid/decklists",
                "/api/devices/register",
                "/api/archetypes/arch-uuid/decklists",
            ]
        );
        assert_eq!(seen[2].header("x-api-key"), Some("new-key"));
    }

    #[test]
    fn a_second_401_is_the_0x_error_and_no_third_try() {
        let stub = Stub::start(|request| {
            if request.target == "/api/devices/register" {
                Reply::json(200, r#"{"api_key":"new-key"}"#)
            } else {
                Reply::json(401, "[]")
            }
        });
        let api = device_api(&stub.base_url, false);
        assert_eq!(
            fetch_decklist(&api.state, "arch-uuid", NOW),
            Err(ApiError::Status(
                401,
                "Failed to download decklist from API.".to_string()
            ))
        );
        let lists = stub
            .requests()
            .iter()
            .filter(|request| request.target == "/api/archetypes/arch-uuid/decklist")
            .count();
        assert_eq!(lists, 2);
    }

    // 0.x DownloadDecklistTest: "throws when API returns non-2xx"; offline
    // the download is refused before a socket opens.
    #[test]
    fn a_failed_or_offline_download_is_an_error() {
        let stub = Stub::start(|_| Reply::json(500, ""));
        let api = device_api(&stub.base_url, false);
        assert!(matches!(
            fetch_decklists(&api.state, "arch-uuid", NOW),
            Err(ApiError::Status(500, _))
        ));
        let offline = Stub::start(|_| Reply::json(200, DECKLISTS));
        let api = self::device_api(&offline.base_url, true);
        assert_eq!(
            fetch_decklists(&api.state, "arch-uuid", NOW),
            Err(ApiError::Offline)
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(offline.connections(), 0);
    }

    const CHECK_ANSWER: &str = r#"{"changed": [{"uuid": "q-arch-1", "name": "Qstorm",
        "format": "modern", "version": "q-v2", "cards": []}],
        "moved": [{"uuid": "q-arch-0", "to": "q-arch-1"}], "retired": ["q-arch-9"]}"#;

    #[test]
    fn check_definitions_posts_the_held_versions_and_reads_the_answer() {
        let stub = Stub::start(|_| Reply::json(200, CHECK_ANSWER));
        let api = device_api(&stub.base_url, false);
        let versions = vec![
            ("q-arch-0".to_string(), Some("q-v1".to_string())),
            ("q-arch-9".to_string(), None),
        ];
        let check = check_definitions(&api.state, &versions, NOW).expect("checked");
        assert_eq!(check.changed[0].uuid, "q-arch-1");
        assert_eq!(
            check.moved,
            vec![("q-arch-0".to_string(), "q-arch-1".to_string())]
        );
        assert_eq!(check.retired, vec!["q-arch-9"]);
        let seen = stub.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].target, "/api/archetypes/definitions/check");
        assert_eq!(seen[0].header("x-api-key"), Some("key-one"));
        assert_eq!(
            seen[0].body,
            br#"{"archetypes":[{"uuid":"q-arch-0","version":"q-v1"},{"uuid":"q-arch-9","version":null}]}"#
        );
    }

    // Every chunk is sent and the answers are joined.
    #[test]
    fn check_definitions_joins_the_answers_of_every_chunk() {
        let stub = Stub::start(|request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).expect("json");
            let first = body["archetypes"][0]["uuid"]
                .as_str()
                .unwrap_or("")
                .to_string();
            Reply::json(
                200,
                &serde_json::json!({"changed": [], "moved": [], "retired": [first]}).to_string(),
            )
        });
        let api = device_api(&stub.base_url, false);
        let versions: Vec<(String, Option<String>)> = (0..501)
            .map(|index| (format!("q-arch-{index:03}"), None))
            .collect();
        let check = check_definitions(&api.state, &versions, NOW).expect("checked");
        assert_eq!(check.retired, vec!["q-arch-000", "q-arch-500"]);
        assert_eq!(stub.requests().len(), 2);
        // Nothing held: nothing asked.
        let idle = Stub::start(|_| Reply::json(200, CHECK_ANSWER));
        let api = self::device_api(&idle.base_url, false);
        assert_eq!(
            check_definitions(&api.state, &[], NOW),
            Ok(DefinitionsCheck::default())
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(idle.connections(), 0);
    }

    // Spec 5: before the API deploys the check, its 404 is "not deployed".
    #[test]
    fn a_missing_check_endpoint_is_a_404() {
        let stub = Stub::start(|_| Reply::json(404, r#"{"message":"Not Found"}"#));
        let api = device_api(&stub.base_url, false);
        let versions = vec![("q-arch".to_string(), None)];
        let error = check_definitions(&api.state, &versions, NOW).expect_err("a 404");
        assert!(mymtgo_api::is_not_deployed(&error), "{error:?}");
    }

    #[test]
    fn check_definitions_retries_once_on_401() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let stub = Stub::start(move |request| {
            if request.target == "/api/devices/register" {
                return Reply::json(200, r#"{"api_key":"new-key"}"#);
            }
            if counted.fetch_add(1, Ordering::SeqCst) == 0 {
                Reply::json(401, "[]")
            } else {
                Reply::json(200, CHECK_ANSWER)
            }
        });
        let api = device_api(&stub.base_url, false);
        let versions = vec![("q-arch-0".to_string(), None)];
        let check = check_definitions(&api.state, &versions, NOW).expect("checked");
        assert_eq!(check.changed.len(), 1);
        let seen = stub.requests();
        let targets: Vec<&str> = seen.iter().map(|r| r.target.as_str()).collect();
        assert_eq!(
            targets,
            vec![
                "/api/archetypes/definitions/check",
                "/api/devices/register",
                "/api/archetypes/definitions/check",
            ]
        );
        assert_eq!(seen[2].header("x-api-key"), Some("new-key"));
    }

    const SHARES: &str = r#"{"format": "modern", "days": 30, "window_start": "2026-09-10",
        "window_end": "2026-10-09", "matches": 10011, "archetypes": [
        {"uuid": "q-arch-1", "name": "Qstorm", "color_identity": "R", "share": 6.2,
         "matches": 1241}], "other_share": 7.4, "unnameable_share": 3.1}"#;

    #[test]
    fn fetch_metagame_gets_the_window_and_reads_the_shares() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let stub = Stub::start(move |request| {
            if request.target == "/api/devices/register" {
                return Reply::json(200, r#"{"api_key":"new-key"}"#);
            }
            if counted.fetch_add(1, Ordering::SeqCst) == 0 {
                Reply::json(401, "[]")
            } else {
                Reply::json(200, SHARES)
            }
        });
        let api = device_api(&stub.base_url, false);
        let shares = fetch_metagame(&api.state, "modern", 30, NOW).expect("fetched");
        assert_eq!(shares.matches, 10_011);
        assert_eq!(shares.archetypes[0].uuid, "q-arch-1");
        assert_eq!(shares.archetypes[0].share, Some(6.2));
        let seen = stub.requests();
        let targets: Vec<&str> = seen.iter().map(|r| r.target.as_str()).collect();
        assert_eq!(
            targets,
            vec![
                "/api/metagame/modern?days=30",
                "/api/devices/register",
                "/api/metagame/modern?days=30",
            ],
            "a 401 recovers and retries once"
        );
        let unknown = Stub::start(|_| Reply::json(404, r#"{"message":"Unknown format."}"#));
        let api = self::device_api(&unknown.base_url, false);
        assert!(matches!(
            fetch_metagame(&api.state, "cube", 30, NOW),
            Err(ApiError::Status(404, _))
        ));
    }

    // Both are `Lookup`: offline they fail before a socket opens.
    #[test]
    fn the_check_and_the_metagame_send_nothing_offline() {
        let stub = Stub::start(|_| Reply::json(200, CHECK_ANSWER));
        let api = device_api(&stub.base_url, true);
        let versions = vec![("q-arch".to_string(), None)];
        assert_eq!(
            check_definitions(&api.state, &versions, NOW),
            Err(ApiError::Offline)
        );
        assert_eq!(
            fetch_metagame(&api.state, "modern", 30, NOW),
            Err(ApiError::Offline)
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(stub.connections(), 0);
    }
}
