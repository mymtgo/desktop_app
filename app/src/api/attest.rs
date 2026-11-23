//! Account attestation as IO (survey section 6, 0.x `AttestAccount` and
//! `AttestKnownAccounts`). Triggers: sign-in success here; a newly seen
//! `(login_id, username)` pair on a worker tick (Q5), leaving offline mode
//! (Q4) and the end of each sync run (Q10). Never on a timer: the API
//! allows 10 an hour.

use mymtgo_api::{
    ApiError, AttestAnswer, AttestSkip, KnownAccount, RETRY_DELAYS_MS, attest_request, attest_skip,
    attested_json, owed_accounts, parse_attest, record_attested,
};

use crate::api::session::ApiState;
use crate::bridge::lock;
use crate::settings::API_ATTESTED;

/// How one attestation ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttestOutcome {
    /// Nothing was sent, and why.
    Skipped(AttestSkip),
    /// Another attestation of this login is running.
    InFlight,
    /// Confirmed and recorded.
    Confirmed,
    /// 409: another MyMTGO user owns this MTGO login. Recorded nothing.
    OwnedByOther,
    /// Every attempt failed; a later trigger tries again.
    Owed,
}

/// Removes a login id from the in-flight set when the attestation ends.
struct InFlight<'a> {
    state: &'a ApiState,
    login_id: i64,
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        lock(&self.state.attesting).remove(&self.login_id);
    }
}

/// Attest one account: skipped offline, signed out, without a login id or
/// when this login is already confirmed with this username; otherwise up to
/// three attempts, `sleep(ms)` before the second and third. A 409 logs a
/// warning and records nothing.
pub fn attest_account(
    state: &ApiState,
    account: &KnownAccount,
    now_ms: i64,
    sleep: &dyn Fn(u64),
) -> AttestOutcome {
    let skip = {
        let session = state.session();
        let mut gate = session.gate;
        gate.linked = session.linked();
        attest_skip(&gate, account, &session.attested)
    };
    if let Some(skip) = skip {
        tracing::info!(login_id = ?account.login_id, "{}", skip.message());
        return AttestOutcome::Skipped(skip);
    }
    let Some(login_id) = account.login_id else {
        return AttestOutcome::Skipped(AttestSkip::NoLoginId);
    };
    if !lock(&state.attesting).insert(login_id) {
        return AttestOutcome::InFlight;
    }
    let _in_flight = InFlight { state, login_id };
    let request = attest_request(login_id, &account.username);
    let delays = [None, Some(RETRY_DELAYS_MS[0]), Some(RETRY_DELAYS_MS[1])];
    let mut last_error = ApiError::NotLinked;
    for delay in delays {
        if let Some(ms) = delay {
            sleep(ms);
        }
        let answer = match state.send(&request, now_ms) {
            Ok(response) => parse_attest(&response),
            // Signed out or gated since the check: a later trigger re-attests.
            Err(error @ (ApiError::NotLinked | ApiError::Offline)) => {
                tracing::info!(%error, login_id, "Attest: stopped before sending.");
                return AttestOutcome::Owed;
            }
            Err(error) => AttestAnswer::Failed(error),
        };
        match answer {
            AttestAnswer::Confirmed => {
                record(state, login_id, &account.username);
                tracing::info!(login_id, "Attest: confirmed the MTGO account.");
                return AttestOutcome::Confirmed;
            }
            AttestAnswer::OwnedByOther => {
                tracing::warn!(
                    login_id,
                    "Attest: MTGO account is owned by another MyMTGO user."
                );
                return AttestOutcome::OwnedByOther;
            }
            AttestAnswer::Failed(error) => last_error = error,
        }
    }
    tracing::warn!(
        login_id,
        error = %last_error,
        "Attest: could not confirm the MTGO account, leaving it owed."
    );
    AttestOutcome::Owed
}

/// Remember a confirmed login in memory and in the settings row.
fn record(state: &ApiState, login_id: i64, username: &str) {
    // Held until the map is stored, so a newer map is always stored last.
    let _one_save = lock(&state.attested_save);
    let json = {
        let mut session = state.session();
        record_attested(&mut session.attested, login_id, username);
        attested_json(&session.attested)
    };
    state.persist(vec![(API_ATTESTED.to_string(), json)]);
}

/// Attest every local account that is owed (0.x `AttestKnownAccounts`),
/// one after another, pausing with a real sleep between attempts.
pub fn attest_known_accounts(state: &ApiState, now_ms: i64) {
    let sleep = |ms: u64| std::thread::sleep(std::time::Duration::from_millis(ms));
    attest_known_accounts_with(state, now_ms, &sleep);
}

/// How many owed accounts one pass confirmed and how many stay owed (0.x
/// `AttestKnownAccounts::run`'s `confirmed` and `owed`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AttestCounts {
    pub confirmed: u32,
    pub owed: u32,
}

/// `attest_known_accounts` with the pause between attempts given, counted
/// (the sync run's activity lines).
pub fn attest_known_accounts_with(
    state: &ApiState,
    now_ms: i64,
    sleep: &dyn Fn(u64),
) -> AttestCounts {
    let attested = state.session().attested.clone();
    let mut counts = AttestCounts::default();
    for account in owed_accounts(&state.accounts(), &attested) {
        match attest_account(state, &account, now_ms, sleep) {
            AttestOutcome::Confirmed => counts.confirmed += 1,
            AttestOutcome::Owed => counts.owed += 1,
            _ => {}
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::secrets::{ACCESS_TOKEN, REFRESH_TOKEN};
    use crate::api::session::testing::{TestApi, test_api};
    use crate::api::stub::{Reply, Stub};
    use crate::bridge::lock;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    const NOW: i64 = 1_800_000_000_000;
    const MONTH: i64 = 30 * 24 * 3_600_000;
    const CONFIRMED: &str = r#"{"id":1,"username":"Qplayer","login_id":6031972,"public":false}"#;

    fn account(username: &str, login_id: Option<i64>) -> KnownAccount {
        KnownAccount {
            username: username.to_string(),
            login_id,
        }
    }

    fn qplayer() -> KnownAccount {
        account("Qplayer", Some(6_031_972))
    }

    /// Signed in, with these confirmed logins.
    fn linked(base_url: &str, attested: &str) -> TestApi {
        let expires = (NOW + MONTH).to_string();
        test_api(
            base_url,
            &[
                ("api_token_expires_at", &expires),
                ("api_attested", attested),
            ],
            &[
                (ACCESS_TOKEN, "access-token"),
                (REFRESH_TOKEN, "refresh-token"),
            ],
        )
    }

    /// Records the pauses instead of sleeping.
    fn recorded_sleeps() -> (Arc<Mutex<Vec<u64>>>, impl Fn(u64)) {
        let sleeps = Arc::new(Mutex::new(Vec::new()));
        let record = {
            let sleeps = Arc::clone(&sleeps);
            move |ms: u64| lock(&sleeps).push(ms)
        };
        (sleeps, record)
    }

    fn no_sleep(_: u64) {}

    // 0.x AttestAccountTest: "posts the login id and username with the bearer".
    #[test]
    fn posts_the_login_id_and_username_with_the_bearer() {
        let stub = Stub::start(|_| Reply::json(200, CONFIRMED));
        let api = linked(&stub.base_url, "{}");
        assert_eq!(
            attest_account(&api.state, &qplayer(), NOW, &no_sleep),
            AttestOutcome::Confirmed
        );
        let seen = stub.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].method, "POST");
        assert_eq!(seen[0].target, "/api/account/players");
        assert_eq!(seen[0].header("authorization"), Some("Bearer access-token"));
        assert_eq!(seen[0].header("x-api-key"), None);
        assert_eq!(
            seen[0].body,
            br#"{"login_id":6031972,"username":"Qplayer"}"#.to_vec()
        );
        assert_eq!(
            api.saved("api_attested").as_deref(),
            Some(r#"{"6031972":"Qplayer"}"#)
        );
    }

    // 0.x AttestAccountTest: "logs and stops on 409 rather than retrying".
    #[test]
    fn logs_and_stops_on_409_rather_than_retrying() {
        let stub = Stub::start(|_| Reply::json(409, r#"{"reason":"owned_by_other"}"#));
        let api = linked(&stub.base_url, "{}");
        let (sleeps, sleep) = recorded_sleeps();
        assert_eq!(
            attest_account(&api.state, &qplayer(), NOW, &sleep),
            AttestOutcome::OwnedByOther
        );
        assert_eq!(stub.requests().len(), 1);
        assert!(lock(&sleeps).is_empty());
    }

    // 0.x AttestAccountTest: "does nothing for an account without a login id
    // or when not linked".
    #[test]
    fn does_nothing_for_an_account_without_a_login_id_or_when_not_linked() {
        let stub = Stub::start(|_| Reply::json(200, CONFIRMED));
        let api = linked(&stub.base_url, "{}");
        assert_eq!(
            attest_account(&api.state, &account("Qa", None), NOW, &no_sleep),
            AttestOutcome::Skipped(AttestSkip::NoLoginId)
        );
        let unlinked = test_api(&stub.base_url, &[], &[]);
        assert_eq!(
            attest_account(&unlinked.state, &account("Qb", Some(5)), NOW, &no_sleep),
            AttestOutcome::Skipped(AttestSkip::NotLinked)
        );
        let offline = linked(&stub.base_url, "{}");
        offline.state.session().gate.offline = true;
        assert_eq!(
            attest_account(&offline.state, &qplayer(), NOW, &no_sleep),
            AttestOutcome::Skipped(AttestSkip::Offline)
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(stub.connections(), 0);
    }

    // 0.x AttestAccountTest: "dispatches when a linked client learns or
    // changes an account login id".
    #[test]
    fn attests_when_a_linked_client_learns_an_account_login_id() {
        let stub = Stub::start(|_| Reply::json(200, CONFIRMED));
        let api = linked(&stub.base_url, "{}");
        *lock(&api.accounts) = vec![account("Qplayer", None)];
        attest_known_accounts(&api.state, NOW);
        assert_eq!(stub.requests().len(), 0, "no login id yet");
        *lock(&api.accounts) = vec![qplayer()];
        attest_known_accounts(&api.state, NOW);
        assert_eq!(stub.requests().len(), 1);
        attest_known_accounts(&api.state, NOW);
        assert_eq!(stub.requests().len(), 1, "confirmed once");
    }

    // 0.x AttestAccountTest: "dispatches nothing when the client is not linked".
    #[test]
    fn attests_nothing_when_the_client_is_not_linked() {
        let stub = Stub::start(|_| Reply::json(200, CONFIRMED));
        let api = test_api(&stub.base_url, &[], &[]);
        *lock(&api.accounts) = vec![qplayer()];
        attest_known_accounts(&api.state, NOW);
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(stub.connections(), 0);
    }

    // 0.x AttestAccountTest: "keeps trying when the first attempts fail, then
    // records the confirmation".
    #[test]
    fn keeps_trying_when_the_first_attempts_fail_then_records_the_confirmation() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let stub = {
            let attempts = Arc::clone(&attempts);
            Stub::start(move |_| {
                if attempts.fetch_add(1, Ordering::SeqCst) < 2 {
                    Reply::json(500, r#"{"message":"Server Error"}"#)
                } else {
                    Reply::json(200, CONFIRMED)
                }
            })
        };
        let api = linked(&stub.base_url, "{}");
        let (sleeps, sleep) = recorded_sleeps();
        assert_eq!(
            attest_account(&api.state, &qplayer(), NOW, &sleep),
            AttestOutcome::Confirmed
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        assert_eq!(*lock(&sleeps), vec![250, 1000]);
        assert_eq!(
            api.saved("api_attested").as_deref(),
            Some(r#"{"6031972":"Qplayer"}"#)
        );
    }

    // 0.x AttestAccountTest: "leaves the account owed when every attempt fails".
    #[test]
    fn leaves_the_account_owed_when_every_attempt_fails() {
        let stub = Stub::start(|_| Reply::json(500, r#"{"message":"Server Error"}"#));
        let api = linked(&stub.base_url, "{}");
        assert_eq!(
            attest_account(&api.state, &qplayer(), NOW, &no_sleep),
            AttestOutcome::Owed
        );
        assert_eq!(stub.requests().len(), 3);
        assert_eq!(api.saved("api_attested"), None);
        assert!(api.state.session().attested.is_empty());
    }

    // 0.x AttestAccountTest: "does not post again once the same login and
    // username are confirmed".
    #[test]
    fn does_not_post_again_once_the_same_login_and_username_are_confirmed() {
        let stub = Stub::start(|_| Reply::json(200, CONFIRMED));
        let api = linked(&stub.base_url, r#"{"6031972":"Qplayer"}"#);
        assert_eq!(
            attest_account(&api.state, &qplayer(), NOW, &no_sleep),
            AttestOutcome::Skipped(AttestSkip::AlreadyAttested)
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(stub.connections(), 0);
    }

    // 0.x AttestAccountTest: "posts again when the username behind a
    // confirmed login changes".
    #[test]
    fn posts_again_when_the_username_behind_a_confirmed_login_changes() {
        let stub = Stub::start(|_| Reply::json(200, CONFIRMED));
        let api = linked(&stub.base_url, r#"{"6031972":"Qoldname"}"#);
        assert_eq!(
            attest_account(&api.state, &qplayer(), NOW, &no_sleep),
            AttestOutcome::Confirmed
        );
        let body = String::from_utf8(stub.requests()[0].body.clone()).expect("utf8");
        assert!(body.contains(r#""username":"Qplayer""#), "{body}");
        assert_eq!(
            api.saved("api_attested").as_deref(),
            Some(r#"{"6031972":"Qplayer"}"#)
        );
    }

    // 0.x AttestAccountTest: "records nothing when the MTGO account belongs
    // to another user".
    #[test]
    fn records_nothing_when_the_mtgo_account_belongs_to_another_user() {
        let stub = Stub::start(|_| Reply::json(409, r#"{"reason":"owned_by_other"}"#));
        let api = linked(&stub.base_url, "{}");
        attest_account(&api.state, &qplayer(), NOW, &no_sleep);
        assert_eq!(api.saved("api_attested"), None);
        assert!(api.state.session().attested.is_empty());
    }

    // 0.x AttestAccountTest: "still attests after an earlier attempt failed
    // inside the unique window": one attest in flight per account, and a
    // finished (failed) one never swallows the next.
    #[test]
    fn still_attests_after_an_earlier_attempt_failed() {
        let refuse = Arc::new(AtomicUsize::new(1));
        let stub = {
            let refuse = Arc::clone(&refuse);
            Stub::start(move |_| {
                if refuse.load(Ordering::SeqCst) == 1 {
                    Reply::json(401, r#"{"message":"Unauthenticated."}"#)
                } else {
                    Reply::json(200, CONFIRMED)
                }
            })
        };
        let api = linked(&stub.base_url, "{}");
        assert_eq!(
            attest_account(&api.state, &qplayer(), NOW, &no_sleep),
            AttestOutcome::Owed
        );
        refuse.store(0, Ordering::SeqCst);
        assert_eq!(
            attest_account(&api.state, &qplayer(), NOW, &no_sleep),
            AttestOutcome::Confirmed
        );
        assert_eq!(
            api.state
                .session()
                .attested
                .get("6031972")
                .map(String::as_str),
            Some("Qplayer")
        );
    }

    #[test]
    fn one_attest_in_flight_per_account() {
        let stub = Stub::start(|_| {
            std::thread::sleep(std::time::Duration::from_millis(300));
            Reply::json(200, CONFIRMED)
        });
        let api = Arc::new(linked(&stub.base_url, "{}"));
        let first = {
            let api = Arc::clone(&api);
            std::thread::spawn(move || attest_account(&api.state, &qplayer(), NOW, &no_sleep))
        };
        // Let the first attempt reach the stub.
        while stub.connections() == 0 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            attest_account(&api.state, &qplayer(), NOW, &no_sleep),
            AttestOutcome::InFlight
        );
        assert_eq!(first.join().expect("joined"), AttestOutcome::Confirmed);
        assert_eq!(stub.requests().len(), 1);
    }

    // Q3 review Minor 6: two confirmations saved at once never leave the
    // older map stored last.
    #[test]
    fn concurrent_confirmations_never_store_an_older_map_last() {
        let api = Arc::new(linked("http://127.0.0.1:1", "{}"));
        let entered = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        let first = Arc::new(std::sync::atomic::AtomicBool::new(true));
        {
            let (entered, release, first) = (
                Arc::clone(&entered),
                Arc::clone(&release),
                Arc::clone(&first),
            );
            *lock(&api.persist_hook) = Some(Arc::new(move || {
                if first.swap(false, Ordering::SeqCst) {
                    entered.wait();
                    release.wait();
                }
                Ok(())
            }));
        }
        let one = {
            let api = Arc::clone(&api);
            std::thread::spawn(move || record(&api.state, 1, "Qone"))
        };
        entered.wait();
        let two = {
            let api = Arc::clone(&api);
            std::thread::spawn(move || record(&api.state, 2, "Qtwo"))
        };
        std::thread::sleep(std::time::Duration::from_millis(100));
        release.wait();
        one.join().expect("joined");
        two.join().expect("joined");
        let stored = mymtgo_api::parse_attested(&api.saved("api_attested").expect("stored"));
        assert_eq!(stored.len(), 2, "{stored:?}");
    }
}
