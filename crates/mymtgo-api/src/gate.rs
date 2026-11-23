//! The one gate every request passes before a socket opens (07 spec
//! section 5). 0.x had three HTTP macros (`mymtgoReference`, `mymtgoApi`,
//! `mymtgoSync`); v1 has one client and a class per request.

use crate::error::ApiError;

/// What kind of request this is, which decides when it may be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestClass {
    /// Device registration: always sent.
    Registration,
    /// Card identity lookups (`/api/cards`, `/api/cards/resolve`): always sent.
    Reference,
    /// `/api/status`: blocked offline.
    Status,
    /// Archetype lookups: blocked offline.
    Lookup,
    /// Observations, tournament metadata, match reports, card stats: blocked
    /// offline. Sent with the device key when signed out, as 0.x (owner
    /// ruling 2026-10-11).
    Community,
    /// `/api/sync/*`, attestation, replays and the hosted overlay: blocked
    /// offline and while signed out.
    Sync,
    /// `/oauth/token`: blocked offline.
    Token,
    /// The archetype catalog the player asks to download while offline
    /// (the offline mode card's button): always sent, with the device key
    /// only. Nothing else uses it.
    OfflineDownload,
}

/// The flags the gate reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GateState {
    pub offline: bool,
    /// An access and a refresh token are stored.
    pub linked: bool,
}

/// Whether a request may be sent, and if not, why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateDecision {
    Send,
    Offline,
    NotLinked,
}

impl GateDecision {
    /// The error a refused request ends with; `None` for `Send`.
    pub fn refusal(self) -> Option<ApiError> {
        match self {
            GateDecision::Send => None,
            GateDecision::Offline => Some(ApiError::Offline),
            GateDecision::NotLinked => Some(ApiError::NotLinked),
        }
    }
}

/// The gate table of 07 spec section 5.
pub fn gate(class: RequestClass, state: &GateState) -> GateDecision {
    match class {
        RequestClass::Registration | RequestClass::Reference | RequestClass::OfflineDownload => {
            GateDecision::Send
        }
        RequestClass::Status
        | RequestClass::Lookup
        | RequestClass::Community
        | RequestClass::Token => {
            if state.offline {
                GateDecision::Offline
            } else {
                GateDecision::Send
            }
        }
        RequestClass::Sync => {
            if state.offline {
                GateDecision::Offline
            } else if !state.linked {
                GateDecision::NotLinked
            } else {
                GateDecision::Send
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The offline archetype download: the one archetype request the
    // player may send offline, signed in or out.
    #[test]
    fn offline_download_is_sent_offline_and_signed_out() {
        for state in all_states() {
            assert_eq!(
                gate(RequestClass::OfflineDownload, &state),
                GateDecision::Send
            );
        }
        let offline = GateState {
            offline: true,
            linked: true,
        };
        assert_eq!(gate(RequestClass::Lookup, &offline), GateDecision::Offline);
    }

    const CLASSES: [RequestClass; 8] = [
        RequestClass::Registration,
        RequestClass::Reference,
        RequestClass::Status,
        RequestClass::Lookup,
        RequestClass::Community,
        RequestClass::Sync,
        RequestClass::Token,
        RequestClass::OfflineDownload,
    ];

    fn all_states() -> Vec<GateState> {
        let mut states = Vec::new();
        for offline in [false, true] {
            for linked in [false, true] {
                states.push(GateState { offline, linked });
            }
        }
        states
    }

    /// The table written out by hand, one rule per class.
    fn expected(class: RequestClass, state: &GateState) -> GateDecision {
        use GateDecision::*;
        match class {
            RequestClass::Registration
            | RequestClass::Reference
            | RequestClass::OfflineDownload => Send,
            RequestClass::Status
            | RequestClass::Lookup
            | RequestClass::Community
            | RequestClass::Token => {
                if state.offline {
                    Offline
                } else {
                    Send
                }
            }
            RequestClass::Sync => {
                if state.offline {
                    Offline
                } else if !state.linked {
                    NotLinked
                } else {
                    Send
                }
            }
        }
    }

    #[test]
    fn every_class_meets_the_gate_table() {
        let states = all_states();
        assert_eq!(states.len(), 4);
        for class in CLASSES {
            for state in &states {
                assert_eq!(
                    gate(class, state),
                    expected(class, state),
                    "{class:?} in {state:?}"
                );
            }
        }
    }

    // 0.x HttpMacroTest: "allows the gated macro while online".
    #[test]
    fn allows_the_gated_class_while_online() {
        let online = GateState {
            offline: false,
            linked: true,
        };
        assert_eq!(gate(RequestClass::Community, &online), GateDecision::Send);
    }

    // 0.x HttpMacroTest: "throws from the gated macro while offline".
    #[test]
    fn a_gated_class_is_refused_offline() {
        let offline = GateState {
            offline: true,
            linked: true,
        };
        for class in [
            RequestClass::Status,
            RequestClass::Lookup,
            RequestClass::Community,
            RequestClass::Sync,
            RequestClass::Token,
        ] {
            assert_eq!(
                gate(class, &offline).refusal(),
                Some(ApiError::Offline),
                "{class:?}"
            );
        }
    }

    // Owner ruling 2026-10-11: community data goes as in 0.x, with the
    // anonymous device key, by a device that never signed in.
    #[test]
    fn community_data_is_sent_by_a_device_that_never_signed_in() {
        let never_signed_in = GateState::default();
        assert_eq!(
            gate(RequestClass::Community, &never_signed_in),
            GateDecision::Send
        );
    }

    // 0.x HttpMacroTest: "allows the reference macro while offline".
    #[test]
    fn the_reference_class_is_allowed_offline() {
        let offline = GateState {
            offline: true,
            linked: false,
        };
        assert_eq!(gate(RequestClass::Reference, &offline), GateDecision::Send);
        assert_eq!(
            gate(RequestClass::Registration, &offline),
            GateDecision::Send
        );
    }
}
