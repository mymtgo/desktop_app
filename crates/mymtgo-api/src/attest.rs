//! Account attestation (survey section 6, 0.x `AttestAccount` and
//! `AccountApi`): a signed-in client tells the API which MTGO login it has
//! seen for each local account, so synced matches show under the right
//! player. Never on a timer: the API allows 10 an hour (`throttle:attest`).

use std::collections::BTreeMap;

use crate::error::ApiError;
use crate::gate::{GateState, RequestClass};
use crate::request::{ApiRequest, ApiResponse, AuthMode, Body, Method};

pub const ATTEST_PATH: &str = "/api/account/players";
/// The pauses before the second and third attempt (0.x `RETRY_DELAYS_MS`).
pub const RETRY_DELAYS_MS: [u64; 2] = [250, 1000];

/// The confirmed logins: login id (as text, the JSON key) to username.
/// Settings row `api_attested`.
pub type Attested = BTreeMap<String, String>;

/// A local account as attestation needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownAccount {
    pub username: String,
    pub login_id: Option<i64>,
}

/// `POST /api/account/players` with `{"login_id": <int>, "username": ".."}`:
/// class `Sync`, bearer only.
pub fn attest_request(login_id: i64, username: &str) -> ApiRequest {
    let mut request = ApiRequest::new(
        Method::Post,
        ATTEST_PATH,
        RequestClass::Sync,
        AuthMode::Bearer,
    );
    let body = serde_json::json!({ "login_id": login_id, "username": username });
    request.body = Body::json(&body);
    request
}

/// What an attestation answer means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttestAnswer {
    /// The login is now confirmed as this user's: record it.
    Confirmed,
    /// 409: another MyMTGO user owns this MTGO login. A support case: log,
    /// record nothing, do not retry.
    OwnedByOther,
    /// Anything else: retry in place.
    Failed(ApiError),
}

pub fn parse_attest(response: &ApiResponse) -> AttestAnswer {
    if response.is_success() {
        AttestAnswer::Confirmed
    } else if response.status == 409 {
        AttestAnswer::OwnedByOther
    } else {
        AttestAnswer::Failed(response.status_error())
    }
}

/// Why an account is not attested now (0.x logged each exit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttestSkip {
    Offline,
    NotLinked,
    NoLoginId,
    /// This login id is already confirmed with this username.
    AlreadyAttested,
}

impl AttestSkip {
    /// The log line (0.x wording).
    pub fn message(self) -> &'static str {
        match self {
            AttestSkip::Offline => "Attest: skipped, offline mode is on.",
            AttestSkip::NotLinked => "Attest: skipped, this device is not linked.",
            AttestSkip::NoLoginId => "Attest: skipped, no MTGO login id for this account.",
            AttestSkip::AlreadyAttested => "Attest: skipped, already confirmed.",
        }
    }
}

/// Whether to skip attesting `account` now, and why.
pub fn attest_skip(
    gate: &GateState,
    account: &KnownAccount,
    attested: &Attested,
) -> Option<AttestSkip> {
    if gate.offline {
        return Some(AttestSkip::Offline);
    }
    if !gate.linked {
        return Some(AttestSkip::NotLinked);
    }
    let Some(login_id) = account.login_id else {
        return Some(AttestSkip::NoLoginId);
    };
    if attested.get(&login_id.to_string()) == Some(&account.username) {
        return Some(AttestSkip::AlreadyAttested);
    }
    None
}

/// The accounts with a login id that are not confirmed with their current
/// username, in the order given.
pub fn owed_accounts(accounts: &[KnownAccount], attested: &Attested) -> Vec<KnownAccount> {
    let mut owed = Vec::new();
    for account in accounts {
        let Some(login_id) = account.login_id else {
            continue;
        };
        if attested.get(&login_id.to_string()) != Some(&account.username) {
            owed.push(account.clone());
        }
    }
    owed
}

/// Remember a confirmed login (replacing an older username for it).
pub fn record_attested(attested: &mut Attested, login_id: i64, username: &str) {
    attested.insert(login_id.to_string(), username.to_string());
}

/// The settings row's value.
pub fn attested_json(attested: &Attested) -> String {
    serde_json::to_string(attested).unwrap_or_else(|_| "{}".to_string())
}

/// The settings row read back; anything that is not a map of strings reads
/// as empty (0.x stored `[]` for an empty map).
pub fn parse_attested(value: &str) -> Attested {
    serde_json::from_str(value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::RequestClass;
    use crate::request::{AuthMode, Body, Method};
    use crate::test_support::response;

    fn account(username: &str, login_id: Option<i64>) -> KnownAccount {
        KnownAccount {
            username: username.to_string(),
            login_id,
        }
    }

    fn attested(list: &[(&str, &str)]) -> Attested {
        list.iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    // 0.x AttestAccountTest: "posts the login id and username with the bearer".
    #[test]
    fn posts_the_login_id_and_username_with_the_bearer() {
        let request = attest_request(6_031_972, "Qplayer");
        assert_eq!(request.method, Method::Post);
        assert_eq!(request.path, "/api/account/players");
        assert_eq!(request.class, RequestClass::Sync);
        assert_eq!(request.auth, AuthMode::Bearer);
        assert_eq!(
            request.body,
            Body::Json(br#"{"login_id":6031972,"username":"Qplayer"}"#.to_vec())
        );
    }

    #[test]
    fn a_2xx_confirms_a_409_is_owned_by_another_and_the_rest_fail() {
        assert_eq!(
            parse_attest(&response(200, r#"{"id":1,"public":false}"#)),
            AttestAnswer::Confirmed
        );
        assert_eq!(parse_attest(&response(201, "")), AttestAnswer::Confirmed);
        assert_eq!(
            parse_attest(&response(409, r#"{"reason":"owned_by_other"}"#)),
            AttestAnswer::OwnedByOther
        );
        assert_eq!(
            parse_attest(&response(500, r#"{"message":"Server Error"}"#)),
            AttestAnswer::Failed(ApiError::Status(
                500,
                r#"{"message":"Server Error"}"#.to_string()
            ))
        );
        assert!(matches!(
            parse_attest(&response(401, "")),
            AttestAnswer::Failed(ApiError::Status(401, _))
        ));
    }

    #[test]
    fn the_retry_delays_are_0x() {
        assert_eq!(RETRY_DELAYS_MS, [250, 1000]);
    }

    // 0.x AttestAccountTest: "does nothing for an account without a login id
    // or when not linked", "does not post again once the same login and
    // username are confirmed", "posts again when the username behind a
    // confirmed login changes".
    #[test]
    fn the_skip_reasons_follow_0x() {
        let online = GateState {
            offline: false,
            linked: true,
        };
        let keyed = account("Qplayer", Some(6_031_972));
        let none = Attested::new();
        assert_eq!(attest_skip(&online, &keyed, &none), None);
        let offline = GateState {
            offline: true,
            ..online
        };
        assert_eq!(
            attest_skip(&offline, &keyed, &none),
            Some(AttestSkip::Offline)
        );
        let unlinked = GateState {
            linked: false,
            ..online
        };
        assert_eq!(
            attest_skip(&unlinked, &keyed, &none),
            Some(AttestSkip::NotLinked)
        );
        assert_eq!(
            attest_skip(&online, &account("Qplayer", None), &none),
            Some(AttestSkip::NoLoginId)
        );
        let same = attested(&[("6031972", "Qplayer")]);
        assert_eq!(
            attest_skip(&online, &keyed, &same),
            Some(AttestSkip::AlreadyAttested)
        );
        let renamed = attested(&[("6031972", "Qoldname")]);
        assert_eq!(attest_skip(&online, &keyed, &renamed), None);
    }

    // 0.x AttestAccountTest: "dispatches when a linked client learns or
    // changes an account login id" and "dispatches nothing when the client is
    // not linked"; DeviceLinkTest "attests every account with a login id
    // once the device is linked".
    #[test]
    fn every_keyed_account_not_yet_confirmed_is_owed() {
        let accounts = vec![
            account("Qkeyed", Some(6_031_972)),
            account("Qunkeyed", None),
            account("Qdone", Some(7)),
            account("Qrenamed", Some(8)),
        ];
        let confirmed = attested(&[("7", "Qdone"), ("8", "Qold")]);
        assert_eq!(
            owed_accounts(&accounts, &confirmed),
            vec![
                account("Qkeyed", Some(6_031_972)),
                account("Qrenamed", Some(8))
            ]
        );
        assert!(owed_accounts(&[account("Qunkeyed", None)], &Attested::new()).is_empty());
    }

    #[test]
    fn the_attested_row_is_a_json_map_of_login_id_to_username() {
        let mut map = Attested::new();
        record_attested(&mut map, 6_031_972, "Qplayer");
        assert_eq!(attested_json(&map), r#"{"6031972":"Qplayer"}"#);
        assert_eq!(parse_attested(r#"{"6031972":"Qplayer"}"#), map);
        assert_eq!(parse_attested("not json"), Attested::new());
        assert_eq!(parse_attested("null"), Attested::new());
        assert_eq!(
            parse_attested("[]"),
            Attested::new(),
            "0.x stored [] when empty"
        );
    }
}
