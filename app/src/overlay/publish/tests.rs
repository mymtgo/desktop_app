use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use fixture_tools::TestDir;
use overlay::{LeagueStatus, OverlayArtwork};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use store::params;

use super::*;
use crate::api::secrets::{ACCESS_TOKEN, REFRESH_TOKEN};
use crate::api::session::testing::{TestApi, test_api};
use crate::api::stub::{Recorded, Reply, Stub};
use crate::bridge::lock;
use crate::overlay::MAIN_WINDOW;
use crate::overlay::net::NetQueue;
use crate::overlay::windows::testing::FakeWindows;
use crate::overlay::worker::Worker;
use crate::settings::{
    BackgroundRemote, Settings, overlay_background_file_row, overlay_background_remote,
    overlay_background_remote_row, overlay_last_published_at, overlay_last_published_at_row,
    overlay_publish_error, overlay_publish_error_row,
};

/// 2026-07-01T10:00:00Z.
const NOW: i64 = 1_782_900_000_000;
const DAY: i64 = 24 * 3_600_000;
const KARN: &str = "https://cards.scryfall.io/art_crop/karn.jpg";
const BACKGROUND_URL: &str = "https://cdn.example/bg.png";

/// The 0.x test's world: the streamer `Qstream` (login id 4242) logged in,
/// a deck with a Scryfall and a local cover art crop, played yesterday.
struct World {
    dir: TestDir,
    conn: Connection,
}

impl World {
    fn new(label: &str) -> World {
        let dir = TestDir::new(label);
        let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        conn.execute_batch(
            "INSERT INTO accounts (id, username, login_id, first_seen_at)
                 VALUES (1, 'Qstream', 4242, 0);
             INSERT INTO cards (catalog_id, oracle_id, name, type, art_crop, local_art_crop)
                 VALUES (5001, 'oracle-karn', 'Qkarn', 'Planeswalker',
                     'https://cards.scryfall.io/art_crop/karn.jpg', '5001_art.jpg');
             INSERT INTO decks (id, mtgo_id, name, format, color_identity, cover_catalog_id,
                     created_at, updated_at, source)
                 VALUES (1, 'q-deck-1', 'Mono Green Tron', 'CMODERN', 'G', 5001, 1, 1, 'file');
             INSERT INTO deck_versions (id, deck_id, signature, modified_at)
                 VALUES (1, 1, 'SIG', 1);",
        )
        .expect("world");
        conn.execute(
            "INSERT INTO matches (id, token, account_id, source, format, state, outcome,
                 started_at, last_activity_at, revision, derived_version, deck_version_id)
             VALUES (1, 'q-match-1', 1, 'log', 'CMODERN', 'complete', 'win', ?1, ?1, 1, 1, 1)",
            [NOW - DAY],
        )
        .expect("match");
        let world = World { dir, conn };
        world.login("Qstream", 4242);
        world.settings(Settings {
            overlay_publish: true,
            ..Settings::default()
        });
        world
    }

    /// `new` with an active league run on the card: the heartbeat beats
    /// only while the card shows a run (#505).
    fn live(label: &str) -> World {
        let world = World::new(label);
        world
            .conn
            .execute(
                "INSERT INTO leagues (id, run_key, account_id, source, kind, state, token, name,
                     format, deck_id, started_at, created_at)
                 VALUES (1, 'run-1', 1, 'log', 'constructed', 'active', 'q-league-1',
                     'Modern League 14-09-2026 11:01am', 'CMODERN', 1, ?1, 1)",
                [NOW - 3_600_000],
            )
            .expect("run");
        world
            .conn
            .execute_batch(
                "UPDATE matches SET league_id = 1, league_token = 'q-league-1' WHERE id = 1",
            )
            .expect("in the run");
        world
    }

    /// A newer `login` event naming `username`.
    fn login(&self, username: &str, login_id: i64) {
        let file: i64 = self
            .conn
            .query_row("SELECT COUNT(*) + 1 FROM log_files", [], |row| row.get(0))
            .expect("count");
        self.conn
            .execute(
                "INSERT INTO log_files (id, path, identity_hash, head_hash, head_len, day_anchor,
                     timezone, clock_date, first_seen_at, last_seen_at)
                 VALUES (?1, 'Q/' || ?1 || '.log', 'id', 'head', 0, '2026-07-01', 'UTC',
                     '2026-07-01', ?2, ?2)",
                params![file, NOW - DAY + file],
            )
            .expect("file");
        let line =
            format!("12:00:00 [INF] (Login|MtGO Login Success) Username: {username} ({login_id})");
        self.conn
            .execute(
                "INSERT INTO log_events (log_file_id, byte_offset, kind, logged_at, raw)
                 VALUES (?1, 0, 'login', ?2, ?3)",
                params![file, NOW - DAY + file, line.into_bytes()],
            )
            .expect("login");
    }

    fn settings(&self, settings: Settings) {
        self.save(settings.to_rows()).expect("settings");
    }

    fn save(&self, rows: Vec<(String, String)>) -> Result<(), String> {
        for (key, value) in rows {
            self.conn
                .execute(
                    "INSERT INTO settings (key, value) VALUES (?1, ?2)
                     ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                    params![key, value],
                )
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn rows(&self) -> Vec<(String, String)> {
        store::read::settings(&self.conn).expect("rows")
    }

    fn folder(&self) -> PathBuf {
        self.dir.join("overlay")
    }

    /// A custom background file `name` with `bytes`, chosen.
    fn background(&self, name: &str, bytes: &[u8]) {
        std::fs::create_dir_all(self.folder()).expect("folder");
        std::fs::write(self.folder().join(name), bytes).expect("file");
        self.save(vec![overlay_background_file_row(Some(name))])
            .expect("row");
    }

    fn artwork(&self, artwork: OverlayArtwork) {
        let mut settings = Settings::from_rows(&self.rows());
        settings.overlay_artwork = Some(artwork);
        self.settings(settings);
    }

    fn run(&self, api: &TestApi, request: NetRequest) -> Option<OverlayMsg> {
        let save = |rows: Vec<(String, String)>| self.save(rows);
        let folder = self.folder();
        let env = PublishEnv {
            api: &api.state,
            conn: &self.conn,
            overlay_folder: Some(&folder),
            save: &save,
            known: None,
        };
        run_request(request, &env, NOW)
    }

    fn publish(&self, api: &TestApi) -> Option<OverlayPublishChanged> {
        match self.run(api, NetRequest::Publish) {
            Some(OverlayMsg::PublishAnswered(status)) => Some(status),
            None => None,
            Some(other) => panic!("unexpected answer {other:?}"),
        }
    }
}

/// A device signed in (when `linked`), online or `offline`.
fn api_for(base_url: &str, linked: bool, offline: bool) -> TestApi {
    let key_valid = (NOW + DAY).to_string();
    let token_valid = (NOW + 30 * DAY).to_string();
    let offline_row = offline.to_string();
    let rows = [
        ("api_device_id", r#""qdevice""#),
        ("api_key_expires_at", key_valid.as_str()),
        ("api_token_expires_at", token_valid.as_str()),
        ("offline_mode", offline_row.as_str()),
    ];
    let mut secrets = vec![("api_key", "qdevice-key")];
    if linked {
        secrets.push((ACCESS_TOKEN, "qaccess"));
        secrets.push((REFRESH_TOKEN, "qrefresh"));
    }
    test_api(base_url, &rows, &secrets)
}

/// The API: the background upload answers `upload`, everything else
/// `status` with an empty body.
fn stub(status: u16, upload: u16) -> Stub {
    Stub::start(move |request: &Recorded| {
        if request.target == "/api/overlay/background" && request.method == "POST" {
            if upload == 201 {
                return Reply::json(201, &format!(r#"{{"url":"{BACKGROUND_URL}"}}"#));
            }
            return Reply::json(upload, r#"{"message":"failed"}"#);
        }
        Reply::json(status, "")
    })
}

fn puts(stub: &Stub) -> Vec<Value> {
    stub.requests()
        .iter()
        .filter(|request| request.method == "PUT" && request.target == "/api/overlay")
        .map(|request| serde_json::from_slice(&request.body).expect("json"))
        .collect()
}

fn uploads(stub: &Stub) -> usize {
    stub.requests()
        .iter()
        .filter(|request| request.method == "POST" && request.target == "/api/overlay/background")
        .count()
}

/// A base URL nothing listens on.
fn unreachable_base_url() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("address").port();
    drop(listener);
    format!("http://127.0.0.1:{port}")
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

// ---- 0.x PublishOverlayStateTest (eleven) ----

#[test]
fn sends_the_remote_deck_art_never_the_local_copy() {
    let world = World::new("publish-deck-art");
    let stub = stub(204, 201);
    let api = api_for(&stub.base_url, true, false);
    let status = world.publish(&api).expect("published");
    let sent = puts(&stub);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["login_id"], 4242);
    assert_eq!(
        sent[0]["state"]["art"],
        json!({"url": KARN, "thumbnail": false})
    );
    assert_eq!(sent[0]["state"]["deck"]["name"], "Mono Green Tron");
    assert!(!sent[0].to_string().contains("5001_art.jpg"), "{}", sent[0]);
    assert_eq!(overlay_last_published_at(&world.rows()), Some(NOW));
    assert_eq!(
        status,
        OverlayPublishChanged {
            last_published_at_ms: Some(NOW),
            error: None
        }
    );
}

/// F4: a save that fails does not push the stale stored state to the card.
#[test]
fn a_failed_save_still_answers_with_the_intended_state() {
    let world = World::new("publish-save-fails");
    let stub = stub(204, 201);
    let api = api_for(&stub.base_url, true, false);
    let save = |_: Vec<(String, String)>| Err("the tracker is busy".to_string());
    let folder = world.folder();
    let env = PublishEnv {
        api: &api.state,
        conn: &world.conn,
        overlay_folder: Some(&folder),
        save: &save,
        known: None,
    };
    let Some(OverlayMsg::PublishAnswered(status)) = run_request(NetRequest::Publish, &env, NOW)
    else {
        panic!("an answer");
    };
    assert_eq!(
        status,
        OverlayPublishChanged {
            last_published_at_ms: Some(NOW),
            error: None
        }
    );
    assert_eq!(overlay_last_published_at(&world.rows()), None, "not stored");
}

#[test]
fn sends_no_art_in_none_mode() {
    let world = World::new("publish-no-art");
    world.artwork(OverlayArtwork::NoArt);
    let stub = stub(204, 201);
    let api = api_for(&stub.base_url, true, false);
    world.publish(&api);
    assert_eq!(puts(&stub)[0]["state"]["art"], Value::Null);
}

#[test]
fn uploads_a_changed_custom_background_once_and_reuses_it_after() {
    let world = World::new("publish-upload-once");
    world.artwork(OverlayArtwork::Custom);
    world.background("background-1.png", b"png-bytes");
    let stub = stub(204, 201);
    let api = api_for(&stub.base_url, true, false);
    world.publish(&api);
    world.publish(&api);
    assert_eq!(uploads(&stub), 1, "uploaded once");
    let upload = stub
        .requests()
        .into_iter()
        .find(|request| request.method == "POST")
        .expect("the upload");
    let body = String::from_utf8_lossy(&upload.body).into_owned();
    assert!(
        body.contains("name=\"image\"; filename=\"background-1.png\""),
        "{body}"
    );
    assert!(body.contains("png-bytes"), "{body}");
    let sent = puts(&stub);
    assert_eq!(sent.len(), 2);
    for push in &sent {
        assert_eq!(
            push["state"]["art"],
            json!({"url": BACKGROUND_URL, "thumbnail": false})
        );
    }
    assert_eq!(
        overlay_background_remote(&world.rows()),
        Some(BackgroundRemote {
            url: BACKGROUND_URL.to_string(),
            sha256: sha256_hex(b"png-bytes"),
        })
    );
}

#[test]
fn a_replaced_background_is_uploaded_again() {
    let world = World::new("publish-upload-again");
    world.artwork(OverlayArtwork::Custom);
    world.background("background-1.png", b"png-bytes");
    let stub = stub(204, 201);
    let api = api_for(&stub.base_url, true, false);
    world.publish(&api);
    world.background("background-2.png", b"other-bytes");
    world.publish(&api);
    assert_eq!(uploads(&stub), 2);
    assert_eq!(
        overlay_background_remote(&world.rows()).map(|remote| remote.sha256),
        Some(sha256_hex(b"other-bytes"))
    );
}

#[test]
fn falls_back_to_deck_art_when_the_custom_upload_fails() {
    let world = World::new("publish-upload-fails");
    world.artwork(OverlayArtwork::Custom);
    world.background("background-1.png", b"png-bytes");
    let stub = stub(204, 500);
    let api = api_for(&stub.base_url, true, false);
    world.publish(&api);
    let sent = puts(&stub);
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0]["state"]["art"],
        json!({"url": KARN, "thumbnail": false})
    );
    assert_eq!(overlay_background_remote(&world.rows()), None);
}

#[test]
fn falls_back_to_deck_art_when_custom_is_chosen_but_no_file_exists() {
    let world = World::new("publish-no-file");
    world.artwork(OverlayArtwork::Custom);
    world
        .save(vec![overlay_background_file_row(Some("background-9.png"))])
        .expect("row");
    let stub = stub(204, 201);
    let api = api_for(&stub.base_url, true, false);
    world.publish(&api);
    assert_eq!(uploads(&stub), 0);
    let sent = puts(&stub);
    assert_eq!(
        sent[0]["state"]["art"],
        json!({"url": KARN, "thumbnail": false})
    );
    let text = sent[0].to_string();
    assert!(!text.contains("127.0.0.1"), "{text}");
    assert!(!text.contains("localhost"), "{text}");
}

#[test]
fn publishes_with_the_league_window_off() {
    let world = World::new("publish-window-off");
    let mut settings = Settings::from_rows(&world.rows());
    settings.league_window = false;
    world.settings(settings);
    let stub = stub(204, 201);
    let api = api_for(&stub.base_url, true, false);
    world.publish(&api);
    assert_eq!(puts(&stub).len(), 1);
}

#[test]
fn does_nothing_when_publishing_is_off_unlinked_or_the_username_is_unknown() {
    // Publishing off.
    let world = World::new("publish-nothing-off");
    world.settings(Settings::default());
    let stub_off = stub(204, 201);
    let api = api_for(&stub_off.base_url, true, false);
    assert_eq!(world.publish(&api), None);
    assert_eq!(stub_off.connections(), 0);
    // Signed out.
    let world = World::new("publish-nothing-unlinked");
    let stub_unlinked = stub(204, 201);
    let api = api_for(&stub_unlinked.base_url, false, false);
    assert_eq!(world.publish(&api), None);
    assert_eq!(stub_unlinked.connections(), 0);
    // The current MTGO account has no login id.
    let world = World::new("publish-nothing-unknown");
    world.login("Qnobody", 9999);
    let stub_unknown = stub(204, 201);
    let api = api_for(&stub_unknown.base_url, true, false);
    world.publish(&api);
    assert_eq!(stub_unknown.connections(), 0);
}

#[test]
fn swallows_a_failed_push_and_keeps_the_last_published_time() {
    let world = World::new("publish-swallows");
    world
        .save(vec![overlay_last_published_at_row(Some(NOW - 60_000))])
        .expect("row");
    let stub = stub(503, 201);
    let api = api_for(&stub.base_url, true, false);
    let status = world.publish(&api).expect("tried");
    assert_eq!(overlay_last_published_at(&world.rows()), Some(NOW - 60_000));
    assert_eq!(status.last_published_at_ms, Some(NOW - 60_000));
}

#[test]
fn records_why_a_push_failed_so_settings_can_say() {
    for (status, reason) in [
        (409, PublishError::NotClaimed),
        (422, PublishError::Rejected),
        (503, PublishError::Unreachable),
        (500, PublishError::Unreachable),
    ] {
        let world = World::new(&format!("publish-failed-{status}"));
        let stub = stub(status, 201);
        let api = api_for(&stub.base_url, true, false);
        let answered = world.publish(&api).expect("tried");
        assert_eq!(
            overlay_publish_error(&world.rows()),
            Some(reason),
            "{status}"
        );
        assert_eq!(answered.error, Some(reason), "{status}");
    }
    // No answer at all.
    let world = World::new("publish-failed-no-answer");
    let api = api_for(&unreachable_base_url(), true, false);
    world.publish(&api);
    assert_eq!(
        overlay_publish_error(&world.rows()),
        Some(PublishError::Unreachable)
    );
}

#[test]
fn records_a_missing_login_id() {
    let world = World::new("publish-no-login-id");
    world.login("Qnobody", 9999);
    let stub = stub(204, 201);
    let api = api_for(&stub.base_url, true, false);
    let status = world.publish(&api).expect("tried");
    assert_eq!(
        overlay_publish_error(&world.rows()),
        Some(PublishError::NoLoginId)
    );
    assert_eq!(status.error, Some(PublishError::NoLoginId));
    assert_eq!(stub.connections(), 0);
}

#[test]
fn clears_the_failure_reason_after_a_successful_push() {
    let world = World::new("publish-clears-error");
    world
        .save(vec![overlay_publish_error_row(Some(
            PublishError::Unreachable,
        ))])
        .expect("row");
    let stub = stub(204, 201);
    let api = api_for(&stub.base_url, true, false);
    world.publish(&api);
    assert_eq!(overlay_publish_error(&world.rows()), None);
}

#[test]
fn nothing_is_published_offline() {
    let world = World::new("publish-offline");
    world.artwork(OverlayArtwork::Custom);
    world.background("background-1.png", b"png-bytes");
    let stub = stub(204, 201);
    let api = api_for(&stub.base_url, true, true);
    assert_eq!(world.publish(&api), None);
    assert_eq!(world.run(&api, NetRequest::ForgetBackground), None);
    world.run(&api, NetRequest::Clear);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(stub.connections(), 0);
}

// ---- 0.x OverlayPublishControllerTest ----

#[test]
fn turning_publishing_off_clears_the_hosted_overlay() {
    let world = World::new("publish-clear");
    world
        .save(vec![
            overlay_last_published_at_row(Some(NOW - 60_000)),
            overlay_publish_error_row(Some(PublishError::Rejected)),
        ])
        .expect("rows");
    world.settings(Settings::default());
    let stub = stub(404, 201);
    let api = api_for(&stub.base_url, true, false);
    let answer = world.run(&api, NetRequest::Clear);
    assert_eq!(
        answer,
        Some(OverlayMsg::PublishAnswered(OverlayPublishChanged {
            last_published_at_ms: None,
            error: None
        }))
    );
    let rows = world.rows();
    assert_eq!(overlay_last_published_at(&rows), None);
    assert_eq!(overlay_publish_error(&rows), None);
    let sent = stub.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].method, "DELETE");
    assert_eq!(sent[0].target, "/api/overlay/4242");
}

#[test]
fn turning_publishing_off_survives_an_unreachable_api() {
    let world = World::new("publish-clear-unreachable");
    world
        .save(vec![overlay_last_published_at_row(Some(NOW - 60_000))])
        .expect("row");
    world.settings(Settings::default());
    let api = api_for(&unreachable_base_url(), true, false);
    let answer = world.run(&api, NetRequest::Clear);
    assert!(matches!(answer, Some(OverlayMsg::PublishAnswered(_))));
    assert_eq!(overlay_last_published_at(&world.rows()), None);
}

#[test]
fn deleting_the_background_forgets_the_uploaded_copy() {
    let world = World::new("publish-forget");
    let remote = BackgroundRemote {
        url: BACKGROUND_URL.to_string(),
        sha256: "abc".to_string(),
    };
    world
        .save(vec![overlay_background_remote_row(Some(&remote))])
        .expect("row");
    let stub = stub(204, 201);
    let api = api_for(&stub.base_url, true, false);
    world.run(&api, NetRequest::ForgetBackground);
    let sent = stub.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].method, "DELETE");
    assert_eq!(sent[0].target, "/api/overlay/background");
    assert_eq!(overlay_background_remote(&world.rows()), None);
}

#[test]
fn a_chosen_background_is_not_forgotten() {
    let world = World::new("publish-forget-kept");
    let remote = BackgroundRemote {
        url: BACKGROUND_URL.to_string(),
        sha256: "abc".to_string(),
    };
    world
        .save(vec![overlay_background_remote_row(Some(&remote))])
        .expect("row");
    world.background("background-2.png", b"new-bytes");
    let stub = stub(204, 201);
    let api = api_for(&stub.base_url, true, false);
    world.run(&api, NetRequest::ForgetBackground);
    assert_eq!(stub.connections(), 0);
    assert_eq!(overlay_background_remote(&world.rows()), Some(remote));
}

#[test]
fn the_settings_card_reads_the_publish_rows() {
    let world = World::new("publish-status");
    assert_eq!(
        publish_status(&world.rows()),
        OverlayPublishChanged {
            last_published_at_ms: None,
            error: None
        }
    );
    world
        .save(vec![
            overlay_last_published_at_row(Some(NOW)),
            overlay_publish_error_row(Some(PublishError::NotClaimed)),
        ])
        .expect("rows");
    assert_eq!(
        publish_status(&world.rows()),
        OverlayPublishChanged {
            last_published_at_ms: Some(NOW),
            error: Some(PublishError::NotClaimed)
        }
    );
}

// ---- 0.x PublishOverlayDispatchTest and the worker's side ----

/// A publish side that records what was queued.
#[derive(Clone, Default)]
struct FakePort {
    allowed: Arc<AtomicBool>,
    submitted: Arc<Mutex<Vec<NetRequest>>>,
}

impl FakePort {
    fn allowed() -> FakePort {
        let port = FakePort::default();
        port.allowed.store(true, Ordering::SeqCst);
        port
    }

    fn taken(&self) -> Vec<NetRequest> {
        std::mem::take(&mut *lock(&self.submitted))
    }
}

impl PublishPort for FakePort {
    fn allowed(&self) -> bool {
        self.allowed.load(Ordering::SeqCst)
    }

    fn submit(&mut self, request: NetRequest) {
        lock(&self.submitted).push(request);
    }
}

fn publishing() -> Settings {
    Settings {
        overlay_publish: true,
        ..Settings::default()
    }
}

fn worker_with(settings: Settings, port: &FakePort) -> Worker {
    Worker::new(settings, false, true).with_publish(Box::new(port.clone()))
}

fn wake(
    worker: &mut Worker,
    conn: &Connection,
    windows: &mut FakeWindows,
    messages: Vec<OverlayMsg>,
    now_ms: i64,
) {
    for message in messages {
        worker.handle(conn, message);
    }
    worker.update(conn, windows, now_ms);
}

/// The world's league card changes: a newer match with the deck.
/// Rename the live run, which the live card shows.
fn change_the_live_card(world: &World, id: i64) {
    world
        .conn
        .execute(
            "UPDATE leagues SET name = 'Qleague ' || ?1 WHERE id = 1",
            [id],
        )
        .expect("rename");
}

fn change_the_card(world: &World, id: i64) {
    world
        .conn
        .execute(
            "INSERT INTO decks (id, mtgo_id, name, format, color_identity, created_at,
                 updated_at, source)
             VALUES (?1, 'q-deck-' || ?1, 'Qdeck ' || ?1, 'CMODERN', 'R', 1, 1, 'file')",
            [id],
        )
        .expect("deck");
    world
        .conn
        .execute(
            "INSERT INTO deck_versions (id, deck_id, signature, modified_at)
             VALUES (?1, ?1, 'SIG', 1)",
            [id],
        )
        .expect("version");
    world
        .conn
        .execute(
            "INSERT INTO matches (id, token, account_id, source, format, state, outcome,
                 started_at, last_activity_at, revision, derived_version, deck_version_id)
             VALUES (?1, 'q-match-' || ?1, 1, 'log', 'CMODERN', 'complete', 'win', ?2, ?2,
                 1, 1, ?1)",
            params![id, NOW - DAY + id],
        )
        .expect("match");
}

#[test]
fn queues_a_publish_on_overlay_changes_while_publishing_is_on_window_or_not() {
    let world = World::new("dispatch-on-change");
    let port = FakePort::allowed();
    let mut worker = worker_with(publishing(), &port);
    let mut windows = FakeWindows::default();
    wake(&mut worker, &world.conn, &mut windows, vec![], NOW);
    assert_eq!(port.taken(), vec![NetRequest::Publish], "at start-up");
    change_the_card(&world, 2);
    wake(
        &mut worker,
        &world.conn,
        &mut windows,
        vec![OverlayMsg::DecksChanged],
        NOW + 1_000,
    );
    assert_eq!(port.taken(), vec![NetRequest::Publish], "the card changed");
    // An unchanged card is not published again before the heartbeat.
    wake(
        &mut worker,
        &world.conn,
        &mut windows,
        vec![OverlayMsg::DecksChanged],
        NOW + 2_000,
    );
    assert_eq!(port.taken(), Vec::<NetRequest>::new());
}

#[test]
fn queues_nothing_while_publishing_is_off() {
    let world = World::new("dispatch-off");
    let port = FakePort::allowed();
    let mut worker = worker_with(Settings::default(), &port);
    let mut windows = FakeWindows::default();
    wake(&mut worker, &world.conn, &mut windows, vec![], NOW);
    change_the_card(&world, 2);
    wake(
        &mut worker,
        &world.conn,
        &mut windows,
        vec![OverlayMsg::DecksChanged],
        NOW + 1_000,
    );
    assert_eq!(port.taken(), Vec::<NetRequest>::new());
    assert_eq!(worker.next_wake(NOW + 1_000), None, "no heartbeat");
}

#[test]
fn a_burst_of_changes_sends_one_publish() {
    let world = World::new("dispatch-burst");
    let port = FakePort::allowed();
    let mut worker = worker_with(publishing(), &port);
    let mut windows = FakeWindows::default();
    wake(&mut worker, &world.conn, &mut windows, vec![], NOW);
    port.taken();
    change_the_card(&world, 2);
    wake(
        &mut worker,
        &world.conn,
        &mut windows,
        vec![
            OverlayMsg::DecksChanged,
            OverlayMsg::LeaguesChanged,
            OverlayMsg::BackgroundChanged,
            OverlayMsg::DecksChanged,
        ],
        NOW + 1_000,
    );
    let publishes = port
        .taken()
        .into_iter()
        .filter(|request| *request == NetRequest::Publish)
        .count();
    assert_eq!(publishes, 1, "one wake, one publish");
}

#[test]
fn a_burst_on_the_network_thread_sends_the_newest_once() {
    // The first push is held by the API until released; four more arrive
    // meanwhile and share one waiting slot.
    let world = Arc::new(Mutex::new(World::new("dispatch-burst-net")));
    let (release, hold) = mpsc::channel::<()>();
    let hold = Mutex::new(hold);
    let first = AtomicBool::new(true);
    let stub = Stub::start(move |_| {
        if first.swap(false, Ordering::SeqCst) {
            let _ = lock(&hold).recv_timeout(Duration::from_secs(5));
        }
        Reply::json(204, "")
    });
    let api = Arc::new(api_for(&stub.base_url, true, false));
    let (answers, answered) = mpsc::channel();
    let queue = NetQueue::start(answers).expect("the thread");
    let job = |world: &Arc<Mutex<World>>, api: &Arc<TestApi>| {
        let world = Arc::clone(world);
        let api = Arc::clone(api);
        NetJob {
            key: NetRequest::Publish.key().to_string(),
            run: Box::new(move || lock(&world).run(&api, NetRequest::Publish)),
        }
    };
    assert!(queue.submit(job(&world, &api)));
    let started = std::time::Instant::now();
    while stub.requests().is_empty() && stub.connections() == 0 {
        assert!(started.elapsed() < Duration::from_secs(5), "the first push");
        std::thread::sleep(Duration::from_millis(1));
    }
    for _ in 0..4 {
        assert!(queue.submit(job(&world, &api)));
    }
    release.send(()).expect("release");
    for _ in 0..2 {
        assert!(matches!(
            answered.recv_timeout(Duration::from_secs(5)),
            Ok(OverlayMsg::PublishAnswered(_))
        ));
    }
    assert!(answered.recv_timeout(Duration::from_millis(200)).is_err());
    queue.stop();
    assert_eq!(puts(&stub).len(), 2, "the held push and the newest");
}

#[test]
fn every_publish_shares_one_waiting_slot() {
    assert_eq!(NetRequest::Publish.key(), NetRequest::Publish.key());
    assert_ne!(NetRequest::Publish.key(), NetRequest::Clear.key());
    assert_ne!(
        NetRequest::Publish.key(),
        NetRequest::ForgetBackground.key()
    );
}

#[test]
fn publishes_run_on_the_dedicated_overlay_net_thread() {
    let (answers, _answered) = mpsc::channel();
    let queue = NetQueue::start(answers).expect("the thread");
    let (seen, names) = mpsc::channel();
    assert!(queue.submit(NetJob {
        key: NetRequest::Publish.key().to_string(),
        run: Box::new(move || {
            let _ = seen.send(std::thread::current().name().map(str::to_string));
            None
        }),
    }));
    assert_eq!(
        names.recv_timeout(Duration::from_secs(5)),
        Ok(Some("overlay-net".to_string()))
    );
    queue.stop();
}

#[test]
fn heartbeats_every_thirty_seconds_only_while_publishing_and_linked() {
    let world = World::live("dispatch-heartbeat");
    let port = FakePort::allowed();
    let mut worker = worker_with(publishing(), &port);
    let mut windows = FakeWindows::default();
    wake(&mut worker, &world.conn, &mut windows, vec![], NOW);
    assert_eq!(port.taken(), vec![NetRequest::Publish]);
    assert_eq!(
        worker.next_wake(NOW),
        Some(Duration::from_millis(HEARTBEAT_MS as u64))
    );
    // Not before the 30 s.
    worker.update(&world.conn, &mut windows, NOW + HEARTBEAT_MS - 1);
    assert_eq!(port.taken(), Vec::<NetRequest>::new());
    worker.update(&world.conn, &mut windows, NOW + HEARTBEAT_MS);
    assert_eq!(port.taken(), vec![NetRequest::Publish], "the heartbeat");
    assert_eq!(
        worker.next_wake(NOW + HEARTBEAT_MS),
        Some(Duration::from_millis(HEARTBEAT_MS as u64))
    );
    // Signed out: no heartbeat, no timer.
    port.allowed.store(false, Ordering::SeqCst);
    wake(
        &mut worker,
        &world.conn,
        &mut windows,
        vec![OverlayMsg::GateChanged],
        NOW + HEARTBEAT_MS + 1,
    );
    assert_eq!(worker.next_wake(NOW + HEARTBEAT_MS + 1), None);
    worker.update(&world.conn, &mut windows, NOW + 3 * HEARTBEAT_MS);
    assert_eq!(port.taken(), Vec::<NetRequest>::new());
    // Publishing off: no heartbeat either.
    let port = FakePort::allowed();
    let mut worker = worker_with(Settings::default(), &port);
    worker.update(&world.conn, &mut windows, NOW);
    worker.update(&world.conn, &mut windows, NOW + HEARTBEAT_MS);
    assert_eq!(port.taken(), Vec::<NetRequest>::new());
    assert_eq!(worker.next_wake(NOW + HEARTBEAT_MS), None);
}

// #505: an idle card is published once and not kept alive; the hosted page
// shows nothing for it either way.
#[test]
fn an_idle_card_is_published_once_without_a_heartbeat() {
    let world = World::new("dispatch-idle");
    let port = FakePort::allowed();
    let mut worker = worker_with(publishing(), &port);
    let mut windows = FakeWindows::default();
    wake(&mut worker, &world.conn, &mut windows, vec![], NOW);
    assert_eq!(port.taken(), vec![NetRequest::Publish], "the first state");
    assert_eq!(
        worker.league_state().map(|card| card.status),
        Some(LeagueStatus::Idle)
    );
    assert_eq!(worker.next_wake(NOW), None, "no heartbeat timer");
    worker.update(&world.conn, &mut windows, NOW + 3 * HEARTBEAT_MS);
    assert_eq!(port.taken(), Vec::<NetRequest>::new());
}

#[test]
fn offline_stops_the_heartbeat() {
    let world = World::live("dispatch-offline");
    let port = FakePort::allowed();
    let mut worker = worker_with(publishing(), &port);
    let mut windows = FakeWindows::default();
    wake(&mut worker, &world.conn, &mut windows, vec![], NOW);
    port.taken();
    port.allowed.store(false, Ordering::SeqCst);
    // The due heartbeat finds the gate closed and stops.
    worker.update(&world.conn, &mut windows, NOW + HEARTBEAT_MS);
    assert_eq!(port.taken(), Vec::<NetRequest>::new());
    assert_eq!(worker.next_wake(NOW + HEARTBEAT_MS), None);
    // A changed card is not published while offline.
    change_the_card(&world, 2);
    wake(
        &mut worker,
        &world.conn,
        &mut windows,
        vec![OverlayMsg::DecksChanged],
        NOW + HEARTBEAT_MS + 1,
    );
    assert_eq!(port.taken(), Vec::<NetRequest>::new());
    // Back online: published at once, then every 30 s again.
    port.allowed.store(true, Ordering::SeqCst);
    let back = NOW + 2 * HEARTBEAT_MS;
    wake(
        &mut worker,
        &world.conn,
        &mut windows,
        vec![OverlayMsg::GateChanged],
        back,
    );
    assert_eq!(port.taken(), vec![NetRequest::Publish]);
    assert_eq!(
        worker.next_wake(back),
        Some(Duration::from_millis(HEARTBEAT_MS as u64))
    );
}

#[test]
fn nothing_is_published_without_sign_in() {
    // The worker queues nothing while the gate is closed ...
    let world = World::new("dispatch-signed-out");
    let port = FakePort::default();
    let mut worker = worker_with(publishing(), &port);
    let mut windows = FakeWindows::default();
    wake(&mut worker, &world.conn, &mut windows, vec![], NOW);
    change_the_card(&world, 2);
    wake(
        &mut worker,
        &world.conn,
        &mut windows,
        vec![OverlayMsg::DecksChanged],
        NOW + HEARTBEAT_MS,
    );
    assert_eq!(port.taken(), Vec::<NetRequest>::new());
    // ... and a job that runs anyway sends nothing and records nothing.
    let stub = stub(204, 201);
    let api = api_for(&stub.base_url, false, false);
    assert_eq!(world.publish(&api), None);
    assert_eq!(overlay_publish_error(&world.rows()), None);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(stub.connections(), 0);
}

#[test]
fn turning_publishing_on_queues_a_push() {
    let world = World::new("dispatch-switch-on");
    let port = FakePort::allowed();
    let mut worker = worker_with(Settings::default(), &port);
    let mut windows = FakeWindows::default();
    wake(&mut worker, &world.conn, &mut windows, vec![], NOW);
    assert_eq!(port.taken(), Vec::<NetRequest>::new());
    wake(
        &mut worker,
        &world.conn,
        &mut windows,
        vec![OverlayMsg::SettingsChanged {
            settings: Box::new(publishing()),
        }],
        NOW + 1_000,
    );
    assert_eq!(port.taken(), vec![NetRequest::Publish]);
}

#[test]
fn turning_publishing_off_queues_the_clear_and_stops_the_heartbeat() {
    let world = World::new("dispatch-switch-off");
    let port = FakePort::allowed();
    let mut worker = worker_with(publishing(), &port);
    let mut windows = FakeWindows::default();
    wake(&mut worker, &world.conn, &mut windows, vec![], NOW);
    port.taken();
    wake(
        &mut worker,
        &world.conn,
        &mut windows,
        vec![OverlayMsg::SettingsChanged {
            settings: Box::new(Settings::default()),
        }],
        NOW + 1_000,
    );
    assert_eq!(port.taken(), vec![NetRequest::Clear]);
    assert_eq!(worker.next_wake(NOW + 1_000), None);
    // An unrelated save while off clears nothing again.
    wake(
        &mut worker,
        &world.conn,
        &mut windows,
        vec![OverlayMsg::SettingsChanged {
            settings: Box::new(Settings::default()),
        }],
        NOW + 2_000,
    );
    assert_eq!(port.taken(), Vec::<NetRequest>::new());
}

#[test]
fn a_changed_background_asks_to_forget_the_uploaded_copy() {
    let world = World::new("dispatch-background");
    let port = FakePort::allowed();
    let mut worker = worker_with(Settings::default(), &port);
    let mut windows = FakeWindows::default();
    wake(&mut worker, &world.conn, &mut windows, vec![], NOW);
    wake(
        &mut worker,
        &world.conn,
        &mut windows,
        vec![OverlayMsg::BackgroundChanged],
        NOW + 1_000,
    );
    assert_eq!(port.taken(), vec![NetRequest::ForgetBackground]);
}

#[test]
fn a_publish_answer_reaches_the_settings_card() {
    let world = World::new("dispatch-answer");
    let port = FakePort::allowed();
    let mut worker = worker_with(publishing(), &port);
    let mut windows = FakeWindows::default();
    let status = OverlayPublishChanged {
        last_published_at_ms: Some(NOW),
        error: None,
    };
    wake(
        &mut worker,
        &world.conn,
        &mut windows,
        vec![OverlayMsg::PublishAnswered(status)],
        NOW,
    );
    let pushed: Vec<Value> = windows
        .emitted()
        .into_iter()
        .filter(|(label, event)| label == MAIN_WINDOW && event.name == "overlay-publish-changed")
        .map(|(_, event)| event.payload)
        .collect();
    assert_eq!(
        pushed,
        vec![json!({"lastPublishedAtMs": NOW, "error": null})]
    );
}

#[test]
fn a_change_inside_the_gap_is_held_then_sent() {
    let world = World::new("dispatch-gap");
    let port = FakePort::allowed();
    let mut worker = worker_with(publishing(), &port);
    let mut windows = FakeWindows::default();
    wake(&mut worker, &world.conn, &mut windows, vec![], NOW);
    assert_eq!(port.taken(), vec![NetRequest::Publish], "at start-up");

    change_the_card(&world, 2);
    wake(
        &mut worker,
        &world.conn,
        &mut windows,
        vec![OverlayMsg::DecksChanged],
        NOW + 400,
    );
    assert_eq!(
        port.taken(),
        Vec::<NetRequest>::new(),
        "held inside the gap"
    );
    assert_eq!(
        worker.next_wake(NOW + 400),
        Some(Duration::from_millis(600)),
        "wakes when the gap ends"
    );

    wake(&mut worker, &world.conn, &mut windows, vec![], NOW + 1_000);
    assert_eq!(
        port.taken(),
        vec![NetRequest::Publish],
        "sent when the gap ends"
    );
}

#[test]
fn a_refused_push_is_not_retried_before_the_next_change_or_heartbeat() {
    let world = World::new("dispatch-refused");
    let port = FakePort::allowed();
    let mut worker = worker_with(publishing(), &port);
    let mut windows = FakeWindows::default();
    wake(&mut worker, &world.conn, &mut windows, vec![], NOW);
    assert_eq!(port.taken(), vec![NetRequest::Publish], "at start-up");

    let refused = OverlayMsg::PublishAnswered(crate::overlay::events::OverlayPublishChanged {
        last_published_at_ms: None,
        error: Some(PublishError::Rejected),
    });
    wake(
        &mut worker,
        &world.conn,
        &mut windows,
        vec![refused],
        NOW + 2_000,
    );
    assert_eq!(
        port.taken(),
        Vec::<NetRequest>::new(),
        "a 422 queues no publish of its own"
    );
}

#[test]
fn switching_publishing_off_then_on_inside_the_gap_waits_for_its_end() {
    let world = World::new("dispatch-gap-toggle");
    let port = FakePort::allowed();
    let mut worker = worker_with(publishing(), &port);
    let mut windows = FakeWindows::default();
    wake(&mut worker, &world.conn, &mut windows, vec![], NOW);
    assert_eq!(port.taken(), vec![NetRequest::Publish], "at start-up");
    let switch = |on: bool| OverlayMsg::SettingsChanged {
        settings: Box::new(if on {
            publishing()
        } else {
            Settings::default()
        }),
    };
    wake(
        &mut worker,
        &world.conn,
        &mut windows,
        vec![switch(false)],
        NOW + 200,
    );
    assert_eq!(port.taken(), vec![NetRequest::Clear], "switched off");
    assert_eq!(
        worker.next_wake(NOW + 200),
        None,
        "nothing is due while off"
    );
    wake(
        &mut worker,
        &world.conn,
        &mut windows,
        vec![switch(true)],
        NOW + 400,
    );
    assert_eq!(
        port.taken(),
        Vec::<NetRequest>::new(),
        "switched back on inside the gap: held"
    );
    assert_eq!(
        worker.next_wake(NOW + 400),
        Some(Duration::from_millis(600)),
        "wakes when the gap ends"
    );
    worker.update(&world.conn, &mut windows, NOW + PUBLISH_GAP_MS);
    assert_eq!(
        port.taken(),
        vec![NetRequest::Publish],
        "sent at the gap's end"
    );
}

#[test]
fn switching_publishing_on_after_the_gap_publishes_at_once() {
    let world = World::live("dispatch-toggle-later");
    let port = FakePort::allowed();
    let mut worker = worker_with(publishing(), &port);
    let mut windows = FakeWindows::default();
    wake(&mut worker, &world.conn, &mut windows, vec![], NOW);
    port.taken();
    wake(
        &mut worker,
        &world.conn,
        &mut windows,
        vec![OverlayMsg::SettingsChanged {
            settings: Box::new(Settings::default()),
        }],
        NOW + 200,
    );
    assert_eq!(port.taken(), vec![NetRequest::Clear]);
    wake(
        &mut worker,
        &world.conn,
        &mut windows,
        vec![OverlayMsg::SettingsChanged {
            settings: Box::new(publishing()),
        }],
        NOW + 5_000,
    );
    assert_eq!(port.taken(), vec![NetRequest::Publish], "pushed at once");
    assert_eq!(
        worker.next_wake(NOW + 5_000),
        Some(Duration::from_millis(HEARTBEAT_MS as u64)),
        "the heartbeat starts again"
    );
}

#[test]
fn no_publish_goes_out_inside_the_gap_and_a_burst_in_it_sends_once() {
    let world = World::new("dispatch-gap-closed");
    let port = FakePort::allowed();
    let mut worker = worker_with(publishing(), &port);
    let mut windows = FakeWindows::default();
    wake(&mut worker, &world.conn, &mut windows, vec![], NOW);
    assert_eq!(port.taken(), vec![NetRequest::Publish], "at start-up");
    // Two card changes in two wakes inside the gap.
    change_the_card(&world, 2);
    wake(
        &mut worker,
        &world.conn,
        &mut windows,
        vec![OverlayMsg::DecksChanged],
        NOW + 300,
    );
    change_the_card(&world, 3);
    wake(
        &mut worker,
        &world.conn,
        &mut windows,
        vec![OverlayMsg::DecksChanged],
        NOW + 700,
    );
    // The gap's last millisecond is still closed.
    worker.update(&world.conn, &mut windows, NOW + PUBLISH_GAP_MS - 1);
    assert_eq!(
        port.taken(),
        Vec::<NetRequest>::new(),
        "held inside the gap"
    );
    assert_eq!(
        worker.next_wake(NOW + PUBLISH_GAP_MS - 1),
        Some(Duration::from_millis(1))
    );
    worker.update(&world.conn, &mut windows, NOW + PUBLISH_GAP_MS);
    assert_eq!(
        port.taken(),
        vec![NetRequest::Publish],
        "both changes go out in one publish"
    );
    // A change right after the held publish starts a new gap from it.
    change_the_card(&world, 4);
    wake(
        &mut worker,
        &world.conn,
        &mut windows,
        vec![OverlayMsg::DecksChanged],
        NOW + PUBLISH_GAP_MS + 1,
    );
    assert_eq!(port.taken(), Vec::<NetRequest>::new());
    assert_eq!(
        worker.next_wake(NOW + PUBLISH_GAP_MS + 1),
        Some(Duration::from_millis(PUBLISH_GAP_MS as u64 - 1))
    );
    worker.update(&world.conn, &mut windows, NOW + 2 * PUBLISH_GAP_MS);
    assert_eq!(port.taken(), vec![NetRequest::Publish]);
}

#[test]
fn the_sync_gate_reopening_inside_the_gap_waits_for_its_end() {
    let world = World::new("dispatch-gate-gap");
    let port = FakePort::allowed();
    let mut worker = worker_with(publishing(), &port);
    let mut windows = FakeWindows::default();
    wake(&mut worker, &world.conn, &mut windows, vec![], NOW);
    port.taken();
    port.allowed.store(false, Ordering::SeqCst);
    wake(
        &mut worker,
        &world.conn,
        &mut windows,
        vec![OverlayMsg::GateChanged],
        NOW + 100,
    );
    assert_eq!(worker.next_wake(NOW + 100), None, "closed: nothing is due");
    port.allowed.store(true, Ordering::SeqCst);
    wake(
        &mut worker,
        &world.conn,
        &mut windows,
        vec![OverlayMsg::GateChanged],
        NOW + 300,
    );
    assert_eq!(
        port.taken(),
        Vec::<NetRequest>::new(),
        "held inside the gap"
    );
    assert_eq!(
        worker.next_wake(NOW + 300),
        Some(Duration::from_millis(700))
    );
    worker.update(&world.conn, &mut windows, NOW + PUBLISH_GAP_MS);
    assert_eq!(
        port.taken(),
        vec![NetRequest::Publish],
        "sent at the gap's end"
    );
}

#[test]
fn a_held_publish_moves_the_heartbeat_to_when_it_was_sent() {
    let world = World::live("dispatch-held-heartbeat");
    let port = FakePort::allowed();
    let mut worker = worker_with(publishing(), &port);
    let mut windows = FakeWindows::default();
    wake(&mut worker, &world.conn, &mut windows, vec![], NOW);
    port.taken();
    change_the_live_card(&world, 2);
    wake(
        &mut worker,
        &world.conn,
        &mut windows,
        vec![OverlayMsg::DecksChanged],
        NOW + 400,
    );
    assert_eq!(port.taken(), Vec::<NetRequest>::new(), "held");
    // While held, the wake is the gap's end, not the heartbeat.
    assert_eq!(
        worker.next_wake(NOW + 400),
        Some(Duration::from_millis(600))
    );
    let sent = NOW + PUBLISH_GAP_MS;
    worker.update(&world.conn, &mut windows, sent);
    assert_eq!(port.taken(), vec![NetRequest::Publish]);
    assert_eq!(
        worker.next_wake(sent),
        Some(Duration::from_millis(HEARTBEAT_MS as u64)),
        "the heartbeat counts from the held publish"
    );
    // Not at the first publish's heartbeat ...
    worker.update(&world.conn, &mut windows, NOW + HEARTBEAT_MS);
    assert_eq!(port.taken(), Vec::<NetRequest>::new());
    // ... but 30 s after the held one, once.
    worker.update(&world.conn, &mut windows, sent + HEARTBEAT_MS);
    assert_eq!(port.taken(), vec![NetRequest::Publish], "the heartbeat");
    worker.update(&world.conn, &mut windows, sent + HEARTBEAT_MS + 1);
    assert_eq!(port.taken(), Vec::<NetRequest>::new());
}

#[test]
fn a_card_change_at_the_heartbeat_sends_one_publish() {
    let world = World::live("dispatch-heartbeat-change");
    let port = FakePort::allowed();
    let mut worker = worker_with(publishing(), &port);
    let mut windows = FakeWindows::default();
    wake(&mut worker, &world.conn, &mut windows, vec![], NOW);
    port.taken();
    change_the_card(&world, 2);
    wake(
        &mut worker,
        &world.conn,
        &mut windows,
        vec![OverlayMsg::DecksChanged],
        NOW + HEARTBEAT_MS,
    );
    assert_eq!(port.taken(), vec![NetRequest::Publish]);
    assert_eq!(
        worker.next_wake(NOW + HEARTBEAT_MS),
        Some(Duration::from_millis(HEARTBEAT_MS as u64))
    );
}
