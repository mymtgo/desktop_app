//! Period markers for the usage counts (usage analytics spec). Aptabase has
//! no user identifier and its session id resets every launch, so the app
//! sends one `daily_active`, `weekly_active` and `monthly_active` event per
//! local day, ISO week and month, `first_run` once per install, and
//! `install_age` (a broad range) with `app_started`. No identifier is kept:
//! the stored row holds only dates.

use chrono::{DateTime, Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Manager};

use crate::bridge::lock;
use crate::commands::Startup;
use crate::usage;

/// How often the running app checks whether a new day, week or month began.
const CHECK_EVERY_SECS: u64 = 3600;

/// `install_age` when the install predates this feature.
const EXISTING: &str = "existing";

const FIRST_RUN: &str = "first_run";

/// The launch event, sent with its `install_age` prop.
const APP_STARTED: &str = "app_started";

/// The stored row `usage_marks`: what was last counted, as plain dates.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageMarks {
    /// `2026-11-26`: the day this install first ran. `None` when the install
    /// predates the markers.
    pub installed_on: Option<String>,
    /// `2026-11-26`: the last day counted.
    pub day: Option<String>,
    /// `2026-W48`: the last ISO week counted.
    pub week: Option<String>,
    /// `2026-11`: the last month counted.
    pub month: Option<String>,
}

/// A period the app counts activity in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    Daily,
    Weekly,
    Monthly,
}

impl Marker {
    const ALL: [Marker; 3] = [Marker::Daily, Marker::Weekly, Marker::Monthly];

    fn event(self) -> &'static str {
        match self {
            Marker::Daily => "daily_active",
            Marker::Weekly => "weekly_active",
            Marker::Monthly => "monthly_active",
        }
    }

    /// The key of the period `date` falls in.
    fn period_key(self, date: NaiveDate) -> String {
        match self {
            Marker::Daily => date.format("%Y-%m-%d").to_string(),
            Marker::Weekly => {
                let week = date.iso_week();
                format!("{}-W{:02}", week.year(), week.week())
            }
            Marker::Monthly => date.format("%Y-%m").to_string(),
        }
    }

    fn stored(self, marks: &UsageMarks) -> Option<&str> {
        match self {
            Marker::Daily => marks.day.as_deref(),
            Marker::Weekly => marks.week.as_deref(),
            Marker::Monthly => marks.month.as_deref(),
        }
    }

    fn record(self, marks: &mut UsageMarks, key: String) {
        match self {
            Marker::Daily => marks.day = Some(key),
            Marker::Weekly => marks.week = Some(key),
            Marker::Monthly => marks.month = Some(key),
        }
    }
}

/// What one check decided: the events to send and the marks to store.
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    pub events: Vec<&'static str>,
    pub marks: UsageMarks,
}

/// Decide what is due on `today` (the local date). `stored` is `None` when
/// the row does not exist yet. While the gate is closed nothing is counted
/// and no period key advances, so turning sharing on later still counts that
/// day; `installed_on` is a local fact and is recorded either way.
pub fn plan(
    today: NaiveDate,
    fresh_install: bool,
    gate_open: bool,
    stored: Option<&UsageMarks>,
) -> Plan {
    let mut marks = stored.cloned().unwrap_or_default();
    let mut events = Vec::new();
    if stored.is_none() && fresh_install {
        marks.installed_on = Some(Marker::Daily.period_key(today));
        if gate_open {
            events.push(FIRST_RUN);
        }
    }
    if gate_open {
        for marker in Marker::ALL {
            let key = marker.period_key(today);
            if marker.stored(&marks) != Some(key.as_str()) {
                events.push(marker.event());
                marker.record(&mut marks, key);
            }
        }
    }
    Plan { events, marks }
}

/// How long ago the install was, in a broad range. A clock that went
/// backwards reads as the first range; no usable install day is `existing`.
pub fn install_age(today: NaiveDate, installed_on: Option<&str>) -> &'static str {
    let Some(installed) =
        installed_on.and_then(|day| NaiveDate::parse_from_str(day, "%Y-%m-%d").ok())
    else {
        return EXISTING;
    };
    match (today - installed).num_days() {
        ..=7 => "0-7d",
        8..=30 => "8-30d",
        31..=90 => "31-90d",
        _ => "90d+",
    }
}

/// Today's date in `timezone`, from the real clock.
fn local_today(timezone: chrono_tz::Tz) -> Option<NaiveDate> {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    let now = DateTime::from_timestamp(i64::try_from(since_epoch.as_secs()).ok()?, 0)?;
    Some(now.with_timezone(&timezone).date_naive())
}

/// Run one check: store the marks, then send what is due. Returns the
/// `install_age` to report. Never fails the app: a problem is logged and
/// nothing is sent for it.
pub fn check(app: &AppHandle) -> &'static str {
    let startup = app.state::<Startup>();
    let Startup::Ready(state) = &*startup else {
        return EXISTING;
    };
    let rows = match store::read::settings(&lock(&state.reader)) {
        Ok(rows) => rows,
        Err(error) => {
            tracing::debug!(%error, "could not read the usage marks");
            return EXISTING;
        }
    };
    let stored = crate::settings::usage_marks(&rows);
    let Some(today) = local_today(state.timezone) else {
        return EXISTING;
    };
    let plan = plan(
        today,
        state.fresh_install,
        usage::gate_open(app),
        stored.as_ref(),
    );
    let age = install_age(today, plan.marks.installed_on.as_deref());
    if stored.as_ref() != Some(&plan.marks) {
        let row = crate::settings::usage_marks_row(&plan.marks);
        if let Err(error) = state.save_setting_rows(vec![row]) {
            tracing::warn!(%error, "could not store the usage marks");
            return age;
        }
    }
    for event in plan.events {
        usage::send(app, event, None);
    }
    age
}

/// At startup: check the markers, then count the launch with its install age.
/// Then check once an hour, so an app left in the tray still counts each day.
pub fn start(app: &AppHandle) {
    let age = check(app);
    usage::send(app, APP_STARTED, Some(json!({ "install_age": age })));
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("usage-marks".to_string())
        .spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(CHECK_EVERY_SECS));
                check(&app);
            }
        });
    if let Err(error) = spawned {
        tracing::debug!(%error, "could not start the usage marks thread");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").expect("date")
    }

    fn marks(day: &str, week: &str, month: &str) -> UsageMarks {
        UsageMarks {
            installed_on: Some("2026-01-01".to_string()),
            day: Some(day.to_string()),
            week: Some(week.to_string()),
            month: Some(month.to_string()),
        }
    }

    #[test]
    fn a_fresh_install_sends_first_run_and_every_period() {
        let plan = plan(date("2026-11-26"), true, true, None);
        assert_eq!(
            plan.events,
            [
                "first_run",
                "daily_active",
                "weekly_active",
                "monthly_active"
            ]
        );
        assert_eq!(plan.marks.installed_on.as_deref(), Some("2026-11-26"));
        assert_eq!(plan.marks.day.as_deref(), Some("2026-11-26"));
        assert_eq!(plan.marks.week.as_deref(), Some("2026-W48"));
        assert_eq!(plan.marks.month.as_deref(), Some("2026-11"));
    }

    #[test]
    fn an_upgrade_sends_no_first_run_and_has_no_install_day() {
        let plan = plan(date("2026-11-26"), false, true, None);
        assert_eq!(
            plan.events,
            ["daily_active", "weekly_active", "monthly_active"]
        );
        assert_eq!(plan.marks.installed_on, None);
        assert_eq!(
            install_age(date("2026-11-26"), plan.marks.installed_on.as_deref()),
            "existing"
        );
    }

    #[test]
    fn a_second_check_the_same_day_sends_nothing() {
        let first = plan(date("2026-11-26"), true, true, None);
        let again = plan(date("2026-11-26"), true, true, Some(&first.marks));
        assert!(again.events.is_empty());
        assert_eq!(again.marks, first.marks);
    }

    #[test]
    fn a_fresh_flag_with_a_stored_row_is_not_a_new_install() {
        let stored = marks("2026-11-26", "2026-W48", "2026-11");
        let again = plan(date("2026-11-27"), true, true, Some(&stored));
        assert_eq!(again.events, ["daily_active"]);
        assert_eq!(again.marks.installed_on.as_deref(), Some("2026-01-01"));
    }

    #[test]
    fn midnight_starts_a_new_day_only() {
        // Thursday to Friday, same week and month.
        let stored = marks("2026-11-26", "2026-W48", "2026-11");
        let next = plan(date("2026-11-27"), false, true, Some(&stored));
        assert_eq!(next.events, ["daily_active"]);
        assert_eq!(next.marks.day.as_deref(), Some("2026-11-27"));
        assert_eq!(next.marks.week.as_deref(), Some("2026-W48"));
    }

    #[test]
    fn a_new_iso_week_is_due() {
        // Sunday to Monday, same month.
        let stored = marks("2026-11-22", "2026-W47", "2026-11");
        let next = plan(date("2026-11-23"), false, true, Some(&stored));
        assert_eq!(next.events, ["daily_active", "weekly_active"]);
        assert_eq!(next.marks.week.as_deref(), Some("2026-W48"));
    }

    #[test]
    fn a_new_month_is_due() {
        let stored = marks("2026-11-30", "2026-W49", "2026-11");
        let next = plan(date("2026-12-01"), false, true, Some(&stored));
        assert_eq!(next.events, ["daily_active", "monthly_active"]);
        assert_eq!(next.marks.month.as_deref(), Some("2026-12"));
    }

    #[test]
    fn the_iso_week_follows_the_iso_year_at_the_year_boundary() {
        assert_eq!(Marker::Weekly.period_key(date("2026-12-31")), "2026-W53");
        assert_eq!(Marker::Weekly.period_key(date("2027-01-01")), "2026-W53");
        assert_eq!(Marker::Weekly.period_key(date("2027-01-04")), "2027-W01");
        let stored = marks("2026-12-31", "2026-W53", "2026-12");
        let next = plan(date("2027-01-01"), false, true, Some(&stored));
        assert_eq!(next.events, ["daily_active", "monthly_active"]);
    }

    #[test]
    fn a_closed_gate_counts_nothing_and_keeps_every_key() {
        let stored = marks("2026-11-25", "2026-W48", "2026-11");
        let closed = plan(date("2026-11-27"), false, false, Some(&stored));
        assert!(closed.events.is_empty());
        assert_eq!(closed.marks, stored);
        let reopened = plan(date("2026-11-27"), false, true, Some(&closed.marks));
        assert_eq!(reopened.events, ["daily_active"]);
    }

    #[test]
    fn a_closed_gate_still_records_the_install_day_but_sends_no_first_run() {
        let plan = plan(date("2026-11-26"), true, false, None);
        assert!(plan.events.is_empty());
        assert_eq!(plan.marks.installed_on.as_deref(), Some("2026-11-26"));
        assert_eq!(plan.marks.day, None);
    }

    #[test]
    fn install_age_buckets_at_their_boundaries() {
        let installed = Some("2026-01-01");
        let age = |today: &str| install_age(date(today), installed);
        assert_eq!(age("2026-01-01"), "0-7d");
        assert_eq!(age("2026-01-08"), "0-7d", "day 7");
        assert_eq!(age("2026-01-09"), "8-30d", "day 8");
        assert_eq!(age("2026-01-31"), "8-30d", "day 30");
        assert_eq!(age("2026-02-01"), "31-90d", "day 31");
        assert_eq!(age("2026-04-01"), "31-90d", "day 90");
        assert_eq!(age("2026-04-02"), "90d+", "day 91");
    }

    #[test]
    fn a_clock_that_went_backwards_is_in_the_first_range() {
        assert_eq!(install_age(date("2025-12-01"), Some("2026-01-01")), "0-7d");
    }

    #[test]
    fn an_unreadable_install_day_is_existing() {
        assert_eq!(
            install_age(date("2026-11-26"), Some("last week")),
            "existing"
        );
    }
}
