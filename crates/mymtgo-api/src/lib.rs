//! The MyMTGO API, without any IO: configuration, the gate that decides
//! whether a request may leave the machine, request and response shapes, the
//! auth headers and PHP-compatible encodings. The app's `api` module sends
//! what this crate builds (07 spec sections 3 and 5).
//!
//! Like a Laravel service class that only builds `Http::` calls and never
//! runs them: everything here is plain data in, plain data out, so it is
//! tested without a network.

pub mod archetypes;
pub mod attest;
pub mod card_stats;
pub mod cards;
pub mod config;
pub mod device;
pub mod error;
pub mod gate;
pub mod loose;
pub mod metagame;
pub mod oauth;
pub mod observations;
pub mod offline;
pub mod outbox;
pub mod overlay;
pub mod php;
pub mod rate_limit;
pub mod replays;
pub mod report;
pub mod request;
pub mod schedule;
pub mod scouting;
pub mod status;
pub mod sync;
pub mod tournaments;

#[cfg(test)]
mod test_support;

pub use archetypes::{
    ArchetypeDeck, ArchetypeEstimate, CatalogArchetype, DECKLIST_FAILED, DEFINITIONS_PER_CHECK,
    DecklistCard, DefinitionsCheck, EstimateCard, EstimateCardId, EstimateOutcome, catalog_request,
    decklist_request, decklists_request, definitions_check_request, estimate_request,
    is_not_deployed, join_definitions_checks, offline_catalog_request,
    offline_definitions_check_request, parse_catalog, parse_decklist, parse_decklists,
    parse_definitions_check, parse_definitions_check_counted, parse_estimate,
};
pub use attest::{
    AttestAnswer, AttestSkip, Attested, KnownAccount, RETRY_DELAYS_MS, attest_request, attest_skip,
    attested_json, owed_accounts, parse_attest, parse_attested, record_attested,
};
pub use card_stats::{
    CAST_METHODS, CardStatsCard, CardStatsGame, add_printing, card_stats_payload,
    card_stats_requests, card_stats_row, card_stats_skip_reason,
};
pub use cards::{
    CardAnswer, CardsRequest, EndstepLookup, LookupPass, RESOLVE_FAILED, Resolution, Resolutions,
    ResolvedCard, Unresolved, UnresolvedCard, cards_request, match_answers, parse_cards,
    parse_resolved, plan_card_lookups, resolve_request, run_card_passes,
};
pub use config::{
    ApiConfig, ConfigError, parse_env_file, process_override_allowed, resolve_config,
    resolve_config_for_build,
};
pub use device::{
    KEY_LIFETIME_MS, key_needs_refresh, parse_register, register_device_request, uuid_v4,
};
pub use error::{ApiError, ConnectKind};
pub use gate::{GateDecision, GateState, RequestClass, gate};
pub use metagame::{
    GlobalCard, GlobalShare, METAGAME_WINDOWS, MetagameShares, format_slug, metagame_request,
    parse_metagame,
};
pub use oauth::{
    Callback, LOOPBACK_PATH, Pkce, TokenError, Tokens, authorize_url, exchange_request,
    loopback_redirect, needs_refresh, parse_callback, parse_token, refresh_request,
};
pub use observations::{
    MAX_PAYLOAD_BYTES, MAX_REQUEST_BYTES, OBSERVATION_KINDS, TournamentEvent, observation_payload,
    observation_row, observation_skip_reason, observations_requests, php_encoded_len,
    refused_observations,
};
pub use offline::{
    KEPT_0X_SETTINGS, OFFLINE_MODE, OFFLINE_MODE_LOCKED_UNTIL, cooldown_message, is_locked,
    lock_until, read_locked_until, read_offline, seed_offline_rows,
};
pub use outbox::{
    CLAIM_LIMIT, ClaimedRow, Failure, MAX_ATTEMPTS, NewOutboxRow, OutboxKind, REAP_AFTER_MS,
    REFUSED_REPORT_BACKOFF_MS, REQUEST_THREW, after_failure, backoff_ms, last_error, refused,
    status_error,
};
pub use overlay::{
    CLEAR_OVERLAY_PASSTHROUGH, clear_overlay_request, delete_overlay_background_request,
    parse_background_url, publish_overlay_request, upload_overlay_background_request,
};
pub use rate_limit::{RateLimits, pause_ms};
pub use replays::{
    SharedReplay, parse_share_response, revoke_ok, revoke_replay_request, share_replay_request,
    share_status,
};
pub use report::{
    ArchetypeRef, CLOCK_FIELDS, HOMEBREW_UUID, MatchArchetypes, MatchReportInput, ROGUE_UUID,
    ReportGame, report_body, report_request, report_requests, report_row,
};
pub use request::{
    ApiRequest, ApiResponse, AuthMode, Body, Credentials, GZIP_LEVEL, Method, auth_headers,
    request_url, wire_headers,
};
pub use schedule::{
    CARD_STATS_LIVE, JOBS, Job, LAST_RUNS_KEY, LastRuns, MATCH_REPORTS_LIVE,
    SYNC_AFTER_COMPLETION_MS, Schedule, TRANSIENT_RETRIES_MS, TransientRetry,
};
pub use scouting::{ScoutedArchetype, Scouting, parse_players, players_request};
pub use status::{ApiStatus, parse_status, status_request};
pub use sync::{
    BLOBS_PER_REQUEST, BlobUpload, BlobsAnswer, COMPRESSED_PER_BLOB, COMPRESSED_PER_REQUEST,
    DECK_SLOT_PASSTHROUGH, DirtyEntry, FetchAnswer, FetchedBlob, MANIFEST_CHUNK, ManifestAnswer,
    RefreshResult, SUPPORTED_PLATFORMS, SlotDeck, SlotError, Slots, SyncAuth, SyncAuthStep,
    SyncFailure, SyncType, Tier, blobs_request, deck_slot_request, fetch_request, gzip_blob,
    manifest_request, parse_blobs, parse_deck_slot, parse_fetch, parse_manifest, parse_slots,
    slots_from_json, slots_json, slots_request, sync_answer, sync_status_request,
};
pub use tournaments::{
    PER_RUN_CAP, TournamentLookup, parse_tournament, round_info_tournament_token,
    tournament_request,
};
