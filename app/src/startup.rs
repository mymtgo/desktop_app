//! Getting ready to track: database migrations, the read connection, stored
//! settings, and the timezone. The Tauri wiring in `lib.rs` uses the result.

use std::path::{Path, PathBuf};

use chrono_tz::Tz;
use store::Connection;

use crate::settings::Settings;

pub const DB_FILE: &str = "mymtgo.sqlite";

pub struct Prepared {
    pub db_path: PathBuf,
    pub reader: Connection,
    pub settings: Settings,
    pub timezone: Tz,
    pub timezone_warning: Option<String>,
    /// Offline mode as the stored rows say (it fails closed).
    pub offline_mode: bool,
    /// A fresh install: the database file did not exist before this start
    /// opened it, or an earlier start created it and ended before its
    /// first-run decision was saved (the `fresh_install` setting, final
    /// review F4). Read here, before the tracker starts: its catch-up may
    /// commit matches before the first-run and What's new decisions look.
    pub fresh_install: bool,
}

/// Whether the install has history of its own: a database that is not a
/// fresh install and holds matches. A fresh install never does, whatever the
/// tracker's catch-up has stored since.
pub fn is_existing_install(fresh_install: bool, match_count: u32) -> bool {
    !fresh_install && match_count > 0
}

/// Migrate the database and read what the tracker needs to start. `zone` is
/// the OS timezone and its warning (`timezone::system_timezone()`).
pub fn prepare(data_dir: &Path, zone: (Tz, Option<String>)) -> Result<Prepared, String> {
    let db_path = data_dir.join(DB_FILE);
    let created = !db_path.exists();
    // Migrations run before anything reads (spec 4.1 rule 8). This writer closes
    // before the tracker opens its own, so there is still one writer at a time.
    let mut writer = store::open_writer(&db_path)
        .map_err(|error| format!("could not open the database: {error}"))?;
    if created {
        // Cleared once the first-run decision is saved (`first_run.rs`).
        let batch = store::write::WriteBatch {
            settings: vec![crate::settings::fresh_install_row(true)],
            ..store::write::WriteBatch::default()
        };
        store::write::commit(&mut writer, &batch)
            .map_err(|error| format!("could not mark a new database: {error}"))?;
    }
    drop(writer);
    let reader = store::open_reader(&db_path)
        .map_err(|error| format!("could not open the database: {error}"))?;
    let rows = store::read::settings(&reader)
        .map_err(|error| format!("could not read settings: {error}"))?;
    Ok(Prepared {
        db_path,
        reader,
        settings: Settings::from_rows(&rows),
        timezone: zone.0,
        timezone_warning: zone.1,
        offline_mode: mymtgo_api::read_offline(Ok(&rows)),
        fresh_install: created || crate::settings::fresh_install(&rows),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Tz;
    use fixture_tools::TestDir;
    use store::write::{WriteBatch, commit};

    #[test]
    fn a_fresh_data_folder_is_prepared_with_default_settings() {
        let dir = TestDir::new("startup-fresh");
        let prepared =
            prepare(&dir.join("data"), (Tz::UTC, Some("tz".to_string()))).expect("prepare");
        assert_eq!(prepared.db_path, dir.join("data").join(DB_FILE));
        assert!(prepared.db_path.exists());
        assert_eq!(prepared.settings, Settings::default());
        assert_eq!(prepared.timezone, Tz::UTC);
        assert_eq!(prepared.timezone_warning.as_deref(), Some("tz"));
        assert!(prepared.fresh_install);
    }

    // Final review F4: a first start that ends before its first-run decision
    // (a crash, or closed at once) leaves its marker, so the next start is
    // still a fresh install even though the catch-up stored matches.
    #[test]
    fn a_start_that_ended_before_its_first_run_decision_is_still_fresh() {
        let dir = TestDir::new("startup-crash-before-decision");
        let data = dir.join("data");
        assert!(
            prepare(&data, (Tz::UTC, None))
                .expect("first")
                .fresh_install
        );
        let writer = store::open_writer(&data.join(DB_FILE)).expect("writer");
        writer
            .execute_batch(
                "INSERT INTO accounts (username, first_seen_at) VALUES ('Qlocal', 0);
                 INSERT INTO matches (token, account_id, source, state, outcome, started_at,
                     last_activity_at, revision, derived_version)
                 VALUES ('q-m', 1, 'log', 'complete', 'win', 1, 1, 1, 1);",
            )
            .expect("a match from the catch-up");
        drop(writer);
        assert!(
            prepare(&data, (Tz::UTC, None))
                .expect("after a crash")
                .fresh_install,
            "the marker is still there"
        );
        // The decision saved clears the marker: from then on, not fresh.
        let mut writer = store::open_writer(&data.join(DB_FILE)).expect("writer");
        commit(
            &mut writer,
            &WriteBatch {
                settings: vec![crate::settings::fresh_install_row(false)],
                ..WriteBatch::default()
            },
        )
        .expect("decided");
        drop(writer);
        assert!(
            !prepare(&data, (Tz::UTC, None))
                .expect("later")
                .fresh_install
        );
    }

    #[test]
    fn only_an_earlier_database_with_matches_is_an_existing_install() {
        assert!(!is_existing_install(true, 3));
        assert!(!is_existing_install(false, 0));
        assert!(is_existing_install(false, 3));
    }

    #[test]
    fn stored_settings_are_read_at_startup() {
        let dir = TestDir::new("startup-settings");
        let data = dir.join("data");
        drop(prepare(&data, (Tz::UTC, None)).expect("first start"));
        let saved = Settings {
            log_folder: Some("/home/q/.wine".to_string()),
            close_to_tray: false,
            mtgo_data_folder: Some("/home/q/mtgo-data".to_string()),
            ..Settings::default()
        };
        let mut writer = store::open_writer(&data.join(DB_FILE)).expect("writer");
        commit(
            &mut writer,
            &WriteBatch {
                settings: saved.to_rows(),
                ..WriteBatch::default()
            },
        )
        .expect("commit");
        drop(writer);
        assert_eq!(
            prepare(&data, (Tz::UTC, None))
                .expect("second start")
                .settings,
            saved
        );
    }

    #[test]
    fn a_data_folder_that_cannot_be_used_fails_with_a_message() {
        let dir = TestDir::new("startup-broken");
        // A file where the data folder should be.
        dir.write("data", b"not a folder");
        let error = prepare(&dir.join("data"), (Tz::UTC, None))
            .err()
            .expect("fails");
        assert!(error.contains("could not open the database"), "{error}");
    }

    // Q4 review M2: offline mode is known before the tracker starts, so its
    // first status already carries the flag.
    #[test]
    fn offline_mode_is_read_at_startup() {
        let dir = TestDir::new("startup-offline");
        let data = dir.join("data");
        assert!(!prepare(&data, (Tz::UTC, None)).expect("first").offline_mode);
        let mut writer = store::open_writer(&data.join(DB_FILE)).expect("writer");
        commit(
            &mut writer,
            &WriteBatch {
                settings: vec![(mymtgo_api::OFFLINE_MODE.to_string(), "true".to_string())],
                ..WriteBatch::default()
            },
        )
        .expect("commit");
        drop(writer);
        assert!(
            prepare(&data, (Tz::UTC, None))
                .expect("second")
                .offline_mode
        );
    }
}
