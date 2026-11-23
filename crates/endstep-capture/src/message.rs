//! Requests the extension sends to the helper, and the helper's replies.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostRequest {
    /// The extension's fields are optional so a plain ping from an older
    /// build still decodes.
    Ping {
        #[serde(default)]
        browser: Option<String>,
        #[serde(default)]
        extension: Option<String>,
    },
    Deliver {
        unit_id: String,
        content: String,
    },
    /// One piece of a capture too big for a single message. Parts are whole
    /// lines, numbered from 0, and arrive in order on one port.
    DeliverPart {
        unit_id: String,
        index: u32,
        content: String,
    },
    /// Closes a unit sent in parts; `parts` is how many were sent.
    DeliverEnd {
        unit_id: String,
        parts: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct HostReply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl HostReply {
    pub fn pong(version: &str) -> HostReply {
        HostReply {
            ok: true,
            version: Some(version.to_string()),
            error: None,
        }
    }

    pub fn stored() -> HostReply {
        HostReply {
            ok: true,
            version: None,
            error: None,
        }
    }

    pub fn failed(message: &str) -> HostReply {
        HostReply {
            ok: false,
            version: None,
            error: Some(message.to_string()),
        }
    }
}

/// True for the shape of `crypto.randomUUID()`: 36 chars, lowercase hex, and
/// dashes at positions 8, 13, 18 and 23. Safe to use as a file name.
pub fn valid_unit_id(id: &str) -> bool {
    if id.len() != 36 {
        return false;
    }
    id.bytes().enumerate().all(|(i, b)| {
        if matches!(i, 8 | 13 | 18 | 23) {
            b == b'-'
        } else {
            b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_decode() {
        let ping: HostRequest = serde_json::from_str(r#"{"type":"ping"}"#).unwrap();
        assert_eq!(
            ping,
            HostRequest::Ping {
                browser: None,
                extension: None
            }
        );
        let deliver: HostRequest =
            serde_json::from_str(r#"{"type":"deliver","unit_id":"u","content":"c"}"#).unwrap();
        assert_eq!(
            deliver,
            HostRequest::Deliver {
                unit_id: "u".into(),
                content: "c".into()
            }
        );
    }

    #[test]
    fn part_requests_decode() {
        let part: HostRequest = serde_json::from_str(
            r#"{"type":"deliver_part","unit_id":"u","index":2,"content":"c"}"#,
        )
        .unwrap();
        assert_eq!(
            part,
            HostRequest::DeliverPart {
                unit_id: "u".into(),
                index: 2,
                content: "c".into()
            }
        );
        let end: HostRequest =
            serde_json::from_str(r#"{"type":"deliver_end","unit_id":"u","parts":3}"#).unwrap();
        assert_eq!(
            end,
            HostRequest::DeliverEnd {
                unit_id: "u".into(),
                parts: 3
            }
        );
    }

    #[test]
    fn an_old_ping_still_decodes() {
        let ping: HostRequest = serde_json::from_str(r#"{"type":"ping"}"#).unwrap();
        assert_eq!(
            ping,
            HostRequest::Ping {
                browser: None,
                extension: None
            }
        );
    }

    #[test]
    fn replies_skip_empty_fields() {
        assert_eq!(
            serde_json::to_string(&HostReply::stored()).unwrap(),
            r#"{"ok":true}"#
        );
    }

    #[test]
    fn unit_ids() {
        let good = "0f8fad5b-d9cb-469f-a165-70867728950e";
        assert!(valid_unit_id(good));
        assert!(!valid_unit_id(""));
        assert!(!valid_unit_id(&good.to_uppercase()));
        assert!(!valid_unit_id("../../etc/passwd"));
        assert!(!valid_unit_id("0f8fad5b-d9cb-469f-a165-7086772895/e"));
    }
}
