//! Real tournament names (survey section 11, 07 spec section 8, ruling 12):
//! `GET /api/tournaments/{mtgo_event_id}` for the stored tournaments whose
//! name the league sync synthesized. Only `name` is kept; `format` and
//! `started_at` stay the league sync's.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::gate::RequestClass;
use crate::request::{ApiRequest, ApiResponse, AuthMode, Method, SHORT_TIMEOUT_MS};

/// 0.x `LinkMatchToTournament`'s pattern: the outer `"Token"` of a round
/// info line (read with a pattern, not a JSON parser: MTGO ships malformed
/// JSON).
static ROUND_INFO_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)"Token"\s*:\s*"(?P<token>[a-f0-9\-]{36})""#)
        .expect("the round info token pattern compiles")
});

/// The tournament token of a `tournament_round_info` line, if it has one.
pub fn round_info_tournament_token(raw: &str) -> Option<String> {
    ROUND_INFO_TOKEN
        .captures(raw)
        .map(|found| found["token"].to_string())
}

/// Lookups per run at most (0.x `RefreshTournamentMetadata::PER_RUN_CAP`).
pub const PER_RUN_CAP: u32 = 50;

/// `GET /api/tournaments/{id}`: class `Community`, 5 s (0.x `timeout(5)`).
pub fn tournament_request(mtgo_event_id: i64) -> ApiRequest {
    let mut request = ApiRequest::new(
        Method::Get,
        &format!("/api/tournaments/{mtgo_event_id}"),
        RequestClass::Community,
        AuthMode::DeviceOrBearer,
    );
    request.timeout_ms = SHORT_TIMEOUT_MS;
    request.connect_timeout_ms = SHORT_TIMEOUT_MS;
    request
}

/// What a lookup learned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TournamentLookup {
    /// 200 with a name: the real name.
    Named(String),
    /// 404: the API does not know the event; it is not asked again.
    NotFound,
    /// Anything else (a 200 without a name included): asked again next run.
    Failed,
}

pub fn parse_tournament(response: &ApiResponse) -> TournamentLookup {
    if response.status == 404 {
        return TournamentLookup::NotFound;
    }
    if !response.is_success() {
        return TournamentLookup::Failed;
    }
    let name = response
        .json::<Value>()
        .ok()
        .and_then(|body| body.get("name").and_then(Value::as_str).map(str::to_string));
    match name {
        Some(name) if !name.trim().is_empty() => TournamentLookup::Named(name),
        _ => TournamentLookup::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::response;

    // 0.x LinkMatchToTournament: the outer token, case-insensitive, also
    // when the JSON is malformed; nothing for an inner `EventToken` alone.
    #[test]
    fn the_round_info_token_is_the_outer_token() {
        let raw = r#"Message: {"Token":"BBBBBBBB-bbbb-bbbb-bbbb-bbbbbbbbbbbb","Round":{"Matches":[{"EventToken":"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"}]} Receiver"#;
        assert_eq!(
            round_info_tournament_token(raw).as_deref(),
            Some("BBBBBBBB-bbbb-bbbb-bbbb-bbbbbbbbbbbb")
        );
        assert_eq!(
            round_info_tournament_token(r#"{"token" : "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"}"#)
                .as_deref(),
            Some("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb")
        );
        assert_eq!(
            round_info_tournament_token(
                r#"{"EventToken":"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa","Token":"not-a-guid"}"#
            ),
            None
        );
    }
    use crate::gate::RequestClass;
    use crate::request::{AuthMode, Body, Method};

    #[test]
    fn the_lookup_gets_the_event_as_community_in_five_seconds() {
        let request = tournament_request(12_841_367);
        assert_eq!(request.method, Method::Get);
        assert_eq!(request.path, "/api/tournaments/12841367");
        assert_eq!(request.class, RequestClass::Community);
        assert_eq!(request.auth, AuthMode::DeviceOrBearer);
        assert_eq!(
            (request.timeout_ms, request.connect_timeout_ms),
            (5_000, 5_000)
        );
        assert_eq!(request.body, Body::Empty);
        assert_eq!(PER_RUN_CAP, 50);
    }

    // 0.x FetchTournamentMetadataTest: "returns metadata when API responds
    // 200" (only the name is kept, ruling 12).
    #[test]
    fn returns_the_name_when_the_api_responds_200() {
        let body = r#"{"mtgo_event_id":12841367,"name":"Qlegacy Challenge 32","format":"CLEGACY",
                       "started_at":"2026-05-03T05:00:00.000000Z"}"#;
        assert_eq!(
            parse_tournament(&response(200, body)),
            TournamentLookup::Named("Qlegacy Challenge 32".to_string())
        );
    }

    // 0.x FetchTournamentMetadataTest: "returns null on 404".
    #[test]
    fn a_404_is_not_found() {
        assert_eq!(
            parse_tournament(&response(404, "{}")),
            TournamentLookup::NotFound
        );
    }

    // 0.x FetchTournamentMetadataTest: "returns null when request throws"
    // (the app maps a request error to `Failed`); any other answer is a
    // failure, asked again next run.
    #[test]
    fn anything_else_is_a_failure() {
        for (status, body) in [
            (500, "{}"),
            (429, "{}"),
            (200, "{}"),
            (200, r#"{"name":""}"#),
            (200, r#"{"name":"   "}"#),
            (200, "[1"),
        ] {
            assert_eq!(
                parse_tournament(&response(status, body)),
                TournamentLookup::Failed,
                "{status} {body}"
            );
        }
    }
}
