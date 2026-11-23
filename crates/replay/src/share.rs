//! The prepared snapshot and the share and switch off outcomes (package
//! `PrepareSharedSnapshot`; 0.x `ShareReplayController` and
//! `RevokeReplayShareController` messages, replays spec section 8).
//!
//! mymtgo.com stores a desktop upload as sent and never redacts it, so an
//! upload is only ever built from a `PreparedSnapshot`, which only
//! `prepare_shared_snapshot` can make: every opponent name redacted and
//! proven gone, and the website's validation passed. There is no size check
//! of any kind: the server's limit is the server's and can change without a
//! client update (owner, 2026-10-09; ruling 14), so its 413 decides.

use crate::redact::{RedactError, php_trim, redact_player};
use crate::snapshot::Snapshot;
use crate::validate::{ValidationErrors, validate_snapshot};

/// A snapshot ready to leave this machine, as its JSON bytes: redacted for
/// every opponent name and validated. Its field is private, so no other code
/// can make one; the share request builder takes this type, never a raw
/// snapshot.
///
/// ```compile_fail
/// // the_request_builder_only_takes_a_prepared_snapshot: a prepared
/// // snapshot cannot be made without `prepare_shared_snapshot`.
/// let forged = replay::PreparedSnapshot { json: b"{}".to_vec() };
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedSnapshot {
    json: Vec<u8>,
}

impl PreparedSnapshot {
    /// The snapshot's JSON, as it goes into the share request.
    pub fn json(&self) -> &[u8] {
        &self.json
    }

    /// The snapshot's JSON, owned.
    pub fn into_json(self) -> Vec<u8> {
        self.json
    }
}

/// Why a snapshot could not be prepared for sharing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareFailure {
    /// An opponent's name survived redaction; nothing may be sent.
    NameSurvived,
    /// The website would refuse the snapshot: every failing dotted path with
    /// its messages, for the share's log line (0.x logs the
    /// `ValidationException`).
    Invalid(ValidationErrors),
}

impl From<PrepareFailure> for ShareFailure {
    fn from(failure: PrepareFailure) -> Self {
        match failure {
            PrepareFailure::NameSurvived => ShareFailure::NameSurvived,
            PrepareFailure::Invalid(_) => ShareFailure::Invalid,
        }
    }
}

/// Package `PrepareSharedSnapshot::run`: skips blank names, redacts each
/// other name in turn, then validates the whole snapshot.
///
/// A name that survives redaction is `NameSurvived`; a snapshot the website
/// would refuse is `Invalid` with its failures. `ShareFailure::from` gives
/// the share outcome (`name_survived`, `invalid`).
pub fn prepare_shared_snapshot(
    snapshot: Snapshot,
    opponent_names: &[String],
) -> Result<PreparedSnapshot, PrepareFailure> {
    let mut snapshot = snapshot;
    for name in opponent_names {
        if php_trim(name).is_empty() {
            continue;
        }

        snapshot = redact_player(snapshot, name).map_err(|error| match error {
            RedactError::NameSurvived(_) => PrepareFailure::NameSurvived,
            // Cannot happen: blank names are skipped above with the same trim.
            RedactError::EmptyUsername => PrepareFailure::Invalid(ValidationErrors::new()),
        })?;
    }

    let value = serde_json::to_value(&snapshot).map_err(|error| unserialisable(&error))?;
    validate_snapshot(&value).map_err(PrepareFailure::Invalid)?;

    let json = serde_json::to_vec(&value).map_err(|error| unserialisable(&error))?;
    Ok(PreparedSnapshot { json })
}

/// A snapshot serde could not write (never for these types), reported under
/// the root path.
fn unserialisable(error: &serde_json::Error) -> PrepareFailure {
    let mut errors = ValidationErrors::new();
    errors.insert(String::new(), vec![error.to_string()]);
    PrepareFailure::Invalid(errors)
}

/// Why a share did not happen (0.x `ReplayShareRefused` reasons plus the
/// controller's own catches).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareFailure {
    /// Not signed in or not activated (Plan Q gate).
    NotSignedIn,
    /// Offline mode is on (Plan Q gate).
    Offline,
    /// 422 `replay_share_requires_supporter`.
    Supporter,
    /// 409: the MTGO account is not claimed by the signed-in user.
    NotClaimed,
    /// 413 (`replay_too_large`).
    TooLarge,
    /// No game with frames, a snapshot that fails validation, or any other
    /// 422.
    Invalid,
    /// The match's account has no login id yet.
    AccountUnknown,
    /// An opponent's name survived redaction; nothing was sent.
    NameSurvived,
    /// A connect or timeout error.
    Unreachable,
    /// Anything else the server answered (429 included).
    Server,
}

impl ShareFailure {
    /// The 0.x message, `host` being the configured API host (Plan Q
    /// `ApiConfig.host`). The sign-in sentence names the Account page
    /// (ruling 10).
    pub fn message(&self, host: &str) -> String {
        match self {
            ShareFailure::NotSignedIn => "Sign in on the Account page to share replays.".to_string(),
            ShareFailure::Offline => {
                "Sharing needs a connection. Turn off offline mode first.".to_string()
            }
            ShareFailure::Supporter => "Sharing replays is a supporter feature.".to_string(),
            ShareFailure::NotClaimed => format!(
                "Link your MTGO account to your {host} account first, so the replay can be filed under your player name. We could not confirm your claim to it."
            ),
            ShareFailure::TooLarge => "This game is too large to share.".to_string(),
            ShareFailure::Invalid => "This replay could not be prepared for sharing.".to_string(),
            ShareFailure::AccountUnknown => {
                "Your MTGO account is not linked yet. Play a game with the app running, then try again."
                    .to_string()
            }
            ShareFailure::NameSurvived => {
                "Couldn't hide your opponent's name, so this replay was not shared.".to_string()
            }
            ShareFailure::Unreachable => format!("Could not reach {host}. Try again in a moment."),
            ShareFailure::Server => {
                format!("{host} could not share this replay right now. Try again in a moment.")
            }
        }
    }
}

/// Why switching a link off did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokeFailure {
    NotSignedIn,
    Offline,
    /// A connect error or any failed answer: the link is still on.
    Unreachable,
}

impl RevokeFailure {
    /// The 0.x message, `host` being the configured API host.
    pub fn message(&self, host: &str) -> String {
        match self {
            RevokeFailure::NotSignedIn => {
                "Sign in on the Account page to switch this link off.".to_string()
            }
            RevokeFailure::Offline => {
                "Switching a link off needs a connection. Turn off offline mode first.".to_string()
            }
            RevokeFailure::Unreachable => {
                format!("Could not reach {host}. The link is still on; try again in a moment.")
            }
        }
    }
}
