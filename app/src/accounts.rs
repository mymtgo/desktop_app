//! Accounts: the header's switcher and the General page's Accounts card
//! (release spec 5; 0.x `SwitchAccountController`,
//! `UpdateAccountTrackingController`, the `accounts` and `activeAccount`
//! shared props). Writes go through the tracker (`AccountEdit`), which
//! reports `AccountChanged` once committed, so every page refetches.

use serde::Serialize;
use store::accounts::AccountEdit;
use tauri::{AppHandle, Manager};

use crate::bridge::lock;
use crate::commands::{AppState, CommandError, Startup, ready, storage};
use crate::logging::Timer;

/// The route a switch opens (0.x redirects to `decks.index`).
pub const SWITCH_REDIRECT: &str = "/decks";

/// One account for the header and the Accounts card (0.x
/// `['id', 'username', 'tracked', 'active']`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    pub username: String,
    /// What the pages show (an untagged Arena account's display name, else
    /// the username).
    pub label: String,
    pub tracked: bool,
    /// The current account (`store::accounts::current_account`): the
    /// "Active" badge, and the header's username.
    pub active: bool,
}

/// Every account by username, and the current one's label (0.x
/// `activeAccount`; `None` when no account is current).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AccountsView {
    pub accounts: Vec<AccountView>,
    pub current: Option<String>,
}

/// What a switch did: where to go next (0.x's redirect).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct Switched {
    pub redirect: String,
}

/// The accounts of the context's platform as the pages show them.
pub fn accounts_view(conn: &store::Connection) -> Result<AccountsView, store::StoreError> {
    let platform = store::platform::current_platform(conn)?;
    let current_id = store::accounts::current_account_id(conn, platform)?;
    let mut accounts = Vec::new();
    let mut current = None;
    for row in store::accounts::accounts(conn, platform)? {
        let active = Some(row.id) == current_id;
        if active {
            current = Some(row.label.clone());
        }
        accounts.push(AccountView {
            id: row.id,
            username: row.username,
            label: row.label,
            tracked: row.tracked,
            active,
        });
    }
    Ok(AccountsView { accounts, current })
}

pub fn read_accounts(state: &AppState) -> Result<AccountsView, CommandError> {
    accounts_view(&lock(&state.reader)).map_err(storage)
}

fn send(state: &AppState, edit: AccountEdit) -> Result<(), CommandError> {
    let editor = state.tracker_part(tracker::TrackerHandle::account_editor)?;
    let outcome = editor.edit(edit).map_err(CommandError::Tracker)?;
    match outcome.refusal() {
        Some(message) => Err(CommandError::Refused(message.to_string())),
        None => Ok(()),
    }
}

/// The header's switch (0.x `SwitchAccountController`): stores the
/// `active_account` setting at `now_ms`, refused for an unknown username.
/// Held under the settings lock, so a settings save cannot write back an
/// older switch (`apply_settings` keeps the stored one).
pub fn switch(state: &AppState, username: &str, now_ms: i64) -> Result<Switched, CommandError> {
    let _one_save = lock(&state.settings_lock);
    send(
        state,
        AccountEdit::Switch {
            username: username.to_string(),
            at_ms: now_ms,
        },
    )?;
    Ok(Switched {
        redirect: SWITCH_REDIRECT.to_string(),
    })
}

/// The Accounts card's switch (0.x `UpdateAccountTrackingController`).
/// Returns the accounts as now stored.
pub fn set_tracked(
    state: &AppState,
    username: &str,
    tracked: bool,
) -> Result<AccountsView, CommandError> {
    send(
        state,
        AccountEdit::SetTracked {
            username: username.to_string(),
            tracked,
        },
    )?;
    read_accounts(state)
}

#[tauri::command]
#[specta::specta]
pub fn accounts_list(startup: tauri::State<'_, Startup>) -> Result<AccountsView, CommandError> {
    let _timer = Timer::start("accounts_list");
    read_accounts(ready(&startup)?)
}

/// Async, and the wait for the tracker runs on a blocking thread.
#[tauri::command]
#[specta::specta]
pub async fn switch_account(app: AppHandle, username: String) -> Result<Switched, CommandError> {
    let _timer = Timer::start("switch_account");
    crate::commands::off_async_workers(move || {
        let startup = app.state::<Startup>();
        switch(ready(&startup)?, &username, crate::wall_clock_ms())
    })
    .await
}

/// Async, and the wait for the tracker runs on a blocking thread.
#[tauri::command]
#[specta::specta]
pub async fn set_account_tracked(
    app: AppHandle,
    username: String,
    tracked: bool,
) -> Result<AccountsView, CommandError> {
    let _timer = Timer::start("set_account_tracked");
    crate::commands::off_async_workers(move || {
        let startup = app.state::<Startup>();
        set_tracked(ready(&startup)?, &username, tracked)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::tests::ready_state;
    use crate::commands::{apply_settings, read_settings};
    use crate::settings::Settings;
    use fixture_tools::TestDir;
    use store::import::ImportedAccount;

    fn add_accounts(state: &AppState, names: &[&str]) {
        let accounts = names
            .iter()
            .map(|name| ImportedAccount {
                username: name.to_string(),
                login_id: None,
                created_at_ms: None,
                tracked: true,
                active_since_ms: None,
            })
            .collect();
        state
            .tracker_part(tracker::TrackerHandle::importer)
            .expect("tracker")
            .import_accounts(accounts)
            .expect("accounts");
    }

    fn stop(state: &AppState) {
        lock(&state.tracker).take().expect("tracker").stop();
    }

    // 0.x Pages/GeneralControllerTest: accounts with username, tracked, active.
    #[test]
    fn the_accounts_list_has_every_account_and_marks_the_current_one() {
        let dir = TestDir::new("accounts-list");
        let (state, _events) = ready_state(&dir);
        assert_eq!(
            read_accounts(&state).expect("read"),
            AccountsView {
                accounts: Vec::new(),
                current: None
            }
        );
        add_accounts(&state, &["Qsecond", "Qplayer"]);
        let view = read_accounts(&state).expect("read");
        let names: Vec<&str> = view.accounts.iter().map(|a| a.username.as_str()).collect();
        assert_eq!(names, ["Qplayer", "Qsecond"], "by username");
        assert!(view.accounts.iter().all(|a| a.tracked && !a.active));
        assert_eq!(view.current, None, "two accounts and no Login");

        assert_eq!(
            switch(&state, "Qsecond", 5_000).expect("switched"),
            Switched {
                redirect: "/decks".to_string()
            },
            "0.x redirects to the deck index"
        );
        let view = read_accounts(&state).expect("read");
        assert_eq!(view.current.as_deref(), Some("Qsecond"));
        assert!(view.accounts[1].active && !view.accounts[0].active);
        stop(&state);
    }

    // 0.x SwitchAccountController: `exists:accounts,username`.
    #[test]
    fn switching_to_an_unknown_username_is_refused() {
        let dir = TestDir::new("accounts-switch-unknown");
        let (state, _events) = ready_state(&dir);
        add_accounts(&state, &["Qplayer"]);
        assert_eq!(
            switch(&state, "Qnobody", 5_000),
            Err(CommandError::Refused(
                "The selected username is invalid.".to_string()
            ))
        );
        assert_eq!(read_settings(&state).expect("read").active_account, None);
        stop(&state);
    }

    // 0.x UpdateAccountTrackingController.
    #[test]
    fn tracking_is_stored_and_unknown_names_are_refused() {
        let dir = TestDir::new("accounts-tracking");
        let (state, _events) = ready_state(&dir);
        add_accounts(&state, &["Qplayer", "Qsecond"]);
        let view = set_tracked(&state, "Qsecond", false).expect("stored");
        assert!(view.accounts[0].tracked);
        assert!(!view.accounts[1].tracked);
        assert_eq!(
            view.current.as_deref(),
            Some("Qplayer"),
            "the only tracked account is current"
        );
        assert_eq!(
            set_tracked(&state, "Qnobody", true),
            Err(CommandError::Refused(
                "The selected username is invalid.".to_string()
            ))
        );
        stop(&state);
    }

    #[test]
    fn a_settings_save_keeps_the_stored_switch() {
        let dir = TestDir::new("accounts-save-keeps-switch");
        let (state, _events) = ready_state(&dir);
        add_accounts(&state, &["Qplayer", "Qsecond"]);
        let stale = read_settings(&state).expect("read");
        switch(&state, "Qsecond", 5_000).expect("switched");
        let stored = read_settings(&state).expect("read").active_account;
        assert!(stored.is_some());
        let saved = apply_settings(&state, Settings { ..stale }).expect("saved");
        assert_eq!(saved.settings.active_account, stored);
        assert_eq!(read_settings(&state).expect("read").active_account, stored);
        stop(&state);
    }

    #[test]
    fn the_account_list_follows_the_context() {
        let dir = TestDir::new("accounts-follow-context");
        let (state, _events) = ready_state(&dir);
        crate::platform::tests::seed(&state, true, true);
        let names = |view: &AccountsView| -> Vec<String> {
            view.accounts.iter().map(|a| a.username.clone()).collect()
        };
        let view = read_accounts(&state).expect("read");
        assert_eq!(names(&view), ["Qplayer"]);
        crate::platform::switch(&state, crate::platform::PlatformView::Arena).expect("switched");
        let view = read_accounts(&state).expect("read");
        assert_eq!(names(&view), ["Qarena#12345"]);
        assert!(view.accounts[0].active, "the only Arena account is current");
        assert_eq!(view.current.as_deref(), Some("Qarena#12345"));
        stop(&state);
    }
}
