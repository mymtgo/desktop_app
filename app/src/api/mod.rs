//! Everything that talks to the MyMTGO API (07 spec). The pure rules live in
//! the `mymtgo-api` crate; this module owns the IO: the HTTP client, the
//! configuration read at start, the randomness source, the secrets, the
//! device key, the API status, sign-in and tokens, account attestation,
//! offline mode, the card and archetype lookups, tournament observations
//! and names, and the `api` worker that ships the outbox.

pub mod attest;
pub mod client;
pub mod device;
#[cfg(test)]
pub mod fake_cloud;
pub mod offline;
pub mod random;
pub mod reference;
pub mod secrets;
pub mod session;
pub mod shipping;
pub mod signin;
pub mod status;
#[cfg(test)]
pub mod stub;
pub mod sync_runner;
pub mod tokens;
pub mod worker;

use mymtgo_api::config::{API_URL_KEY, CA_KEY, CLIENT_ID_KEY};
use mymtgo_api::{ApiConfig, ConfigError, resolve_config_for_build};

/// The values `build.rs` baked in from the build environment or the repo's
/// `.env.local`.
pub fn baked_config() -> Vec<(String, String)> {
    let baked = [
        (API_URL_KEY, option_env!("MYMTGO_BAKED_API_URL")),
        (CLIENT_ID_KEY, option_env!("MYMTGO_BAKED_OAUTH_CLIENT_ID")),
        (CA_KEY, option_env!("MYMTGO_BAKED_API_CA")),
    ];
    baked
        .into_iter()
        .filter_map(|(key, value)| value.map(|value| (key.to_string(), value.to_string())))
        .collect()
}

/// The configuration at app start: process environment over baked values
/// (a release build ignores `MYMTGO_API_URL` and `MYMTGO_API_CA` from the
/// process: `mymtgo_api::config::DEV_ONLY_KEYS`).
///
/// Dev setup: after creating or editing `.env.local`, run `touch app/build.rs`
/// once so the build picks it up.
pub fn load_config() -> Result<ApiConfig, ConfigError> {
    resolve_config_for_build(
        &baked_config(),
        &|key| std::env::var(key).ok(),
        cfg!(debug_assertions),
    )
}
