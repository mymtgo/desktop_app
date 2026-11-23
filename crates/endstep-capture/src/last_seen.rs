//! `last_seen.json`: what the helper records on every request so MyMTGO can
//! tell whether the browser extension is alive (endstep spec 8.1). It holds
//! a time, a browser and an extension version, never capture data.

use serde::{Deserialize, Serialize};

/// One day: past this the extension counts as not seen lately.
pub const RECENT_MS: i64 = 86_400_000;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct LastSeen {
    /// UTC unix milliseconds of the helper's clock.
    pub at: i64,
    pub browser: String,
    pub extension: String,
}

pub fn last_seen_json(seen: &LastSeen) -> String {
    // Three plain fields cannot fail to serialise.
    serde_json::to_string(seen).unwrap_or_default()
}

/// `None` for a file that is not a last-seen record (a half-written or
/// foreign file reads as never seen).
pub fn parse_last_seen(text: &str) -> Option<LastSeen> {
    serde_json::from_str(text).ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connection {
    NeverSeen,
    Recent { seen: LastSeen },
    Stale { seen: LastSeen },
}

/// A time in the future (a clock that moved back) counts as recent.
pub fn connection(seen: Option<LastSeen>, now_ms: i64) -> Connection {
    match seen {
        None => Connection::NeverSeen,
        Some(seen) => {
            if now_ms - seen.at <= RECENT_MS {
                Connection::Recent { seen }
            } else {
                Connection::Stale { seen }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(at: i64) -> LastSeen {
        LastSeen {
            at,
            browser: "chrome".to_string(),
            extension: "0.1.0".to_string(),
        }
    }

    #[test]
    fn connection_states() {
        let now = 100_000_000_000;
        assert_eq!(connection(None, now), Connection::NeverSeen);
        let hour = seen(now - 3_600_000);
        assert_eq!(
            connection(Some(hour.clone()), now),
            Connection::Recent { seen: hour }
        );
        let day_plus = seen(now - 25 * 3_600_000);
        assert_eq!(
            connection(Some(day_plus.clone()), now),
            Connection::Stale { seen: day_plus }
        );
        let exactly_a_day = seen(now - 86_400_000);
        assert!(matches!(
            connection(Some(exactly_a_day), now),
            Connection::Recent { .. }
        ));
        let future = seen(now + 5_000);
        assert!(matches!(
            connection(Some(future), now),
            Connection::Recent { .. }
        ));
        assert_eq!(parse_last_seen("{"), None);
    }

    #[test]
    fn last_seen_round_trips() {
        let value = seen(42);
        assert_eq!(parse_last_seen(&last_seen_json(&value)), Some(value));
    }
}
