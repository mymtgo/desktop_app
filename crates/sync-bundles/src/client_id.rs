//! Sync client ids (0.x `DeckClientId`, `LeagueClientId`): the server
//! accepts `[A-Za-z0-9][A-Za-z0-9_-]{0,190}`. A match's client id is its
//! token.

use chrono::DateTime;

/// 0.x `DeckClientId::for`: every byte outside `[A-Za-z0-9_-]` becomes `_`
/// (`limited:{key}` becomes `limited_{key}`; a multi-byte character becomes
/// one underscore per byte).
pub fn deck_client_id(mtgo_id: &str) -> String {
    mtgo_id
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-' {
                byte as char
            } else {
                '_'
            }
        })
        .collect()
}

/// 0.x `LeagueClientId::for`: the league token is reused across runs, so
/// the run's start (UTC, `Ymd\THis\Z`) makes it unique.
pub fn league_client_id(token: &str, started_at_ms: i64) -> String {
    let started = DateTime::from_timestamp_millis(started_at_ms).unwrap_or_default();
    format!("{token}_{}", started.format("%Y%m%dT%H%M%SZ"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn golden() -> Value {
        serde_json::from_str(include_str!(
            "../../../fixtures/parity/sync_canonical.golden.json"
        ))
        .expect("the canonical golden is JSON")
    }

    // 0.x BundleBuilderTest: "sanitizes a limited deck mtgo_id into a
    // server-legal client id".
    #[test]
    fn sanitizes_a_limited_deck_mtgo_id_into_a_server_legal_client_id() {
        let id = deck_client_id("limited:791bacca-caea-4d88-b6c7-3bc067d412c2");
        assert_eq!(id, "limited_791bacca-caea-4d88-b6c7-3bc067d412c2");
        let legal = id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
            && id.len() <= 191
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        assert!(legal, "{id}");
        // Every byte outside the rule becomes one underscore.
        assert_eq!(deck_client_id("é"), "__");
    }

    #[test]
    fn client_ids_match_0x() {
        let golden = golden();
        for case in golden["deck_client_ids"].as_array().expect("deck ids") {
            assert_eq!(
                deck_client_id(case["mtgo_id"].as_str().expect("mtgo id")),
                case["client_id"].as_str().expect("client id")
            );
        }
        for case in golden["league_client_ids"].as_array().expect("league ids") {
            assert_eq!(
                league_client_id(
                    case["token"].as_str().expect("token"),
                    case["started_at_ms"].as_i64().expect("ms")
                ),
                case["client_id"].as_str().expect("client id")
            );
        }
    }

    // 0.x "gives two leagues sharing a token two distinct client ids".
    #[test]
    fn two_leagues_sharing_a_token_get_two_client_ids() {
        assert_ne!(
            league_client_id("q-token", 1_787_133_605_000),
            league_client_id("q-token", 1_787_133_606_000)
        );
    }
}
