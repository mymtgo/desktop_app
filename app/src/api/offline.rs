//! Offline mode as IO (07 spec section 5, survey section 5, 0.x
//! `UpdateOfflineModeController`): reading and switching it, the cooldown
//! after coming back online, and what follows a rejoin.
//!
//! The gate's flag lives in `ApiState`; the status bar's copy lives in the
//! shared status (`get_status`). Both change here, the gate first.

use chrono_tz::Tz;
use mymtgo_api::{
    OFFLINE_MODE, OFFLINE_MODE_LOCKED_UNTIL, cooldown_message, is_locked, lock_until, read_offline,
};
use serde::Serialize;
use tauri::{AppHandle, Manager};
use tauri_specta::Event;

use crate::api::attest::attest_known_accounts;
use crate::api::session::ApiState;
use crate::api::worker::WorkerSignal;
use crate::bridge::lock;
use crate::commands::{CommandError, Startup, off_async_workers, ready};
use crate::events::{ApiChanged, OfflineModeChanged};

/// What the offline mode card shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct OfflineModeState {
    pub enabled: bool,
    /// When the cooldown after coming back online ends (ms), as stored: the
    /// card compares it with the clock (0.x `offlineModeLockedUntil`).
    #[specta(type = Option<specta_typescript::Number>)]
    pub locked_until_ms: Option<i64>,
}

/// Offline mode as the gate has it now.
pub fn offline_mode_now(state: &ApiState) -> OfflineModeState {
    let (enabled, locked_until_ms) = state.offline_mode();
    OfflineModeState {
        enabled,
        locked_until_ms,
    }
}

/// Switch offline mode (0.x `UpdateOfflineModeController`).
///
/// - On: refused with the cooldown message while online and on cooldown.
///   Otherwise the gate goes offline first (and a sign-in waiting for the
///   browser is cancelled), then the row is stored; if the
///   store fails the app stays offline for this run and says so.
/// - Off: the row (and, when leaving offline mode, the cooldown until the
///   next local midnight) is stored first, then the gate opens; if the store
///   fails the app stays offline. Before any of that, leaving offline mode
///   runs `before_rejoin` (the archetypes held from the offline download go
///   back to Server, 2026-10-27 design section 5); if it fails nothing is
///   stored and the app stays offline. Leaving offline mode then runs `on_rejoin`
///   (attestation and the API worker's wake; the archetype resync arrives
///   with #9).
pub fn set_offline_mode_now(
    state: &ApiState,
    enabled: bool,
    now_ms: i64,
    tz: Tz,
    before_rejoin: &dyn Fn() -> Result<(), String>,
    on_rejoin: &dyn Fn(),
) -> Result<OfflineModeState, String> {
    let _one_switch = lock(&state.offline_switch);
    let (was_offline, locked_until) = state.offline_mode();
    if enabled {
        if !was_offline && is_locked(locked_until, now_ms) {
            return Err(cooldown_message(locked_until.unwrap_or(now_ms), now_ms));
        }
        state.set_offline_mode(true, None);
        // A sign-in waiting for the browser could only fail now: end it.
        state.sign_in.cancel_waiting();
        tracing::info!("offline mode switched on");
        state
            .try_persist(vec![(OFFLINE_MODE.to_string(), "true".to_string())])
            .map_err(|error| {
                format!("Offline mode is on, but it could not be saved, so it may be off after a restart: {error}")
            })?;
        return Ok(offline_mode_now(state));
    }
    if was_offline {
        before_rejoin().map_err(|error| {
            format!("Offline mode is still on: archetypes could not be returned to the server's: {error}")
        })?;
    }
    let mut rows = vec![(OFFLINE_MODE.to_string(), "false".to_string())];
    let new_lock = if was_offline {
        let until = lock_until(now_ms, tz);
        rows.push((OFFLINE_MODE_LOCKED_UNTIL.to_string(), until.to_string()));
        Some(until)
    } else {
        None
    };
    state
        .try_persist(rows)
        .map_err(|error| format!("Offline mode is still on: it could not be saved: {error}"))?;
    state.set_offline_mode(false, new_lock);
    if was_offline {
        tracing::info!("offline mode switched off");
        on_rejoin();
    }
    Ok(offline_mode_now(state))
}

/// `set_offline_mode_now` under the app's settings lock.
/// The 0.x import holds it from reading the stored rows to saving its
/// offline seed, so the switch waits and a fresh choice is never overwritten
/// by the seed (Q4 review M1). Taken first, as `AppState.settings_lock` is.
pub fn set_offline_mode_locked(
    settings_lock: &std::sync::Mutex<()>,
    state: &ApiState,
    enabled: bool,
    now_ms: i64,
    tz: Tz,
    before_rejoin: &dyn Fn() -> Result<(), String>,
    on_rejoin: &dyn Fn(),
) -> Result<OfflineModeState, String> {
    let _one_save = lock(settings_lock);
    set_offline_mode_now(state, enabled, now_ms, tz, before_rejoin, on_rejoin)
}

/// After leaving offline mode: a signed-in client attests its MTGO logins,
/// which were skipped while offline (0.x `AttestKnownAccounts` on rejoin).
pub fn after_rejoin(state: &ApiState, now_ms: i64) {
    if state.session().linked() {
        attest_known_accounts(state, now_ms);
    }
}

/// After a 0.x import stored its rows: if they now say offline (an offline
/// 0.x user), the gate goes offline too. An import never brings the app
/// online. Returns whether the gate changed.
pub fn offline_after_import(state: &ApiState, rows: Result<&[(String, String)], ()>) -> bool {
    let _one_switch = lock(&state.offline_switch);
    if state.offline_mode().0 || !read_offline(rows) {
        return false;
    }
    state.set_offline_mode(true, None);
    tracing::info!("offline mode switched on by the 0.x import");
    true
}

/// Tell every window: the status bar's pill, the offline mode card and the
/// account card.
pub fn announce(app: &AppHandle) {
    let state = offline_mode_now(&app.state::<ApiState>());
    let startup = app.state::<Startup>();
    if let Ok(ready_state) = ready(&startup) {
        let changed = crate::bridge::set_offline_mode(&ready_state.shared, state.enabled);
        if let Err(error) = changed.emit(app) {
            tracing::warn!(%error, "could not push the status");
        }
    }
    if let Err(error) = (OfflineModeChanged { state }).emit(app) {
        tracing::warn!(%error, "could not push the offline mode change");
    }
    if let Err(error) = (ApiChanged {}).emit(app) {
        tracing::warn!(%error, "could not tell the UI the account changed");
    }
    // The hosted overlay's heartbeat follows offline mode (overlay spec 10).
    crate::overlay::signal(app, crate::overlay::worker::OverlayMsg::GateChanged);
}

/// The 0.x import may have stored `offline_mode`: follow it.
pub fn follow_import(app: &AppHandle) {
    let startup = app.state::<Startup>();
    let Ok(ready_state) = ready(&startup) else {
        return;
    };
    let rows = store::read::settings(&lock(&ready_state.reader)).map_err(|error| {
        tracing::warn!(%error, "could not read the settings after the import");
    });
    if offline_after_import(&app.state::<ApiState>(), rows.as_deref().map_err(|_| ())) {
        announce(app);
    }
}

/// Run the follow-up of a rejoin on its own thread: attestation retries
/// with pauses, which must not hold up the switch.
fn spawn_rejoin(app: &AppHandle) {
    let thread_app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("rejoin".to_string())
        .spawn(move || {
            after_rejoin(&thread_app.state::<ApiState>(), crate::wall_clock_ms());
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "could not attest after leaving offline mode");
    }
}

/// What leaving offline mode tells the archetypes worker: the gate is open
/// again (the queue and missing definitions may ask the API) and the
/// definitions change check runs now (archetypes spec section 5; 0.x
/// resynced its archetype catalog here).
pub fn archetype_rejoin_signals() -> [crate::archetypes::worker::ArchetypeSignal; 3] {
    [
        crate::archetypes::worker::ArchetypeSignal::GateChanged,
        crate::archetypes::worker::ArchetypeSignal::CheckDefinitions,
        crate::archetypes::worker::ArchetypeSignal::FetchDefinitions,
    ]
}

/// Going online: the archetypes held from the offline download become
/// Server archetypes again, through the tracker's writer.
fn release_offline_archetypes(
    state: &crate::commands::AppState,
    now_ms: i64,
) -> Result<(), String> {
    let editor = state
        .tracker_part(tracker::TrackerHandle::archetype_editor)
        .map_err(|_| "tracking is restarting; try again".to_string())?;
    editor
        .edit(store::archetype_edit::ArchetypeEdit::ReleaseOffline { now_ms })
        .map(|_| ())
}

#[tauri::command]
#[specta::specta]
pub async fn offline_mode(app: AppHandle) -> Result<OfflineModeState, CommandError> {
    off_async_workers(move || Ok(offline_mode_now(&app.state::<ApiState>()))).await
}

/// The offline mode switch. Refused while on cooldown, with the 0.x message.
#[tauri::command]
#[specta::specta]
pub async fn set_offline_mode(
    app: AppHandle,
    enabled: bool,
) -> Result<OfflineModeState, CommandError> {
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let ready_state = ready(&startup).ok();
        let tz = ready_state.map_or(Tz::UTC, |state| state.timezone);
        let rejoin_app = app.clone();
        let on_rejoin = || {
            spawn_rejoin(&rejoin_app);
            crate::api::worker::signal(&rejoin_app, WorkerSignal::GateChanged);
            for signal in archetype_rejoin_signals() {
                crate::archetypes::worker::signal(&rejoin_app, signal);
            }
        };
        let api = app.state::<ApiState>();
        let now = crate::wall_clock_ms();
        let result = match ready_state {
            Some(state) => {
                let release = || release_offline_archetypes(state, now);
                set_offline_mode_locked(
                    &state.settings_lock,
                    &api,
                    enabled,
                    now,
                    tz,
                    &release,
                    &on_rejoin,
                )
            }
            None => set_offline_mode_now(&api, enabled, now, tz, &|| Ok(()), &on_rejoin),
        };
        // Also after a failure: the gate may have changed anyway.
        announce(&app);
        result.map_err(CommandError::Refused)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::secrets::{ACCESS_TOKEN, REFRESH_TOKEN};
    use crate::api::session::testing::{TestApi, test_api};
    use crate::api::stub::{Reply, Stub};
    use crate::bridge::lock;
    use chrono_tz::Tz;
    use mymtgo_api::{
        ApiRequest, ApiStatus, AuthMode, GateDecision, KnownAccount, Method, OFFLINE_MODE,
        OFFLINE_MODE_LOCKED_UNTIL, RequestClass, attest_request, cooldown_message,
        exchange_request, gate, lock_until, refresh_request, register_device_request,
        status_request,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};

    const NOW: i64 = 1_800_000_000_000;
    const DAY: i64 = 24 * 3_600_000;
    const TZ: Tz = Tz::Europe__London;
    const CONFIRMED: &str = r#"{"id":1,"username":"Qplayer","login_id":6031972,"public":false}"#;

    /// A device with a fresh key, signed in when `linked`, offline when
    /// `offline`.
    fn api(base_url: &str, offline: bool, linked: bool) -> TestApi {
        let key_valid = (NOW + DAY).to_string();
        let token_valid = (NOW + 30 * DAY).to_string();
        let offline_row = offline.to_string();
        let rows = [
            ("api_device_id", r#""qdevice""#),
            ("api_key_expires_at", key_valid.as_str()),
            ("api_token_expires_at", token_valid.as_str()),
            (OFFLINE_MODE, offline_row.as_str()),
        ];
        let mut secrets = vec![("api_key", "qdevice-key")];
        if linked {
            secrets.push((ACCESS_TOKEN, "qaccess"));
            secrets.push((REFRESH_TOKEN, "qrefresh"));
        }
        test_api(base_url, &rows, &secrets)
    }

    fn never() {}

    fn set(api: &TestApi, enabled: bool, now_ms: i64) -> Result<OfflineModeState, String> {
        set_offline_mode_now(&api.state, enabled, now_ms, TZ, &|| Ok(()), &never)
    }

    // 0.x OfflineModeRejoinLockTest: "locks offline mode until tomorrow when
    // rejoining".
    #[test]
    fn locks_offline_mode_until_tomorrow_when_rejoining() {
        let api = api("http://127.0.0.1:1", true, false);
        let state = set(&api, false, NOW).expect("rejoined");
        let until = lock_until(NOW, TZ);
        assert_eq!(
            state,
            OfflineModeState {
                enabled: false,
                locked_until_ms: Some(until),
            }
        );
        assert!(!api.state.session().gate.offline);
        assert_eq!(api.saved(OFFLINE_MODE).as_deref(), Some("false"));
        assert_eq!(
            api.saved(OFFLINE_MODE_LOCKED_UNTIL),
            Some(until.to_string())
        );
    }

    // 0.x OfflineModeRejoinLockTest: "does not lock when the user was
    // already online".
    #[test]
    fn does_not_lock_when_the_user_was_already_online() {
        let api = api("http://127.0.0.1:1", false, false);
        let state = set(&api, false, NOW).expect("stays online");
        assert_eq!(state.locked_until_ms, None);
        assert_eq!(api.saved(OFFLINE_MODE_LOCKED_UNTIL), None);
    }

    // 0.x OfflineModeRejoinLockTest: "refuses to re-enable offline mode
    // while locked".
    #[test]
    fn refuses_to_re_enable_offline_mode_while_locked() {
        let api = api("http://127.0.0.1:1", true, false);
        set(&api, false, NOW).expect("rejoined");
        let saved_before = lock(&api.saved).len();
        let error = set(&api, true, NOW + 1_000).expect_err("on cooldown");
        assert_eq!(error, cooldown_message(lock_until(NOW, TZ), NOW + 1_000));
        assert!(!api.state.session().gate.offline);
        assert_eq!(lock(&api.saved).len(), saved_before, "nothing stored");
    }

    // 0.x OfflineModeRejoinLockTest: "allows offline mode again once the
    // lock has expired".
    #[test]
    fn allows_offline_mode_again_once_the_lock_has_expired() {
        let api = api("http://127.0.0.1:1", true, false);
        set(&api, false, NOW).expect("rejoined");
        let state = set(&api, true, NOW + 2 * DAY).expect("allowed");
        assert!(state.enabled);
        assert!(api.state.session().gate.offline);
        assert_eq!(api.saved(OFFLINE_MODE).as_deref(), Some("true"));
    }

    // 0.x OfflineModeRejoinLockTest: "reports not locked when no lock has
    // ever been written"; PrivacyControllerTest: "renders the privacy
    // settings page" (no cooldown).
    #[test]
    fn reports_not_locked_when_no_lock_has_ever_been_written() {
        let api = test_api("http://127.0.0.1:1", &[], &[]);
        assert_eq!(
            offline_mode_now(&api.state),
            OfflineModeState {
                enabled: false,
                locked_until_ms: None,
            }
        );
    }

    // 0.x PrivacyControllerTest: "sends the offline mode state to the
    // privacy page" and "sends the rejoin cooldown to the privacy page".
    #[test]
    fn the_privacy_page_gets_the_offline_state_and_the_cooldown() {
        let offline = test_api("http://127.0.0.1:1", &[(OFFLINE_MODE, "true")], &[]);
        assert!(offline_mode_now(&offline.state).enabled);
        let until = (NOW + DAY).to_string();
        let cooling = test_api(
            "http://127.0.0.1:1",
            &[(OFFLINE_MODE_LOCKED_UNTIL, until.as_str())],
            &[],
        );
        assert_eq!(
            offline_mode_now(&cooling.state).locked_until_ms,
            Some(NOW + DAY)
        );
    }

    // 0.x UpdateOfflineModeControllerTest: "enables offline mode without
    // contacting the api".
    #[test]
    fn enables_offline_mode_without_contacting_the_api() {
        let stub = Stub::start(|_| Reply::json(200, CONFIRMED));
        let api = api(&stub.base_url, false, true);
        let rejoins = AtomicUsize::new(0);
        let state = set_offline_mode_now(&api.state, true, NOW, TZ, &|| Ok(()), &|| {
            rejoins.fetch_add(1, Ordering::SeqCst);
        })
        .expect("enabled");
        assert!(state.enabled);
        assert!(api.state.session().gate.offline);
        assert_eq!(stub.connections(), 0);
        assert_eq!(rejoins.load(Ordering::SeqCst), 0);
    }

    // 0.x UpdateOfflineModeControllerTest: "re-attests known accounts when a
    // linked client rejoins". (Its "resyncs the archetype catalog when
    // rejoining" is `archetype_rejoin_signals` below and the archetypes
    // worker's `leaving_offline_mode_runs_the_change_check`.)
    #[test]
    fn re_attests_known_accounts_when_a_linked_client_rejoins() {
        let stub = Stub::start(|_| Reply::json(200, CONFIRMED));
        let api = api(&stub.base_url, true, true);
        *lock(&api.accounts) = vec![
            KnownAccount {
                username: "Qplayer".to_string(),
                login_id: Some(6_031_972),
            },
            KnownAccount {
                username: "Qunknown".to_string(),
                login_id: None,
            },
        ];
        set_offline_mode_now(&api.state, false, NOW, TZ, &|| Ok(()), &|| {
            after_rejoin(&api.state, NOW);
        })
        .expect("rejoined");
        let seen = stub.requests();
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert_eq!(seen[0].target, "/api/account/players");
        assert_eq!(
            seen[0].body,
            br#"{"login_id":6031972,"username":"Qplayer"}"#.to_vec()
        );
    }

    // The offline archetype download (2026-10-27 design section 5): the
    // held rows go back to Server before the gate opens; if that fails,
    // offline mode stays on and nothing is stored.
    #[test]
    fn a_failed_release_keeps_offline_mode_on() {
        let api = api("http://127.0.0.1:1", true, false);
        let rejoins = AtomicUsize::new(0);
        let error = set_offline_mode_now(
            &api.state,
            false,
            NOW,
            TZ,
            &|| Err("disk".to_string()),
            &|| {
                rejoins.fetch_add(1, Ordering::SeqCst);
            },
        )
        .expect_err("refused");
        assert!(error.contains("Offline mode is still on"), "{error}");
        assert!(error.contains("disk"), "{error}");
        assert!(api.state.offline_mode().0);
        assert_eq!(api.saved(OFFLINE_MODE), None);
        assert_eq!(rejoins.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_release_runs_before_the_gate_opens() {
        let api = api("http://127.0.0.1:1", true, false);
        let ran_offline = AtomicUsize::new(0);
        set_offline_mode_now(
            &api.state,
            false,
            NOW,
            TZ,
            &|| {
                if api.state.offline_mode().0 && api.saved(OFFLINE_MODE).is_none() {
                    ran_offline.fetch_add(1, Ordering::SeqCst);
                }
                Ok(())
            },
            &never,
        )
        .expect("online");
        assert_eq!(ran_offline.load(Ordering::SeqCst), 1);
        assert!(!api.state.offline_mode().0);
    }

    #[test]
    fn the_release_runs_only_when_leaving_offline_mode() {
        let going_offline = api("http://127.0.0.1:1", false, false);
        let releases = AtomicUsize::new(0);
        let release = || {
            releases.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        set_offline_mode_now(&going_offline.state, true, NOW, TZ, &release, &never).expect("on");
        assert_eq!(releases.load(Ordering::SeqCst), 0, "switching on");
        let online = api("http://127.0.0.1:1", false, false);
        set_offline_mode_now(&online.state, false, NOW, TZ, &release, &never).expect("online");
        assert_eq!(releases.load(Ordering::SeqCst), 0, "already online");
    }

    /// Rejoining also downloads the definitions the released rows need.
    #[test]
    fn rejoining_fetches_the_released_definitions() {
        assert!(archetype_rejoin_signals().iter().any(|signal| matches!(
            signal,
            crate::archetypes::worker::ArchetypeSignal::FetchDefinitions
        )));
    }

    // 0.x UpdateOfflineModeControllerTest: "resyncs the archetype catalog
    // when rejoining": leaving offline mode asks for the definitions check.
    #[test]
    fn rejoining_checks_the_archetype_definitions() {
        let signals = archetype_rejoin_signals();
        assert!(signals.iter().any(|signal| matches!(
            signal,
            crate::archetypes::worker::ArchetypeSignal::CheckDefinitions
        )));
        assert!(signals.iter().any(|signal| matches!(
            signal,
            crate::archetypes::worker::ArchetypeSignal::GateChanged
        )));
    }

    // 0.x UpdateOfflineModeControllerTest: "does not attest when rejoining
    // unlinked".
    #[test]
    fn does_not_attest_when_rejoining_unlinked() {
        let stub = Stub::start(|_| Reply::json(200, CONFIRMED));
        let api = api(&stub.base_url, true, false);
        *lock(&api.accounts) = vec![KnownAccount {
            username: "Qplayer".to_string(),
            login_id: Some(6_031_972),
        }];
        set_offline_mode_now(&api.state, false, NOW, TZ, &|| Ok(()), &|| {
            after_rejoin(&api.state, NOW);
        })
        .expect("rejoined");
        assert_eq!(stub.connections(), 0);
    }

    // 0.x UpdateOfflineModeControllerTest: "does not resync when already
    // online".
    #[test]
    fn nothing_follows_a_rejoin_when_already_online() {
        let api = api("http://127.0.0.1:1", false, true);
        let rejoins = AtomicUsize::new(0);
        set_offline_mode_now(&api.state, false, NOW, TZ, &|| Ok(()), &|| {
            rejoins.fetch_add(1, Ordering::SeqCst);
        })
        .expect("online");
        assert_eq!(rejoins.load(Ordering::SeqCst), 0);
    }

    // Spec section 5: the in-memory gate flag changes before the settings
    // write returns, so no request starts once the switch is on.
    #[test]
    fn the_gate_is_offline_before_the_write_returns() {
        let stub = Stub::start(|_| Reply::json(200, r#"{"status":"ok"}"#));
        let api = Arc::new(api(&stub.base_url, false, false));
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        {
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            *lock(&api.persist_hook) = Some(Arc::new(move || {
                entered.wait();
                release.wait();
                Ok(())
            }));
        }
        let switching = {
            let api = Arc::clone(&api);
            std::thread::spawn(move || set(&api, true, NOW))
        };
        entered.wait();
        // The write is still running.
        assert!(api.state.offline_mode().0);
        assert_eq!(
            crate::api::status::check_status(&api.state, NOW),
            ApiStatus::Offline
        );
        release.wait();
        switching.join().expect("joined").expect("enabled");
        assert_eq!(stub.connections(), 0);
    }

    // Offline mode fails closed: a switch on stays on in memory even when
    // it cannot be stored; a switch off that cannot be stored stays offline.
    #[test]
    fn a_failed_write_leaves_the_app_offline() {
        let api = api("http://127.0.0.1:1", false, false);
        *lock(&api.persist_hook) = Some(Arc::new(|| Err("disk full".to_string())));
        let error = set(&api, true, NOW).expect_err("not stored");
        assert!(error.contains("disk full"), "{error}");
        assert!(api.state.offline_mode().0);
        let error = set(&api, false, NOW).expect_err("not stored");
        assert!(error.contains("disk full"), "{error}");
        assert!(api.state.offline_mode().0, "still offline");
        assert_eq!(api.state.offline_mode().1, None, "no cooldown started");
    }

    // 0.x DegradedRoutesTest: "reports offline rather than pinging the api".
    #[test]
    fn reports_offline_rather_than_pinging_the_api() {
        let stub = Stub::start(|_| Reply::json(200, r#"{"status":"ok"}"#));
        let api = api(&stub.base_url, false, false);
        set(&api, true, NOW).expect("enabled");
        assert_eq!(
            crate::api::status::check_status(&api.state, NOW),
            ApiStatus::Offline
        );
        assert_eq!(stub.connections(), 0);
    }

    // 0.x CallSiteGatingTest: "still allows card identity requests while
    // offline".
    #[test]
    fn still_allows_card_identity_requests_while_offline() {
        let stub = Stub::start(|_| Reply::json(200, r#"{"cards":[]}"#));
        let api = api(&stub.base_url, false, false);
        set(&api, true, NOW).expect("enabled");
        let mut resolve = ApiRequest::new(
            Method::Post,
            "/api/cards/resolve",
            RequestClass::Reference,
            AuthMode::DeviceOrBearer,
        );
        resolve.body = mymtgo_api::Body::Json(br#"{"cards":[]}"#.to_vec());
        let response = api.state.send(&resolve, NOW).expect("sent offline");
        assert_eq!(response.status, 200);
        assert_eq!(stub.requests().len(), 1);
    }

    /// Every request builder so far (Q2, Q3, Q6, Q7, Q8, Q9, T3 and the overlay's). Later
    /// tasks add theirs.
    fn every_builder(api: &TestApi) -> Vec<(&'static str, ApiRequest)> {
        let config = api.state.config().expect("config").clone();
        let cards = mymtgo_api::CardsRequest {
            ids: vec![1],
            tokens: vec!["Qtoken".to_string()],
            arena_ids: Vec::new(),
            names: None,
            endstep: Vec::new(),
        };
        vec![
            ("register device", register_device_request("qdevice")),
            ("status", status_request()),
            (
                "exchange code",
                exchange_request(&config, "qcode", "qverifier", "http://127.0.0.1:1/x"),
            ),
            ("refresh", refresh_request(&config, "qrefresh")),
            ("attest", attest_request(6_031_972, "Qplayer")),
            ("cards", mymtgo_api::cards_request(&cards)),
            ("resolve cards", mymtgo_api::resolve_request(&[1, 2])),
            (
                "estimate archetype",
                mymtgo_api::estimate_request(
                    "CMODERN",
                    &[mymtgo_api::EstimateCard {
                        id: mymtgo_api::EstimateCardId::Mtgo(1),
                        quantity: 4,
                    }],
                ),
            ),
            ("decklist", mymtgo_api::decklist_request("q-arch")),
            ("decklists", mymtgo_api::decklists_request("q-arch")),
            (
                "definitions check",
                definitions_check(&[("q-arch".to_string(), None)]),
            ),
            ("metagame", mymtgo_api::metagame_request("modern", 30)),
            ("tournament observations", observations_request()),
            (
                "tournament name",
                mymtgo_api::tournament_request(12_841_367),
            ),
            (
                "match report",
                mymtgo_api::report_request(&serde_json::json!({"match_token": "q-match"})),
            ),
            ("card stats", card_stats_request()),
            ("sync status", mymtgo_api::sync_status_request()),
            (
                "sync manifest",
                mymtgo_api::manifest_request(
                    mymtgo_api::SyncType::Deck,
                    None,
                    None,
                    None,
                    &[],
                    &[],
                    true,
                ),
            ),
            (
                "sync blobs",
                mymtgo_api::blobs_request(
                    mymtgo_api::SyncType::Match,
                    None,
                    &[mymtgo_api::BlobUpload {
                        client_id: "q-match".to_string(),
                        hash: "q-hash".to_string(),
                        updated_at: "2026-10-09T00:00:00Z".to_string(),
                        gz: mymtgo_api::gzip_blob("{}"),
                        sidecar: serde_json::json!({"format": "CMODERN"}),
                    }],
                    "qboundary",
                ),
            ),
            (
                "sync fetch",
                mymtgo_api::fetch_request(mymtgo_api::SyncType::League, &["q-league".to_string()]),
            ),
            ("sync slots", mymtgo_api::slots_request()),
            ("deck slot", mymtgo_api::deck_slot_request("12345", true)),
            (
                "opponent scouting",
                mymtgo_api::players_request("Qopponent", "modern"),
            ),
            (
                "overlay publish",
                mymtgo_api::publish_overlay_request(
                    6_031_972,
                    &serde_json::json!({"status": "idle", "art": null}),
                ),
            ),
            (
                "overlay clear",
                mymtgo_api::clear_overlay_request(6_031_972),
            ),
            (
                "overlay background upload",
                mymtgo_api::upload_overlay_background_request(
                    b"png-bytes",
                    "background-1.png",
                    "qboundary",
                ),
            ),
            (
                "overlay background delete",
                mymtgo_api::delete_overlay_background_request(),
            ),
            (
                "share replay",
                mymtgo_api::share_replay_request("q-match", 6_031_972, &prepared_replay()),
            ),
            ("revoke replay", mymtgo_api::revoke_replay_request("q-uuid")),
        ]
    }

    // The gate class of every overlay builder (overlay spec sections 7.3 and
    // 10): scouting is a lookup, publishing is sync.
    #[test]
    fn the_overlay_builders_have_their_gate_classes() {
        let api = api("http://127.0.0.1:1", false, true);
        for (name, request) in every_builder(&api) {
            let expected = match name {
                "opponent scouting" => RequestClass::Lookup,
                "overlay publish"
                | "overlay clear"
                | "overlay background upload"
                | "overlay background delete" => RequestClass::Sync,
                _ => continue,
            };
            assert_eq!(request.class, expected, "{name}");
        }
    }

    // Overlay spec ruling 14: publishing needs sign-in (and online); signed
    // out, nothing reaches the API.
    #[test]
    fn publish_is_blocked_before_sign_in() {
        let stub = Stub::start(|_| Reply::json(204, ""));
        let api = api(&stub.base_url, false, false);
        let state = serde_json::json!({"status": "idle"});
        for request in [
            mymtgo_api::publish_overlay_request(6_031_972, &state),
            mymtgo_api::clear_overlay_request(6_031_972),
            mymtgo_api::upload_overlay_background_request(b"png", "background-1.png", "qb"),
            mymtgo_api::delete_overlay_background_request(),
        ] {
            assert_eq!(
                api.state.send(&request, NOW).err(),
                Some(mymtgo_api::ApiError::NotLinked),
                "{}",
                request.path
            );
            assert_eq!(
                crate::api::sync_runner::sync_send(&api.state, &request, &[], NOW).err(),
                Some(crate::api::sync_runner::SyncError::NotLinked)
            );
        }
        // Scouting is a lookup: it goes out signed out (device key).
        let scouting = mymtgo_api::players_request("Qopponent", "modern");
        assert_eq!(scouting.class, RequestClass::Lookup);
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(stub.connections(), 0);
    }

    /// A signed-in, online stub API answering `status` and `body`.
    fn overlay_stub(status: u16, body: &'static str) -> (Stub, TestApi) {
        let stub = Stub::start(move |_| Reply::json(status, body));
        let api = api(&stub.base_url, false, true);
        (stub, api)
    }

    // 0.x SyncApiOverlayTest "puts the overlay state with the login id".
    #[test]
    fn puts_the_overlay_state_with_the_login_id() {
        let (stub, api) = overlay_stub(204, "");
        let state = serde_json::json!({"status": "idle", "art": null});
        let request = mymtgo_api::publish_overlay_request(4242, &state);
        crate::api::sync_runner::sync_send(&api.state, &request, &[], NOW).expect("published");
        let sent = stub.requests();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].method, "PUT");
        assert_eq!(sent[0].target, "/api/overlay");
        assert_eq!(sent[0].header("authorization"), Some("Bearer qaccess"));
        let body: serde_json::Value = serde_json::from_slice(&sent[0].body).expect("json");
        assert_eq!(body, serde_json::json!({"login_id": 4242, "state": state}));
    }

    // 0.x "throws when the publish fails".
    #[test]
    fn a_failed_publish_is_an_error() {
        let (_stub, api) = overlay_stub(409, r#"{"error":"player_not_claimed"}"#);
        let request = mymtgo_api::publish_overlay_request(4242, &serde_json::json!({}));
        let sent = crate::api::sync_runner::sync_send(&api.state, &request, &[], NOW);
        assert!(
            matches!(sent, Err(crate::api::sync_runner::SyncError::Failed(_))),
            "{sent:?}"
        );
    }

    // 0.x "clears the overlay and treats a 404 as done".
    #[test]
    fn clears_the_overlay_and_treats_a_404_as_done() {
        let (stub, api) = overlay_stub(404, r#"{"message":"No overlay"}"#);
        let request = mymtgo_api::clear_overlay_request(4242);
        let answer = crate::api::sync_runner::sync_send(
            &api.state,
            &request,
            &mymtgo_api::CLEAR_OVERLAY_PASSTHROUGH,
            NOW,
        )
        .expect("a 404 is done");
        assert_eq!(answer.status, 404);
        let sent = stub.requests();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].method, "DELETE");
        assert_eq!(sent[0].target, "/api/overlay/4242");
    }

    // 0.x "uploads the background and returns its url".
    #[test]
    fn uploads_the_background_and_returns_its_url() {
        let (stub, api) = overlay_stub(201, r#"{"url":"https://cdn.example/bg.png"}"#);
        let request =
            mymtgo_api::upload_overlay_background_request(b"png-bytes", "background.png", "qb");
        let answer =
            crate::api::sync_runner::sync_send(&api.state, &request, &[], NOW).expect("uploaded");
        assert_eq!(
            mymtgo_api::parse_background_url(&answer).as_deref(),
            Some("https://cdn.example/bg.png")
        );
        let sent = stub.requests();
        assert_eq!(sent[0].method, "POST");
        assert_eq!(sent[0].target, "/api/overlay/background");
        assert_eq!(
            sent[0].header("content-type"),
            Some("multipart/form-data; boundary=qb")
        );
    }

    // 0.x "deletes the remote background".
    #[test]
    fn deletes_the_remote_background() {
        let (stub, api) = overlay_stub(204, "");
        let request = mymtgo_api::delete_overlay_background_request();
        crate::api::sync_runner::sync_send(&api.state, &request, &[], NOW).expect("deleted");
        let sent = stub.requests();
        assert_eq!(sent[0].method, "DELETE");
        assert_eq!(sent[0].target, "/api/overlay/background");
    }

    // 0.x FetchOpponentScoutingTest "returns archetype with colors from local
    // archetype lookup on 200": the POST the stub sees (device key, signed
    // out), and its answer read.
    #[test]
    fn scouting_posts_the_opponent_and_reads_the_answer() {
        let stub = Stub::start(|_| {
            Reply::json(
                200,
                r#"{"data":{"player":"Qfoo","league_result":{"archetype":{"uuid":"arch-uuid-1","name":"Izzet Phoenix","slug":"izzet-phoenix"}}}}"#,
            )
        });
        let api = api(&stub.base_url, false, false);
        let answer = api
            .state
            .send(&mymtgo_api::players_request("Qfoo", "modern"), NOW)
            .expect("answered");
        let scouting = mymtgo_api::parse_players(&answer).expect("scouted");
        assert_eq!(
            scouting.league.map(|archetype| archetype.uuid).as_deref(),
            Some("arch-uuid-1")
        );
        let sent = stub.requests();
        assert_eq!(sent[0].method, "POST");
        assert_eq!(sent[0].target, "/api/players");
        assert_eq!(sent[0].header("x-device-id"), Some("qdevice"));
        assert_eq!(
            sent[0].body,
            br#"{"username":"Qfoo","format":"modern"}"#.to_vec()
        );
    }

    /// The package's API snapshot, prepared as a share prepares it.
    fn prepared_replay() -> ::replay::PreparedSnapshot {
        let snapshot: ::replay::Snapshot = serde_json::from_str(include_str!(
            "../../../crates/replay/tests/fixtures/api_snapshot.json"
        ))
        .expect("the fixture is a snapshot");
        ::replay::prepare_shared_snapshot(snapshot, &["Opp_Name".to_string()]).expect("prepared")
    }

    /// The one definitions check request for a short list.
    fn definitions_check(versions: &[(String, Option<String>)]) -> ApiRequest {
        let mut requests = mymtgo_api::definitions_check_request(versions);
        assert_eq!(requests.len(), 1);
        requests.remove(0)
    }

    /// The card stats request the worker builds for one queued game.
    fn card_stats_request() -> ApiRequest {
        let game = mymtgo_api::CardStatsGame {
            match_token: "q-match".to_string(),
            mtgo_game_id: 1,
            local_player: Some("Qplayer".to_string()),
            player_archetype_uuid: Some("q-archetype".to_string()),
            opponent_archetype_uuid: None,
            format: Some("CMODERN".to_string()),
            won: true,
            on_play: Some(true),
            game_number: 1,
            game_started_at_ms: Some(NOW),
            match_started_at_ms: Some(NOW),
            cards: vec![mymtgo_api::CardStatsCard {
                oracle_id: Some("q-oracle".to_string()),
                quantity: 4,
                kept: 1,
                seen: 1,
                hand_seen: 0,
                graveyard_seen: 0,
                exile_seen: 0,
                battlefield_seen: 0,
                discarded: 0,
                cast_turn: None,
                has_zone_data: false,
                cast: 0,
                played: None,
                activated: None,
                casts: Vec::new(),
                sided_in: 0,
                sided_out: 0,
                pregame_revealed: 0,
                pregame_played: 0,
            }],
        };
        let row = mymtgo_api::card_stats_row(&game, NOW).expect("a row");
        let claimed = mymtgo_api::ClaimedRow {
            subject: row.subject,
            payload: row.payload.expect("a payload"),
        };
        let mut requests = mymtgo_api::card_stats_requests(&[claimed]);
        assert_eq!(requests.len(), 1);
        requests.remove(0).1
    }

    /// The observations request the worker builds for one queued event.
    fn observations_request() -> ApiRequest {
        let event = mymtgo_api::TournamentEvent {
            log_file_id: 1,
            byte_offset: 2,
            kind: "tournament_ended".to_string(),
            raw: r#"Message: {"Token":"tok-1","ReturnCode":0}"#.to_string(),
            tournament_token: Some("tok-1".to_string()),
            logged_at_ms: NOW,
        };
        let row = mymtgo_api::observation_row(&event, NOW);
        let claimed = mymtgo_api::ClaimedRow {
            subject: row.subject,
            payload: row.payload.expect("a payload"),
        };
        let mut requests = mymtgo_api::observations_requests(&[claimed]);
        assert_eq!(requests.len(), 1);
        requests.remove(0).1
    }

    /// Whether sending `request` opened a connection to a fresh stub.
    fn connects(offline: bool, linked: bool, request: &ApiRequest) -> bool {
        let stub = Stub::start(|_| Reply::json(200, "{}"));
        let api = api(&stub.base_url, offline, linked);
        let _ = api.state.send(request, NOW);
        stub.connections() > 0
    }

    // Review Focus 1: a counting stub sees no connection for any request
    // while offline, except device registration and card lookups.
    #[test]
    fn no_traffic_while_offline() {
        let api = api("http://127.0.0.1:1", true, true);
        for (name, request) in every_builder(&api) {
            let allowed = matches!(
                request.class,
                RequestClass::Registration | RequestClass::Reference
            );
            assert_eq!(connects(true, true, &request), allowed, "{name}");
            // The control: online, every one of them goes out.
            assert!(connects(false, true, &request), "{name} online");
        }
        // The flows built on them, offline.
        let stub = Stub::start(|_| Reply::json(200, CONFIRMED));
        let api = self::api(&stub.base_url, true, true);
        *lock(&api.accounts) = vec![KnownAccount {
            username: "Qplayer".to_string(),
            login_id: Some(6_031_972),
        }];
        assert_eq!(
            crate::api::status::check_status(&api.state, NOW),
            ApiStatus::Offline
        );
        let _ = crate::api::tokens::refresh(&api.state, NOW);
        let _ = crate::api::tokens::recover_unauthorized(&api.state, Some("qaccess"), NOW);
        crate::api::attest::attest_known_accounts(&api.state, NOW);
        let opened = |_: &str| -> Result<(), String> { panic!("no browser offline") };
        assert!(crate::api::signin::begin_sign_in(&api.state, &opened).is_err());
        // Q4 review M3: the code exchange as the sign-in calls it, not only
        // its builder; and registration as the flows call it (a signed-in
        // session never registers, so nothing goes out here either).
        let exchanged = crate::api::tokens::exchange_code(
            &api.state,
            "qcode",
            "qverifier",
            "http://127.0.0.1:9/oauth/callback",
            NOW,
        );
        assert_eq!(exchanged.err(), Some(mymtgo_api::ApiError::Offline));
        assert!(!crate::api::device::register_device(&api.state, NOW));
        // Q6: the archetype lookups as #9 will call them.
        assert_eq!(
            crate::api::reference::estimate_archetype(
                &api.state,
                "CMODERN",
                &[mymtgo_api::EstimateCard {
                    id: mymtgo_api::EstimateCardId::Mtgo(1),
                    quantity: 4
                }],
                NOW
            ),
            mymtgo_api::EstimateOutcome::Failed(mymtgo_api::ApiError::Offline.to_string())
        );
        assert_eq!(
            crate::api::reference::fetch_decklists(&api.state, "q-arch", NOW),
            Err(mymtgo_api::ApiError::Offline)
        );
        assert_eq!(
            crate::api::reference::fetch_decklist(&api.state, "q-arch", NOW),
            Err(mymtgo_api::ApiError::Offline)
        );
        // Plan T Tasks 4 and 5: the definition download and the check as
        // the worker calls them.
        let mut pacer = crate::archetypes::definitions::Pacer::new(Box::new(
            crate::archetypes::definitions::SystemClock,
        ));
        assert_eq!(
            crate::archetypes::definitions::fetch_definition(&api.state, &mut pacer, "q-arch"),
            crate::archetypes::definitions::DefinitionReply::Failed
        );
        assert_eq!(
            crate::archetypes::definitions::check_versions(
                &api.state,
                &mut pacer,
                &[("q-arch".to_string(), None)]
            ),
            crate::archetypes::definitions::CheckReply::Failed(mymtgo_api::ApiError::Offline)
        );
        // T3: the definitions check and the metagame shares.
        assert_eq!(
            crate::api::reference::check_definitions(
                &api.state,
                &[("q-arch".to_string(), None)],
                NOW
            ),
            Err(mymtgo_api::ApiError::Offline)
        );
        assert_eq!(
            crate::api::reference::fetch_metagame(&api.state, "modern", 30, NOW),
            Err(mymtgo_api::ApiError::Offline)
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(stub.connections(), 0);
    }

    // Owner ruling 2026-10-11: a device that never signed in sends
    // community data with its device key, as 0.x; only sync waits for a
    // sign-in.
    #[test]
    fn a_device_that_never_signed_in_sends_community_data_but_never_syncs() {
        let api = api("http://127.0.0.1:1", false, true);
        for (name, request) in every_builder(&api) {
            let held = request.class == RequestClass::Sync;
            let state = mymtgo_api::GateState {
                offline: false,
                linked: false,
            };
            assert_eq!(
                gate(request.class, &state) == GateDecision::Send,
                !held,
                "{name}"
            );
            assert_eq!(connects(false, false, &request), !held, "{name}");
        }
        let stub = Stub::start(|_| Reply::json(200, CONFIRMED));
        let api = self::api(&stub.base_url, false, false);
        *lock(&api.accounts) = vec![KnownAccount {
            username: "Qplayer".to_string(),
            login_id: Some(6_031_972),
        }];
        crate::api::attest::attest_known_accounts(&api.state, NOW);
        assert_eq!(stub.connections(), 0);
    }

    #[test]
    fn an_import_that_seeds_offline_mode_switches_the_gate_off_the_network() {
        let api = api("http://127.0.0.1:1", false, false);
        let rows = vec![(OFFLINE_MODE.to_string(), "true".to_string())];
        assert!(offline_after_import(&api.state, Ok(&rows)));
        assert!(api.state.offline_mode().0);
        // Never the other way: an import does not bring the app online.
        let api = self::api("http://127.0.0.1:1", true, false);
        let rows = vec![(OFFLINE_MODE.to_string(), "false".to_string())];
        assert!(!offline_after_import(&api.state, Ok(&rows)));
        assert!(api.state.offline_mode().0);
    }

    // Q4 review M1: a 0.x import holds the settings lock from reading the
    // stored rows to saving its seed, so the switch waits for it and a
    // fresh choice is never overwritten.
    #[test]
    fn the_switch_waits_for_a_settings_save_in_progress() {
        let api = api("http://127.0.0.1:1", false, false);
        let settings_lock = std::sync::Mutex::new(());
        let held = lock(&settings_lock);
        std::thread::scope(|scope| {
            let switching = scope.spawn(|| {
                set_offline_mode_locked(
                    &settings_lock,
                    &api.state,
                    true,
                    NOW,
                    TZ,
                    &|| Ok(()),
                    &never,
                )
            });
            std::thread::sleep(std::time::Duration::from_millis(100));
            assert_eq!(api.saved(OFFLINE_MODE), None, "waits for the save");
            drop(held);
            switching.join().expect("joined").expect("switched");
        });
        assert_eq!(api.saved(OFFLINE_MODE).as_deref(), Some("true"));
    }

    // Q4 review M4: switching offline mode on ends a sign-in that waits for
    // the browser.
    #[test]
    fn switching_offline_on_cancels_a_waiting_sign_in() {
        let api = api("http://127.0.0.1:1", false, false);
        let opened = |_: &str| -> Result<(), String> { Ok(()) };
        let (_pending, cancel) =
            crate::api::signin::claim_and_begin(&api.state, &opened).expect("begun");
        assert!(!cancel.load(Ordering::SeqCst));
        set(&api, true, NOW).expect("offline");
        assert!(cancel.load(Ordering::SeqCst));
    }
}
