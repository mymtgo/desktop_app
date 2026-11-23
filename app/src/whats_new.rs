//! What's new (release spec section 7): whether the main window opens the
//! page by itself after start. The content lives in the interface
//! (`ui/src/content/whats-new/`), which passes the `major.minor` of each entry.

use std::sync::atomic::Ordering;

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::bridge::lock;
use crate::commands::{AppState, CommandError, Startup, ready, storage};
use crate::logging::Timer;

/// 0.x `ShouldShowWhatsNew`, with the 0.x user addition (ruling 11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Open the page and save the running version as seen.
    Show,
    /// A fresh install: save the running version as seen, open nothing.
    Seed,
    Nothing,
}

/// The `major.minor` of a version, as 0.x reads it: a leading `v` is
/// ignored, each part is its leading digits (so `0-beta.2` is 0), and a
/// missing part is 0. Patches, prereleases and build metadata never count.
pub fn major_minor(version: &str) -> (u64, u64) {
    let trimmed = version.trim().trim_start_matches('v');
    let mut parts = trimmed.split('.');
    let major = leading_number(parts.next().unwrap_or(""));
    let minor = leading_number(parts.next().unwrap_or(""));
    (major, minor)
}

/// The digits at the start of `part` as a number (PHP's `(int)` cast).
fn leading_number(part: &str) -> u64 {
    let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().unwrap_or(0)
}

/// Whether the interface has a page for `running`: an entry (its
/// `major.minor`) not newer than the running version (spec section 7).
pub fn has_content(running: &str, entries: &[String]) -> bool {
    let running = major_minor(running);
    entries.iter().any(|entry| major_minor(entry) <= running)
}

/// Whether What's new opens by itself (spec section 7). `seen` is the
/// stored `whats_new_seen_version`; `has_matches` whether any match is
/// stored; `legacy_detected` whether a 0.x database is on this machine;
/// `content` whether `has_content` holds.
pub fn decide(
    running: &str,
    seen: Option<&str>,
    has_matches: bool,
    legacy_detected: bool,
    content: bool,
) -> Decision {
    if !content {
        return Decision::Nothing;
    }
    let seen = match seen {
        Some(seen) => seen,
        // A fresh install has nothing new yet, unless it moves from 0.x.
        None if !has_matches && !legacy_detected => return Decision::Seed,
        None => "0.0.0",
    };
    if major_minor(running) > major_minor(seen) {
        Decision::Show
    } else {
        Decision::Nothing
    }
}

/// The start command's answer to the main window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum WhatsNewAnswer {
    /// Open `/whats-new` (the seen version is already saved).
    Show,
    /// Not yet: the first-run choice is open or a 0.x import is running.
    /// Ask again once that is over.
    Wait,
    Nothing,
}

/// What the start command knows besides the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartInput {
    pub window_label: String,
    pub running: String,
    /// A development build (`debug_assertions`): never shows by itself.
    pub development: bool,
    /// The first-run choice (13.2) is settled: no question open and no
    /// migration under way (`first_run::first_run_settled`).
    pub first_run_settled: bool,
    pub legacy_detected: bool,
    /// The `major.minor` of each What's new entry the interface has.
    pub content: Vec<String>,
}

/// Decide once after start and save the seen version before the page opens,
/// so it shows once (0.x `RedirectToWhatsNew`). Only the main window asks:
/// the overlay and replay windows never open it.
pub fn on_start(state: &AppState, input: &StartInput) -> Result<WhatsNewAnswer, CommandError> {
    if input.window_label != crate::overlay::MAIN_WINDOW || input.development {
        return Ok(WhatsNewAnswer::Nothing);
    }
    if !input.first_run_settled || state.import_running.load(Ordering::SeqCst) {
        return Ok(WhatsNewAnswer::Wait);
    }
    let (seen, has_matches) = {
        let reader = lock(&state.reader);
        let rows = store::read::settings(&reader).map_err(storage)?;
        let count = store::read::count_matches(&reader).map_err(storage)?;
        (
            crate::settings::whats_new_seen_version(&rows),
            crate::startup::is_existing_install(state.fresh_install, count),
        )
    };
    let decision = decide(
        &input.running,
        seen.as_deref(),
        has_matches,
        input.legacy_detected,
        has_content(&input.running, &input.content),
    );
    if decision == Decision::Nothing {
        return Ok(WhatsNewAnswer::Nothing);
    }
    save_seen(state, &input.running)?;
    if decision == Decision::Show {
        Ok(WhatsNewAnswer::Show)
    } else {
        Ok(WhatsNewAnswer::Nothing)
    }
}

/// Store the seen version through the tracker (the only writer).
fn save_seen(state: &AppState, running: &str) -> Result<(), CommandError> {
    state
        .save_setting_rows(vec![crate::settings::whats_new_seen_version_row(running)])
        .map_err(CommandError::Tracker)
}

/// Asked by the main window after start (spec section 7). `content` is the
/// `major.minor` of each entry in `ui/src/content/whats-new/index.ts`.
/// Async so the wait for the tracker's commit never blocks the window.
#[tauri::command]
#[specta::specta]
pub async fn whats_new_on_start(
    app: AppHandle,
    window: tauri::WebviewWindow,
    content: Vec<String>,
) -> Result<WhatsNewAnswer, CommandError> {
    let _timer = Timer::start("whats_new_on_start");
    let startup = app.state::<Startup>();
    let state = ready(&startup)?;
    let input = StartInput {
        window_label: window.label().to_string(),
        running: env!("CARGO_PKG_VERSION").to_string(),
        development: cfg!(debug_assertions),
        first_run_settled: crate::first_run::first_run_settled(&app),
        legacy_detected: crate::legacy::detected_path().is_some(),
        content,
    };
    on_start(state, &input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::tests::{ready_state, store_a_match};
    use crate::commands::{AppState, apply_settings, read_settings};
    use crate::first_run::FirstRunSlot;
    use crate::settings::{Settings, whats_new_seen_version};
    use fixture_tools::TestDir;
    use std::time::Instant;

    const MAIN: &str = "main";

    fn content() -> Vec<String> {
        vec!["0.45".to_string(), "1.0".to_string()]
    }

    /// A start in the main window of an installed build, first run settled.
    fn start(running: &str) -> StartInput {
        StartInput {
            window_label: MAIN.to_string(),
            running: running.to_string(),
            development: false,
            first_run_settled: true,
            legacy_detected: false,
            content: content(),
        }
    }

    fn seen(state: &AppState) -> Option<String> {
        let rows = store::read::settings(&crate::bridge::lock(&state.reader)).expect("rows");
        whats_new_seen_version(&rows)
    }

    fn set_seen(state: &AppState, version: &str) {
        state
            .save_setting_rows(vec![crate::settings::whats_new_seen_version_row(version)])
            .expect("saved");
    }

    fn stop(state: &AppState) {
        crate::bridge::lock(&state.tracker)
            .take()
            .expect("tracker")
            .stop();
    }

    // RedirectToWhatsNewTest: "redirects once on a minor bump".
    #[test]
    fn shows_once_on_a_minor_bump() {
        let dir = TestDir::new("whats-new-minor");
        let (state, _receiver) = ready_state(&dir);
        set_seen(&state, "0.44.1");
        assert_eq!(on_start(&state, &start("0.45.0")), Ok(WhatsNewAnswer::Show));
        assert_eq!(seen(&state).as_deref(), Some("0.45.0"));
        assert_eq!(
            on_start(&state, &start("0.45.0")),
            Ok(WhatsNewAnswer::Nothing)
        );
        stop(&state);
    }

    // "redirects on a major bump and across skipped versions".
    #[test]
    fn shows_on_a_major_bump_and_across_skipped_versions() {
        for (seen, running) in [("0.45.0", "1.0.0"), ("0.41.1", "0.45.2")] {
            assert_eq!(
                decide(running, Some(seen), true, false, true),
                Decision::Show,
                "{seen} to {running}"
            );
        }
    }

    // "does not redirect on a patch bump".
    #[test]
    fn a_patch_bump_shows_nothing() {
        assert_eq!(
            decide("0.45.1", Some("0.45.0"), true, false, true),
            Decision::Nothing
        );
    }

    // "seeds the version on a fresh install without redirecting".
    #[test]
    fn a_fresh_install_seeds_the_version_and_shows_nothing() {
        assert_eq!(decide("0.45.0", None, false, false, true), Decision::Seed);
        let dir = TestDir::new("whats-new-seed");
        let (state, _receiver) = ready_state(&dir);
        assert_eq!(
            on_start(&state, &start("0.45.0")),
            Ok(WhatsNewAnswer::Nothing)
        );
        assert_eq!(seen(&state).as_deref(), Some("0.45.0"));
        stop(&state);
    }

    // #316: the tracker's catch-up stores matches before the decision reads,
    // so a database created by this start is still a fresh install.
    #[test]
    fn a_new_database_that_gained_matches_is_still_a_fresh_install() {
        let dir = TestDir::new("whats-new-fresh-catch-up");
        let (state, _receiver) = ready_state(&dir);
        assert!(state.fresh_install);
        store_a_match(&state);
        assert_eq!(
            on_start(&state, &start("0.45.0")),
            Ok(WhatsNewAnswer::Nothing)
        );
        assert_eq!(seen(&state).as_deref(), Some("0.45.0"));
        stop(&state);
    }

    // "redirects existing users who have never seen the page".
    #[test]
    fn existing_users_who_never_saw_it_see_it() {
        assert_eq!(decide("0.45.0", None, true, false, true), Decision::Show);
        let dir = TestDir::new("whats-new-existing");
        let (mut state, _receiver) = ready_state(&dir);
        state.fresh_install = false;
        store_a_match(&state);
        assert_eq!(on_start(&state, &start("0.45.0")), Ok(WhatsNewAnswer::Show));
        assert_eq!(seen(&state).as_deref(), Some("0.45.0"));
        stop(&state);
    }

    // "does not redirect when the file is missing or empty": no entry, or
    // only entries newer than the running version.
    #[test]
    fn without_content_nothing_shows_and_nothing_is_saved() {
        assert_eq!(
            decide("0.45.0", Some("0.44.0"), true, false, false),
            Decision::Nothing
        );
        assert_eq!(
            decide("0.45.0", None, false, false, false),
            Decision::Nothing
        );
        let dir = TestDir::new("whats-new-no-content");
        let (state, _receiver) = ready_state(&dir);
        set_seen(&state, "0.44.0");
        for content in [Vec::new(), vec!["0.46".to_string()]] {
            let input = StartInput {
                content,
                ..start("0.45.0")
            };
            assert_eq!(on_start(&state, &input), Ok(WhatsNewAnswer::Nothing));
            assert_eq!(seen(&state).as_deref(), Some("0.44.0"));
        }
        stop(&state);
    }

    // "does not redirect partial reloads or posts": only the start command
    // decides; a settings save keeps the seen version as it was.
    #[test]
    fn other_saves_never_change_the_seen_version() {
        let dir = TestDir::new("whats-new-other-saves");
        let (state, _receiver) = ready_state(&dir);
        set_seen(&state, "0.44.0");
        let settings = Settings {
            log_folder: Some(dir.join("logs-a").to_string_lossy().into_owned()),
            ..read_settings(&state).expect("settings")
        };
        apply_settings(&state, settings).expect("saved");
        assert_eq!(seen(&state).as_deref(), Some("0.44.0"));
        stop(&state);
    }

    // "never redirects overlay windows": the overlay and replay windows are
    // other entries; only the main window asks, and any other is refused.
    #[test]
    fn the_page_never_opens_itself_from_overlay_or_replay_windows() {
        let dir = TestDir::new("whats-new-overlay");
        let (state, _receiver) = ready_state(&dir);
        set_seen(&state, "0.44.0");
        let mut labels: Vec<&str> = crate::overlay::OVERLAY_LABELS.to_vec();
        labels.push(crate::replay::window::GAME_REPLAY);
        for label in labels {
            let input = StartInput {
                window_label: label.to_string(),
                ..start("0.45.0")
            };
            assert_eq!(
                on_start(&state, &input),
                Ok(WhatsNewAnswer::Nothing),
                "{label}"
            );
        }
        assert_eq!(seen(&state).as_deref(), Some("0.44.0"));
        stop(&state);
    }

    // "does not redirect in local development": nothing shows and nothing is
    // seeded.
    #[test]
    fn development_builds_never_show_it() {
        let dir = TestDir::new("whats-new-development");
        let (state, _receiver) = ready_state(&dir);
        let input = StartInput {
            development: true,
            legacy_detected: true,
            ..start("1.0.0")
        };
        assert_eq!(on_start(&state, &input), Ok(WhatsNewAnswer::Nothing));
        assert_eq!(seen(&state), None);
        stop(&state);
    }

    // Ruling 11: the 1.0 page welcomes 0.x users and points to the import.
    #[test]
    fn a_0x_user_sees_the_1_0_page_on_first_run() {
        assert_eq!(decide("1.0.0", None, false, true, true), Decision::Show);
        let dir = TestDir::new("whats-new-0x-user");
        let (state, _receiver) = ready_state(&dir);
        let input = StartInput {
            legacy_detected: true,
            content: vec!["1.0".to_string()],
            ..start("1.0.0")
        };
        assert_eq!(on_start(&state, &input), Ok(WhatsNewAnswer::Show));
        assert_eq!(seen(&state).as_deref(), Some("1.0.0"));
        assert_eq!(on_start(&state, &input), Ok(WhatsNewAnswer::Nothing));
        stop(&state);
    }

    #[test]
    fn a_fresh_user_never_sees_it() {
        let dir = TestDir::new("whats-new-fresh-user");
        let (state, _receiver) = ready_state(&dir);
        let input = StartInput {
            content: vec!["1.0".to_string()],
            ..start("1.0.0")
        };
        assert_eq!(on_start(&state, &input), Ok(WhatsNewAnswer::Nothing));
        assert_eq!(seen(&state).as_deref(), Some("1.0.0"));
        assert_eq!(on_start(&state, &input), Ok(WhatsNewAnswer::Nothing));
        stop(&state);
    }

    #[test]
    fn prerelease_versions_compare_by_major_minor() {
        assert_eq!(
            decide("1.0.0", Some("1.0.0-beta.2"), true, false, true),
            Decision::Nothing
        );
        assert_eq!(
            decide("1.0.0-beta.3", Some("0.46.0"), true, false, true),
            Decision::Show
        );
        assert_eq!(
            decide("1.1.0-beta.1", Some("1.0.0"), true, false, true),
            Decision::Show
        );
        assert_eq!(major_minor("v1.2.3-beta.4+build"), (1, 2));
        assert_eq!(major_minor("junk"), (0, 0));
    }

    #[test]
    fn the_content_is_the_newest_entry_not_newer_than_the_running_version() {
        let entries = vec!["0.45".to_string(), "1.0".to_string()];
        assert!(has_content("1.0.0-beta.2", &entries));
        assert!(has_content("0.45.3", &entries));
        assert!(has_content("1.3.0", &entries));
        assert!(!has_content("0.44.9", &entries));
        assert!(!has_content("1.0.0", &[]));
    }

    // 13.2: it never covers the first-run choice or the 0.x import progress.
    #[test]
    fn whats_new_waits_for_the_first_run_choice() {
        let dir = TestDir::new("whats-new-waits");
        let (state, _receiver) = ready_state(&dir);
        let unsettled = StartInput {
            first_run_settled: false,
            legacy_detected: true,
            ..start("1.0.0")
        };
        assert_eq!(on_start(&state, &unsettled), Ok(WhatsNewAnswer::Wait));
        assert_eq!(seen(&state), None);
        state
            .import_running
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let importing = StartInput {
            legacy_detected: true,
            ..start("1.0.0")
        };
        assert_eq!(on_start(&state, &importing), Ok(WhatsNewAnswer::Wait));
        assert_eq!(seen(&state), None);
        state
            .import_running
            .store(false, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(on_start(&state, &importing), Ok(WhatsNewAnswer::Show));
        stop(&state);
    }

    /// The answer at start of a 0.x user (content present, nothing seen)
    /// with the first-run slot as `setup` leaves it, and whether an import
    /// runs. A fresh store each time, as each case is its own first start.
    fn first_run_answer(
        name: &str,
        importing: bool,
        setup: impl FnOnce(&FirstRunSlot),
    ) -> WhatsNewAnswer {
        let dir = TestDir::new(name);
        let (state, _receiver) = ready_state(&dir);
        let slot = FirstRunSlot::default();
        setup(&slot);
        state
            .import_running
            .store(importing, std::sync::atomic::Ordering::SeqCst);
        let input = StartInput {
            first_run_settled: crate::first_run::slot_settled(Some(&slot)),
            legacy_detected: true,
            ..start("1.0.0")
        };
        let answer = on_start(&state, &input).expect("answer");
        stop(&state);
        answer
    }

    // W12 seam (spec sections 7 and 13.2): never during the first-run
    // choice or a running migration; after the choice, a finished
    // migration, or "Continue without importing".
    #[test]
    fn whats_new_follows_the_first_run_choice() {
        use crate::first_run::{FirstRunDecision, dismiss};
        use crate::legacy::ImportProgress;
        let failed = ImportProgress::Failed {
            message: "the 0.x data could not be read".to_string(),
        };
        let finished = ImportProgress::Finished {
            summary: Box::new(crate::legacy::tests::summary("p")),
        };

        let asking = first_run_answer("whats-new-fr-ask", false, |slot| {
            slot.set(FirstRunDecision::Ask)
        });
        assert_eq!(asking, WhatsNewAnswer::Wait);
        let migrating = first_run_answer("whats-new-fr-migrating", true, |slot| {
            slot.set(FirstRunDecision::Migrate)
        });
        assert_eq!(migrating, WhatsNewAnswer::Wait);
        // A failed migration keeps its screen (Try again) and still waits.
        let failed_screen = first_run_answer("whats-new-fr-failed", false, |slot| {
            slot.set(FirstRunDecision::Migrate);
            slot.import_ended(&failed);
        });
        assert_eq!(failed_screen, WhatsNewAnswer::Wait);

        let fresh = first_run_answer("whats-new-fr-fresh", false, |slot| {
            slot.set(FirstRunDecision::Fresh)
        });
        assert_eq!(fresh, WhatsNewAnswer::Show);
        let migrated = first_run_answer("whats-new-fr-migrated", false, |slot| {
            slot.set(FirstRunDecision::Migrate);
            slot.import_ended(&finished);
        });
        assert_eq!(migrated, WhatsNewAnswer::Show);
        let dismissed = first_run_answer("whats-new-fr-dismissed", false, |slot| {
            slot.set(FirstRunDecision::Migrate);
            slot.import_ended(&failed);
            dismiss(slot, false).expect("dismissed");
        });
        assert_eq!(dismissed, WhatsNewAnswer::Show);
        // Nothing to ask (a later start, or a failed start with no slot).
        let nothing = first_run_answer("whats-new-fr-nothing", false, |slot| {
            slot.set(FirstRunDecision::Nothing)
        });
        assert_eq!(nothing, WhatsNewAnswer::Show);
        assert!(crate::first_run::slot_settled(None));
    }

    // Spec section 19: the decision at start is under 5 ms.
    #[test]
    fn decide_under_5_ms() {
        let entries: Vec<String> = (0..50).map(|minor| format!("1.{minor}")).collect();
        let started = Instant::now();
        let content = has_content("1.49.2", &entries);
        let decision = decide("1.49.2", Some("1.48.0"), true, true, content);
        let elapsed = started.elapsed();
        assert_eq!(decision, Decision::Show);
        assert!(elapsed.as_millis() < 5, "took {elapsed:?}");
    }
}
