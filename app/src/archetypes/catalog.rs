//! The server's archetypes in the picker (catalog picker addendum, #489).
//! v1 holds only the archetypes it has met, so a picker reading held rows
//! alone cannot offer one never met. The picker also lists the server's
//! active archetypes of the format (`GET /api/archetypes`, fetched on the
//! `archetypes` worker and kept an hour); picking one it does not hold
//! writes the overlay's stub (a downloaded row known only by uuid), which
//! the worker fills with its definition, and answers the new id. Nothing
//! else of the list is stored.

use mymtgo_api::CatalogArchetype;
use serde::Serialize;
use store::archetype_edit::ArchetypeEdit;
use store::{Connection, StoreError};
use tauri::{AppHandle, Manager};

use crate::api::session::ApiState;
use crate::archetypes::cache::{ArchetypeCaches, Cached};
use crate::archetypes::definitions::Pacer;
use crate::archetypes::worker::{ArchetypeSignal, ArchetypesWorker, STOPPED};
use crate::bridge::lock;
use crate::commands::{AppState, CommandError, Startup, off_async_workers, ready, storage};

/// The refusal for a pick without a constructed format.
pub const NO_FORMAT: &str = "The archetype needs a constructed format.";

/// One server archetype v1 does not hold, as the picker offers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ArchetypeCatalogOption {
    pub uuid: String,
    pub name: String,
    /// The archetypes format key (`modern`).
    pub format: String,
    /// Colour identity letters in WUBRG order.
    pub colors: Vec<String>,
    /// As the server sends it (`U,B`), passed back when picked.
    pub color_identity: Option<String>,
}

/// The server's archetypes of a format key, paced like every archetype
/// request. Offline it fails before any request.
pub fn fetch_paced_catalog(
    api: &ApiState,
    pacer: &mut Pacer,
    format: &str,
) -> Result<Vec<CatalogArchetype>, String> {
    if api.offline_mode().0 {
        return Err("offline".to_string());
    }
    pacer.before_request();
    let answer = crate::api::reference::fetch_catalog(api, format, pacer.now_ms());
    pacer.after_request();
    answer.map_err(|error| error.to_string())
}

/// The catalog entries the held picker list does not show (not held, or
/// held retired), by name, then uuid. The shown uuids are read once.
pub fn unheld_options(
    conn: &Connection,
    format: &str,
    catalog: &[CatalogArchetype],
) -> Result<Vec<ArchetypeCatalogOption>, StoreError> {
    let shown = store::archetypes::shown_uuids(conn)?;
    let mut options = Vec::new();
    for entry in catalog {
        if shown.contains(&entry.uuid) {
            continue;
        }
        options.push(ArchetypeCatalogOption {
            uuid: entry.uuid.clone(),
            name: entry.name.clone(),
            format: format.to_string(),
            colors: crate::summary::archetype_colors(entry.color_identity.as_deref()),
            color_identity: entry.color_identity.clone(),
        });
    }
    options.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.uuid.cmp(&b.uuid)));
    Ok(options)
}

/// The held row of a picked server archetype: its row when v1 holds the
/// uuid already (a retired one reinstated, a merged one followed one hop),
/// else a new stub. Answers the id and whether a stub was written (the
/// worker then downloads its definition).
pub fn hold(
    state: &AppState,
    uuid: &str,
    name: &str,
    format: &str,
    color_identity: Option<String>,
    now_ms: i64,
) -> Result<(i64, bool), CommandError> {
    let held = store::archetypes::held_uuid(&lock(&state.reader), uuid).map_err(storage)?;
    let (id, stubbed) = match held {
        Some(row) => {
            if row.retired {
                crate::matches::archetype_editor(state)?
                    .edit(ArchetypeEdit::Reinstate {
                        uuid: uuid.to_string(),
                        now_ms,
                    })
                    .map_err(CommandError::Tracker)?;
            }
            (row.id, false)
        }
        None => (
            stub(state, uuid, name, format, color_identity, now_ms)?,
            true,
        ),
    };
    let parent = store::archetypes::merged_into(&lock(&state.reader), id).map_err(storage)?;
    Ok((archetypes::merge_hop(id, |_| parent), stubbed))
}

/// A new stub for an unmet uuid (the overlay's `EnsureStub`); its id.
fn stub(
    state: &AppState,
    uuid: &str,
    name: &str,
    format: &str,
    color_identity: Option<String>,
    now_ms: i64,
) -> Result<i64, CommandError> {
    // The picker sends the key (`modern`); `format_key` keeps a key as it is.
    let key = archetypes::format_key(format)
        .ok_or_else(|| CommandError::Refused(NO_FORMAT.to_string()))?;
    crate::matches::archetype_editor(state)?
        .edit(ArchetypeEdit::EnsureStub {
            uuid: uuid.to_string(),
            name: name.to_string(),
            format: key,
            color_identity,
            now_ms,
        })
        .map_err(CommandError::Tracker)?;
    store::archetypes::held_uuid(&lock(&state.reader), uuid)
        .map_err(storage)?
        .map(|row| row.id)
        .ok_or_else(|| {
            CommandError::Refused(crate::overlay::commands::UNKNOWN_ARCHETYPE.to_string())
        })
}

/// How long the picker waits for the worker's catalog: it may be inside a
/// paced request. The fetch still lands in the cache for the next open.
pub const CATALOG_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Ask the worker for the catalog and wait for its answer, at most
/// `CATALOG_WAIT`.
fn ask_catalog(app: &AppHandle, format: String) -> Result<Vec<CatalogArchetype>, String> {
    let worker = app
        .try_state::<ArchetypesWorker>()
        .ok_or_else(|| "the archetype worker is not running".to_string())?;
    let (reply, answer) = std::sync::mpsc::channel();
    if !worker.signal(ArchetypeSignal::FetchCatalog { format, reply }) {
        return Err(STOPPED.to_string());
    }
    match answer.recv_timeout(CATALOG_WAIT) {
        Ok(answer) => answer,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            Err("the archetype worker is busy".to_string())
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(STOPPED.to_string()),
    }
}

// ---- Commands ----------------------------------------------------------------

/// The server's archetypes of a format (an MTGO code or label) that v1 does
/// not hold, for the picker. Empty offline, without a format, for a
/// limited one, and when the server cannot be asked (the picker then
/// offers the held ones only).
#[tauri::command]
#[specta::specta]
pub async fn archetype_catalog(
    app: AppHandle,
    format: Option<String>,
) -> Result<Vec<ArchetypeCatalogOption>, CommandError> {
    off_async_workers(move || {
        let Some(key) = format.as_deref().and_then(archetypes::format_key) else {
            return Ok(Vec::new());
        };
        let online = app
            .try_state::<ApiState>()
            .is_some_and(|api| !api.offline_mode().0);
        if !online {
            return Ok(Vec::new());
        }
        let cached = match app.try_state::<ArchetypeCaches>() {
            Some(caches) => lock(&caches.catalog).get(&key, crate::wall_clock_ms()),
            None => Cached::Failed,
        };
        let catalog = match cached {
            Cached::Fresh(catalog) => catalog,
            Cached::Failed => return Ok(Vec::new()),
            Cached::Missing | Cached::Pending => match ask_catalog(&app, key.clone()) {
                Ok(catalog) => catalog,
                Err(error) => {
                    tracing::warn!(%error, "the archetype catalog could not be fetched");
                    return Ok(Vec::new());
                }
            },
        };
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        unheld_options(&lock(&state.reader), &key, &catalog).map_err(storage)
    })
    .await
}

/// Hold a server archetype picked in the picker and answer its id, which
/// the caller then assigns as any other. A new stub's definition is
/// downloaded by the archetypes worker in the background.
#[tauri::command]
#[specta::specta]
pub async fn hold_archetype(
    app: AppHandle,
    uuid: String,
    name: String,
    format: String,
    color_identity: Option<String>,
) -> Result<u32, CommandError> {
    off_async_workers(move || {
        let (id, stubbed) = {
            let startup = app.state::<Startup>();
            let state = ready(&startup)?;
            hold(
                state,
                &uuid,
                &name,
                &format,
                color_identity,
                crate::wall_clock_ms(),
            )?
        };
        if stubbed {
            crate::archetypes::worker::signal(&app, ArchetypeSignal::FetchDefinitions);
        }
        u32::try_from(id).map_err(|_| {
            CommandError::Refused(crate::overlay::commands::UNKNOWN_ARCHETYPE.to_string())
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use fixture_tools::TestDir;

    use super::*;
    use crate::commands::stop_tracker;
    use crate::commands::tests::ready_state;

    const NOW: i64 = 1_800_000_000_000;

    fn entry(uuid: &str, name: &str, colors: Option<&str>) -> CatalogArchetype {
        CatalogArchetype {
            uuid: uuid.to_string(),
            name: name.to_string(),
            color_identity: colors.map(str::to_string),
        }
    }

    fn held(writer: &Connection, id: i64, uuid: &str, name: &str) {
        writer
            .execute_batch(&format!(
                "INSERT INTO archetypes (id, uuid, name, format, source, created_at, updated_at)
                 VALUES ({id}, '{uuid}', '{name}', 'modern', 'downloaded', 1, 1);"
            ))
            .expect("archetype");
    }

    // #489: the held Frog is not offered twice; the unmet ones are, by name.
    #[test]
    fn the_catalog_offers_only_what_v1_does_not_hold() {
        let dir = TestDir::new("catalog-unheld");
        let (state, _events) = ready_state(&dir);
        let writer = store::open_writer(&state.db_path).expect("writer");
        held(&writer, 7, "q-frog", "Qdimir Frog");
        let catalog = [
            entry("q-reanimator", "Qgrixis Reanimator", Some("U,B,R")),
            entry("q-frog", "Qdimir Frog", Some("U,B")),
            entry("q-oculus", "Qdimir Oculus", Some("B,U")),
        ];
        let options = unheld_options(&lock(&state.reader), "modern", &catalog).expect("options");
        let names: Vec<&str> = options.iter().map(|option| option.name.as_str()).collect();
        assert_eq!(names, ["Qdimir Oculus", "Qgrixis Reanimator"]);
        assert_eq!(options[0].colors, ["U", "B"], "WUBRG order");
        assert_eq!(options[0].format, "modern");
        assert_eq!(options[0].color_identity.as_deref(), Some("B,U"));
        stop_tracker(&state);
    }

    #[test]
    fn holding_an_unmet_archetype_writes_a_stub_and_answers_its_id() {
        let dir = TestDir::new("catalog-hold");
        let (state, _events) = ready_state(&dir);
        let writer = store::open_writer(&state.db_path).expect("writer");
        let (id, stubbed) = hold(
            &state,
            "q-oculus",
            "Qdimir Oculus",
            "CMODERN",
            Some("U,B".to_string()),
            NOW,
        )
        .expect("hold");
        assert!(stubbed);
        let (uuid, name, format, source): (String, String, String, String) = writer
            .query_row(
                "SELECT uuid, name, format, source FROM archetypes WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("stub");
        assert_eq!(
            (
                uuid.as_str(),
                name.as_str(),
                format.as_str(),
                source.as_str()
            ),
            ("q-oculus", "Qdimir Oculus", "modern", "downloaded")
        );
        assert!(
            store::archetypes::undefined_uuids(&writer)
                .expect("read")
                .contains(&"q-oculus".to_string()),
            "waiting for the worker's definition download"
        );
        stop_tracker(&state);
    }

    // Review: the picker sends the key (`modern`), not a code.
    #[test]
    fn holding_takes_the_format_key_the_picker_sends() {
        let dir = TestDir::new("catalog-hold-key");
        let (state, _events) = ready_state(&dir);
        let (id, stubbed) =
            hold(&state, "q-oculus", "Qdimir Oculus", "modern", None, NOW).expect("hold");
        assert!(stubbed);
        let writer = store::open_writer(&state.db_path).expect("writer");
        let format: String = writer
            .query_row("SELECT format FROM archetypes WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .expect("stub");
        assert_eq!(format, "modern");
        stop_tracker(&state);
    }

    // Review: a held row the picker hides (retired) is offered again and
    // comes back when picked.
    #[test]
    fn a_retired_held_archetype_is_offered_and_reinstated() {
        let dir = TestDir::new("catalog-retired");
        let (state, _events) = ready_state(&dir);
        let writer = store::open_writer(&state.db_path).expect("writer");
        held(&writer, 7, "q-frog", "Qdimir Frog");
        writer
            .execute_batch("UPDATE archetypes SET retired_at = 5 WHERE id = 7")
            .expect("retire");
        let catalog = [entry("q-frog", "Qdimir Frog", None)];
        let options = unheld_options(&lock(&state.reader), "modern", &catalog).expect("options");
        assert_eq!(options.len(), 1);
        assert_eq!(
            hold(&state, "q-frog", "Qdimir Frog", "modern", None, NOW),
            Ok((7, false))
        );
        let retired: bool = writer
            .query_row(
                "SELECT retired_at IS NOT NULL FROM archetypes WHERE id = 7",
                [],
                |row| row.get(0),
            )
            .expect("row");
        assert!(!retired);
        stop_tracker(&state);
    }

    // Review: a merged held row answers its parent, one hop.
    #[test]
    fn holding_a_merged_archetype_answers_its_parent() {
        let dir = TestDir::new("catalog-merged");
        let (state, _events) = ready_state(&dir);
        let writer = store::open_writer(&state.db_path).expect("writer");
        held(&writer, 7, "q-frog", "Qdimir Frog");
        held(&writer, 8, "q-frog-old", "Qold Frog");
        writer
            .execute_batch("UPDATE archetypes SET merged_into_id = 7 WHERE id = 8")
            .expect("merge");
        assert_eq!(
            hold(&state, "q-frog-old", "Qold Frog", "modern", None, NOW),
            Ok((7, false))
        );
        stop_tracker(&state);
    }

    #[test]
    fn holding_a_held_archetype_answers_its_row() {
        let dir = TestDir::new("catalog-hold-held");
        let (state, _events) = ready_state(&dir);
        let writer = store::open_writer(&state.db_path).expect("writer");
        held(&writer, 7, "q-frog", "Qdimir Frog");
        assert_eq!(
            hold(&state, "q-frog", "Qdimir Frog", "CMODERN", None, NOW),
            Ok((7, false))
        );
        stop_tracker(&state);
    }

    #[test]
    fn holding_needs_a_constructed_format() {
        let dir = TestDir::new("catalog-hold-limited");
        let (state, _events) = ready_state(&dir);
        assert_eq!(
            hold(&state, "q-x", "Qx", "DHOBHOBHOB", None, NOW),
            Err(CommandError::Refused(NO_FORMAT.to_string()))
        );
        stop_tracker(&state);
    }
}
