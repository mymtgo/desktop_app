//! MyMTGO Wrapped's commands (spec
//! `docs/superpowers/specs/2026-11-24-wrapped-design.md`): whether Wrapped
//! shows now, whether the player dismissed its dashboard card, and the bundle.
//!
//! The bundle is built on read from the local database (`store::wrapped_read`
//! into the pure `wrapped::build`) while Wrapped shows, and stays `None`
//! below the 30-match floor. Dev builds can preview a year out of season with
//! `MYMTGO_WRAPPED_PREVIEW=<year>`.

use chrono_tz::Tz;
use serde::Serialize;
use tauri::{AppHandle, Manager};
use wrapped::{WrappedBundle, wrapped_year, year_window};

use crate::bridge::lock;
use crate::commands::{CommandError, Startup, off_async_workers, ready, storage};
use crate::logging::Timer;

/// What the Wrapped page and the dashboard card read.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct WrappedState {
    /// The Wrapped year shown now; `None` outside December.
    pub year: Option<i32>,
    /// The player closed this year's dashboard card.
    pub dismissed: bool,
    /// This year's bundle; `None` below the 30-match floor, and always
    /// `None` when `year` is.
    pub bundle: Option<WrappedBundle>,
}

/// The settings key that remembers a dismissed dashboard card, one per year.
pub fn dismissed_key(year: i32) -> String {
    format!("wrapped_dismissed_{year}")
}

/// The environment variable a dev build reads to preview a year out of season.
pub const PREVIEW_VAR: &str = "MYMTGO_WRAPPED_PREVIEW";

/// The year shown at `now_ms`: December's year, or in a dev build (`dev`)
/// outside December the `preview` year when one was given.
pub fn shown_year(now_ms: i64, tz: Tz, dev: bool, preview: Option<&str>) -> Option<i32> {
    wrapped_year(now_ms, tz).or_else(|| {
        if !dev {
            return None;
        }
        preview.and_then(|year| year.trim().parse::<i32>().ok())
    })
}

/// The state for `year` from the stored settings rows, without a bundle.
pub fn state_of(year: Option<i32>, settings: &[(String, String)]) -> WrappedState {
    let mut dismissed = false;
    if let Some(year) = year {
        let key = dismissed_key(year);
        for (stored_key, value) in settings {
            if *stored_key == key && value == "1" {
                dismissed = true;
            }
        }
    }
    WrappedState {
        year,
        dismissed,
        bundle: None,
    }
}

/// The year shown now, from the clock, the timezone and (dev builds only)
/// the preview variable.
fn year_now(tz: Tz) -> Option<i32> {
    let preview = std::env::var(PREVIEW_VAR).ok();
    shown_year(
        crate::wall_clock_ms(),
        tz,
        cfg!(debug_assertions),
        preview.as_deref(),
    )
}

/// The state with this year's bundle built from the database.
fn read_state(state: &crate::commands::AppState) -> Result<WrappedState, CommandError> {
    let year = year_now(state.timezone);
    let conn = lock(&state.reader);
    let rows = store::read::settings(&conn).map_err(storage)?;
    let mut shown = state_of(year, &rows);
    if let Some(year) = year {
        let now = crate::wall_clock_ms();
        let window = year_window(year, state.timezone);
        let input = store::wrapped_read::wrapped_input(&conn, year, window, state.timezone, now)
            .map_err(storage)?;
        shown.bundle = wrapped::build(&input);
    }
    Ok(shown)
}

#[tauri::command]
#[specta::specta]
pub async fn wrapped_state(app: AppHandle) -> Result<WrappedState, CommandError> {
    let _timer = Timer::start("wrapped_state");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        read_state(ready(&startup)?)
    })
    .await
}

/// Hides the shown year's dashboard card for good. The page stays
/// reachable. The year comes from the clock here, never from the caller, so
/// a stale page cannot dismiss another year; with no year shown nothing is
/// stored.
#[tauri::command]
#[specta::specta]
pub async fn dismiss_wrapped(app: AppHandle) -> Result<WrappedState, CommandError> {
    let _timer = Timer::start("dismiss_wrapped");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        if let Some(year) = year_now(state.timezone) {
            state
                .save_setting_rows(vec![(dismissed_key(year), "1".to_string())])
                .map_err(CommandError::Tracker)?;
        }
        read_state(state)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use chrono_tz::Europe::London;

    fn at(y: i32, m: u32, d: u32) -> i64 {
        London
            .with_ymd_and_hms(y, m, d, 12, 0, 0)
            .single()
            .expect("a local time")
            .timestamp_millis()
    }

    #[test]
    fn outside_december_there_is_no_year() {
        assert_eq!(shown_year(at(2026, 6, 15), London, false, None), None);
        assert_eq!(
            shown_year(at(2026, 12, 10), London, false, None),
            Some(2026)
        );
        assert_eq!(state_of(None, &[]).bundle, None);
    }

    #[test]
    fn only_a_dev_build_previews_a_year() {
        let november = at(2026, 11, 25);
        assert_eq!(shown_year(november, London, true, Some("2026")), Some(2026));
        assert_eq!(
            shown_year(november, London, false, Some("2026")),
            None,
            "release builds ignore it"
        );
        assert_eq!(shown_year(november, London, true, Some("soon")), None);
        assert_eq!(shown_year(november, London, true, None), None);
        // December wins over a preview.
        assert_eq!(
            shown_year(at(2026, 12, 10), London, true, Some("2025")),
            Some(2026)
        );
    }

    #[test]
    fn dismissed_is_per_year() {
        let rows = vec![(dismissed_key(2026), "1".to_string())];
        assert_eq!(dismissed_key(2026), "wrapped_dismissed_2026");
        assert!(state_of(Some(2026), &rows).dismissed);
        assert!(!state_of(Some(2027), &rows).dismissed);
        assert!(!state_of(None, &rows).dismissed);
    }
}
