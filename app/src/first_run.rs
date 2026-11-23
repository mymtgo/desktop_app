//! The first run's 0.x question (release spec section 13.2): migrate from
//! MyMTGO 0.x or start fresh.
//!
//! The Windows installer asks it and leaves the answer in
//! `<app data>/first-run-choice.txt` (`app/installer/hooks.nsh`). At start
//! the app reads and deletes that file, decides once (`decide_first_run`),
//! and saves the answer as the `first_run_choice` setting, so it is acted on
//! once, ever. Linux, development builds and Windows installs that never
//! asked get the same question in the app. Migrating runs the existing 0.x
//! import (`legacy.rs`: 0.x read only, the tracker writes); starting fresh
//! never touches 0.x data.

use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tauri_specta::Event;

use crate::bridge::lock;
use crate::commands::{AppState, CommandError, Startup};
use crate::legacy::ImportProgress;

/// The installer's answer, in the app data folder.
pub const CHOICE_FILE: &str = "first-run-choice.txt";

/// The user's answer to the 0.x question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum FirstRunChoice {
    Migrate,
    Fresh,
}

impl FirstRunChoice {
    /// The installer's word (`migrate` or `fresh`), spaces and line breaks
    /// around it ignored; any other text is none.
    pub fn from_word(word: &str) -> Option<FirstRunChoice> {
        match word.trim() {
            "migrate" => Some(FirstRunChoice::Migrate),
            "fresh" => Some(FirstRunChoice::Fresh),
            _ => None,
        }
    }
}

/// What this start does about 0.x (Plan W ruling W12: not `FirstRun`,
/// which the UI already has for its empty states).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum FirstRunDecision {
    /// Import 0.x now, with its progress.
    Migrate,
    /// The normal first run; the Import page keeps the 0.x section.
    Fresh,
    /// Ask in the app: 0.x data is here and nobody answered yet.
    Ask,
    /// Nothing to ask or do.
    #[default]
    Nothing,
}

impl FirstRunDecision {
    fn from_choice(choice: FirstRunChoice) -> FirstRunDecision {
        match choice {
            FirstRunChoice::Migrate => FirstRunDecision::Migrate,
            FirstRunChoice::Fresh => FirstRunDecision::Fresh,
        }
    }
}

/// What the main window shows for the first run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct FirstRunState {
    pub decision: FirstRunDecision,
    /// Why the migration import failed or did not start ("Try again").
    pub error: Option<String>,
}

/// Decide what this start does (release spec section 13.2 step 2).
/// `file_choice` is the installer file's text, if the file was there. A saved
/// answer or any v1 match means nothing: a choice is acted on once, ever.
/// Otherwise the file's answer wins; with no file (or a word that is not an
/// answer) 0.x data on this computer means asking.
pub fn decide_first_run(
    file_choice: Option<&str>,
    saved_choice: Option<FirstRunChoice>,
    has_matches: bool,
    legacy_detected: bool,
) -> FirstRunDecision {
    if saved_choice.is_some() || has_matches {
        return FirstRunDecision::Nothing;
    }
    if let Some(choice) = file_choice.and_then(FirstRunChoice::from_word) {
        return FirstRunDecision::from_choice(choice);
    }
    if legacy_detected {
        FirstRunDecision::Ask
    } else {
        FirstRunDecision::Nothing
    }
}

/// Read the installer's file and delete it, whatever it says, so it is read
/// once. Text that is not UTF-8 reads as an empty word.
fn take_choice_file(data_dir: &Path) -> Option<String> {
    let path = data_dir.join(CHOICE_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::warn!(%error, "could not read the installer's first-run choice");
            Vec::new()
        }
    };
    if let Err(error) = std::fs::remove_file(&path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%error, "could not delete the installer's first-run choice");
    }
    Some(String::from_utf8(bytes).unwrap_or_default())
}

/// At start: take the installer's file, decide, and save an answer it gave.
/// `legacy_detected` is whether a 0.x database is in its usual place.
pub fn at_start(state: &AppState, legacy_detected: bool) -> FirstRunDecision {
    let file = take_choice_file(&state.data_dir);
    let (saved, has_matches, marked) = {
        let reader = lock(&state.reader);
        let rows = store::read::settings(&reader);
        let count = store::read::count_matches(&reader);
        match (rows, count) {
            (Ok(rows), Ok(count)) => (
                crate::settings::first_run_choice(&rows),
                crate::startup::is_existing_install(state.fresh_install, count),
                crate::settings::fresh_install(&rows),
            ),
            (Err(error), _) | (_, Err(error)) => {
                // Never ask (or import) over data that could not be read.
                tracing::warn!(%error, "could not read the first-run state");
                return FirstRunDecision::Nothing;
            }
        }
    };
    let decision = decide_first_run(file.as_deref(), saved, has_matches, legacy_detected);
    let chosen = match decision {
        FirstRunDecision::Migrate => Some(FirstRunChoice::Migrate),
        FirstRunDecision::Fresh => Some(FirstRunChoice::Fresh),
        FirstRunDecision::Ask | FirstRunDecision::Nothing => None,
    };
    if let Some(choice) = chosen {
        tracing::info!(?choice, "the installer's first-run choice");
        // Unsaved, the file is still gone: the next start asks in the app.
        if let Err(error) = save_choice(state, choice) {
            tracing::warn!(%error, "could not save the first-run choice");
        }
    } else if decision == FirstRunDecision::Nothing && marked {
        // Nothing to ask: the decision is made, the install no longer new.
        if let Err(error) = state.save_setting_rows(vec![crate::settings::fresh_install_row(false)])
        {
            tracing::warn!(%error, "could not clear the fresh install marker");
        }
    }
    decision
}

/// Store the answer through the tracker's writer, clearing the fresh
/// install marker with it (the decision is saved).
fn save_choice(state: &AppState, choice: FirstRunChoice) -> Result<(), String> {
    state.save_setting_rows(vec![
        crate::settings::first_run_choice_row(choice),
        crate::settings::fresh_install_row(false),
    ])
}

/// This run's first-run state, managed by the app.
#[derive(Debug, Default)]
pub struct FirstRunSlot(Mutex<FirstRunState>);

impl FirstRunSlot {
    pub fn state(&self) -> FirstRunState {
        lock(&self.0).clone()
    }

    pub fn set(&self, decision: FirstRunDecision) {
        *lock(&self.0) = FirstRunState {
            decision,
            error: None,
        };
    }

    fn set_error(&self, error: Option<String>) {
        let mut state = lock(&self.0);
        if state.decision == FirstRunDecision::Migrate {
            state.error = error;
        }
    }

    /// An import ended. A migration that finished is done (the normal app,
    /// then What's new); one that failed keeps the import screen with its
    /// error. Imports started from the Import page change nothing here.
    pub fn import_ended(&self, last: &ImportProgress) {
        let mut state = lock(&self.0);
        if state.decision != FirstRunDecision::Migrate {
            return;
        }
        match last {
            ImportProgress::Finished { .. } => {
                *state = FirstRunState {
                    decision: FirstRunDecision::Nothing,
                    error: None,
                };
            }
            ImportProgress::Failed { message } => state.error = Some(message.clone()),
            _ => {}
        }
    }

    /// No question open and no migration under way.
    pub fn settled(&self) -> bool {
        let decision = lock(&self.0).decision;
        !matches!(decision, FirstRunDecision::Ask | FirstRunDecision::Migrate)
    }

    /// Whether What's new may open by itself (release spec section 7): never
    /// over the question or the migration, nor while any 0.x import runs.
    pub fn whats_new_may_open(&self, import_running: bool) -> bool {
        !import_running && self.settled()
    }
}

/// Start the migration import if this start migrates. `start_import` claims
/// the import slot and starts the import; a refusal shows as the error.
pub fn act_on_start(
    slot: &FirstRunSlot,
    start_import: &mut dyn FnMut() -> Result<(), CommandError>,
) {
    if slot.state().decision != FirstRunDecision::Migrate {
        return;
    }
    if let Err(error) = start_import() {
        tracing::warn!(?error, "could not start the 0.x migration");
        slot.set_error(Some(error.message().to_string()));
    }
}

/// The user's answer in the app, or "Try again" after a failed migration
/// (`Migrate` again). Anything else is refused: a choice is made once.
pub fn choose(
    state: &AppState,
    slot: &FirstRunSlot,
    choice: FirstRunChoice,
    start_import: &mut dyn FnMut() -> Result<(), CommandError>,
) -> Result<FirstRunState, CommandError> {
    let current = slot.state().decision;
    let retry = current == FirstRunDecision::Migrate && choice == FirstRunChoice::Migrate;
    if current != FirstRunDecision::Ask && !retry {
        return Err(CommandError::Refused(
            "the first-run choice is already made".to_string(),
        ));
    }
    if !retry {
        save_choice(state, choice).map_err(CommandError::Tracker)?;
        slot.set(FirstRunDecision::from_choice(choice));
    }
    if choice == FirstRunChoice::Migrate {
        slot.set_error(None);
        if let Err(error) = start_import() {
            slot.set_error(Some(error.message().to_string()));
            return Err(error);
        }
    }
    Ok(slot.state())
}

/// "Continue without importing" after a failed migration: the app shows
/// normally for this run (the Import page carries the retry). The saved
/// choice stays `migrate` (release spec section 13.2 step 3).
pub fn dismiss(slot: &FirstRunSlot, import_running: bool) -> Result<FirstRunState, CommandError> {
    let current = slot.state();
    let failed = current.decision == FirstRunDecision::Migrate && current.error.is_some();
    if !failed || import_running {
        return Err(CommandError::Refused(
            "only a failed 0.x import can be dismissed".to_string(),
        ));
    }
    slot.set(FirstRunDecision::Nothing);
    Ok(slot.state())
}

/// At app start, after the workers and before the main window: decide,
/// remember it for the window, and start a migration.
pub fn start(app: &AppHandle) {
    let slot = FirstRunSlot::default();
    let startup = app.state::<Startup>();
    if let Startup::Ready(state) = startup.inner() {
        let decision = at_start(state, crate::legacy::detected_path().is_some());
        tracing::info!(?decision, "first run");
        slot.set(decision);
        act_on_start(&slot, &mut || {
            crate::commands::spawn_legacy_import(app, state)
        });
    }
    app.manage(slot);
}

/// Whether the first-run choice is settled: no question open and no
/// migration under way. `whats_new.rs` passes it as `StartInput`'s
/// `first_run_settled` (it checks a running import itself). True when the
/// slot is not managed (a failed start never asks).
pub fn first_run_settled(app: &AppHandle) -> bool {
    let slot = app.try_state::<FirstRunSlot>();
    slot_settled(slot.as_deref())
}

/// `first_run_settled` for a slot that may be missing (a failed start never
/// manages one, and never asks).
pub fn slot_settled(slot: Option<&FirstRunSlot>) -> bool {
    slot.is_none_or(FirstRunSlot::settled)
}

/// What the main window shows for the first run.
#[tauri::command]
#[specta::specta]
pub fn first_run_state(slot: State<'_, FirstRunSlot>) -> FirstRunState {
    slot.state()
}

/// Leave a failed migration for this run ("Continue without importing").
#[tauri::command]
#[specta::specta]
pub fn dismiss_first_run(
    app: AppHandle,
    startup: State<'_, Startup>,
    slot: State<'_, FirstRunSlot>,
) -> Result<FirstRunState, CommandError> {
    let state = crate::commands::ready(&startup)?;
    let running = state
        .import_running
        .load(std::sync::atomic::Ordering::SeqCst);
    let dismissed = dismiss(&slot, running)?;
    settings_changed(&app, state);
    Ok(dismissed)
}

/// Tell the windows the settings may have changed: the main window's What's
/// new check waits for this or the import's end (release spec section 7).
fn settings_changed(app: &AppHandle, state: &AppState) {
    match crate::commands::read_settings(state) {
        Ok(settings) => {
            if let Err(error) = (crate::events::SettingsChanged { settings }).emit(app) {
                tracing::warn!(%error, "could not push the settings change");
            }
        }
        Err(error) => tracing::warn!(?error, "could not read the settings"),
    }
}

/// The in-app answer ("Migrate from MyMTGO 0.x" or "Start fresh"), or "Try
/// again" after a failed migration.
#[tauri::command]
#[specta::specta]
pub fn choose_first_run(
    app: AppHandle,
    startup: State<'_, Startup>,
    slot: State<'_, FirstRunSlot>,
    choice: FirstRunChoice,
) -> Result<FirstRunState, CommandError> {
    let state = crate::commands::ready(&startup)?;
    let chosen = choose(state, &slot, choice, &mut || {
        crate::commands::spawn_legacy_import(&app, state)
    })?;
    // The answer is a setting.
    settings_changed(&app, state);
    Ok(chosen)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::lock;
    use crate::commands::tests::ready_state;
    use crate::legacy::fake_launch::FakeLaunchAtLogin;
    use crate::legacy::run_import;
    use crate::legacy::tests::legacy_install;
    use crate::settings::FIRST_RUN_CHOICE;
    use fixture_tools::TestDir;

    use FirstRunDecision::{Ask, Fresh, Migrate, Nothing};

    #[test]
    fn a_file_choice_wins_and_is_saved() {
        for detected in [true, false] {
            assert_eq!(
                decide_first_run(Some("migrate"), None, false, detected),
                Migrate
            );
            assert_eq!(
                decide_first_run(Some("fresh"), None, false, detected),
                Fresh
            );
        }
        // The installer may leave a line break or spaces.
        assert_eq!(
            decide_first_run(Some(" migrate\r\n"), None, false, true),
            Migrate
        );

        let dir = TestDir::new("first-run-file-saved");
        let (state, _events) = ready_state(&dir);
        dir.write("data/first-run-choice.txt", b"fresh");
        assert_eq!(at_start(&state, true), Fresh);
        assert_eq!(saved(&state), Some(r#""fresh""#.to_string()));
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn a_saved_choice_ignores_a_new_file() {
        for saved in [FirstRunChoice::Migrate, FirstRunChoice::Fresh] {
            for file in [Some("migrate"), Some("fresh"), Some("what"), None] {
                assert_eq!(decide_first_run(file, Some(saved), false, true), Nothing);
            }
        }
    }

    #[test]
    fn existing_matches_mean_nothing_to_do() {
        for file in [Some("migrate"), Some("fresh"), Some("what"), None] {
            assert_eq!(decide_first_run(file, None, true, true), Nothing);
        }
    }

    #[test]
    fn no_file_with_0x_data_asks() {
        assert_eq!(decide_first_run(None, None, false, true), Ask);
    }

    #[test]
    fn no_file_without_0x_data_does_nothing() {
        assert_eq!(decide_first_run(None, None, false, false), Nothing);
    }

    #[test]
    fn an_unknown_word_in_the_file_asks() {
        for word in ["", "yes", "Migrate!", "fresh migrate"] {
            assert_eq!(
                decide_first_run(Some(word), None, false, true),
                Ask,
                "{word}"
            );
            // Without 0.x data there is nothing to migrate.
            assert_eq!(
                decide_first_run(Some(word), None, false, false),
                Nothing,
                "{word}"
            );
        }
    }

    #[test]
    fn the_choice_file_is_deleted_whatever_it_says() {
        let dir = TestDir::new("first-run-file-deleted");
        let (state, _events) = ready_state(&dir);
        let file = dir.join("data").join(CHOICE_FILE);
        for (word, expected) in [
            (&b"nonsense"[..], Nothing),
            (&b"\xff\xfe"[..], Nothing),
            (&b"fresh"[..], Fresh),
            // Stale: a choice is saved now.
            (&b"migrate"[..], Nothing),
        ] {
            dir.write("data/first-run-choice.txt", word);
            assert_eq!(at_start(&state, false), expected);
            assert!(!file.exists(), "{word:?}");
        }
        // No file: nothing to delete, nothing fails.
        assert_eq!(at_start(&state, false), Nothing);
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn a_choice_is_acted_on_once() {
        let dir = TestDir::new("first-run-once");
        let (state, _events) = ready_state(&dir);
        let slot = FirstRunSlot::default();
        let mut imports = 0;
        for _ in 0..2 {
            // The installer's file again: the second start ignores it.
            dir.write("data/first-run-choice.txt", b"migrate");
            let decision = at_start(&state, true);
            slot.set(decision);
            act_on_start(&slot, &mut || {
                imports += 1;
                Ok(())
            });
        }
        assert_eq!(imports, 1);
        assert_eq!(saved(&state), Some(r#""migrate""#.to_string()));
        lock(&state.tracker).take().expect("tracker").stop();
    }

    /// Runs the import of `path` on this thread, as the app's import thread
    /// does, and tells the slot how it ended.
    fn import_now(
        state: &crate::commands::AppState,
        slot: &FirstRunSlot,
        path: &str,
    ) -> Result<(), CommandError> {
        let path = crate::legacy::begin(state, Some(path.to_string()))?;
        let mut last = None;
        run_import(
            state,
            &path,
            &FakeLaunchAtLogin::default(),
            &mut |progress| last = Some(progress),
        );
        slot.import_ended(last.as_ref().expect("a last report"));
        Ok(())
    }

    fn match_count(state: &crate::commands::AppState) -> u32 {
        store::read::count_matches(&lock(&state.reader)).expect("count")
    }

    #[test]
    fn migrate_starts_the_import_through_the_tracker() {
        let dir = TestDir::new("first-run-migrate");
        let (state, _events) = ready_state(&dir);
        let legacy = legacy_install(&dir).to_string_lossy().into_owned();
        dir.write("data/first-run-choice.txt", b"migrate");
        let slot = FirstRunSlot::default();
        slot.set(at_start(&state, true));
        assert_eq!(slot.state().decision, Migrate);
        assert!(!slot.whats_new_may_open(false), "the import comes first");
        act_on_start(&slot, &mut || import_now(&state, &slot, &legacy));
        // The tracker wrote the 0.x match (the reader sees it).
        assert_eq!(match_count(&state), 1);
        // Done: the normal app, then What's new.
        assert_eq!(
            slot.state(),
            FirstRunState {
                decision: Nothing,
                error: None
            }
        );
        assert!(slot.whats_new_may_open(false));
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn a_failed_import_keeps_the_choice_and_can_retry() {
        let dir = TestDir::new("first-run-retry");
        let (state, _events) = ready_state(&dir);
        let missing = dir
            .join("nowhere/database.sqlite")
            .to_string_lossy()
            .into_owned();
        dir.write("data/first-run-choice.txt", b"migrate");
        let slot = FirstRunSlot::default();
        slot.set(at_start(&state, true));
        act_on_start(&slot, &mut || import_now(&state, &slot, &missing));
        let failed = slot.state();
        assert_eq!(failed.decision, Migrate);
        assert!(failed.error.is_some(), "the error shows with Try again");
        assert!(!slot.whats_new_may_open(false));
        assert_eq!(saved(&state), Some(r#""migrate""#.to_string()));

        // Starting fresh instead is refused: the choice stays migrate.
        let refused = choose(&state, &slot, FirstRunChoice::Fresh, &mut || Ok(()));
        assert!(
            matches!(refused, Err(CommandError::Refused(_))),
            "{refused:?}"
        );

        // "Try again", now that the 0.x database is there.
        let legacy = legacy_install(&dir).to_string_lossy().into_owned();
        let retried = choose(&state, &slot, FirstRunChoice::Migrate, &mut || {
            import_now(&state, &slot, &legacy)
        });
        assert!(retried.is_ok(), "{retried:?}");
        assert_eq!(slot.state().decision, Nothing);
        assert_eq!(match_count(&state), 1);
        assert_eq!(saved(&state), Some(r#""migrate""#.to_string()));
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn a_failed_migration_can_be_dismissed_for_this_run() {
        let dir = TestDir::new("first-run-dismiss");
        let (state, _events) = ready_state(&dir);
        let missing = dir
            .join("nowhere/database.sqlite")
            .to_string_lossy()
            .into_owned();
        dir.write("data/first-run-choice.txt", b"migrate");
        let slot = FirstRunSlot::default();
        slot.set(at_start(&state, true));
        // Not before it failed: the import screen stays while it runs.
        assert!(matches!(
            dismiss(&slot, false),
            Err(CommandError::Refused(_))
        ));
        act_on_start(&slot, &mut || import_now(&state, &slot, &missing));
        // Not while a retry runs.
        assert!(matches!(
            dismiss(&slot, true),
            Err(CommandError::Refused(_))
        ));
        let dismissed = dismiss(&slot, false).expect("dismissed");
        assert_eq!(
            dismissed,
            FirstRunState {
                decision: Nothing,
                error: None
            }
        );
        assert!(slot.whats_new_may_open(false), "the app shows normally");
        // The saved choice stays migrate (spec 13.2 step 3), so the next
        // start does nothing more (step 2): the Import page carries the retry.
        assert_eq!(saved(&state), Some(r#""migrate""#.to_string()));
        assert_eq!(at_start(&state, true), Nothing);
        lock(&state.tracker).take().expect("tracker").stop();
    }

    // #316: the tracker's catch-up stores matches before the decision reads,
    // so a database created by this start still honours the installer's word.
    #[test]
    fn a_new_database_that_gained_matches_still_honours_the_installer() {
        let dir = TestDir::new("first-run-fresh-catch-up");
        let (state, _events) = ready_state(&dir);
        assert!(state.fresh_install);
        crate::commands::tests::store_a_match(&state);
        assert!(match_count(&state) > 0);
        dir.write("data/first-run-choice.txt", b"migrate");
        assert_eq!(at_start(&state, true), Migrate);
        assert_eq!(saved(&state), Some(r#""migrate""#.to_string()));
        lock(&state.tracker).take().expect("tracker").stop();
    }

    // An earlier database that holds matches is an existing user: no answer.
    #[test]
    fn an_earlier_database_with_matches_asks_nothing() {
        let dir = TestDir::new("first-run-existing");
        let (mut state, _events) = ready_state(&dir);
        state.fresh_install = false;
        crate::commands::tests::store_a_match(&state);
        dir.write("data/first-run-choice.txt", b"migrate");
        assert_eq!(at_start(&state, true), Nothing);
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn asking_saves_the_answer_and_continues() {
        let dir = TestDir::new("first-run-ask");
        let (state, _events) = ready_state(&dir);
        let slot = FirstRunSlot::default();
        slot.set(at_start(&state, true));
        assert_eq!(slot.state().decision, Ask);
        assert!(!slot.whats_new_may_open(false), "never over the question");
        let mut imports = 0;
        let fresh = choose(&state, &slot, FirstRunChoice::Fresh, &mut || {
            imports += 1;
            Ok(())
        })
        .expect("fresh");
        assert_eq!(fresh.decision, Fresh);
        assert_eq!(imports, 0, "start fresh never touches 0.x data");
        assert_eq!(saved(&state), Some(r#""fresh""#.to_string()));
        assert!(slot.whats_new_may_open(false));
        // Asked once: a second answer is refused.
        let again = choose(&state, &slot, FirstRunChoice::Migrate, &mut || Ok(()));
        assert!(matches!(again, Err(CommandError::Refused(_))), "{again:?}");
        // The next start asks nothing.
        assert_eq!(at_start(&state, true), Nothing);
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn whats_new_waits_for_a_running_import() {
        let slot = FirstRunSlot::default();
        assert!(slot.whats_new_may_open(false));
        assert!(!slot.whats_new_may_open(true));
    }

    fn marked(state: &crate::commands::AppState) -> bool {
        crate::settings::fresh_install(
            &store::read::settings(&lock(&state.reader)).expect("settings"),
        )
    }

    // Final review F4: the fresh install marker set by the start that made
    // the database stays until a decision is saved: not while the question
    // is open, cleared by the answer or by a start with nothing to ask.
    #[test]
    fn the_fresh_install_marker_clears_once_the_decision_is_saved() {
        let dir = TestDir::new("first-run-marker-ask");
        let (state, _events) = ready_state(&dir);
        assert!(marked(&state));
        let slot = FirstRunSlot::default();
        slot.set(at_start(&state, true));
        assert_eq!(slot.state().decision, Ask);
        assert!(marked(&state), "the question is still open");
        choose(&state, &slot, FirstRunChoice::Fresh, &mut || Ok(())).expect("fresh");
        assert!(!marked(&state), "answered");
        lock(&state.tracker).take().expect("tracker").stop();

        let dir = TestDir::new("first-run-marker-nothing");
        let (state, _events) = ready_state(&dir);
        assert_eq!(at_start(&state, false), Nothing);
        assert!(!marked(&state), "nothing to ask");
        lock(&state.tracker).take().expect("tracker").stop();

        let dir = TestDir::new("first-run-marker-file");
        let (state, _events) = ready_state(&dir);
        dir.write("data/first-run-choice.txt", b"migrate");
        assert_eq!(at_start(&state, true), Migrate);
        assert!(!marked(&state), "the installer's answer");
        lock(&state.tracker).take().expect("tracker").stop();
    }

    fn saved(state: &crate::commands::AppState) -> Option<String> {
        store::read::settings(&lock(&state.reader))
            .expect("settings")
            .into_iter()
            .find(|(key, _)| key == FIRST_RUN_CHOICE)
            .map(|(_, value)| value)
    }
}
