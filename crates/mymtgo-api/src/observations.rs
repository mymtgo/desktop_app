//! Tournament observations (survey section 11, 07 spec section 8, rulings
//! 6 and 15): every stored tournament event is queued once in the outbox
//! with 0.x's payload (`ExtractTournamentPayload`) and shipped as a gzip
//! JSON array to `POST /api/tournament-observations`. The API drops repeats
//! by content hash, so a row sent twice (a crash between send and mark) is
//! harmless.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use crate::gate::RequestClass;
use crate::outbox::{ClaimedRow, NewOutboxRow, OutboxKind, pack_payload, readable_rows};
use crate::php::iso8601_utc;
use crate::request::{ApiRequest, AuthMode, Body, MIB, Method, gzip};

pub const OBSERVATIONS_PATH: &str = "/api/tournament-observations";

/// No observation request's JSON is larger than this before gzip (ruling
/// 15, #498): the API refuses a body over 8 MiB inflated, and a first
/// start's backlog of 200 `EventSyncData_t` lines (about 1 MB raw each)
/// must not time out forever.
pub const MAX_REQUEST_BYTES: u64 = 6 * MIB;

/// The API refuses an observation whose payload, as PHP's `json_encode`
/// writes it, is over this (the API `MAX_PAYLOAD_BYTES`), and the whole
/// batch with it.
pub const MAX_PAYLOAD_BYTES: u64 = 4 * MIB;

/// Why an event over `MAX_PAYLOAD_BYTES` is skipped.
pub const PAYLOAD_TOO_LARGE: &str = "payload larger than the API accepts";

/// Why an event with nothing to send is skipped.
pub const PAYLOAD_EMPTY: &str = "empty payload";

/// Why an event without a tournament token is skipped.
pub const NO_TOURNAMENT_TOKEN: &str = "no tournament token";

/// The kinds shipped, as stored in `log_events.kind` (0.x
/// `LogEventType::tournamentValues()`).
pub const OBSERVATION_KINDS: [&str; 6] = [
    "tournament_sync",
    "tournament_state_changed",
    "tournament_round_result",
    "tournament_round_info",
    "tournament_player_eliminated",
    "tournament_ended",
];

/// The kinds whose payload is the first JSON value in the line.
const JSON_KINDS: [&str; 5] = [
    "tournament_sync",
    "tournament_round_result",
    "tournament_round_info",
    "tournament_player_eliminated",
    "tournament_ended",
];

static STATE_CHANGED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)Tournament State Changed for [a-f0-9\-]{36} from (?P<from>\S+) to (?P<to>\S+?)\)?$",
    )
    .expect("the state change pattern compiles")
});

/// A stored tournament event, its raw text decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TournamentEvent {
    pub log_file_id: i64,
    pub byte_offset: i64,
    /// `log_events.kind`.
    pub kind: String,
    pub raw: String,
    pub tournament_token: Option<String>,
    pub logged_at_ms: i64,
}

/// What one observation sends, its keys in 0.x's order.
#[derive(Serialize)]
struct Observation<'a> {
    tournament_token: &'a str,
    /// v1 sends none: tournament events carry no match token.
    match_token: Option<&'a str>,
    event_type: &'a str,
    payload: Value,
    client_observed_at: String,
}

/// 0.x `ExtractTournamentPayload`: the first JSON object or list in the
/// line for the five JSON kinds, `{"from", "to"}` for a state change.
/// `None` for any other kind or when there is nothing (an empty object or
/// list counts as nothing, as PHP's `empty([])`).
pub fn observation_payload(kind: &str, raw: &str) -> Option<Value> {
    let payload = if JSON_KINDS.contains(&kind) {
        decks::first_json(raw)?
    } else if kind == "tournament_state_changed" {
        let found = STATE_CHANGED.captures(raw.trim())?;
        serde_json::json!({ "from": &found["from"], "to": &found["to"] })
    } else {
        return None;
    };
    let filled = match &payload {
        Value::Object(fields) => !fields.is_empty(),
        Value::Array(items) => !items.is_empty(),
        _ => false,
    };
    filled.then_some(payload)
}

/// The outbox row for an event: subject `"{log_file_id}:{byte_offset}"`,
/// payload the zstd JSON of the observation (`client_observed_at` the
/// event's `logged_at`, ruling 6). An event with no payload, or without a
/// tournament token (the API refuses an observation with neither token,
/// and v1 sends no match token), is a terminal `skipped` row.
pub fn observation_row(event: &TournamentEvent, now_ms: i64) -> NewOutboxRow {
    let subject = format!("{}:{}", event.log_file_id, event.byte_offset);
    let payload = observation_payload(&event.kind, &event.raw)
        .filter(|payload| php_encoded_len(payload) <= MAX_PAYLOAD_BYTES);
    let token = event.tournament_token.as_deref().filter(|t| !t.is_empty());
    let stored = match (token, payload) {
        (Some(tournament_token), Some(payload)) => {
            let observation = Observation {
                tournament_token,
                match_token: None,
                event_type: &event.kind,
                payload,
                client_observed_at: iso8601_utc(event.logged_at_ms),
            };
            pack_payload(&observation)
        }
        _ => None,
    };
    NewOutboxRow {
        kind: OutboxKind::TournamentObservation,
        subject,
        payload: stored,
        now_ms,
    }
}

/// Why `observation_row` stores an event as `skipped`, for the log; `None`
/// when it is queued to send.
pub fn observation_skip_reason(event: &TournamentEvent) -> Option<&'static str> {
    if event.tournament_token.as_deref().is_none_or(str::is_empty) {
        return Some(NO_TOURNAMENT_TOKEN);
    }
    match observation_payload(&event.kind, &event.raw) {
        None => Some(PAYLOAD_EMPTY),
        Some(payload) if php_encoded_len(&payload) > MAX_PAYLOAD_BYTES => Some(PAYLOAD_TOO_LARGE),
        Some(_) => None,
    }
}

/// How long `value` is as PHP's `json_encode` writes it with no flags: like
/// `serde_json`, but `/` is escaped as `\/` and every non-ASCII character as
/// `\uXXXX` (a surrogate pair for one outside the BMP).
pub fn php_encoded_len(value: &Value) -> u64 {
    let json = serde_json::to_string(value).unwrap_or_default();
    let mut len: u64 = 0;
    for c in json.chars() {
        len += if c == '/' {
            2
        } else if c.is_ascii() {
            1
        } else {
            6 * c.len_utf16() as u64
        };
    }
    len
}

/// The indexes of the observations a 422 names (`observations.{index}` and
/// `observations.{index}.{field}` error keys), sorted, each once. Empty when
/// it names none (a refusal of the whole body) or is not JSON.
pub fn refused_observations(body: &[u8]) -> Vec<usize> {
    let Ok(answer) = serde_json::from_slice::<Value>(body) else {
        return Vec::new();
    };
    let Some(errors) = answer.get("errors").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut indexes = Vec::new();
    for key in errors.keys() {
        let Some(rest) = key.strip_prefix("observations.") else {
            continue;
        };
        let number = rest.split('.').next().unwrap_or_default();
        if let Ok(index) = number.parse::<usize>()
            && !indexes.contains(&index)
        {
            indexes.push(index);
        }
    }
    indexes.sort_unstable();
    indexes
}

/// The requests for one claim (the worker's builder): one gzip request for
/// the whole claim, halved until each one's JSON is at most
/// `MAX_REQUEST_BYTES`. A row whose payload is over `MAX_PAYLOAD_BYTES`
/// (queued before that check) is left out, as an unreadable one is: the
/// API would refuse it, and the batch with it, every time.
pub fn observations_requests(rows: &[ClaimedRow]) -> Vec<(Vec<String>, ApiRequest)> {
    observations_requests_within(rows, MAX_REQUEST_BYTES)
}

/// `observations_requests` with any limit. A row whose stored payload
/// cannot be read is left out (the worker fails it as unreadable); a single row over
/// the limit still goes, alone.
pub fn observations_requests_within(
    rows: &[ClaimedRow],
    limit: u64,
) -> Vec<(Vec<String>, ApiRequest)> {
    let readable: Vec<(String, Value)> = readable_rows(rows)
        .into_iter()
        .filter(|(_, observation)| {
            observation
                .get("payload")
                .is_none_or(|payload| php_encoded_len(payload) <= MAX_PAYLOAD_BYTES)
        })
        .collect();
    let mut requests = Vec::new();
    split_into(&readable, limit, &mut requests);
    requests
}

/// Add the request for `rows`, or for each half when it is over `limit`.
fn split_into(rows: &[(String, Value)], limit: u64, requests: &mut Vec<(Vec<String>, ApiRequest)>) {
    if rows.is_empty() {
        return;
    }
    let (request, raw_len) = observations_request(rows);
    if raw_len <= limit || rows.len() == 1 {
        let subjects = rows.iter().map(|(subject, _)| subject.clone()).collect();
        requests.push((subjects, request));
        return;
    }
    let (first, second) = rows.split_at(rows.len() / 2);
    split_into(first, limit, requests);
    split_into(second, limit, requests);
}

/// `POST /api/tournament-observations`: the observations as a bare JSON
/// array, gzip (`Content-Encoding: gzip`, `Content-Type:
/// application/json`), class `Community`, 30 s total and 10 s connect.
/// Beside it, the length of its JSON before gzip.
fn observations_request(rows: &[(String, Value)]) -> (ApiRequest, u64) {
    let array: Vec<&Value> = rows.iter().map(|(_, observation)| observation).collect();
    let json = serde_json::to_vec(&array).unwrap_or_default();
    let mut request = ApiRequest::new(
        Method::Post,
        OBSERVATIONS_PATH,
        RequestClass::Community,
        AuthMode::DeviceOrBearer,
    );
    request.body = Body::GzipJson(gzip(&json));
    (request, json.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::RequestClass;
    use crate::request::{AuthMode, Body, Method};
    use crate::test_support::gunzip;

    const STATE_CHANGED: &str = "16:34:31 [INF] (Game Management|Tournament State Changed for b197b9e8-0d08-4227-aa17-ba38cb4c1731 from TournamentUninitializedState to PremierNotJoinedAwaitingMinPlayersState)";
    const SYNC: &str = r#"16:34:31 [INF] (Game Management|Processing Registered Handler for EventSyncData_t in TournamentUninitializedState) Processor: TournamentUninitializedState Message: {"EventToken":"b197b9e8-0d08-4227-aa17-ba38cb4c1731","EventID":12839726,"PlayerEventType":4,"Description":"Contraption Challenge 32"} Receiver: WotC.MtGO.Client.Model.Play.TournamentEvent.Tournament"#;
    const ROUND_INFO: &str = r#"15:22:57 [INF] (Game Management|Processing Registered Handler for FlsTournamentRoundInfoMessage in TournamentNotJoinedWaitingForFirstRoundToStartState) Processor: TournamentNotJoinedWaitingForFirstRoundToStartState Message: {"Token":"44b6fb76-9171-4e21-a8b7-ff635ed06c8f","ID":12839079,"Round":{"Number":1,"Matches":[{"MatchCreateInfo":{"MatchToken":"15228233-706f-4e34-a1e7-7fdb0d02b704","MatchID":286237349}}]}} Receiver: WotC.MtGO.Client.Model.Play.TournamentEvent.Tournament"#;
    const ROUND_RESULT: &str = r#"16:35:24 [INF] (Game Management|Processing Registered Handler for FlsTournamentRoundResultMessage in TournamentNotJoinedRoundInProgressState) Processor: TournamentNotJoinedRoundInProgressState Message: {"Token":"32d98b9d-2af6-4cc7-a95d-cd7471d75809","Round":2,"Results":[{"LoginID":3316456,"Rank":1,"Points":6}]} Receiver: WotC.MtGO.Client.Model.Play.TournamentEvent.Tournament"#;
    const ELIMINATED: &str = r#"16:35:24 [INF] (Game Management|Processing Registered Handler for FlsTournamentPlayerIsEliminatedMessage in TournamentNotJoinedRoundInProgressState) Processor: TournamentNotJoinedRoundInProgressState Message: {"Token":"32d98b9d-2af6-4cc7-a95d-cd7471d75809","ID":12839833,"LoginID":2320156,"EliminationReason":1} Receiver: WotC.MtGO.Client.Model.Play.TournamentEvent.Tournament"#;
    const ENDED: &str = r#"16:42:17 [INF] (Game Management|Processing Registered Handler for FlsTournamentEndRespMessage in TournamentNotJoinedBetweenRoundsState) Processor: TournamentNotJoinedBetweenRoundsState Message: {"Token":"87b97a55-bc56-47f9-866b-ca51ed0db0d5","ID":12839828,"LoginID":9001,"EndDate":"2026-04-16T16:42:17.7340000+01:00","ReturnCode":0} Receiver: WotC.MtGO.Client.Model.Play.TournamentEvent.Tournament"#;

    /// 2026-04-16T16:34:31Z.
    const LOGGED_AT: i64 = 1_776_357_271_000;

    fn event(kind: &str, raw: &str, token: Option<&str>) -> TournamentEvent {
        TournamentEvent {
            log_file_id: 3,
            byte_offset: 1_234,
            kind: kind.to_string(),
            raw: raw.to_string(),
            tournament_token: token.map(str::to_string),
            logged_at_ms: LOGGED_AT,
        }
    }

    fn payload(kind: &str, raw: &str) -> Value {
        observation_payload(kind, raw).expect("a payload")
    }

    fn claimed(subject: &str, observation: &Value) -> ClaimedRow {
        ClaimedRow {
            subject: subject.to_string(),
            payload: zstd::encode_all(observation.to_string().as_bytes(), 0).expect("zstd"),
        }
    }

    /// The request's JSON before gzip.
    fn raw_len(request: &ApiRequest) -> u64 {
        let Body::GzipJson(bytes) = &request.body else {
            panic!("a gzip body: {:?}", request.body);
        };
        gunzip(bytes).len() as u64
    }

    fn sent_array(request: &ApiRequest) -> Vec<Value> {
        let Body::GzipJson(bytes) = &request.body else {
            panic!("a gzip body: {:?}", request.body);
        };
        serde_json::from_slice(&gunzip(bytes)).expect("a JSON array")
    }

    // 0.x ExtractTournamentPayloadTest: "extracts from/to state names for
    // tournament_state_changed".
    #[test]
    fn extracts_from_and_to_state_names_for_tournament_state_changed() {
        assert_eq!(
            payload("tournament_state_changed", STATE_CHANGED),
            serde_json::json!({
                "from": "TournamentUninitializedState",
                "to": "PremierNotJoinedAwaitingMinPlayersState",
            })
        );
        // Trailing whitespace is trimmed first; any case matches.
        let shouted = format!("{}  \r\n", STATE_CHANGED.to_uppercase());
        assert_eq!(
            payload("tournament_state_changed", &shouted)["to"],
            "PREMIERNOTJOINEDAWAITINGMINPLAYERSSTATE"
        );
    }

    // 0.x: "extracts JSON body for tournament_sync".
    #[test]
    fn extracts_the_json_body_for_tournament_sync() {
        let found = payload("tournament_sync", SYNC);
        assert_eq!(found["EventToken"], "b197b9e8-0d08-4227-aa17-ba38cb4c1731");
        assert_eq!(found["EventID"], 12_839_726);
    }

    // 0.x: "extracts JSON body for tournament_round_info".
    #[test]
    fn extracts_the_json_body_for_tournament_round_info() {
        let found = payload("tournament_round_info", ROUND_INFO);
        assert_eq!(found["Token"], "44b6fb76-9171-4e21-a8b7-ff635ed06c8f");
        assert!(found["Round"].is_object());
    }

    // 0.x: "extracts JSON body for tournament_round_result".
    #[test]
    fn extracts_the_json_body_for_tournament_round_result() {
        let found = payload("tournament_round_result", ROUND_RESULT);
        assert_eq!(found["Token"], "32d98b9d-2af6-4cc7-a95d-cd7471d75809");
        assert_eq!(found["Round"], 2);
    }

    // 0.x: "extracts JSON body for tournament_player_eliminated".
    #[test]
    fn extracts_the_json_body_for_tournament_player_eliminated() {
        let found = payload("tournament_player_eliminated", ELIMINATED);
        assert_eq!(found["Token"], "32d98b9d-2af6-4cc7-a95d-cd7471d75809");
        assert_eq!(found["LoginID"], 2_320_156);
    }

    // 0.x: "extracts JSON body for tournament_ended".
    #[test]
    fn extracts_the_json_body_for_tournament_ended() {
        let found = payload("tournament_ended", ENDED);
        assert_eq!(found["Token"], "87b97a55-bc56-47f9-866b-ca51ed0db0d5");
        assert_eq!(found["ReturnCode"], 0);
    }

    // 0.x: "returns empty array for unclassified events".
    #[test]
    fn returns_nothing_for_unclassified_or_empty_events() {
        assert_eq!(observation_payload("match_state_changed", "nothing"), None);
        assert_eq!(observation_payload("tournament_sync", "nothing"), None);
        assert_eq!(
            observation_payload("tournament_state_changed", "garbage line with no from/to"),
            None
        );
        // An empty object or list, or a bare JSON value, is no payload.
        assert_eq!(
            observation_payload("tournament_ended", "Message: {} x"),
            None
        );
        assert_eq!(
            observation_payload("tournament_ended", "Message: [] x"),
            None
        );
        assert_eq!(
            observation_payload("tournament_ended", "Message: [1, 2] x"),
            Some(serde_json::json!([1, 2]))
        );
    }

    #[test]
    fn a_row_is_the_zstd_observation_keyed_by_file_and_offset() {
        let row = observation_row(
            &event(
                "tournament_sync",
                SYNC,
                Some("b197b9e8-0d08-4227-aa17-ba38cb4c1731"),
            ),
            LOGGED_AT + 5_000,
        );
        assert_eq!(row.kind, OutboxKind::TournamentObservation);
        assert_eq!(row.subject, "3:1234");
        assert_eq!(row.now_ms, LOGGED_AT + 5_000);
        let stored = zstd::decode_all(row.payload.expect("pending").as_slice()).expect("zstd");
        assert_eq!(
            String::from_utf8(stored).expect("utf8"),
            r#"{"tournament_token":"b197b9e8-0d08-4227-aa17-ba38cb4c1731","match_token":null,"event_type":"tournament_sync","payload":{"Description":"Contraption Challenge 32","EventID":12839726,"EventToken":"b197b9e8-0d08-4227-aa17-ba38cb4c1731","PlayerEventType":4},"client_observed_at":"2026-04-16T16:34:31+00:00"}"#
        );
    }

    // 0.x EnqueueTournamentObservationsTest: "records a terminal skipped row
    // for events whose extracted payload is empty". v1 also skips an event
    // without a tournament token: it sends no match token, and the API
    // refuses an observation with neither (the whole batch, every time).
    #[test]
    fn an_empty_payload_or_no_token_is_a_skipped_row() {
        let garbage = event(
            "tournament_state_changed",
            "garbage line with no from/to",
            Some("deadbeef-dead-beef-dead-beefdeadbeef"),
        );
        assert_eq!(observation_row(&garbage, LOGGED_AT).payload, None);
        let tokenless = event("tournament_ended", ENDED, None);
        assert_eq!(observation_row(&tokenless, LOGGED_AT).payload, None);
    }

    // 0.x ShipTournamentObservationsTest: "sends gzipped body with auth
    // headers" (the request as built) and "sends a body that satisfies the
    // API validator".
    #[test]
    fn the_request_is_a_gzipped_bare_array_the_validator_accepts() {
        let row = observation_row(
            &event("tournament_state_changed", STATE_CHANGED, Some("tok-1")),
            LOGGED_AT,
        );
        let rows = vec![ClaimedRow {
            subject: row.subject.clone(),
            payload: row.payload.expect("pending"),
        }];
        let requests = observations_requests(&rows);
        assert_eq!(requests.len(), 1);
        let (subjects, request) = &requests[0];
        assert_eq!(subjects, &vec!["3:1234".to_string()]);
        assert_eq!(request.method, Method::Post);
        assert_eq!(request.path, "/api/tournament-observations");
        assert_eq!(request.class, RequestClass::Community);
        assert_eq!(request.auth, AuthMode::DeviceOrBearer);
        assert_eq!(
            (request.timeout_ms, request.connect_timeout_ms),
            (30_000, 10_000)
        );
        let headers = request.body.content_headers();
        assert!(headers.contains(&("Content-Encoding".to_string(), "gzip".to_string())));
        assert!(headers.contains(&("Content-Type".to_string(), "application/json".to_string())));
        let sent = sent_array(request);
        assert_eq!(sent.len(), 1);
        let observation = sent[0].as_object().expect("an object");
        let mut keys: Vec<&str> = observation.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "client_observed_at",
                "event_type",
                "match_token",
                "payload",
                "tournament_token"
            ]
        );
        assert_eq!(observation["tournament_token"], "tok-1");
        assert_eq!(observation["match_token"], Value::Null);
        assert_eq!(observation["event_type"], "tournament_state_changed");
        assert!(
            observation["payload"]
                .as_object()
                .is_some_and(|p| !p.is_empty())
        );
        assert_eq!(
            observation["client_observed_at"],
            "2026-04-16T16:34:31+00:00"
        );
    }

    #[test]
    fn one_request_carries_a_whole_claim_in_order() {
        let rows: Vec<ClaimedRow> = (0..200)
            .map(|index| {
                claimed(
                    &format!("1:{index}"),
                    &serde_json::json!({ "tournament_token": "q", "payload": { "n": index } }),
                )
            })
            .collect();
        let requests = observations_requests(&rows);
        assert_eq!(requests.len(), 1);
        let (subjects, request) = &requests[0];
        assert_eq!(subjects.len(), 200);
        assert_eq!(subjects[199], "1:199");
        assert_eq!(sent_array(request)[17]["payload"]["n"], 17);
        assert!(observations_requests(&[]).is_empty());
    }

    #[test]
    fn requests_split_until_each_fits_the_limit() {
        let rows: Vec<ClaimedRow> = (0..10)
            .map(|index| {
                claimed(
                    &format!("1:{index}"),
                    &serde_json::json!({ "payload": { "noise": noise(index, 4_000) } }),
                )
            })
            .collect();
        let whole = observations_requests_within(&rows, u64::MAX);
        assert_eq!(whole.len(), 1);
        let limit = raw_len(&whole[0].1) / 3;
        let split = observations_requests_within(&rows, limit);
        assert!(split.len() >= 3, "{}", split.len());
        let mut subjects = Vec::new();
        for (carried, request) in &split {
            assert!(raw_len(request) <= limit);
            assert_eq!(sent_array(request).len(), carried.len());
            subjects.extend(carried.iter().cloned());
        }
        let expected: Vec<String> = (0..10).map(|index| format!("1:{index}")).collect();
        assert_eq!(subjects, expected, "every row once, in order");
        // A single row over the limit still goes, alone.
        let alone = observations_requests_within(&rows[..1], 10);
        assert_eq!(alone.len(), 1);
    }

    // The API caps a body at 8 MiB inflated: a claim whose gzip is small
    // but whose JSON is large is split on the JSON (#498).
    #[test]
    fn a_claim_is_split_on_its_raw_json_not_its_gzip() {
        let rows: Vec<ClaimedRow> = (0..12)
            .map(|index| {
                claimed(
                    &format!("1:{index}"),
                    &serde_json::json!({ "payload": { "same": "A".repeat(1_000_000) } }),
                )
            })
            .collect();
        let requests = observations_requests(&rows);
        assert!(requests.len() >= 2, "{}", requests.len());
        let total: usize = requests.iter().map(|(subjects, _)| subjects.len()).sum();
        assert_eq!(total, 12);
        for (_, request) in &requests {
            assert!(raw_len(request) <= MAX_REQUEST_BYTES);
        }
        assert_eq!(MAX_REQUEST_BYTES, 6 * 1024 * 1024);
    }

    // PHP's `json_encode` escapes `/` and every non-ASCII character, and the
    // API measures a payload that way.
    #[test]
    fn the_php_encoded_length_counts_escaped_slashes_and_unicode() {
        assert_eq!(php_encoded_len(&serde_json::json!({ "a": "b" })), 9);
        assert_eq!(php_encoded_len(&serde_json::json!("a/b")), 6);
        assert_eq!(php_encoded_len(&serde_json::json!("é")), 8);
        assert_eq!(php_encoded_len(&serde_json::json!("😀")), 14);
    }

    // The API refuses any payload over 4 MiB, the whole batch with it
    // (#498): such an event is skipped when queued.
    #[test]
    fn a_payload_over_4_mib_is_a_skipped_row() {
        let big = format!(
            r#"16:34:31 [INF] (Game Management|Processing Registered Handler for EventSyncData_t in TournamentUninitializedState) Processor: TournamentUninitializedState Message: {{"EventToken":"tok-1","Description":"{}"}} Receiver: WotC.MtGO.Client.Model.Play.TournamentEvent.Tournament"#,
            "/".repeat(2_200_000)
        );
        let huge = event("tournament_sync", &big, Some("tok-1"));
        assert_eq!(observation_skip_reason(&huge), Some(PAYLOAD_TOO_LARGE));
        assert_eq!(observation_row(&huge, LOGGED_AT).payload, None);
        let small = event("tournament_sync", SYNC, Some("tok-1"));
        assert_eq!(observation_skip_reason(&small), None);
    }

    // A row queued before the 4 MiB check is left out of every request (the
    // worker fails it for good rather than resending it forever).
    #[test]
    fn a_queued_row_over_4_mib_is_left_out() {
        let rows = vec![
            claimed(
                "1:1",
                &serde_json::json!({ "payload": { "big": "/".repeat(2_200_000) } }),
            ),
            claimed("1:2", &serde_json::json!({ "payload": { "n": 2 } })),
        ];
        let requests = observations_requests(&rows);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, vec!["1:2".to_string()]);
    }

    // The API's 422 names the refused observation by its index.
    #[test]
    fn a_422_names_the_refused_observations() {
        let body = br#"{"message":"bad","errors":{"observations.7.event_type":["x"],"observations.3":["y"],"observations.7.client_observed_at":["z"]}}"#;
        assert_eq!(refused_observations(body), vec![3, 7]);
        let whole = br#"{"message":"Invalid gzip body.","errors":{"body":"Invalid gzip body."}}"#;
        assert!(refused_observations(whole).is_empty());
        assert!(refused_observations(b"<html>").is_empty());
    }

    #[test]
    fn a_row_that_cannot_be_read_is_left_out() {
        let rows = vec![
            ClaimedRow {
                subject: "1:1".to_string(),
                payload: b"not zstd".to_vec(),
            },
            claimed("1:2", &serde_json::json!({ "payload": { "n": 2 } })),
        ];
        let requests = observations_requests(&rows);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, vec!["1:2".to_string()]);
    }

    /// Text that barely compresses: base64-like characters from a small
    /// linear congruential generator.
    fn noise(seed: u32, len: usize) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut state = u64::from(seed) * 2_654_435_761 + 1;
        let mut out = String::with_capacity(len);
        for _ in 0..len {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            out.push(ALPHABET[(state >> 58) as usize] as char);
        }
        out
    }
}
