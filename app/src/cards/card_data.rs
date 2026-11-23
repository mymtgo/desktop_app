//! The Settings "Card data" card (cards survey section 5): how many cards
//! are stored and how many still lack details, and "Download card data"
//! (0.x `RunPopulateCardsController`: the full stub scan, then population,
//! answered when done).

use serde::Serialize;
use tauri::AppHandle;
use tauri::Manager;

use crate::bridge::lock;
use crate::cards::worker::populate_now;
use crate::commands::{CommandError, Startup, off_async_workers, ready, storage};

/// The prefix of every failure the button shows (0.x's flash).
pub const POPULATION_FAILED: &str = "Card population failed";

/// The card's summary numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CardDataStatus {
    /// Rows in `cards`.
    pub total: u32,
    /// Rows without a name.
    pub incomplete: u32,
}

/// The stored card counts.
#[tauri::command]
#[specta::specta]
pub async fn card_data_status(app: AppHandle) -> Result<CardDataStatus, CommandError> {
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let counts = store::cards::card_data_counts(&lock(&state.reader)).map_err(storage)?;
        Ok(CardDataStatus {
            total: counts.total,
            incomplete: counts.incomplete,
        })
    })
    .await
}

/// "Download card data": a stub for every catalog id stored anywhere (a
/// device whose history came by sync has none), then a population run,
/// answered when it is done. A failure is "Card population failed:
/// {message}".
#[tauri::command]
#[specta::specta]
pub async fn download_card_data(app: AppHandle) -> Result<(), CommandError> {
    run_download(app).await
}

/// The stub scan then population, off the IPC thread (the Cards page's
/// "Fetch missing" runs the same thing).
pub async fn run_download(app: AppHandle) -> Result<(), CommandError> {
    off_async_workers(move || {
        let scan = || {
            let startup = app.state::<Startup>();
            let state = ready(&startup).map_err(|_| "the app is not ready".to_string())?;
            let scanner = state
                .tracker_part(tracker::TrackerHandle::card_stub_scanner)
                .map_err(|_| "tracking is restarting; try again".to_string())?;
            scanner.scan()
        };
        let populate = |scanned| populate_now(&app, scanned);
        download(&scan, &populate).map_err(CommandError::Refused)
    })
    .await
}

/// The scan, then population (told how many stubs the scan added, so their
/// `StubsAdded` report does not queue a second run); either failing is the
/// button's error.
pub fn download(
    scan: &dyn Fn() -> Result<u32, String>,
    populate: &dyn Fn(u32) -> Result<(), String>,
) -> Result<(), String> {
    scan()
        .and_then(populate)
        .map_err(|message| format!("{POPULATION_FAILED}: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::populate::tests::tracked;

    // 0.x RunPopulateCardsControllerTest: "creates the missing card stubs
    // the local data already references".
    #[test]
    fn creates_the_missing_card_stubs_the_local_data_already_references() {
        let run = tracked("card-data-download");
        let signature = decks::signature(&[decks::DeckCard {
            catalog_id: 4242,
            quantity: 4,
            sideboard: false,
        }]);
        run.write(&format!(
            "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source)
                 VALUES (900, 'q-deck', 'Q deck', 'CMODERN', 0, 0, 'imported');
             INSERT INTO deck_versions (deck_id, signature, modified_at)
                 VALUES (900, '{signature}', 0);"
        ));
        let scanner = lock(&run.state.tracker)
            .as_ref()
            .expect("tracker")
            .card_stub_scanner();
        let populated = std::sync::atomic::AtomicBool::new(false);
        let scanned = std::sync::atomic::AtomicU32::new(u32::MAX);
        let result = download(&|| scanner.scan(), &|added| {
            populated.store(true, std::sync::atomic::Ordering::SeqCst);
            scanned.store(added, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        });
        assert_eq!(result, Ok(()));
        assert!(populated.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(
            scanned.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "population is told what the scan added"
        );
        assert_eq!(run.card(4242).1, None, "a stub, filled by the run");
        let counts = store::cards::card_data_counts(
            &store::open_reader(&run.state.db_path).expect("reader"),
        )
        .expect("counts");
        assert_eq!((counts.total, counts.incomplete), (1, 1));
    }

    // 0.x "reports a failure back to the page rather than throwing".
    #[test]
    fn reports_a_failure_back_to_the_page_rather_than_throwing() {
        let result = download(&|| Ok(0), &|_| Err("Scryfall unreachable".to_string()));
        assert_eq!(
            result,
            Err("Card population failed: Scryfall unreachable".to_string())
        );
        let scan_failed = download(&|| Err("Qbusy".to_string()), &|_| Ok(()));
        assert_eq!(
            scan_failed,
            Err("Card population failed: Qbusy".to_string())
        );
    }
}
