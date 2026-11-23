//! The offline archetype download (2026-10-27 offline download design):
//! the player's one request while offline mode is on, and running
//! detection over every match on request. Nothing here runs on its own.

use mymtgo_api::{
    ApiError, offline_catalog_request, offline_definitions_check_request, parse_catalog,
    parse_definitions_check,
};
use serde::Serialize;
use store::archetype_edit::{ArchetypeEdit, ArchetypeEditOutcome, OfflineHeld};
use tauri::{AppHandle, Manager};

use crate::api::session::ApiState;
use crate::archetypes::definitions::Pacer;
use crate::archetypes::detect::{DEFINITIONS_PER_EDIT, Writer};
use crate::archetypes::worker::{ArchetypeSignal, ArchetypesWorker, STOPPED, TrackerWriter};
use crate::bridge::lock;
use crate::commands::{CommandError, off_async_workers};

/// The download's answer while offline mode is off.
pub const ONLY_OFFLINE: &str = "Downloading archetypes is only for offline mode.";

/// What the offline mode card shows after a download.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct OfflineDownloadView {
    pub added: u32,
    pub converted: u32,
    pub skipped: u32,
}

impl From<OfflineHeld> for OfflineDownloadView {
    fn from(held: OfflineHeld) -> OfflineDownloadView {
        OfflineDownloadView {
            added: held.added,
            converted: held.converted,
            skipped: held.skipped,
        }
    }
}

/// Every active server archetype with its full definition, the ones the
/// download holds (`archetypes::holdable_offline`): one list request, then
/// the definitions check in chunks, paced. Signed with the device key only;
/// any failure (a 401 too: no refresh offline) ends the download.
pub fn download_catalog(
    api: &ApiState,
    pacer: &mut Pacer,
) -> Result<Vec<archetypes::Definition>, String> {
    let failed = |error: ApiError| format!("Could not download the archetypes: {error}");
    pacer.before_request();
    let listed = api.send(&offline_catalog_request(), pacer.now_ms());
    pacer.after_request();
    let catalog = parse_catalog(&listed.map_err(failed)?).map_err(failed)?;
    let uuids: Vec<String> = catalog.into_iter().map(|entry| entry.uuid).collect();
    let mut definitions = Vec::new();
    for request in offline_definitions_check_request(&uuids) {
        pacer.before_request();
        let answer = api.send(&request, pacer.now_ms());
        pacer.after_request();
        let check = parse_definitions_check(&answer.map_err(failed)?).map_err(failed)?;
        definitions.extend(check.changed);
    }
    Ok(archetypes::holdable_offline(definitions))
}

/// The download's answer when offline mode was turned off while it ran.
pub const WENT_ONLINE: &str =
    "Offline mode was turned off before the download finished, so it stopped.";

/// Writes the definitions in slices of `DEFINITIONS_PER_EDIT` and sums
/// what each slice did. Each slice is written under the offline switch's
/// lock and only while offline mode is still on: the switch releases the
/// held rows under the same lock, so none is written after it.
pub fn hold_in_slices(
    api: &ApiState,
    writer: &mut dyn Writer,
    definitions: Vec<archetypes::Definition>,
    now_ms: i64,
) -> Result<OfflineHeld, String> {
    let mut total = OfflineHeld::default();
    for slice in definitions.chunks(DEFINITIONS_PER_EDIT) {
        let _one_switch = lock(&api.offline_switch);
        if !api.offline_mode().0 {
            return Err(WENT_ONLINE.to_string());
        }
        let outcome = writer.edit(ArchetypeEdit::HoldOffline {
            definitions: slice.to_vec(),
            now_ms,
        })?;
        let ArchetypeEditOutcome::OfflineHeld(held) = outcome else {
            return Err(format!("the archetypes could not be saved: {outcome:?}"));
        };
        total.added += held.added;
        total.converted += held.converted;
        total.skipped += held.skipped;
    }
    Ok(total)
}

/// The download job: refused while online, else the catalog downloaded
/// and held.
pub fn download_offline(
    api: &ApiState,
    pacer: &mut Pacer,
    writer: &mut dyn Writer,
    now_ms: i64,
) -> Result<OfflineHeld, String> {
    if !api.offline_mode().0 {
        return Err(ONLY_OFFLINE.to_string());
    }
    let definitions = download_catalog(api, pacer)?;
    hold_in_slices(api, writer, definitions, now_ms)
}

/// Rows still held offline while the app is online (a flip-back skipped
/// before the app was ready, or one whose settings save then failed) are
/// released, as going online does. Under the offline switch's lock, so it
/// never races the switch or a download. True when it released them.
pub fn reconcile_offline(
    api: &ApiState,
    held: bool,
    writer: &mut dyn Writer,
    now_ms: i64,
) -> Result<bool, String> {
    let _one_switch = lock(&api.offline_switch);
    if !held || api.offline_mode().0 {
        return Ok(false);
    }
    writer.edit(ArchetypeEdit::ReleaseOffline { now_ms })?;
    tracing::info!("archetypes held offline were released while online");
    Ok(true)
}

/// The answer to "Run detection" while offline mode is off.
pub const REDETECT_ONLY_OFFLINE: &str =
    "Running detection on every match is only for offline mode.";

/// "Run detection against your matches now": every detected match is
/// queued for detection again; the worker's backfill then detects them.
/// Answers how many were queued. Refused while online.
pub fn redetect_offline(api: &ApiState, writer: &mut dyn Writer) -> Result<u32, String> {
    if !api.offline_mode().0 {
        return Err(REDETECT_ONLY_OFFLINE.to_string());
    }
    match writer.edit(ArchetypeEdit::ClearAllDetected)? {
        ArchetypeEditOutcome::Done { matches, .. } => {
            Ok(u32::try_from(matches.len()).unwrap_or(u32::MAX))
        }
        outcome => Err(format!("detection could not be queued: {outcome:?}")),
    }
}

// ---- Commands ----------------------------------------------------------------

/// The offline mode card's "Download archetypes": the whole active catalog,
/// held as Custom archetypes. Waits for the worker, which always answers.
#[tauri::command]
#[specta::specta]
pub async fn download_offline_archetypes(
    app: AppHandle,
) -> Result<OfflineDownloadView, CommandError> {
    off_async_workers(move || {
        let worker = app.try_state::<ArchetypesWorker>().ok_or_else(|| {
            CommandError::Refused("the archetype worker is not running".to_string())
        })?;
        let (reply, answer) = std::sync::mpsc::channel();
        if !worker.signal(ArchetypeSignal::DownloadOffline { reply }) {
            return Err(CommandError::Refused(STOPPED.to_string()));
        }
        let held = answer
            .recv()
            .unwrap_or_else(|_| Err(STOPPED.to_string()))
            .map_err(CommandError::Refused)?;
        Ok(OfflineDownloadView::from(held))
    })
    .await
}

/// The offline mode card's "Run detection against your matches now".
#[tauri::command]
#[specta::specta]
pub async fn redetect_offline_matches(app: AppHandle) -> Result<u32, CommandError> {
    off_async_workers(move || {
        let api = app.state::<ApiState>();
        let mut writer = TrackerWriter::new(&app);
        let queued = redetect_offline(&api, &mut writer).map_err(CommandError::Refused)?;
        crate::archetypes::worker::signal(&app, ArchetypeSignal::MatchCompleted);
        Ok(queued)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::session::testing::device_api;
    use crate::api::stub::{Reply, Stub};
    use crate::archetypes::definitions::tests::{DEFINITION, pacer};
    use store::archetype_edit::{ArchetypeEdit, ArchetypeEditOutcome, OfflineHeld};

    const FALLBACK: &str = r#"{"uuid":"q-any","name":"Qany","format":"modern",
        "fallback":true,"common_cards":["Qcard A"],"cards":[]}"#;

    fn catalog_stub() -> Stub {
        Stub::start(|request| {
            if request.target.starts_with("/api/archetypes?") {
                Reply::json(
                    200,
                    r#"[{"uuid":"q-new","name":"Qnew Storm"},{"uuid":"q-any","name":"Qany"},
                        {"uuid":"q-gone","name":"Qgone"}]"#,
                )
            } else {
                Reply::json(
                    200,
                    &format!(
                        r#"{{"changed":[{DEFINITION},{FALLBACK}],"moved":[],"retired":["q-gone"]}}"#
                    ),
                )
            }
        })
    }

    /// Writes nothing, records each edit and answers `OfflineHeld` for one
    /// added row per definition.
    #[derive(Default)]
    struct Recording(Vec<usize>);

    impl Writer for Recording {
        fn edit(&mut self, edit: ArchetypeEdit) -> Result<ArchetypeEditOutcome, String> {
            let ArchetypeEdit::HoldOffline { definitions, .. } = edit else {
                return Err("unexpected edit".to_string());
            };
            self.0.push(definitions.len());
            Ok(ArchetypeEditOutcome::OfflineHeld(OfflineHeld {
                added: u32::try_from(definitions.len()).expect("small"),
                converted: 0,
                skipped: 0,
            }))
        }
    }

    #[test]
    fn download_catalog_reads_the_list_then_full_definitions() {
        let stub = catalog_stub();
        let api = device_api(&stub.base_url, true);
        let (mut pacer, _) = pacer();
        let definitions = download_catalog(&api.state, &mut pacer).expect("downloaded");
        let uuids: Vec<&str> = definitions.iter().map(|d| d.uuid.as_str()).collect();
        assert_eq!(uuids, ["q-new"], "the fallback definition is not held");
        let seen = stub.requests();
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert_eq!(seen[0].target, "/api/archetypes?active=1");
        assert_eq!(
            String::from_utf8(seen[1].body.clone()).expect("utf8"),
            r#"{"archetypes":[{"uuid":"q-new","version":null},{"uuid":"q-any","version":null},{"uuid":"q-gone","version":null}]}"#
        );
    }

    #[test]
    fn download_catalog_fails_on_a_401() {
        let stub = Stub::start(|_| Reply::json(401, r#"{"message":"Unauthenticated."}"#));
        let api = device_api(&stub.base_url, true);
        let (mut pacer, _) = pacer();
        assert!(download_catalog(&api.state, &mut pacer).is_err());
        let seen = stub.requests();
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert!(seen.iter().all(|request| request.target != "/oauth/token"));
    }

    #[test]
    fn download_offline_refuses_online() {
        let stub = catalog_stub();
        let api = device_api(&stub.base_url, false);
        let (mut pacer, _) = pacer();
        let mut writer = Recording::default();
        assert_eq!(
            download_offline(&api.state, &mut pacer, &mut writer, 7),
            Err(ONLY_OFFLINE.to_string())
        );
        assert!(stub.requests().is_empty());
        assert!(writer.0.is_empty());
    }

    /// Answers `ClearAllDetected` with three matches.
    struct Clearing(u32);

    impl Writer for Clearing {
        fn edit(&mut self, edit: ArchetypeEdit) -> Result<ArchetypeEditOutcome, String> {
            assert!(matches!(edit, ArchetypeEdit::ClearAllDetected));
            self.0 += 1;
            Ok(ArchetypeEditOutcome::Done {
                matches: vec![1, 2, 3],
                decks: Vec::new(),
            })
        }
    }

    #[test]
    fn redetection_queues_every_detected_match_offline_only() {
        let online = device_api("http://127.0.0.1:1", false);
        let mut writer = Clearing(0);
        assert_eq!(
            redetect_offline(&online.state, &mut writer),
            Err(REDETECT_ONLY_OFFLINE.to_string())
        );
        assert_eq!(writer.0, 0);
        let offline = device_api("http://127.0.0.1:1", true);
        assert_eq!(redetect_offline(&offline.state, &mut writer), Ok(3));
        assert_eq!(writer.0, 1);
    }

    /// Goes online (as the switch does) after its first edit.
    struct GoesOnline<'a>(&'a ApiState, u32);

    impl Writer for GoesOnline<'_> {
        fn edit(&mut self, _: ArchetypeEdit) -> Result<ArchetypeEditOutcome, String> {
            self.1 += 1;
            self.0.set_offline_mode(false, None);
            Ok(ArchetypeEditOutcome::OfflineHeld(OfflineHeld::default()))
        }
    }

    /// Review I1: going online while a download is still writing stops it,
    /// so no Custom row is written after the switch released the others.
    #[test]
    fn going_online_mid_download_stops_the_writes() {
        let api = device_api("http://127.0.0.1:1", true);
        let definitions: Vec<archetypes::Definition> = (0..DEFINITIONS_PER_EDIT + 3)
            .map(|n| archetypes::Definition {
                uuid: format!("q-{n}"),
                ..archetypes::Definition::default()
            })
            .collect();
        let mut writer = GoesOnline(&api.state, 0);
        assert_eq!(
            hold_in_slices(&api.state, &mut writer, definitions, 7),
            Err(WENT_ONLINE.to_string())
        );
        assert_eq!(writer.1, 1, "the second slice is never written");
    }

    /// Records `ReleaseOffline` edits.
    #[derive(Default)]
    struct Releasing(u32);

    impl Writer for Releasing {
        fn edit(&mut self, edit: ArchetypeEdit) -> Result<ArchetypeEditOutcome, String> {
            assert!(matches!(edit, ArchetypeEdit::ReleaseOffline { .. }));
            self.0 += 1;
            Ok(ArchetypeEditOutcome::Done {
                matches: Vec::new(),
                decks: Vec::new(),
            })
        }
    }

    /// PR #517 review: rows held offline while the app is online (a
    /// skipped or failed flip-back) are released at start or the next gate
    /// change; offline, or with none held, nothing is written.
    #[test]
    fn reconcile_releases_rows_left_held_while_online() {
        let online = device_api("http://127.0.0.1:1", false);
        let mut writer = Releasing::default();
        assert_eq!(
            reconcile_offline(&online.state, true, &mut writer, 7),
            Ok(true)
        );
        assert_eq!(writer.0, 1);
        assert_eq!(
            reconcile_offline(&online.state, false, &mut writer, 7),
            Ok(false)
        );
        let offline = device_api("http://127.0.0.1:1", true);
        assert_eq!(
            reconcile_offline(&offline.state, true, &mut writer, 7),
            Ok(false)
        );
        assert_eq!(writer.0, 1, "nothing more written");
    }

    #[test]
    fn download_offline_writes_in_slices_and_sums_them() {
        let definitions: Vec<archetypes::Definition> = (0..DEFINITIONS_PER_EDIT + 3)
            .map(|n| archetypes::Definition {
                uuid: format!("q-{n}"),
                ..archetypes::Definition::default()
            })
            .collect();
        let api = device_api("http://127.0.0.1:1", true);
        let mut writer = Recording::default();
        let held = hold_in_slices(&api.state, &mut writer, definitions, 7).expect("held");
        assert_eq!(writer.0, [DEFINITIONS_PER_EDIT, 3]);
        assert_eq!(
            held,
            OfflineHeld {
                added: u32::try_from(DEFINITIONS_PER_EDIT + 3).expect("small"),
                converted: 0,
                skipped: 0,
            }
        );
    }
}
