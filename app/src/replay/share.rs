//! Share a match's replay and switch its link off (replays spec section 8;
//! 0.x `ShareReplay`, `RevokeReplayShare` and their controllers).
//!
//! Every function here blocks (the network, the tracker's commit), so the
//! commands run them inside `spawn_blocking`, never on an IPC thread or in
//! an async task.

use std::collections::BTreeMap;

use ::replay::{
    CardInfo, PrepareFailure, RevokeFailure, ShareFailure, build_snapshot, link_to_game,
    prepare_shared_snapshot,
};
use chrono_tz::Tz;
use game_detail::GameLogEntry;
use mymtgo_api::{
    ApiError, ApiRequest, ApiResponse, Tier, parse_share_response, revoke_ok,
    revoke_replay_request, share_replay_request, share_status,
};
use serde::Serialize;
use store::Connection;
use store::match_edits::MatchEdit;
use store::replay::{GameShare, game_share, replay_inputs};
use tracker::MatchEditor;

use crate::api::session::ApiState;

/// The host named in messages when the API is not configured (0.x's
/// default).
const DEFAULT_HOST: &str = "mymtgo.com";
/// How many failing paths a validation log line names (0.x
/// `array_slice($e->errors(), 0, 5)`).
const LOGGED_ERRORS: usize = 5;
/// The message for a game that is gone.
const GAME_GONE: &str = "This match no longer exists.";
/// The message for an Arena game: Arena replays stay on this machine.
const ARENA_NOT_SHARED: &str = "Arena replays can't be shared.";
const ENDSTEP_NOT_SHARED: &str = "Endstep replays can't be shared.";
const RECORDED_NOT_SHARED: &str = "A recorded game cannot be shared";

/// The share dialog's state for one game (0.x `share` page prop).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct ReplayShareState {
    /// Signed in to MyMTGO (Plan Q session).
    pub linked: bool,
    /// The stored plan is `supporter` (informational only: the server
    /// decides).
    pub supporter: bool,
    /// The match's shared link opening on this game (`?game=N`), when it is
    /// shared.
    pub url: Option<String>,
    /// `false` for an Arena or Endstep game: nothing of theirs is shared.
    pub shareable: bool,
}

/// What a share or switch off works with.
pub struct ShareContext<'a> {
    pub api: &'a ApiState,
    pub editor: &'a MatchEditor,
    /// A read-only connection.
    pub conn: &'a Connection,
    pub timezone: Tz,
    pub now_ms: i64,
}

/// The share state of game row `game_id`.
pub fn share_state(
    api: &ApiState,
    conn: &Connection,
    game_id: i64,
) -> Result<ReplayShareState, String> {
    let game = find_game(conn, game_id)?;
    let recorded = store::recorded_replays::recorded_game(conn, game_id)
        .map_err(|error| error.to_string())?
        .is_some();
    let shareable = not_shared_message(conn, &game)?.is_none() && !recorded;
    let supporter = match store::sync::tier(conn) {
        Ok(tier) => tier == Tier::Supporter,
        Err(error) => {
            tracing::warn!(%error, "could not read the sync plan");
            false
        }
    };
    Ok(ReplayShareState {
        linked: api.session().linked(),
        supporter,
        url: if shareable {
            game.share
                .map(|share| link_to_game(&share.url, game.game_number))
        } else {
            None
        },
        shareable,
    })
}

/// Share the match of game row `game_id` (spec section 8 order). `dat`
/// gives a match token's `.dat` game log entries (the match page's
/// `match_dat_entries`). A failure is the message to show.
pub fn share(
    ctx: &ShareContext,
    game_id: i64,
    dat: &dyn Fn(&str) -> Vec<GameLogEntry>,
) -> Result<ReplayShareState, String> {
    let game = find_shareable_game(ctx.conn, game_id)?;
    // A recorded game opens in the .mtgr viewer: sharing is lost for it.
    // Revoking stays allowed, removing a public link is always safe.
    if store::recorded_replays::recorded_game(ctx.conn, game_id)
        .map_err(|error| error.to_string())?
        .is_some()
    {
        return Err(RECORDED_NOT_SHARED.to_string());
    }
    let host = host(ctx.api);
    let shared = match upload(ctx, &game, dat) {
        Ok(shared) => shared,
        Err(failure) => return Err(failure.message(&host)),
    };
    let edit = MatchEdit::SetReplayShare {
        token: game.token.clone(),
        uuid: shared.uuid,
        url: shared.url,
        shared_at_ms: ctx.now_ms,
    };
    store_edit(ctx.editor, edit)?;
    share_state(ctx.api, ctx.conn, game_id)
}

/// Switch the match's link off (0.x `RevokeReplayShare`): with a stored
/// link, `DELETE` it; a 2xx or 404 (already gone) forgets it here.
pub fn revoke(ctx: &ShareContext, game_id: i64) -> Result<ReplayShareState, String> {
    let game = find_shareable_game(ctx.conn, game_id)?;
    if let Some(share) = &game.share {
        let request = revoke_replay_request(&share.uuid);
        let failure = match send(ctx.api, &request, ctx.now_ms) {
            Ok(response) if revoke_ok(response.status) => None,
            Ok(response) => {
                tracing::warn!(
                    game_id,
                    status = response.status,
                    "replay share revoke failed"
                );
                Some(RevokeFailure::Unreachable)
            }
            Err(ApiError::Offline) => Some(RevokeFailure::Offline),
            Err(ApiError::NotLinked) => Some(RevokeFailure::NotSignedIn),
            Err(error) => {
                tracing::warn!(game_id, %error, "replay share revoke failed");
                Some(RevokeFailure::Unreachable)
            }
        };
        if let Some(failure) = failure {
            return Err(failure.message(&host(ctx.api)));
        }
    }
    let edit = MatchEdit::ClearReplayShare {
        token: game.token.clone(),
    };
    store_edit(ctx.editor, edit)?;
    share_state(ctx.api, ctx.conn, game_id)
}

/// Build, prepare and upload the match's snapshot (0.x `ShareReplay::run`
/// up to the store): the login id, the snapshot, the redaction and
/// validation, then `POST /api/replays`.
fn upload(
    ctx: &ShareContext,
    game: &GameShare,
    dat: &dyn Fn(&str) -> Vec<GameLogEntry>,
) -> Result<mymtgo_api::SharedReplay, ShareFailure> {
    let entries = dat(&game.token);
    let inputs = match replay_inputs(ctx.conn, game.match_id, ctx.timezone, &entries) {
        Ok(Some(inputs)) => inputs,
        Ok(None) => return Err(ShareFailure::Invalid),
        Err(error) => {
            tracing::warn!(match_id = game.match_id, %error, "could not read the replay");
            return Err(ShareFailure::Invalid);
        }
    };
    let Some(login_id) = inputs.login_id else {
        return Err(ShareFailure::AccountUnknown);
    };

    let conn = ctx.conn;
    let cards = |ids: &[u32]| -> BTreeMap<u32, CardInfo> {
        store::replay::card_infos(conn, ids).unwrap_or_else(|error| {
            tracing::warn!(%error, "could not read the replay's cards");
            BTreeMap::new()
        })
    };
    let snapshot = build_snapshot(inputs.snapshot_input(), &cards);
    if snapshot.games.is_empty() {
        return Err(ShareFailure::Invalid);
    }

    let prepared = match prepare_shared_snapshot(snapshot, &inputs.opponent_names) {
        Ok(prepared) => prepared,
        Err(failure) => {
            log_prepare_failure(game.match_id, &failure);
            return Err(ShareFailure::from(failure));
        }
    };

    let request = share_replay_request(&inputs.token, login_id, &prepared);
    let response = match send(ctx.api, &request, ctx.now_ms) {
        Ok(response) => response,
        Err(error) => return Err(send_failure(game.match_id, error)),
    };
    if !response.is_success() {
        let failure = share_status(response.status, &response.body);
        if failure == ShareFailure::Server {
            tracing::warn!(
                match_id = game.match_id,
                status = response.status,
                "replay share failed"
            );
        }
        return Err(failure);
    }
    parse_share_response(&response.body).map_err(|error| {
        tracing::warn!(match_id = game.match_id, %error, "replay share answer unreadable");
        ShareFailure::Server
    })
}

/// 0.x logs a snapshot that fails validation (its first five paths); a
/// name that survived redaction is logged too, without the name.
fn log_prepare_failure(match_id: i64, failure: &PrepareFailure) {
    match failure {
        PrepareFailure::NameSurvived => {
            tracing::warn!(
                match_id,
                "an opponent name survived redaction; nothing sent"
            );
        }
        PrepareFailure::Invalid(errors) => {
            let first: BTreeMap<&String, &Vec<String>> =
                errors.iter().take(LOGGED_ERRORS).collect();
            tracing::warn!(match_id, errors = ?first, "Replay snapshot failed validation");
        }
    }
}

/// A send that ended without an answer, as the share's failure.
fn send_failure(match_id: i64, error: ApiError) -> ShareFailure {
    match error {
        ApiError::Offline => ShareFailure::Offline,
        ApiError::NotLinked => ShareFailure::NotSignedIn,
        ApiError::Timeout | ApiError::Connect(..) => {
            tracing::warn!(match_id, %error, "Replay share could not connect");
            ShareFailure::Unreachable
        }
        other => {
            tracing::warn!(match_id, error = %other, "replay share failed");
            ShareFailure::Server
        }
    }
}

/// Send through 0.x `SyncApi::send`'s 401 loop, shared with the sync
/// (`tokens::send_with_refresh`): every answer but a 401 is the caller's
/// to read; a dead refresh token is `NotLinked`.
fn send(api: &ApiState, request: &ApiRequest, now_ms: i64) -> Result<ApiResponse, ApiError> {
    crate::api::tokens::send_with_refresh(api, request, now_ms)
}

/// Game row `game_id` with its match and link, or the message for a game
/// that is gone.
fn find_game(conn: &Connection, game_id: i64) -> Result<GameShare, String> {
    match game_share(conn, game_id) {
        Ok(Some(game)) => Ok(game),
        Ok(None) => Err(GAME_GONE.to_string()),
        Err(error) => Err(error.to_string()),
    }
}

/// Like `find_game`, but an Arena or Endstep game is refused: nothing of
/// theirs leaves the machine.
fn find_shareable_game(conn: &Connection, game_id: i64) -> Result<GameShare, String> {
    let game = find_game(conn, game_id)?;
    if let Some(refusal) = not_shared_message(conn, &game)? {
        return Err(refusal.to_string());
    }
    Ok(game)
}

/// Why the game's match cannot be shared: it is an Arena or Endstep match
/// (only MTGO replays are shared). `None` for any other match.
fn not_shared_message(conn: &Connection, game: &GameShare) -> Result<Option<&'static str>, String> {
    let platform =
        store::platform::match_platform(conn, &game.token).map_err(|error| error.to_string())?;
    Ok(match platform {
        Some(store::Platform::Arena) => Some(ARENA_NOT_SHARED),
        Some(store::Platform::Endstep) => Some(ENDSTEP_NOT_SHARED),
        _ => None,
    })
}

/// Write a share edit through the tracker, the only writer.
fn store_edit(editor: &MatchEditor, edit: MatchEdit) -> Result<(), String> {
    let outcome = editor.edit(edit)?;
    match outcome.refusal() {
        Some(message) => Err(message.to_string()),
        None => Ok(()),
    }
}

/// The configured API host for messages (Plan Q `ApiConfig.host`).
fn host(api: &ApiState) -> String {
    api.config()
        .map(|config| config.host.clone())
        .unwrap_or_else(|_| DEFAULT_HOST.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::secrets::{ACCESS_TOKEN, REFRESH_TOKEN};
    use crate::api::session::testing::{TestApi, test_api};
    use crate::api::stub::{Recorded, Reply, Stub};
    use crate::bridge::lock;
    use crate::commands::AppState;
    use crate::commands::tests::ready_state;
    use fixture_tools::TestDir;
    use flate2::read::GzDecoder;
    use serde_json::Value;
    use std::io::Read;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::Receiver;
    use tracker::TrackerEvent;

    const NOW: i64 = 1_800_000_000_000;
    const DAY: i64 = 86_400_000;
    const TOKEN: &str = "a1b2c3d4-0000-4000-8000-000000000001";
    const UUID: &str = "5f0c6a8e-1111-4000-8000-000000000001";
    const URL: &str =
        "https://mymtgo.test/players/Qplayer/replays/5f0c6a8e-1111-4000-8000-000000000001";
    const SIGN_IN: &str = "Sign in on the Account page to share replays.";

    struct Device {
        _dir: TestDir,
        state: AppState,
        _events: Receiver<TrackerEvent>,
    }

    fn device(label: &str) -> Device {
        let dir = TestDir::new(label);
        let (state, events) = ready_state(&dir);
        Device {
            _dir: dir,
            state,
            _events: events,
        }
    }

    fn writer(device: &Device) -> store::Connection {
        store::open_writer(&device.state.db_path).expect("writer")
    }

    fn reader(device: &Device) -> store::Connection {
        store::open_reader(&device.state.db_path).expect("reader")
    }

    fn editor(device: &Device) -> MatchEditor {
        lock(&device.state.tracker)
            .as_ref()
            .expect("tracker")
            .match_editor()
    }

    /// Signed in, tokens fresh for a month.
    fn linked(base_url: &str) -> TestApi {
        let token_valid = (NOW + 30 * DAY).to_string();
        test_api(
            base_url,
            &[
                ("api_device_id", r#""qdevice""#),
                ("api_token_expires_at", token_valid.as_str()),
            ],
            &[(ACCESS_TOKEN, "qaccess"), (REFRESH_TOKEN, "qrefresh")],
        )
    }

    /// Frame `index` of Qplayer (seat 1) against `foe` (seat 2). The
    /// counts change from frame to frame and do not repeat, so a long game
    /// stays large once compressed.
    fn board(foe: &str, index: u32) -> String {
        let life = 20 - index % 20;
        let library = (index * 7_919 + 13) % 100_000;
        format!(
            r#"{{"Players":[{{"Id":1,"Name":"Qplayer","Life":{life},"HandCount":7,"LibraryCount":{library}}},{{"Id":2,"Name":"{foe}","Life":20,"HandCount":7,"LibraryCount":53}}],"Cards":[]}}"#
        )
    }

    /// 0.x `shareableGame`: an imported match of Qplayer (login 3000001)
    /// against Qfoe, played with a cloud-synced deck, one won game whose
    /// frames name the opponent `frame_foe`; `frames` frames in all.
    /// Returns the game id.
    fn shareable_game(device: &Device, frame_foe: &str, frames: u32) -> i64 {
        let mut conn = writer(device);
        conn.execute_batch(
            "INSERT INTO accounts (username, login_id, first_seen_at) VALUES ('Qplayer', 3000001, 0);
             INSERT INTO opponents (username, first_seen_at) VALUES ('Qfoe', 0);
             INSERT INTO decks (mtgo_id, name, format, created_at, updated_at, source,
                 cloud_sync_enabled)
             VALUES ('111', 'Qdeck', 'CMODERN', 0, 0, 'file', 1);
             INSERT INTO deck_versions (deck_id, signature, modified_at)
             VALUES ((SELECT id FROM decks WHERE mtgo_id = '111'), 'Qsig', 0);",
        )
        .expect("registries");
        conn.execute(
            "INSERT INTO matches (token, account_id, opponent_id, source, format, match_type,
                 state, outcome, started_at, ended_at, last_activity_at, revision,
                 derived_version, deck_version_id)
             VALUES (?1, (SELECT id FROM accounts WHERE username = 'Qplayer'),
                 (SELECT id FROM opponents WHERE username = 'Qfoe'), 'imported', 'CModern',
                 'Constructed', 'complete', 'win', 1000, 90000, 90000, 1, 0,
                 (SELECT id FROM deck_versions WHERE signature = 'Qsig'))",
            [TOKEN],
        )
        .expect("a match");
        conn.execute(
            "INSERT INTO games (match_id, game_number, result, end_reason, started_at)
             VALUES ((SELECT id FROM matches WHERE token = ?1), 1, 'win', 'unknown', 1000)",
            [TOKEN],
        )
        .expect("a game");
        let game_id = conn.last_insert_rowid();
        let texts: Vec<::replay::FrameText> = (0..frames)
            .map(|index| ::replay::FrameText {
                time: format!("18:{:02}:{:02}", index / 60 % 60, index % 60),
                text: board(frame_foe, index),
            })
            .collect();
        // Immediate: the tracker writes too, so a read-then-write
        // transaction could find its snapshot stale.
        let tx = conn
            .transaction_with_behavior(store::TransactionBehavior::Immediate)
            .expect("tx");
        store::game_sync::store_timeline(&tx, game_id, &texts).expect("timeline");
        tx.commit().expect("commit");
        game_id
    }

    fn share_columns(device: &Device) -> (Option<String>, Option<String>, Option<i64>) {
        reader(device)
            .query_row(
                "SELECT replay_share_uuid, replay_share_url, replay_shared_at FROM matches
                 WHERE token = ?1",
                [TOKEN],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("columns")
    }

    fn set_share(device: &Device) {
        writer(device)
            .execute(
                "UPDATE matches SET replay_share_uuid = ?2, replay_share_url = ?3,
                     replay_shared_at = 5 WHERE token = ?1",
                [TOKEN, UUID, URL],
            )
            .expect("shared");
    }

    fn touch_key(device: &Device) -> String {
        store::sync::match_candidates(&reader(device), mymtgo_api::Tier::Supporter, i64::MAX)
            .expect("candidates")
            .into_iter()
            .find(|candidate| candidate.client_id == TOKEN)
            .expect("a candidate")
            .touch_key
    }

    fn shared_answer() -> String {
        format!(r#"{{"message":"Replay shared.","uuid":"{UUID}","url":"{URL}"}}"#)
    }

    fn replays_stub(status: u16, body: String) -> Stub {
        Stub::start(move |request| {
            if request.target == "/api/replays" {
                Reply::json(status, &body)
            } else {
                Reply::json(500, "{}")
            }
        })
    }

    fn no_dat(_: &str) -> Vec<GameLogEntry> {
        Vec::new()
    }

    fn run_share(api: &TestApi, device: &Device, game_id: i64) -> Result<ReplayShareState, String> {
        let conn = reader(device);
        let editor = editor(device);
        let ctx = ShareContext {
            api: &api.state,
            editor: &editor,
            conn: &conn,
            timezone: Tz::UTC,
            now_ms: NOW,
        };
        share(&ctx, game_id, &no_dat)
    }

    fn run_revoke(
        api: &TestApi,
        device: &Device,
        game_id: i64,
    ) -> Result<ReplayShareState, String> {
        let conn = reader(device);
        let editor = editor(device);
        let ctx = ShareContext {
            api: &api.state,
            editor: &editor,
            conn: &conn,
            timezone: Tz::UTC,
            now_ms: NOW,
        };
        revoke(&ctx, game_id)
    }

    fn uploaded(request: &Recorded) -> Value {
        let mut json = Vec::new();
        GzDecoder::new(request.body.as_slice())
            .read_to_end(&mut json)
            .expect("a gzip body");
        serde_json::from_slice(&json).expect("json")
    }

    fn replay_posts(stub: &Stub) -> Vec<Recorded> {
        stub.requests()
            .into_iter()
            .filter(|request| request.method == "POST" && request.target == "/api/replays")
            .collect()
    }

    // ---- 0.x ShareReplayTest ------------------------------------------------

    #[test]
    fn uploads_the_redacted_match_as_gzip_and_stores_the_link() {
        let device = device("replay-share-upload");
        let game_id = shareable_game(&device, "Qfoe", 1);
        let before = touch_key(&device);
        let stub = replays_stub(201, shared_answer());
        let api = linked(&stub.base_url);

        let state = run_share(&api, &device, game_id).expect("shared");

        let posts = replay_posts(&stub);
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].header("content-encoding"), Some("gzip"));
        assert_eq!(posts[0].header("content-type"), Some("application/json"));
        assert_eq!(posts[0].header("authorization"), Some("Bearer qaccess"));
        let body = uploaded(&posts[0]);
        assert_eq!(body["client_match_key"], TOKEN);
        assert_eq!(body["login_id"], 3_000_001);
        assert!(!body["snapshot"].to_string().contains("Qfoe"), "{body}");
        assert_eq!(
            body["snapshot"]["games"][0]["frames"][0]["content"]["Players"][1]["Name"],
            "Opponent"
        );
        assert_eq!(
            share_columns(&device),
            (Some(UUID.to_string()), Some(URL.to_string()), Some(NOW))
        );
        // The link is local bookkeeping: the match must not look edited to
        // cloud sync.
        assert_eq!(touch_key(&device), before);
        assert_eq!(
            state,
            ReplayShareState {
                linked: true,
                supporter: false,
                url: Some(format!("{URL}?game=1")),
                shareable: true,
            }
        );
    }

    #[test]
    fn uploads_again_on_a_repeat_share_so_later_games_are_included() {
        let device = device("replay-share-repeat");
        let game_id = shareable_game(&device, "Qfoe", 1);
        set_share(&device);
        let stub = replays_stub(200, shared_answer());
        let api = linked(&stub.base_url);

        run_share(&api, &device, game_id).expect("shared");

        assert_eq!(replay_posts(&stub).len(), 1);
        assert_eq!(share_columns(&device).1.as_deref(), Some(URL));
        assert_eq!(share_columns(&device).2, Some(NOW), "a new share time");
    }

    #[test]
    fn uploads_nothing_when_the_opponent_name_cannot_be_hidden() {
        let device = device("replay-share-glued");
        let game_id = shareable_game(&device, "xQfoe", 1);
        let stub = replays_stub(201, shared_answer());
        let api = linked(&stub.base_url);

        let refused = run_share(&api, &device, game_id);

        assert_eq!(
            refused,
            Err("Couldn't hide your opponent's name, so this replay was not shared.".to_string())
        );
        assert_eq!(stub.connections(), 0);
        assert_eq!(share_columns(&device), (None, None, None));
    }

    #[test]
    fn explains_each_server_refusal() {
        let cases = [
            (
                422,
                r#"{"error":"replay_share_requires_supporter"}"#,
                "supporter",
            ),
            (409, r#"{"error":"player_not_claimed"}"#, "claim"),
            (413, r#"{"error":"replay_too_large"}"#, "too large"),
        ];
        for (status, body, phrase) in cases {
            let device = device(&format!("replay-share-refused-{status}"));
            let game_id = shareable_game(&device, "Qfoe", 1);
            let stub = replays_stub(status, body.to_string());
            let api = linked(&stub.base_url);

            let message = run_share(&api, &device, game_id).expect_err("refused");

            assert!(message.contains(phrase), "{status}: {message}");
            assert_eq!(share_columns(&device), (None, None, None), "{status}");
        }
        let device = device("replay-share-refused-messages");
        let game_id = shareable_game(&device, "Qfoe", 1);
        let messages: Vec<String> = [
            (422, r#"{"error":"replay_share_requires_supporter"}"#),
            (409, r#"{"error":"player_not_claimed"}"#),
            (413, r#"{"error":"replay_too_large"}"#),
            (422, r#"{"message":"The snapshot field is invalid."}"#),
        ]
        .into_iter()
        .map(|(status, body)| {
            let stub = replays_stub(status, body.to_string());
            let api = linked(&stub.base_url);
            run_share(&api, &device, game_id).expect_err("refused")
        })
        .collect();
        assert_eq!(
            messages,
            vec![
                "Sharing replays is a supporter feature.".to_string(),
                "Link your MTGO account to your 127.0.0.1 account first, so the replay can be filed under your player name. We could not confirm your claim to it.".to_string(),
                "This game is too large to share.".to_string(),
                "This replay could not be prepared for sharing.".to_string(),
            ]
        );
    }

    #[test]
    fn asks_to_sign_in_when_the_device_is_not_linked() {
        let device = device("replay-share-signed-out");
        let game_id = shareable_game(&device, "Qfoe", 1);
        let stub = replays_stub(201, shared_answer());
        // Signed out (or never signed in).
        let api = test_api(&stub.base_url, &[], &[]);

        assert_eq!(run_share(&api, &device, game_id), Err(SIGN_IN.to_string()));

        assert_eq!(stub.connections(), 0);
        assert_eq!(share_columns(&device), (None, None, None));
    }

    /// `shareable_game` with the account turned into an Arena one.
    fn arena_game(device: &Device) -> i64 {
        let game_id = shareable_game(device, "Qfoe", 1);
        writer(device)
            .execute(
                "UPDATE accounts SET platform = 'arena', arena_user_id = 'Qarena-id'",
                [],
            )
            .expect("arena account");
        game_id
    }

    #[test]
    fn share_refuses_arena_games() {
        let device = device("replay-share-arena");
        let game_id = arena_game(&device);
        let stub = replays_stub(201, shared_answer());
        let api = linked(&stub.base_url);
        let refused = Err("Arena replays can't be shared.".to_string());

        assert_eq!(run_share(&api, &device, game_id), refused);
        assert_eq!(run_revoke(&api, &device, game_id), refused);
        assert!(stub.requests().is_empty(), "nothing left the machine");
        assert_eq!(share_columns(&device), (None, None, None));
        let state = share_state(&api.state, &reader(&device), game_id).expect("state");
        assert!(!state.shareable);
        assert_eq!(state.url, None);
    }

    #[test]
    fn share_never_sends_endstep() {
        let device = device("replay-share-endstep");
        let game_id = crate::endstep_seed::seed_endstep_match(&mut writer(&device), TOKEN);
        let stub = replays_stub(201, shared_answer());
        let api = linked(&stub.base_url);
        let refused = Err("Endstep replays can't be shared.".to_string());

        assert_eq!(run_share(&api, &device, game_id), refused);
        assert_eq!(run_revoke(&api, &device, game_id), refused);
        assert!(stub.requests().is_empty(), "nothing left the machine");
        assert_eq!(share_columns(&device), (None, None, None));
        let state = share_state(&api.state, &reader(&device), game_id).expect("state");
        assert!(!state.shareable);
        assert_eq!(state.url, None);
    }

    /// Records game 1 of the shareable game's match.
    fn record_game(device: &Device) {
        writer(device)
            .execute_batch(&format!(
                "INSERT INTO recorded_replays (token, text, file_hash, imported_at)
                 VALUES ('{TOKEN}', '[Game]', 'h', 0);
                 INSERT INTO recorded_replay_games (token, position, game_number)
                 VALUES ('{TOKEN}', 0, 1);"
            ))
            .expect("a recording");
    }

    #[test]
    fn sharing_a_recorded_game_is_refused_and_sends_nothing() {
        let device = device("replay-share-recorded-refused");
        let game_id = shareable_game(&device, "Qfoe", 1);
        record_game(&device);
        let stub = replays_stub(201, shared_answer());
        let api = linked(&stub.base_url);

        assert_eq!(
            run_share(&api, &device, game_id),
            Err("A recorded game cannot be shared".to_string())
        );
        assert!(stub.requests().is_empty(), "nothing left the machine");
        assert_eq!(share_columns(&device), (None, None, None));
    }

    #[test]
    fn revoking_a_recorded_game_still_works() {
        let device = device("replay-share-recorded-revoke");
        let game_id = shareable_game(&device, "Qfoe", 1);
        record_game(&device);
        let stub = replays_stub(201, shared_answer());
        let api = linked(&stub.base_url);

        let state = run_revoke(&api, &device, game_id).expect("revoked");
        assert!(!state.shareable);
        assert_eq!(state.url, None);
    }

    #[test]
    fn a_recorded_game_is_not_shareable() {
        let device = device("replay-share-recorded");
        let game_id = shareable_game(&device, "Qfoe", 1);
        let api = linked("http://127.0.0.1:1");
        let before = share_state(&api.state, &reader(&device), game_id).expect("state");
        assert!(before.shareable);

        writer(&device)
            .execute_batch(&format!(
                "INSERT INTO recorded_replays (token, text, file_hash, imported_at)
                 VALUES ('{TOKEN}', '[Game]', 'h', 0);
                 INSERT INTO recorded_replay_games (token, position, game_number)
                 VALUES ('{TOKEN}', 0, 1);"
            ))
            .expect("a recording");
        let state = share_state(&api.state, &reader(&device), game_id).expect("state");
        assert!(!state.shareable);
        assert_eq!(state.url, None);
    }

    #[test]
    fn refuses_when_the_local_mtgo_login_id_is_unknown() {
        let device = device("replay-share-no-login");
        let game_id = shareable_game(&device, "Qfoe", 1);
        writer(&device)
            .execute("UPDATE accounts SET login_id = NULL", [])
            .expect("no login id");
        let stub = replays_stub(201, shared_answer());
        let api = linked(&stub.base_url);

        assert_eq!(
            run_share(&api, &device, game_id),
            Err(
                "Your MTGO account is not linked yet. Play a game with the app running, then try again."
                    .to_string()
            )
        );
        assert_eq!(stub.connections(), 0);
    }

    #[test]
    fn switches_the_link_off_and_forgets_it() {
        for status in [204, 404] {
            let device = device(&format!("replay-revoke-{status}"));
            let game_id = shareable_game(&device, "Qfoe", 1);
            set_share(&device);
            let before = touch_key(&device);
            let stub = Stub::start(move |_| Reply::Respond {
                status,
                headers: Vec::new(),
                body: Vec::new(),
            });
            let api = linked(&stub.base_url);

            let state = run_revoke(&api, &device, game_id).expect("switched off");

            let seen = stub.requests();
            assert_eq!(seen.len(), 1, "{status}");
            assert_eq!(seen[0].method, "DELETE");
            assert_eq!(seen[0].target, format!("/api/replays/{UUID}"));
            assert_eq!(seen[0].header("authorization"), Some("Bearer qaccess"));
            assert_eq!(share_columns(&device), (None, None, None), "{status}");
            assert_eq!(touch_key(&device), before, "{status}");
            assert_eq!(state.url, None);
            assert!(state.linked);
        }
    }

    #[test]
    fn a_failed_switch_off_keeps_the_link_and_says_so() {
        let device = device("replay-revoke-failed");
        let game_id = shareable_game(&device, "Qfoe", 1);
        set_share(&device);
        let stub = Stub::start(|_| Reply::json(500, "{}"));
        let api = linked(&stub.base_url);
        assert_eq!(
            run_revoke(&api, &device, game_id),
            Err(
                "Could not reach 127.0.0.1. The link is still on; try again in a moment."
                    .to_string()
            )
        );
        assert_eq!(share_columns(&device).0.as_deref(), Some(UUID));

        api.state.set_offline_mode(true, None);
        assert_eq!(
            run_revoke(&api, &device, game_id),
            Err(
                "Switching a link off needs a connection. Turn off offline mode first.".to_string()
            )
        );
        let signed_out = test_api(&stub.base_url, &[], &[]);
        assert_eq!(
            run_revoke(&signed_out, &device, game_id),
            Err("Sign in on the Account page to switch this link off.".to_string())
        );
        assert_eq!(stub.requests().len(), 1, "only the online, signed-in try");
        assert_eq!(share_columns(&device).0.as_deref(), Some(UUID));
    }

    #[test]
    fn passes_the_share_state_linking_to_the_game_on_screen() {
        let device = device("replay-share-state");
        let game_id = shareable_game(&device, "Qfoe", 1);
        let api = linked("http://127.0.0.1:1");
        let state = share_state(&api.state, &reader(&device), game_id).expect("state");
        assert_eq!(
            state,
            ReplayShareState {
                linked: true,
                supporter: false,
                url: None,
                shareable: true,
            }
        );

        set_share(&device);
        writer(&device)
            .execute(
                "INSERT INTO settings (key, value) VALUES ('sync_slots',
                     '{\"limit\":5,\"used\":0,\"tier\":\"supporter\"}')",
                [],
            )
            .expect("slots");
        let state = share_state(&api.state, &reader(&device), game_id).expect("state");
        assert_eq!(state.url, Some(format!("{URL}?game=1")));
        assert!(
            state
                .url
                .as_deref()
                .is_some_and(|url| url.ends_with("?game=1"))
        );
        assert!(state.supporter);

        let signed_out = test_api("http://127.0.0.1:1", &[], &[]);
        let state = share_state(&signed_out.state, &reader(&device), game_id).expect("state");
        assert!(!state.linked);

        assert!(share_state(&api.state, &reader(&device), 999_999).is_err());
    }

    // ---- Gate, auth and the server's word ------------------------------------

    #[test]
    fn share_is_refused_offline_with_the_0x_message() {
        let device = device("replay-share-offline");
        let game_id = shareable_game(&device, "Qfoe", 1);
        let stub = replays_stub(201, shared_answer());
        let api = linked(&stub.base_url);
        api.state.set_offline_mode(true, None);

        assert_eq!(
            run_share(&api, &device, game_id),
            Err("Sharing needs a connection. Turn off offline mode first.".to_string())
        );
        assert_eq!(stub.connections(), 0);
        assert_eq!(share_columns(&device), (None, None, None));
    }

    #[test]
    fn a_401_refreshes_once_and_retries() {
        let device = device("replay-share-401");
        let game_id = shareable_game(&device, "Qfoe", 1);
        let posts = Arc::new(AtomicUsize::new(0));
        let stub = {
            let posts = Arc::clone(&posts);
            Stub::start(move |request| match request.target.as_str() {
                "/oauth/token" => Reply::json(
                    200,
                    r#"{"token_type":"Bearer","access_token":"qfresh","refresh_token":"qrefresh2","expires_in":2592000}"#,
                ),
                "/api/replays" if posts.fetch_add(1, Ordering::SeqCst) == 0 => {
                    Reply::json(401, r#"{"message":"Unauthenticated."}"#)
                }
                "/api/replays" => Reply::json(201, &shared_answer()),
                _ => Reply::json(500, "{}"),
            })
        };
        let api = linked(&stub.base_url);

        let state = run_share(&api, &device, game_id).expect("shared after a refresh");

        let seen: Vec<(String, Option<String>)> = stub
            .requests()
            .iter()
            .map(|request| {
                (
                    request.target.clone(),
                    request.header("authorization").map(str::to_string),
                )
            })
            .collect();
        assert_eq!(
            seen,
            vec![
                (
                    "/api/replays".to_string(),
                    Some("Bearer qaccess".to_string())
                ),
                ("/oauth/token".to_string(), None),
                (
                    "/api/replays".to_string(),
                    Some("Bearer qfresh".to_string())
                ),
            ]
        );
        assert_eq!(state.url, Some(format!("{URL}?game=1")));
        assert_eq!(share_columns(&device).0.as_deref(), Some(UUID));
    }

    // #307 F2: a 401 whose refresh fails is the server message; nothing is
    // stored and the device stays signed in.
    #[test]
    fn a_401_whose_refresh_fails_is_the_server_message() {
        let device = device("replay-share-refresh-500");
        let game_id = shareable_game(&device, "Qfoe", 1);
        let stub = Stub::start(|request| match request.target.as_str() {
            "/oauth/token" => Reply::json(500, r#"{"message":"Server Error"}"#),
            _ => Reply::json(401, r#"{"message":"Unauthenticated."}"#),
        });
        let api = linked(&stub.base_url);
        assert_eq!(
            run_share(&api, &device, game_id),
            Err(
                "127.0.0.1 could not share this replay right now. Try again in a moment."
                    .to_string()
            )
        );
        assert_eq!(replay_posts(&stub).len(), 1, "never resent");
        assert_eq!(share_columns(&device), (None, None, None));
        assert!(api.state.session().linked());
    }

    #[test]
    fn a_429_is_the_server_message() {
        let device = device("replay-share-429");
        let game_id = shareable_game(&device, "Qfoe", 1);
        let stub = replays_stub(429, r#"{"message":"Too Many Attempts."}"#.to_string());
        let api = linked(&stub.base_url);
        assert_eq!(
            run_share(&api, &device, game_id),
            Err(
                "127.0.0.1 could not share this replay right now. Try again in a moment."
                    .to_string()
            )
        );
        assert_eq!(replay_posts(&stub).len(), 1);
        assert_eq!(share_columns(&device), (None, None, None));
    }

    #[test]
    fn an_unreachable_server_is_the_connection_message() {
        let device = device("replay-share-unreachable");
        let game_id = shareable_game(&device, "Qfoe", 1);
        // Nothing listens on port 1.
        let api = linked("http://127.0.0.1:1");
        assert_eq!(
            run_share(&api, &device, game_id),
            Err("Could not reach 127.0.0.1. Try again in a moment.".to_string())
        );
    }

    #[test]
    fn a_413_is_too_large_whatever_its_body() {
        let device = device("replay-share-413");
        let game_id = shareable_game(&device, "Qfoe", 1);
        for body in [
            r#"{"error":"replay_too_large"}"#,
            "<html><body>413 Request Entity Too Large</body></html>",
            "",
        ] {
            let stub = Stub::start(move |_| Reply::Respond {
                status: 413,
                headers: vec![("Content-Type".to_string(), "text/html".to_string())],
                body: body.as_bytes().to_vec(),
            });
            let api = linked(&stub.base_url);
            assert_eq!(
                run_share(&api, &device, game_id),
                Err("This game is too large to share.".to_string()),
                "{body}"
            );
        }
        assert_eq!(share_columns(&device), (None, None, None));
    }

    // Owner, 2026-10-09: the client never assumes the server's size limit;
    // it uploads and follows the server's answer.
    #[test]
    fn a_large_snapshot_is_uploaded_and_the_server_decides() {
        let device = device("replay-share-large");
        let game_id = shareable_game(&device, "Qfoe", 400);
        let limit = Arc::new(AtomicUsize::new(2_048));
        let stub = {
            let limit = Arc::clone(&limit);
            Stub::start(move |request| {
                if request.body.len() > limit.load(Ordering::SeqCst) {
                    Reply::json(413, r#"{"error":"replay_too_large"}"#)
                } else {
                    Reply::json(201, &shared_answer())
                }
            })
        };
        let api = linked(&stub.base_url);

        assert_eq!(
            run_share(&api, &device, game_id),
            Err("This game is too large to share.".to_string())
        );
        let posts = replay_posts(&stub);
        assert_eq!(posts.len(), 1, "uploaded, then refused");
        assert!(posts[0].body.len() > 2_048, "{}", posts[0].body.len());
        assert_eq!(share_columns(&device), (None, None, None));

        // The server raises its limit: the same snapshot is shared, with no
        // client update.
        limit.store(64 * 1024 * 1024, Ordering::SeqCst);
        let state = run_share(&api, &device, game_id).expect("shared");
        assert_eq!(state.url, Some(format!("{URL}?game=1")));
        let posts = replay_posts(&stub);
        assert_eq!(posts.len(), 2);
        assert_eq!(posts[0].body, posts[1].body, "the same upload");
    }
}
