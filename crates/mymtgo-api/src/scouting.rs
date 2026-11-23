//! Opponent scouting for the game overlay (overlay spec section 7.3, 0.x
//! `FetchOpponentScouting`): what the API knows about an opponent's deck in
//! one format, by source. Class `Lookup`: sent with the device key or
//! bearer, never offline. The colours 0.x read from its archetype table come
//! from v1's held rows in the overlay worker, not from here.

use serde::Serialize;
use serde_json::Value;

use crate::gate::RequestClass;
use crate::loose;
use crate::request::{ApiRequest, ApiResponse, AuthMode, Body, Method};

pub const PLAYERS_PATH: &str = "/api/players";

/// One archetype a scouting source names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoutedArchetype {
    pub uuid: String,
    pub name: String,
}

/// The three sources 0.x reads: the opponent's latest 5-0 list (`league`),
/// the last deck they tracked (`tracked`) and the deck other tracker users
/// last saw them on (`observed`). At least one is set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scouting {
    pub league: Option<ScoutedArchetype>,
    pub tracked: Option<ScoutedArchetype>,
    pub observed: Option<ScoutedArchetype>,
}

/// `POST /api/players` with `{"username", "format"}`; `format_key` is the
/// bare constructed key (`modern` for `CModern`, 0.x `MtgoFormat::key`).
pub fn players_request(username: &str, format_key: &str) -> ApiRequest {
    let mut request = ApiRequest::new(
        Method::Post,
        PLAYERS_PATH,
        RequestClass::Lookup,
        AuthMode::DeviceOrBearer,
    );
    let body = PlayersBody {
        username: username.to_string(),
        format: format_key.to_string(),
    };
    request.body = Body::json(&body);
    request
}

/// The players body, its keys in 0.x's order.
#[derive(Serialize)]
struct PlayersBody {
    username: String,
    format: String,
}

/// The sources of a 2xx answer (0.x `$response->successful()`); `None` for
/// any other status, an answer that is not JSON, or one where no source
/// carries an archetype with a uuid and a name.
pub fn parse_players(response: &ApiResponse) -> Option<Scouting> {
    if !response.is_success() {
        return None;
    }
    let body: Value = response.json().ok()?;
    let data = body.get("data")?;
    let scouting = Scouting {
        league: source(data, "league_result"),
        tracked: source(data, "tracked"),
        observed: source(data, "observed"),
    };
    if scouting.league.is_none() && scouting.tracked.is_none() && scouting.observed.is_none() {
        return None;
    }
    Some(scouting)
}

/// `data.{key}.archetype` when it carries a uuid and a name (0.x `isset`).
fn source(data: &Value, key: &str) -> Option<ScoutedArchetype> {
    let archetype = data.get(key)?.get("archetype")?;
    Some(ScoutedArchetype {
        uuid: loose::text(archetype, "uuid")?,
        name: loose::text(archetype, "name")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::response;

    fn archetype(uuid: &str, name: &str) -> Option<ScoutedArchetype> {
        Some(ScoutedArchetype {
            uuid: uuid.to_string(),
            name: name.to_string(),
        })
    }

    // 0.x "returns archetype with colors from local archetype lookup on 200":
    // the request it sends (the colours are U7's, from held rows).
    #[test]
    fn the_players_request_posts_the_username_and_format_key() {
        let request = players_request("Qfoo", "modern");
        assert_eq!(request.method, Method::Post);
        assert_eq!(request.path, "/api/players");
        assert_eq!(request.class, RequestClass::Lookup);
        assert_eq!(request.auth, AuthMode::DeviceOrBearer);
        assert_eq!(
            request.body.bytes(),
            br#"{"username":"Qfoo","format":"modern"}"#.as_slice()
        );
    }

    // 0.x "returns archetype with colors from local archetype lookup on 200".
    #[test]
    fn a_league_result_is_read_on_200() {
        let body = r#"{"data":{"player":"Qfoo","league_result":{"archetype":
            {"uuid":"arch-uuid-1","name":"Izzet Phoenix","slug":"izzet-phoenix"}}}}"#;
        assert_eq!(
            parse_players(&response(200, body)),
            Some(Scouting {
                league: archetype("arch-uuid-1", "Izzet Phoenix"),
                tracked: None,
                observed: None,
            })
        );
    }

    // 0.x "returns archetype with null colors when local archetype is
    // missing": an archetype v1 may not hold is read all the same.
    #[test]
    fn an_unknown_archetype_is_read_by_its_uuid_and_name() {
        let body = r#"{"data":{"league_result":{"archetype":
            {"uuid":"unknown-uuid","name":"Mystery Brew","slug":"mystery"}}}}"#;
        assert_eq!(
            parse_players(&response(200, body)),
            Some(Scouting {
                league: archetype("unknown-uuid", "Mystery Brew"),
                tracked: None,
                observed: None,
            })
        );
    }

    // 0.x "returns null on 404 response".
    #[test]
    fn a_404_is_nothing() {
        let body = r#"{"message":"not found"}"#;
        assert_eq!(parse_players(&response(404, body)), None);
        assert_eq!(parse_players(&response(500, body)), None);
    }

    // 0.x "returns null when no source carries an archetype".
    #[test]
    fn no_source_with_an_archetype_is_nothing() {
        let body = r#"{"data":{"league_result":{"archetype":null}}}"#;
        assert_eq!(parse_players(&response(200, body)), None);
        // An archetype without a uuid or a name does not count (0.x isset).
        let partial = r#"{"data":{"tracked":{"archetype":{"name":"Qdeck"}},
            "observed":{"archetype":{"uuid":"q-uuid","name":null}}}}"#;
        assert_eq!(parse_players(&response(200, partial)), None);
        assert_eq!(parse_players(&response(200, "{}")), None);
    }

    // 0.x "returns tracked and observed archetypes alongside the league one".
    #[test]
    fn tracked_and_observed_are_read_alongside_the_league_one() {
        let body = r#"{"data":{"league_result":null,
            "tracked":{"archetype":{"uuid":"tracked-uuid","name":"Boros Energy","slug":"boros-energy"}},
            "observed":{"archetype":{"uuid":"observed-uuid","name":"Amulet Titan","slug":"amulet-titan"}}}}"#;
        assert_eq!(
            parse_players(&response(200, body)),
            Some(Scouting {
                league: None,
                tracked: archetype("tracked-uuid", "Boros Energy"),
                observed: archetype("observed-uuid", "Amulet Titan"),
            })
        );
    }

    // 0.x "returns null when http throws": an answer that is not JSON reads
    // as nothing (a failed connection never reaches the parser; the worker
    // treats it as nothing too).
    #[test]
    fn an_unreadable_answer_is_nothing() {
        assert_eq!(parse_players(&response(200, "<html>")), None);
        assert_eq!(parse_players(&response(200, "")), None);
    }
}
