//! A small in-memory MyMTGO sync server for tests, on top of `stub`: it
//! keeps what devices upload and answers the manifest, fetch, status and
//! deck slot calls the way the API does (`ManifestDiff`, `DeclaredPlatforms`,
//! `DeckSlots`, `UploadBlobsController`, `SidecarValidator`,
//! `FetchBlobsController`), so two devices can sync through it. Simplified
//! where the tests need no more: one account on the Supporter tier, no
//! tombstones or quotas, and the cursor is a counter of stored rows written
//! as a time, held back one row as the API holds its cursor back two minutes.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::api::stub::{Recorded, Reply, Stub};

/// One row the server holds.
#[derive(Debug, Clone)]
pub struct CloudRow {
    pub hash: String,
    pub updated_at: String,
    /// The unzipped bundle, as uploaded.
    pub canonical: String,
    /// The sidecar's `platform`; `None` is MTGO.
    pub platform: Option<String>,
    pub deck_client_id: Option<String>,
    pub stored_by: Option<String>,
    /// The order it was stored in (the cursor counts these).
    pub stored: u64,
}

/// Everything the server holds.
#[derive(Debug, Default)]
pub struct CloudState {
    /// By `(type, client id)`.
    pub rows: BTreeMap<(String, String), CloudRow>,
    /// The client ids of the decks whose slot is on.
    pub enabled: BTreeSet<String>,
    /// An API from before Arena and Endstep sync: its manifest names no
    /// `platforms` and it drops an upload's `platform`, storing the row as
    /// MTGO.
    pub old_api: bool,
    stored: u64,
}

/// A running fake server and what it holds.
pub struct FakeCloud {
    pub stub: Stub,
    pub state: Arc<Mutex<CloudState>>,
}

impl FakeCloud {
    pub fn start() -> FakeCloud {
        let state: Arc<Mutex<CloudState>> = Arc::default();
        let held = Arc::clone(&state);
        let stub = Stub::start(move |request| {
            let mut state = held.lock().expect("cloud lock");
            answer(&mut state, request)
        });
        FakeCloud { stub, state }
    }

    /// Store `canonical` as `device` uploaded it, past every check (another
    /// device's upload the test does not run).
    pub fn store_as(&self, device: &str, sync_type: &str, client_id: &str, row: StoredAs) {
        let mut state = self.state.lock().expect("cloud lock");
        state.stored += 1;
        let stored = state.stored;
        state.rows.insert(
            (sync_type.to_string(), client_id.to_string()),
            CloudRow {
                hash: sha256_hex(&row.canonical),
                updated_at: row.updated_at,
                canonical: row.canonical,
                platform: row.platform,
                deck_client_id: row.deck_client_id,
                stored_by: Some(device.to_string()),
                stored,
            },
        );
    }
}

/// A row `FakeCloud::store_as` stores.
pub struct StoredAs {
    pub canonical: String,
    pub updated_at: String,
    pub platform: Option<String>,
    pub deck_client_id: Option<String>,
}

fn answer(state: &mut CloudState, request: &Recorded) -> Reply {
    let path = request.target.split('?').next().unwrap_or_default();
    let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
    match (request.method.as_str(), path) {
        ("POST", "/api/sync/manifest") => manifest(state, &body),
        ("POST", "/api/sync/blobs") => upload(state, request),
        ("POST", "/api/sync/blobs/fetch") => fetch(state, &body),
        ("GET", "/api/sync/status") => status(state, &request.target),
        ("GET", "/api/sync/decks/slots") => Reply::json(200, &ledger(state).to_string()),
        ("PUT", _) if path.starts_with("/api/sync/decks/") => {
            let client_id = path.trim_start_matches("/api/sync/decks/").to_string();
            if body["enabled"] == json!(true) {
                state.enabled.insert(client_id);
            } else {
                state.enabled.remove(&client_id);
            }
            Reply::json(201, &ledger(state).to_string())
        }
        _ => Reply::json(200, "{}"),
    }
}

/// The deck slot ledger: unlimited, Supporter, every enabled deck.
fn ledger(state: &CloudState) -> Value {
    let decks: Vec<Value> = state
        .enabled
        .iter()
        .map(|client_id| {
            json!({
                "client_id": client_id,
                "enabled_at": "2026-01-01 00:00:00",
                "disabled_at": null,
                "frees_at": null,
            })
        })
        .collect();
    json!({
        "limit": null,
        "used": state.enabled.len(),
        "tier": "supporter",
        "decks": decks,
    })
}

/// Whether a row's deck holds a slot (match and league rows need one).
fn deck_enabled(state: &CloudState, row: &CloudRow) -> bool {
    row.deck_client_id
        .as_ref()
        .is_some_and(|deck| state.enabled.contains(deck))
}

/// the API `ClientId::RULE`: `^[A-Za-z0-9][A-Za-z0-9_-]{0,190}$`.
fn is_client_id(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && text.chars().count() <= 191
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// the API `meta.*.hash`: 64 lowercase hex characters.
fn is_hash(text: &str) -> bool {
    text.len() == 64
        && text
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The non-MTGO platforms a body declared (unknown values dropped).
fn declared(body: &Value) -> Vec<String> {
    body["platforms"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|platform| *platform == "arena" || *platform == "endstep")
        .map(str::to_string)
        .collect()
}

/// Whether a row of `platform` is served to a client declaring `declared`
/// (the API `servedTo`: MTGO rows always).
fn served(platform: &Option<String>, declared: &[String]) -> bool {
    match platform {
        None => true,
        Some(platform) => declared.contains(platform),
    }
}

fn cursor_text(stored: u64) -> String {
    format!("2026-01-01T00:00:00.{stored:06}Z")
}

fn cursor_count(text: &str) -> Option<u64> {
    text.trim_end_matches('Z').rsplit('.').next()?.parse().ok()
}

fn texts(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// the API `ManifestDiff::resolve` for one call.
fn manifest(state: &CloudState, body: &Value) -> Reply {
    let sync_type = body["type"].as_str().unwrap_or_default().to_string();
    let platforms = declared(body);
    let last = body["last"] == json!(true);
    let device = body["device"].as_str().map(str::to_string);
    let cursor = body["cursor"].as_str().and_then(cursor_count);
    let since = body["since"].as_str().map(str::to_string);
    let per_deck = sync_type == "match" || sync_type == "league";
    let visible: BTreeMap<String, &CloudRow> = state
        .rows
        .iter()
        .filter(|((kind, _), _)| *kind == sync_type)
        .filter(|(_, row)| served(&row.platform, &platforms))
        .filter(|(_, row)| !per_deck || deck_enabled(state, row))
        .map(|((_, client_id), row)| (client_id.clone(), row))
        .collect();
    let mut upload = Vec::new();
    for entry in body["dirty"].as_array().into_iter().flatten() {
        let client_id = entry["client_id"].as_str().unwrap_or_default().to_string();
        // A match or league the server holds under a deck that is off (or
        // none): asking for it would only be refused as deck_not_synced.
        let held_elsewhere = per_deck
            && state
                .rows
                .get(&(sync_type.clone(), client_id.clone()))
                .is_some_and(|row| !deck_enabled(state, row));
        match visible.get(&client_id) {
            None if !held_elsewhere => upload.push(client_id),
            None => {}
            Some(row) => {
                let hash = entry["hash"].as_str().unwrap_or_default();
                let updated_at = entry["updated_at"].as_str().unwrap_or_default();
                if row.hash != hash && updated_at > row.updated_at.as_str() {
                    upload.push(client_id);
                }
            }
        }
    }
    let mut download = Vec::new();
    if last {
        let known: BTreeSet<String> = texts(&body["known"]).into_iter().collect();
        for (client_id, row) in &visible {
            if upload.contains(client_id) {
                continue;
            }
            let changed = match cursor {
                Some(cursor) => row.stored_by != device && row.stored > cursor,
                None => since
                    .as_ref()
                    .is_none_or(|since| row.updated_at.as_str() > since.as_str()),
            };
            if !known.contains(client_id) || changed {
                download.push(client_id.clone());
            }
        }
    }
    let mut answer = json!({
        "upload": upload,
        "download": download,
        "tombstones": [],
        "slots": ledger(state),
        "cursor": cursor_text(state.stored.saturating_sub(1)),
    });
    if !state.old_api {
        // the API echoes the platforms it took from the declaration.
        answer["platforms"] = json!(platforms);
    }
    Reply::json(200, &answer.to_string())
}

/// One blob of an upload, read from its multipart fields.
struct Upload {
    client_id: String,
    hash: String,
    updated_at: String,
    gz: Vec<u8>,
    platform: Option<String>,
    deck_client_id: Option<String>,
    occurred_at: Option<String>,
}

/// `POST /api/sync/blobs` as the API takes it:
/// - the request rules (`UploadBlobsRequest`): every client id matches
///   `ClientId::RULE`, every hash is 64 hex characters and every blob has
///   its `updated_at`, else 422 for the whole batch;
/// - per blob, in the API's order: the sidecar (`SidecarValidator`: a match
///   or league needs `occurred_at`, a `deck_client_id` must be a client
///   id; `sidecar_invalid`), the deck gate (a match or league whose deck
///   is missing or off; `deck_not_synced`), the hash of the unzipped bytes
///   (`hash_mismatch`), then last write wins: a row whose stored
///   `updated_at` is the same or newer is kept and the blob still listed
///   as stored.
fn upload(state: &mut CloudState, request: &Recorded) -> Reply {
    let fields = multipart_fields(request);
    let field = |name: &str| -> Option<String> {
        fields
            .get(name)
            .map(|bytes| String::from_utf8_lossy(bytes).to_string())
    };
    let sync_type = field("type").unwrap_or_default();
    let device = field("device");
    let mut uploads = Vec::new();
    let mut index = 0;
    while let Some(client_id) = field(&format!("meta[{index}][client_id]")) {
        let sidecar = |key: &str| field(&format!("meta[{index}][sidecar][{key}]"));
        uploads.push(Upload {
            client_id,
            hash: field(&format!("meta[{index}][hash]")).unwrap_or_default(),
            updated_at: field(&format!("meta[{index}][updated_at]")).unwrap_or_default(),
            gz: fields
                .get(&format!("blobs[{index}]"))
                .cloned()
                .unwrap_or_default(),
            platform: sidecar("platform"),
            deck_client_id: sidecar("deck_client_id"),
            occurred_at: sidecar("occurred_at"),
        });
        index += 1;
    }
    let valid = uploads.iter().all(|upload| {
        is_client_id(&upload.client_id) && is_hash(&upload.hash) && !upload.updated_at.is_empty()
    });
    if !valid {
        return Reply::json(422, r#"{"message":"The given data was invalid."}"#);
    }
    let per_deck = sync_type == "match" || sync_type == "league";
    let mut stored = Vec::new();
    let mut rejected = Vec::new();
    for upload in uploads {
        let refuse = |reason: &str| json!({ "client_id": upload.client_id, "reason": reason });
        let sidecar_ok = upload.deck_client_id.as_deref().is_none_or(is_client_id)
            && (!per_deck || upload.occurred_at.is_some());
        if !sidecar_ok {
            rejected.push(refuse("sidecar_invalid"));
            continue;
        }
        let deck_on = upload
            .deck_client_id
            .as_ref()
            .is_some_and(|deck| state.enabled.contains(deck));
        if per_deck && !deck_on {
            rejected.push(refuse("deck_not_synced"));
            continue;
        }
        let mut canonical = String::new();
        let unzipped = flate2::read::GzDecoder::new(upload.gz.as_slice())
            .read_to_string(&mut canonical)
            .is_ok();
        if !unzipped || sha256_hex(&canonical) != upload.hash {
            rejected.push(refuse("hash_mismatch"));
            continue;
        }
        let key = (sync_type.clone(), upload.client_id.clone());
        let newer_held = state
            .rows
            .get(&key)
            .is_some_and(|row| row.updated_at >= upload.updated_at);
        if newer_held {
            stored.push(upload.client_id);
            continue;
        }
        state.stored += 1;
        let row = CloudRow {
            hash: upload.hash,
            updated_at: upload.updated_at,
            canonical,
            platform: upload
                .platform
                .filter(|platform| platform != "mtgo" && !state.old_api),
            deck_client_id: upload.deck_client_id,
            stored_by: device.clone(),
            stored: state.stored,
        };
        state.rows.insert(key, row);
        stored.push(upload.client_id);
    }
    Reply::json(
        200,
        &json!({ "stored": stored, "rejected": rejected }).to_string(),
    )
}

/// `POST /api/sync/blobs/fetch`: the rows asked for that this client may
/// see: its declared platforms, and for a match or league an enabled deck.
fn fetch(state: &CloudState, body: &Value) -> Reply {
    let sync_type = body["type"].as_str().unwrap_or_default().to_string();
    let platforms = declared(body);
    let mut blobs = Vec::new();
    for client_id in texts(&body["client_ids"]) {
        let Some(row) = state.rows.get(&(sync_type.clone(), client_id.clone())) else {
            continue;
        };
        if !served(&row.platform, &platforms) {
            continue;
        }
        // A match or league of a deck that is off is withheld.
        if (sync_type == "match" || sync_type == "league") && !deck_enabled(state, row) {
            continue;
        }
        let data = base64(&mymtgo_api::gzip_blob(&row.canonical));
        let mut blob = json!({ "client_id": client_id, "hash": row.hash, "data": data });
        if !state.old_api {
            // the API sends the row's `updated_at` with its bytes.
            blob["updated_at"] = json!(row.updated_at);
        }
        blobs.push(blob);
    }
    Reply::json(200, &json!({ "blobs": blobs }).to_string())
}

/// `GET /api/sync/status`: counts per type of the rows the query's
/// `platforms[]` may see.
fn status(state: &CloudState, target: &str) -> Reply {
    let query = target.split_once('?').map(|(_, query)| query).unwrap_or("");
    let declared: Vec<String> = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .filter(|(key, _)| *key == "platforms%5B%5D" || *key == "platforms[]")
        .map(|(_, value)| value.to_string())
        .collect();
    let count = |sync_type: &str| {
        state
            .rows
            .iter()
            .filter(|((kind, _), row)| kind == sync_type && served(&row.platform, &declared))
            .count()
    };
    let counts = json!({ "counts": {
        "match": count("match"),
        "deck": count("deck"),
        "league": count("league"),
    }});
    Reply::json(200, &counts.to_string())
}

/// A multipart body's parts by field name.
pub fn multipart_fields(request: &Recorded) -> BTreeMap<String, Vec<u8>> {
    let mut fields = BTreeMap::new();
    let Some(boundary) = request
        .header("content-type")
        .and_then(|value| value.split("boundary=").nth(1))
    else {
        return fields;
    };
    let delimiter = format!("--{boundary}");
    let body = &request.body;
    let mut starts = Vec::new();
    let mut at = 0;
    while let Some(offset) = find(&body[at..], delimiter.as_bytes()) {
        starts.push(at + offset);
        at += offset + delimiter.len();
    }
    for pair in starts.windows(2) {
        let part = &body[pair[0] + delimiter.len()..pair[1]];
        let part = part.strip_prefix(b"\r\n").unwrap_or(part);
        let part = part.strip_suffix(b"\r\n").unwrap_or(part);
        let Some(head_end) = find(part, b"\r\n\r\n") else {
            continue;
        };
        let head = String::from_utf8_lossy(&part[..head_end]).to_string();
        let Some(name) = head
            .split("name=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
        else {
            continue;
        };
        fields.insert(name.to_string(), part[head_end + 4..].to_vec());
    }
    fields
}

/// Standard base64 with padding (the app has no base64 dependency).
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * index)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
