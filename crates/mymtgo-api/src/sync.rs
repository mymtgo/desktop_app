//! Cross-device sync requests (survey section 12, 07 spec section 10):
//! every `/api/sync/*` call as data, its answers, and the 401 loop as a
//! pure state machine (0.x `SyncApi`). All class `Sync`, bearer only,
//! `Accept: application/json`, 30 s.

use std::fmt;
use std::io::Read;

use base64::Engine;
use flate2::read::GzDecoder;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::error::ApiError;
use crate::gate::RequestClass;
use crate::php::urlencode;
use crate::request::{ApiRequest, ApiResponse, AuthMode, Body, MIB, Method, file_part, gzip};

/// Blobs per upload request (PHP servers truncate multipart at 20 files).
pub const BLOBS_PER_REQUEST: usize = 20;
/// Compressed bytes per upload request at most.
pub const COMPRESSED_PER_REQUEST: u64 = 8 * MIB;
/// A blob compressed larger than this is not sent (`too_large_local`).
/// 2 MiB since cross-device sync of Endstep matches (about 0.9 MiB each);
/// the API takes the same.
pub const COMPRESSED_PER_BLOB: u64 = 2 * MIB;
/// Dirty entries per manifest call.
pub const MANIFEST_CHUNK: usize = 2000;
/// A fetched blob larger than this once unzipped is skipped (a hostile
/// answer must not exhaust memory).
pub const MAX_BLOB_BYTES: u64 = 64 * MIB;
/// The statuses `deck_slot_request`'s caller reads itself.
pub const DECK_SLOT_PASSTHROUGH: [u16; 1] = [422];
/// The platforms beyond MTGO this client can import, declared on every
/// manifest, fetch and status call (cross-device sync spec section 10). The
/// server serves Arena and Endstep rows only to a client that declares them;
/// an older client sends nothing and sees MTGO only.
pub const SUPPORTED_PLATFORMS: [&str; 2] = ["arena", "endstep"];
/// How much of a failed answer's body a failure message keeps.
const FAILURE_BODY_CHARS: usize = 500;

/// What a sync call is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SyncType {
    Deck,
    League,
    Match,
}

impl SyncType {
    pub fn as_str(self) -> &'static str {
        match self {
            SyncType::Deck => "deck",
            SyncType::League => "league",
            SyncType::Match => "match",
        }
    }
}

fn sync_request(method: Method, path: &str) -> ApiRequest {
    ApiRequest::new(method, path, RequestClass::Sync, AuthMode::Bearer)
}

fn with_json(mut request: ApiRequest, body: &Value) -> ApiRequest {
    request.body = Body::json(body);
    request
}

/// `GET /api/sync/status`: the server's counts per type, with the declared
/// platforms as `platforms[]` query parameters.
pub fn sync_status_request() -> ApiRequest {
    let mut request = sync_request(Method::Get, "/api/sync/status");
    for platform in SUPPORTED_PLATFORMS {
        request
            .query
            .push(("platforms[]".to_string(), platform.to_string()));
    }
    request
}

/// One dirty row in a manifest call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirtyEntry {
    pub client_id: String,
    pub hash: String,
    /// `Y-m-d\TH:i:s\Z`.
    pub updated_at: String,
}

/// `POST /api/sync/manifest` with `{type, since, cursor, device, known,
/// dirty, last, platforms}` (`platforms`: `SUPPORTED_PLATFORMS`). `cursor`
/// is the string an earlier manifest answer gave, sent back exactly as
/// received; the server uses it instead of `since` when it is given.
/// `device` is this device's id, so the server does not offer its own
/// uploads back. Both keys are always written, null when absent.
pub fn manifest_request(
    sync_type: SyncType,
    since: Option<&str>,
    cursor: Option<&str>,
    device: Option<&str>,
    known: &[String],
    dirty: &[DirtyEntry],
    last: bool,
) -> ApiRequest {
    let dirty: Vec<Value> = dirty
        .iter()
        .map(|entry| {
            json!({
                "client_id": entry.client_id,
                "hash": entry.hash,
                "updated_at": entry.updated_at,
            })
        })
        .collect();
    with_json(
        sync_request(Method::Post, "/api/sync/manifest"),
        &json!({
            "type": sync_type.as_str(),
            "since": since,
            "cursor": cursor,
            "device": device,
            "known": known,
            "dirty": dirty,
            "last": last,
            "platforms": SUPPORTED_PLATFORMS,
        }),
    )
}

/// A blob to upload: the canonical bundle gzipped and its metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobUpload {
    pub client_id: String,
    pub hash: String,
    pub updated_at: String,
    /// `gzip_blob` of the canonical JSON.
    pub gz: Vec<u8>,
    pub sidecar: Value,
}

/// The canonical bundle compressed as 0.x (`gzencode`, level 6).
pub fn gzip_blob(canonical: &str) -> Vec<u8> {
    gzip(canonical.as_bytes())
}

/// `POST /api/sync/blobs`, multipart around `boundary` (random, from the
/// app): the field `type`, the field `device` when given (the server
/// stores it as the row's uploader), then per blob `i` a file part `blobs[i]` named
/// `{client_id}.json.gz` and the fields `meta[i][client_id]`,
/// `meta[i][hash]`, `meta[i][updated_at]` and `meta[i][sidecar][..]`
/// (flattened PHP-bracket style, null leaves dropped).
pub fn blobs_request(
    sync_type: SyncType,
    device: Option<&str>,
    blobs: &[BlobUpload],
    boundary: &str,
) -> ApiRequest {
    let mut bytes = Vec::new();
    text_part(&mut bytes, boundary, "type", sync_type.as_str());
    if let Some(device) = device {
        text_part(&mut bytes, boundary, "device", device);
    }
    for (index, blob) in blobs.iter().enumerate() {
        file_part(
            &mut bytes,
            boundary,
            &format!("blobs[{index}]"),
            &format!("{}.json.gz", blob.client_id),
            "application/gzip",
            &blob.gz,
        );
        let meta = format!("meta[{index}]");
        text_part(
            &mut bytes,
            boundary,
            &format!("{meta}[client_id]"),
            &blob.client_id,
        );
        text_part(&mut bytes, boundary, &format!("{meta}[hash]"), &blob.hash);
        text_part(
            &mut bytes,
            boundary,
            &format!("{meta}[updated_at]"),
            &blob.updated_at,
        );
        let mut fields = Vec::new();
        flatten(&blob.sidecar, &format!("{meta}[sidecar]"), &mut fields);
        for (name, value) in fields {
            text_part(&mut bytes, boundary, &name, &value);
        }
    }
    bytes.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    let mut request = sync_request(Method::Post, "/api/sync/blobs");
    request.body = Body::Multipart {
        boundary: boundary.to_string(),
        bytes,
    };
    request
}

fn text_part(bytes: &mut Vec<u8>, boundary: &str, name: &str, value: &str) {
    let part = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
    );
    bytes.extend_from_slice(part.as_bytes());
}

/// 0.x `flattenForMultipart`: nested maps and lists as `prefix[key]`, null
/// leaves dropped, scalars as PHP casts them to strings (`true` is `1`,
/// `false` is empty).
fn flatten(value: &Value, prefix: &str, fields: &mut Vec<(String, String)>) {
    match value {
        Value::Null => {}
        Value::Bool(flag) => {
            fields.push((prefix.to_string(), if *flag { "1" } else { "" }.to_string()))
        }
        Value::Number(number) => fields.push((prefix.to_string(), number.to_string())),
        Value::String(text) => fields.push((prefix.to_string(), text.clone())),
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                flatten(item, &format!("{prefix}[{index}]"), fields);
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                flatten(item, &format!("{prefix}[{key}]"), fields);
            }
        }
    }
}

/// `POST /api/sync/blobs/fetch` with `{type, client_ids, platforms}` (ids
/// as strings; `platforms`: `SUPPORTED_PLATFORMS`).
pub fn fetch_request(sync_type: SyncType, client_ids: &[String]) -> ApiRequest {
    with_json(
        sync_request(Method::Post, "/api/sync/blobs/fetch"),
        &json!({
            "type": sync_type.as_str(),
            "client_ids": client_ids,
            "platforms": SUPPORTED_PLATFORMS,
        }),
    )
}

/// `GET /api/sync/decks/slots`.
pub fn slots_request() -> ApiRequest {
    sync_request(Method::Get, "/api/sync/decks/slots")
}

/// `PUT /api/sync/decks/{client_id}` with `{enabled, kind}`; the kind comes
/// from the client id (`limited_` prefix), never from the caller. Its
/// caller passes `DECK_SLOT_PASSTHROUGH` to `SyncAuth`.
pub fn deck_slot_request(client_id: &str, enabled: bool) -> ApiRequest {
    let kind = if client_id.starts_with("limited_") {
        "limited"
    } else {
        "constructed"
    };
    with_json(
        sync_request(
            Method::Put,
            &format!("/api/sync/decks/{}", urlencode(client_id)),
        ),
        &json!({ "enabled": enabled, "kind": kind }),
    )
}

/// The manifest's answer: what to upload and download (ids as strings: a
/// numeric deck id can come back as a JSON number) and the deck slots on
/// the final call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestAnswer {
    pub upload: Vec<String>,
    pub download: Vec<String>,
    /// Matches deleted on the website since `since` (every one when it is
    /// null), on the final call (league self-management spec 6.4).
    pub tombstones: Vec<String>,
    pub slots: Option<Slots>,
    /// The server's download cursor, kept exactly as sent so it can be
    /// sent back; `None` from an older server.
    pub cursor: Option<String>,
    /// The platforms the server took from this call's declaration
    /// (`platforms`); `None` from a server that does not confirm them,
    /// which may drop the `platform` of an upload.
    pub platforms: Option<Vec<String>>,
}

/// A client id as a string (numbers stringified); `None` for anything else.
fn id_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn ids(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(id_text).collect())
        .unwrap_or_default()
}

pub fn parse_manifest(response: &ApiResponse) -> Result<ManifestAnswer, ApiError> {
    let body: Value = response.json()?;
    Ok(ManifestAnswer {
        upload: ids(&body["upload"]),
        download: ids(&body["download"]),
        tombstones: ids(&body["tombstones"]),
        slots: slots_from_json(&body["slots"]),
        cursor: body["cursor"].as_str().map(str::to_string),
        platforms: body["platforms"].as_array().map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        }),
    })
}

/// The upload's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobsAnswer {
    pub stored: Vec<String>,
    /// `(client_id, reason)`.
    pub rejected: Vec<(String, String)>,
}

pub fn parse_blobs(response: &ApiResponse) -> Result<BlobsAnswer, ApiError> {
    let body: Value = response.json()?;
    let rejected = body["rejected"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let reason = item["reason"].as_str().unwrap_or_default().to_string();
                    id_text(&item["client_id"]).map(|id| (id, reason))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(BlobsAnswer {
        stored: ids(&body["stored"]),
        rejected,
    })
}

/// One fetched blob whose bytes hash to its `hash`: the canonical JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedBlob {
    pub client_id: String,
    pub hash: String,
    pub canonical: String,
    /// The row's `updated_at` on the server, as sent; `None` from an API
    /// that does not send it.
    pub updated_at: Option<String>,
}

/// The fetch's answer; it may hold fewer blobs than asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchAnswer {
    pub blobs: Vec<FetchedBlob>,
    /// The client ids returned with a blob that did not decode or hash to
    /// its `hash` (skipped, unlike an id the server did not return).
    pub unverified: Vec<String>,
}

/// The most decompressed bytes one fetch answer may hold: each blob is
/// capped at `MAX_BLOB_BYTES`, and this caps their sum, so an answer full of
/// gzip bombs cannot exhaust memory.
pub const MAX_FETCH_BYTES: u64 = 128 * MIB;

/// Each blob base64-decoded, gunzipped and checked: its sha256 must equal
/// its `hash`, else it is skipped (0.x `SyncRunner` pull) and its client id
/// listed as unverified. Only the ids in `requested` are decoded, each once;
/// once the answer's decoded bytes reach `MAX_FETCH_BYTES` the rest is left
/// out (neither kept nor unverified), so it is asked for again.
pub fn parse_fetch(response: &ApiResponse, requested: &[String]) -> Result<FetchAnswer, ApiError> {
    parse_fetch_within(response, requested, MAX_FETCH_BYTES)
}

/// `parse_fetch` with its byte budget given.
fn parse_fetch_within(
    response: &ApiResponse,
    requested: &[String],
    budget: u64,
) -> Result<FetchAnswer, ApiError> {
    let body: Value = response.json()?;
    let mut blobs = Vec::new();
    let mut unverified = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut left = budget;
    for item in body["blobs"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        let Some(client_id) = id_text(&item["client_id"]) else {
            continue;
        };
        if !requested.contains(&client_id) || seen.contains(&client_id) {
            continue;
        }
        seen.push(client_id.clone());
        match verified_blob(item, left) {
            Decoded::Good(blob) => {
                left -= blob.canonical.len() as u64;
                blobs.push(blob);
            }
            Decoded::Bad => unverified.push(client_id),
            Decoded::OverBudget => break,
        }
    }
    Ok(FetchAnswer { blobs, unverified })
}

/// What decoding one fetched blob gave.
enum Decoded {
    Good(FetchedBlob),
    /// It did not decode, was too big on its own, or did not hash to its
    /// `hash`.
    Bad,
    /// It would take the answer past its byte budget.
    OverBudget,
}

fn verified_blob(item: &Value, budget: u64) -> Decoded {
    let Some(client_id) = id_text(&item["client_id"]) else {
        return Decoded::Bad;
    };
    let Some(hash) = item["hash"].as_str() else {
        return Decoded::Bad;
    };
    let Some(gz) = item["data"]
        .as_str()
        .and_then(|data| base64::engine::general_purpose::STANDARD.decode(data).ok())
    else {
        return Decoded::Bad;
    };
    let limit = MAX_BLOB_BYTES.min(budget);
    let mut bytes = Vec::new();
    if GzDecoder::new(gz.as_slice())
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return Decoded::Bad;
    }
    if bytes.len() as u64 > limit {
        // Too big on its own is a bad blob; too big only for what is left
        // of the answer is asked for again (the first blob of an answer
        // always has the full MAX_BLOB_BYTES, so a pull always progresses).
        return if limit == MAX_BLOB_BYTES {
            Decoded::Bad
        } else {
            Decoded::OverBudget
        };
    }
    let digest: String = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if digest != hash {
        return Decoded::Bad;
    }
    match String::from_utf8(bytes) {
        Ok(canonical) => Decoded::Good(FetchedBlob {
            client_id,
            hash: hash.to_string(),
            canonical,
            updated_at: item["updated_at"].as_str().map(str::to_string),
        }),
        Err(_) => Decoded::Bad,
    }
}

/// The account's plan: free unless the server says exactly `supporter`
/// (fails closed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Free,
    Supporter,
}

/// One deck in the server's slot ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotDeck {
    pub client_id: String,
    pub enabled_at: Option<String>,
    pub disabled_at: Option<String>,
    pub frees_at: Option<String>,
}

/// The deck sync slots (the settings row `sync_slots`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slots {
    /// `None`: unlimited.
    pub limit: Option<u32>,
    pub used: u32,
    pub tier: Tier,
    /// `None` when the answer had no deck list.
    pub decks: Option<Vec<SlotDeck>>,
}

/// Slots from their JSON (an answer or the stored row); `None` when it is
/// not a map.
pub fn slots_from_json(value: &Value) -> Option<Slots> {
    let map = value.as_object()?;
    let count = |key: &str| {
        map.get(key)
            .and_then(Value::as_u64)
            .and_then(|count| u32::try_from(count).ok())
    };
    let text = |deck: &Value, key: &str| deck[key].as_str().map(str::to_string);
    let decks = map.get("decks").and_then(Value::as_array).map(|items| {
        items
            .iter()
            .filter_map(|deck| {
                Some(SlotDeck {
                    client_id: id_text(&deck["client_id"])?,
                    enabled_at: text(deck, "enabled_at"),
                    disabled_at: text(deck, "disabled_at"),
                    frees_at: text(deck, "frees_at"),
                })
            })
            .collect()
    });
    let tier = if map.get("tier").and_then(Value::as_str) == Some("supporter") {
        Tier::Supporter
    } else {
        Tier::Free
    };
    Some(Slots {
        limit: count("limit"),
        used: count("used").unwrap_or(0),
        tier,
        decks,
    })
}

/// The stored form of the slots.
pub fn slots_json(slots: &Slots) -> Value {
    let mut map = Map::new();
    map.insert("limit".to_string(), json!(slots.limit));
    map.insert("used".to_string(), json!(slots.used));
    let tier = match slots.tier {
        Tier::Free => "free",
        Tier::Supporter => "supporter",
    };
    map.insert("tier".to_string(), json!(tier));
    if let Some(decks) = &slots.decks {
        let decks: Vec<Value> = decks
            .iter()
            .map(|deck| {
                json!({
                    "client_id": deck.client_id,
                    "enabled_at": deck.enabled_at,
                    "disabled_at": deck.disabled_at,
                    "frees_at": deck.frees_at,
                })
            })
            .collect();
        map.insert("decks".to_string(), Value::Array(decks));
    }
    Value::Object(map)
}

pub fn parse_slots(response: &ApiResponse) -> Result<Slots, ApiError> {
    let body: Value = response.json()?;
    slots_from_json(&body)
        .ok_or_else(|| ApiError::Malformed("the slots answer is not a map".to_string()))
}

/// Why a deck could not be switched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotError {
    /// 422 `slot_limit`: the free slot is taken or cooling down.
    SlotLimit {
        limit: u32,
        used: u32,
        frees_at: Option<String>,
    },
    /// 422 `limited_requires_supporter`.
    LimitedRequiresSupporter,
    /// Any other refusal or an unreadable answer, as 0.x words it.
    Failed(String),
}

/// The answer to `deck_slot_request` (a 2xx, or a 422 passed through).
pub fn parse_deck_slot(response: &ApiResponse) -> Result<Slots, SlotError> {
    if response.status == 422 {
        let body: Value = response.json().unwrap_or(Value::Null);
        let count = |key: &str| {
            body[key]
                .as_u64()
                .and_then(|count| u32::try_from(count).ok())
                .unwrap_or(0)
        };
        return Err(match body["error"].as_str() {
            Some("slot_limit") => SlotError::SlotLimit {
                limit: count("limit"),
                used: count("used"),
                frees_at: body["frees_at"].as_str().map(str::to_string),
            },
            Some("limited_requires_supporter") => SlotError::LimitedRequiresSupporter,
            _ => SlotError::Failed(failure_message(response.status, &response.body)),
        });
    }
    if !response.is_success() {
        return Err(SlotError::Failed(failure_message(
            response.status,
            &response.body,
        )));
    }
    parse_slots(response).map_err(|error| SlotError::Failed(error.to_string()))
}

/// 0.x `SyncApi::send`'s failure: "Sync API request failed with status N:
/// <500 characters of the body>".
fn failure_message(status: u16, body: &[u8]) -> String {
    let text: String = String::from_utf8_lossy(body)
        .chars()
        .take(FAILURE_BODY_CHARS)
        .collect();
    format!("Sync API request failed with status {status}: {text}")
}

/// What a token refresh did (Q3's `refresh`). Any other refresh failure
/// ends the send with its own error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshResult {
    /// Refreshed, or another caller already replaced the token that failed.
    Refreshed,
    /// The refresh token was refused; the credentials are cleared.
    InvalidGrant,
}

/// A sync answer the caller cannot read: a failed status, as 0.x words it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncFailure {
    pub status: u16,
    pub message: String,
}

impl fmt::Display for SyncFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// The next thing a send does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncAuthStep {
    /// Send the request again with the current bearer.
    Send,
    /// Refresh the token, then `after_refresh`.
    Refresh,
    /// The answer stands: the caller reads it (`sync_answer` for sync).
    Done,
    /// The refresh token was refused: signed out.
    SignedOut,
}

/// 0.x `SyncApi::send`'s 401 loop: a 401 refreshes and retries once; a
/// second 401 refreshes again (so `invalid_grant` signs out in one place)
/// and then stands, because the device is provably still linked.
#[derive(Debug, Clone, Default)]
pub struct SyncAuth {
    unauthorized: u8,
}

impl SyncAuth {
    /// After an answer: a 401 asks for a refresh, anything else stands.
    pub fn after_response(&mut self, response: &ApiResponse) -> SyncAuthStep {
        if response.status == 401 {
            self.unauthorized = self.unauthorized.saturating_add(1);
            return SyncAuthStep::Refresh;
        }
        SyncAuthStep::Done
    }

    /// After the refresh a 401 asked for.
    pub fn after_refresh(&mut self, refresh: RefreshResult) -> SyncAuthStep {
        match refresh {
            RefreshResult::InvalidGrant => SyncAuthStep::SignedOut,
            RefreshResult::Refreshed if self.unauthorized <= 1 => SyncAuthStep::Send,
            RefreshResult::Refreshed => SyncAuthStep::Done,
        }
    }
}

/// Whether a sync caller may read an answer the 401 loop let stand: a 2xx
/// or one of its `passthrough` statuses. A 401 that stands came after a
/// successful refresh; any other failed status fails with 0.x's message.
pub fn sync_answer(response: &ApiResponse, passthrough: &[u16]) -> Result<(), SyncFailure> {
    if response.is_success() || passthrough.contains(&response.status) {
        return Ok(());
    }
    let message = if response.status == 401 {
        "Sync API request failed with status 401 after a successful token refresh.".to_string()
    } else {
        failure_message(response.status, &response.body)
    };
    Err(SyncFailure {
        status: response.status,
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::RequestClass;
    use crate::request::{AuthMode, Body, Credentials, Method, wire_headers};
    use crate::test_support::{gunzip, response};
    use serde_json::{Value, json};
    use std::io::Write;

    fn json_body(request: &ApiRequest) -> Value {
        let Body::Json(bytes) = &request.body else {
            panic!("a JSON body: {:?}", request.body);
        };
        serde_json::from_slice(bytes).expect("json")
    }

    fn assert_sync(request: &ApiRequest, method: Method, path: &str) {
        assert_eq!(request.method, method);
        assert_eq!(request.path, path);
        assert_eq!(request.class, RequestClass::Sync);
        assert_eq!(request.auth, AuthMode::Bearer);
        assert_eq!(
            (request.timeout_ms, request.connect_timeout_ms),
            (30_000, 10_000)
        );
    }

    // 0.x SyncApiTest: "sends the manifest body shape with the bearer
    // header".
    #[test]
    fn sends_the_manifest_body_shape_with_the_bearer_header() {
        let dirty = [DirtyEntry {
            client_id: "dirty-1".to_string(),
            hash: "hash-1".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }];
        let request = manifest_request(
            SyncType::Match,
            Some("2026-01-01T00:00:00Z"),
            Some("2026-01-01T00:00:00.123456Z"),
            Some("qdevice"),
            &["known-1".to_string()],
            &dirty,
            true,
        );
        assert_sync(&request, Method::Post, "/api/sync/manifest");
        assert_eq!(
            json_body(&request),
            json!({
                "type": "match",
                "since": "2026-01-01T00:00:00Z",
                "cursor": "2026-01-01T00:00:00.123456Z",
                "device": "qdevice",
                "known": ["known-1"],
                "dirty": [{"client_id": "dirty-1", "hash": "hash-1", "updated_at": "2026-01-01T00:00:00Z"}],
                "last": true,
                "platforms": ["arena", "endstep"],
            })
        );
        let credentials = Credentials {
            device_id: "qdevice".to_string(),
            api_key: Some("qkey".to_string()),
            access_token: Some("access-token".to_string()),
        };
        let headers = wire_headers(&request, &credentials).expect("headers");
        assert!(headers.contains(&(
            "Authorization".to_string(),
            "Bearer access-token".to_string()
        )));
        assert!(headers.contains(&("Accept".to_string(), "application/json".to_string())));
        assert!(!headers.iter().any(|(name, _)| name == "X-Api-Key"));
        // A full run sends neither `since` nor `cursor`, but always both keys.
        let full = manifest_request(SyncType::Deck, None, None, Some("qdevice"), &[], &[], false);
        let full = json_body(&full);
        assert_eq!(full["since"], Value::Null);
        assert_eq!(full["cursor"], Value::Null);
        assert!(full.as_object().expect("body").contains_key("cursor"));
        assert_eq!(full["device"], json!("qdevice"));
        assert_eq!(full["type"], json!("deck"));
        let anonymous = manifest_request(SyncType::Deck, None, None, None, &[], &[], false);
        let anonymous = json_body(&anonymous);
        assert!(anonymous.as_object().expect("body").contains_key("device"));
        assert_eq!(anonymous["device"], Value::Null);
    }

    #[test]
    fn a_manifest_answer_keeps_the_server_cursor_exactly_and_an_older_server_gives_none() {
        let answer = parse_manifest(&response(
            200,
            r#"{"upload":[],"download":[],"tombstones":[],"cursor":"2026-10-16T09:58:00.123456Z"}"#,
        ))
        .expect("parsed");
        assert_eq!(
            answer.cursor.as_deref(),
            Some("2026-10-16T09:58:00.123456Z")
        );
        let older = parse_manifest(&response(
            200,
            r#"{"upload":[],"download":[],"tombstones":[]}"#,
        ))
        .expect("parsed");
        assert_eq!(older.cursor, None);
        let null = parse_manifest(&response(
            200,
            r#"{"upload":[],"download":[],"tombstones":[],"cursor":null}"#,
        ))
        .expect("parsed");
        assert_eq!(null.cursor, None);
    }

    // An API that may drop `platform` never echoes it: the runner sends
    // Arena and Endstep rows only to one that names them.
    #[test]
    fn a_manifest_answer_reads_the_platforms_the_server_took() {
        let answer = parse_manifest(&response(
            200,
            r#"{"upload":[],"download":[],"tombstones":[],"platforms":["arena","endstep"]}"#,
        ))
        .expect("parsed");
        assert_eq!(
            answer.platforms,
            Some(vec!["arena".to_string(), "endstep".to_string()])
        );
        let none = parse_manifest(&response(
            200,
            r#"{"upload":[],"download":[],"tombstones":[],"platforms":[]}"#,
        ))
        .expect("parsed");
        assert_eq!(none.platforms, Some(Vec::new()));
        let older = parse_manifest(&response(
            200,
            r#"{"upload":[],"download":[],"tombstones":[]}"#,
        ))
        .expect("parsed");
        assert_eq!(older.platforms, None);
        let odd = parse_manifest(&response(
            200,
            r#"{"upload":[],"download":[],"tombstones":[],"platforms":["arena",["x"],7]}"#,
        ))
        .expect("parsed");
        assert_eq!(odd.platforms, Some(vec!["arena".to_string()]));
    }

    #[test]
    fn a_manifest_answer_stringifies_its_ids_and_reads_the_slots() {
        let answer = parse_manifest(&response(
            200,
            r#"{"upload":["a",12],"download":["b",345],"tombstones":["q-gone",77],"slots":{"limit":1,"used":0,"tier":"supporter","decks":[]}}"#,
        ))
        .expect("parsed");
        assert_eq!(answer.upload, vec!["a".to_string(), "12".to_string()]);
        assert_eq!(answer.download, vec!["b".to_string(), "345".to_string()]);
        assert_eq!(
            answer.tombstones,
            vec!["q-gone".to_string(), "77".to_string()]
        );
        let slots = answer.slots.expect("slots");
        assert_eq!(
            (slots.limit, slots.used, slots.tier),
            (Some(1), 0, Tier::Supporter)
        );
        assert_eq!(slots.decks, Some(Vec::new()));
        let bare = parse_manifest(&response(
            200,
            r#"{"upload":[],"download":[],"tombstones":[]}"#,
        ))
        .expect("parsed");
        assert_eq!(bare.slots, None);
        assert!(matches!(
            parse_manifest(&response(200, "nope")),
            Err(ApiError::Malformed(_))
        ));
    }

    fn after(auth: &mut SyncAuth, status: u16) -> SyncAuthStep {
        auth.after_response(&response(status, r#"{"message":"Unauthenticated."}"#))
    }

    // 0.x "refreshes the token once and retries after a single 401".
    #[test]
    fn refreshes_the_token_once_and_retries_after_a_single_401() {
        let mut auth = SyncAuth::default();
        assert_eq!(after(&mut auth, 401), SyncAuthStep::Refresh);
        assert_eq!(
            auth.after_refresh(RefreshResult::Refreshed),
            SyncAuthStep::Send
        );
        assert_eq!(after(&mut auth, 200), SyncAuthStep::Done);
    }

    // 0.x "throws NotLinked and clears credentials on a second consecutive
    // 401" (the refresh's invalid_grant clears them).
    #[test]
    fn a_second_401_whose_refresh_is_refused_is_signed_out() {
        let mut auth = SyncAuth::default();
        assert_eq!(after(&mut auth, 401), SyncAuthStep::Refresh);
        assert_eq!(
            auth.after_refresh(RefreshResult::Refreshed),
            SyncAuthStep::Send
        );
        assert_eq!(after(&mut auth, 401), SyncAuthStep::Refresh);
        assert_eq!(
            auth.after_refresh(RefreshResult::InvalidGrant),
            SyncAuthStep::SignedOut
        );
        // A first refresh refused the same way.
        let mut auth = SyncAuth::default();
        after(&mut auth, 401);
        assert_eq!(
            auth.after_refresh(RefreshResult::InvalidGrant),
            SyncAuthStep::SignedOut
        );
    }

    // 0.x "throws a plain RuntimeException (not NotLinked) on a second 401
    // when the second refresh succeeds".
    #[test]
    fn a_second_401_after_a_successful_refresh_is_a_plain_failure() {
        let mut auth = SyncAuth::default();
        after(&mut auth, 401);
        auth.after_refresh(RefreshResult::Refreshed);
        assert_eq!(after(&mut auth, 401), SyncAuthStep::Refresh);
        assert_eq!(
            auth.after_refresh(RefreshResult::Refreshed),
            SyncAuthStep::Done,
            "the 401 stands"
        );
        assert_eq!(
            sync_answer(&response(401, "{}"), &[]),
            Err(SyncFailure {
                status: 401,
                message:
                    "Sync API request failed with status 401 after a successful token refresh."
                        .to_string()
            })
        );
    }

    // 0.x "throws a RuntimeException carrying the status on a 429", "on a
    // 5xx", and "on a 422 instead of returning the validation body as
    // success".
    #[test]
    fn any_other_failed_status_is_a_failure_carrying_it() {
        for (status, body) in [
            (429, r#"{"message":"Too Many Requests"}"#),
            (503, r#"{"message":"Server Error"}"#),
            (
                422,
                r#"{"message":"The dirty field is invalid.","errors":[]}"#,
            ),
        ] {
            let mut auth = SyncAuth::default();
            assert_eq!(
                auth.after_response(&response(status, body)),
                SyncAuthStep::Done,
                "{status}: not the loop's"
            );
            assert_eq!(
                sync_answer(&response(status, body), &[]),
                Err(SyncFailure {
                    status,
                    message: format!("Sync API request failed with status {status}: {body}"),
                }),
                "{status}"
            );
        }
        let long = "é".repeat(600);
        let Err(SyncFailure { message, .. }) = sync_answer(&response(500, &long), &[]) else {
            panic!("a failure");
        };
        assert_eq!(
            message.chars().count(),
            "Sync API request failed with status 500: ".len() + 500
        );
        // A passthrough status is the caller's.
        assert_eq!(sync_answer(&response(422, "{}"), &[422]), Ok(()));
        assert_eq!(sync_answer(&response(204, ""), &[]), Ok(()));
    }

    // 0.x "uploads blobs as multipart carrying meta fields and a file part
    // per blob".
    #[test]
    fn uploads_blobs_as_multipart_carrying_meta_fields_and_a_file_part_per_blob() {
        let blobs = [
            BlobUpload {
                client_id: "client-1".to_string(),
                hash: "hash-1".to_string(),
                updated_at: "2026-01-01T00:00:00Z".to_string(),
                gz: b"gzipped-bytes".to_vec(),
                sidecar: json!({"format": "Standard", "games_won": 2, "dropped": null,
                    "nested": {"a": "x", "b": null, "list": [1, true, false]}}),
            },
            BlobUpload {
                client_id: "client-2".to_string(),
                hash: "hash-2".to_string(),
                updated_at: "2026-01-02T00:00:00Z".to_string(),
                gz: vec![0, 159, 255],
                sidecar: json!({}),
            },
        ];
        let request = blobs_request(SyncType::Match, None, &blobs, "qboundary");
        assert_sync(&request, Method::Post, "/api/sync/blobs");
        let Body::Multipart { boundary, bytes } = &request.body else {
            panic!("multipart: {:?}", request.body);
        };
        assert_eq!(boundary, "qboundary");
        let text = String::from_utf8_lossy(bytes);
        let field = |name: &str, value: &str| {
            format!(
                "--qboundary\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
        };
        for expected in [
            field("type", "match"),
            field("meta[0][client_id]", "client-1"),
            field("meta[0][hash]", "hash-1"),
            field("meta[0][updated_at]", "2026-01-01T00:00:00Z"),
            field("meta[0][sidecar][format]", "Standard"),
            field("meta[0][sidecar][games_won]", "2"),
            field("meta[0][sidecar][nested][a]", "x"),
            field("meta[0][sidecar][nested][list][0]", "1"),
            field("meta[0][sidecar][nested][list][1]", "1"),
            field("meta[0][sidecar][nested][list][2]", ""),
            field("meta[1][client_id]", "client-2"),
        ] {
            assert!(text.contains(&expected), "{expected:?} in {text}");
        }
        assert!(!text.contains("dropped") && !text.contains("[nested][b]"));
        assert!(text.contains(
            "--qboundary\r\nContent-Disposition: form-data; name=\"blobs[0]\"; filename=\"client-1.json.gz\"\r\nContent-Type: application/gzip\r\n\r\ngzipped-bytes\r\n"
        ));
        let file_two = b"filename=\"client-2.json.gz\"\r\nContent-Type: application/gzip\r\n\r\n\x00\x9f\xff\r\n";
        assert!(
            bytes
                .windows(file_two.len())
                .any(|window| window == file_two)
        );
        assert!(text.ends_with("--qboundary--\r\n"));
        assert!(!text.contains("name=\"device\""));
    }

    #[test]
    fn an_upload_names_its_device_right_after_the_type() {
        let request = blobs_request(SyncType::Deck, Some("qdevice"), &[], "qboundary");
        let Body::Multipart { bytes, .. } = &request.body else {
            panic!("multipart: {:?}", request.body);
        };
        let text = String::from_utf8_lossy(bytes);
        assert!(text.starts_with(
            "--qboundary\r\nContent-Disposition: form-data; name=\"type\"\r\n\r\ndeck\r\n--qboundary\r\nContent-Disposition: form-data; name=\"device\"\r\n\r\nqdevice\r\n"
        ), "{text}");
    }

    #[test]
    fn a_blob_is_gzip_level_6_of_the_canonical_bytes() {
        let gz = gzip_blob(r#"{"a":1}"#);
        assert_eq!(gunzip(&gz), br#"{"a":1}"#);
        let mut expected = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
        expected.write_all(br#"{"a":1}"#).expect("write");
        assert_eq!(gz, expected.finish().expect("gzip"));
    }

    #[test]
    fn a_blobs_answer_lists_stored_and_rejected() {
        let answer = parse_blobs(&response(
            200,
            r#"{"stored":["client-1",7],"rejected":[{"client_id":"client-2","reason":"hash_mismatch"}]}"#,
        ))
        .expect("parsed");
        assert_eq!(answer.stored, vec!["client-1".to_string(), "7".to_string()]);
        assert_eq!(
            answer.rejected,
            vec![("client-2".to_string(), "hash_mismatch".to_string())]
        );
    }

    // 0.x "fetches blobs with the given client ids" (ids always strings).
    #[test]
    fn fetches_blobs_with_the_given_client_ids() {
        let request = fetch_request(
            SyncType::Match,
            &["client-1".to_string(), "12345".to_string()],
        );
        assert_sync(&request, Method::Post, "/api/sync/blobs/fetch");
        assert_eq!(request.body_limit, 64 * crate::request::MIB);
        assert_eq!(
            json_body(&request),
            json!({
                "type": "match",
                "client_ids": ["client-1", "12345"],
                "platforms": ["arena", "endstep"],
            })
        );
    }

    // Cross-device sync spec 10: the server serves Arena and Endstep rows
    // only to a client that declares them; absent means MTGO only.
    #[test]
    fn manifest_request_declares_platforms() {
        for sync_type in [SyncType::Deck, SyncType::League, SyncType::Match] {
            for last in [false, true] {
                let request = manifest_request(sync_type, None, None, None, &[], &[], last);
                assert_eq!(
                    json_body(&request)["platforms"],
                    json!(["arena", "endstep"]),
                    "{sync_type:?} last {last}"
                );
            }
        }
    }

    #[test]
    fn fetch_request_declares_platforms() {
        for sync_type in [SyncType::Deck, SyncType::League, SyncType::Match] {
            let request = fetch_request(sync_type, &["client-1".to_string()]);
            assert_eq!(
                json_body(&request)["platforms"],
                json!(["arena", "endstep"]),
                "{sync_type:?}"
            );
        }
    }

    // The status counts take the declaration as a query parameter (a GET
    // has no body): `platforms[]=arena&platforms[]=endstep`.
    #[test]
    fn status_request_declares_platforms() {
        let request = sync_status_request();
        assert_eq!(
            request.query,
            vec![
                ("platforms[]".to_string(), "arena".to_string()),
                ("platforms[]".to_string(), "endstep".to_string()),
            ]
        );
        assert_eq!(request.body, Body::Empty);
        assert_eq!(SUPPORTED_PLATFORMS, ["arena", "endstep"]);
    }

    fn fetched(client_id: &str, canonical: &str, hash: &str) -> Value {
        use base64::Engine;
        json!({
            "client_id": client_id,
            "hash": hash,
            "data": base64::engine::general_purpose::STANDARD.encode(gzip_blob(canonical)),
        })
    }

    fn sha256_hex(text: &str) -> String {
        use sha2::Digest;
        sha2::Sha256::digest(text.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    // The server's `updated_at` of a fetched row: an Arena run sent after a
    // pull must be newer than the copy it pulled. Absent from an older API.
    #[test]
    fn a_fetched_blob_keeps_the_servers_updated_at() {
        let good = r#"{"league":{}}"#;
        let mut item = fetched("client-1", good, &sha256_hex(good));
        item["updated_at"] = json!("2026-11-19T10:00:03Z");
        let body = json!({"blobs": [item, fetched("client-2", good, &sha256_hex(good))]});
        let asked = ["client-1", "client-2"].map(String::from);
        let answer = parse_fetch(&response(200, &body.to_string()), &asked).expect("parsed");
        let times: Vec<Option<String>> = answer
            .blobs
            .iter()
            .map(|blob| blob.updated_at.clone())
            .collect();
        assert_eq!(times, vec![Some("2026-11-19T10:00:03Z".to_string()), None]);
    }

    // 0.x pull: base64, gunzip, and the sha256 must equal the hash, else the
    // blob is skipped.
    #[test]
    fn a_fetched_blob_is_kept_only_when_its_hash_matches() {
        let good = r#"{"deck":{"mtgo_id":"1"}}"#;
        let body = json!({"blobs": [
            fetched("client-1", good, &sha256_hex(good)),
            fetched("client-2", good, "0000"),
            {"client_id": 77, "hash": sha256_hex(good), "data": "not base64!"},
            {"client_id": "client-4", "hash": sha256_hex(good), "data": "aGVsbG8="},
        ]});
        let asked = ["client-1", "client-2", "77", "client-4"].map(String::from);
        let answer = parse_fetch(&response(200, &body.to_string()), &asked).expect("parsed");
        assert_eq!(
            answer.blobs,
            vec![FetchedBlob {
                client_id: "client-1".to_string(),
                hash: sha256_hex(good),
                canonical: good.to_string(),
                updated_at: None,
            }]
        );
        assert_eq!(
            answer.unverified,
            vec![
                "client-2".to_string(),
                "77".to_string(),
                "client-4".to_string()
            ],
            "returned, but not a good blob: asked again at the next run"
        );
    }

    // A hostile or broken server cannot make the client decode blobs it did
    // not ask for, or the same blob twice.
    #[test]
    fn a_fetched_blob_not_asked_for_or_repeated_is_dropped_undecoded() {
        let good = r#"{"deck":{"mtgo_id":"1"}}"#;
        let body = json!({"blobs": [
            fetched("client-1", good, &sha256_hex(good)),
            fetched("client-9", good, &sha256_hex(good)),
            fetched("client-1", good, &sha256_hex(good)),
            fetched("client-8", good, "0000"),
        ]});
        let asked = ["client-1".to_string()];
        let answer = parse_fetch(&response(200, &body.to_string()), &asked).expect("parsed");
        assert_eq!(answer.blobs.len(), 1);
        assert_eq!(answer.blobs[0].client_id, "client-1");
        assert!(
            answer.unverified.is_empty(),
            "ids not asked for are not listed"
        );
    }

    // A gzip bomb per blob stays under MAX_BLOB_BYTES; many of them in one
    // answer must not add up to an unbounded allocation.
    #[test]
    fn a_fetch_answer_stops_decoding_at_its_byte_budget() {
        let first = r#"{"deck":{"mtgo_id":"0000000001"}}"#;
        let second = r#"{"deck":{"mtgo_id":"0000000002"}}"#;
        let body = json!({"blobs": [
            fetched("client-1", first, &sha256_hex(first)),
            fetched("client-2", second, &sha256_hex(second)),
        ]});
        let asked = ["client-1".to_string(), "client-2".to_string()];
        let budget = (first.len() + second.len() - 1) as u64;
        let answer =
            parse_fetch_within(&response(200, &body.to_string()), &asked, budget).expect("parsed");
        assert_eq!(answer.blobs.len(), 1, "the second blob is over the budget");
        assert_eq!(answer.blobs[0].client_id, "client-1");
        assert!(
            answer.unverified.is_empty(),
            "a blob over the budget is not unverified: it is asked for again"
        );
        let roomy = (first.len() + second.len()) as u64;
        let both =
            parse_fetch_within(&response(200, &body.to_string()), &asked, roomy).expect("parsed");
        assert_eq!(both.blobs.len(), 2);
    }

    // 0.x "reads deck slots".
    #[test]
    fn reads_deck_slots() {
        assert_sync(&slots_request(), Method::Get, "/api/sync/decks/slots");
        let slots =
            parse_slots(&response(200, r#"{"limit":1,"used":0,"decks":[]}"#)).expect("slots");
        assert_eq!(
            slots,
            Slots {
                limit: Some(1),
                used: 0,
                tier: Tier::Free,
                decks: Some(Vec::new()),
            }
        );
        let unlimited = parse_slots(&response(
            200,
            r#"{"limit":null,"used":3,"tier":"Supporter","decks":[{"client_id":12345,"enabled_at":"x","disabled_at":null,"frees_at":"y"}]}"#,
        ))
        .expect("slots");
        assert_eq!(unlimited.limit, None);
        assert_eq!(unlimited.tier, Tier::Free, "only exactly supporter");
        assert_eq!(
            unlimited.decks,
            Some(vec![SlotDeck {
                client_id: "12345".to_string(),
                enabled_at: Some("x".to_string()),
                disabled_at: None,
                frees_at: Some("y".to_string()),
            }])
        );
        let status = sync_status_request();
        assert_sync(&status, Method::Get, "/api/sync/status");
    }

    // 0.x "enables a deck and returns the slot summary".
    #[test]
    fn enables_a_deck_and_returns_the_slot_summary() {
        let request = deck_slot_request("12345", true);
        assert_sync(&request, Method::Put, "/api/sync/decks/12345");
        assert_eq!(json_body(&request)["enabled"], json!(true));
        let answer = parse_deck_slot(&response(
            201,
            r#"{"limit":1,"used":1,"decks":[{"client_id":"12345","enabled_at":"x","disabled_at":null,"frees_at":null}]}"#,
        ));
        assert_eq!(answer.expect("slots").used, 1);
    }

    // 0.x "derives kind from the client id itself rather than accepting one".
    #[test]
    fn derives_kind_from_the_client_id_itself() {
        assert_eq!(
            json_body(&deck_slot_request("12345", true))["kind"],
            json!("constructed")
        );
        assert_eq!(
            json_body(&deck_slot_request("limited_abc123", false)),
            json!({"enabled": false, "kind": "limited"})
        );
    }

    // 0.x "throws SlotLimitException on a slot_limit 422", and the supporter
    // refusal.
    #[test]
    fn a_slot_limit_and_a_supporter_refusal_are_typed() {
        assert_eq!(
            parse_deck_slot(&response(
                422,
                r#"{"error":"slot_limit","limit":1,"used":1,"frees_at":"2026-10-10T12:00:00Z"}"#
            )),
            Err(SlotError::SlotLimit {
                limit: 1,
                used: 1,
                frees_at: Some("2026-10-10T12:00:00Z".to_string()),
            })
        );
        assert_eq!(
            parse_deck_slot(&response(422, r#"{"error":"limited_requires_supporter"}"#)),
            Err(SlotError::LimitedRequiresSupporter)
        );
        assert_eq!(
            parse_deck_slot(&response(422, r#"{"message":"invalid"}"#)),
            Err(SlotError::Failed(
                r#"Sync API request failed with status 422: {"message":"invalid"}"#.to_string()
            ))
        );
        assert_eq!(DECK_SLOT_PASSTHROUGH, [422]);
    }

    // 0.x "stores and reads sync slots in app settings": the settings row
    // `sync_slots` holds them as JSON.
    #[test]
    fn slots_round_trip_through_their_stored_json() {
        let slots = Slots {
            limit: Some(1),
            used: 1,
            tier: Tier::Supporter,
            decks: Some(vec![SlotDeck {
                client_id: "12345".to_string(),
                enabled_at: Some("x".to_string()),
                disabled_at: None,
                frees_at: None,
            }]),
        };
        assert_eq!(slots_from_json(&slots_json(&slots)), Some(slots));
        assert_eq!(slots_from_json(&json!("nope")), None);
    }

    #[test]
    fn the_limits_are_0x_values() {
        assert_eq!(BLOBS_PER_REQUEST, 20);
        assert_eq!(COMPRESSED_PER_REQUEST, 8 * 1024 * 1024);
        assert_eq!(COMPRESSED_PER_BLOB, 2 * 1024 * 1024);
        assert_eq!(MANIFEST_CHUNK, 2000);
    }
}
