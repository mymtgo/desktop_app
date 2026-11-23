//! Every way an API call can fail. No variant ever holds a secret: messages
//! come from the transport or from the server's body, never from headers.

use std::fmt;

/// How much of a failed response's body `ApiError::Status` keeps (0.x
/// truncated `last_error` to 500 characters).
pub const STATUS_BODY_CHARS: usize = 500;

/// Why a connection failed, so a log or a report can tell a name that did
/// not resolve from a refused port, a reset or a TLS failure (#508).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectKind {
    /// The host name did not resolve.
    Dns,
    /// Nothing accepted the connection.
    Refused,
    /// The connection was reset or aborted part way (what some ISP blocks do).
    Reset,
    /// The connection did not come up in time.
    Timeout,
    /// The TLS handshake or a certificate failed.
    Tls,
    /// Any other failure.
    Other,
}

impl ConnectKind {
    /// A short lower-case name for logs.
    pub fn name(self) -> &'static str {
        match self {
            ConnectKind::Dns => "dns",
            ConnectKind::Refused => "refused",
            ConnectKind::Reset => "reset",
            ConnectKind::Timeout => "timeout",
            ConnectKind::Tls => "tls",
            ConnectKind::Other => "other",
        }
    }

    /// The kind of an I/O failure: its `ErrorKind` first, then its message
    /// (the OS words DNS and TLS failures differently per platform).
    pub fn from_io(kind: std::io::ErrorKind, message: &str) -> ConnectKind {
        use std::io::ErrorKind;
        match kind {
            ErrorKind::ConnectionRefused => return ConnectKind::Refused,
            ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted | ErrorKind::BrokenPipe => {
                return ConnectKind::Reset;
            }
            ErrorKind::TimedOut | ErrorKind::WouldBlock => return ConnectKind::Timeout,
            _ => {}
        }
        let text = message.to_lowercase();
        let dns_words = [
            "failed to lookup",
            "no such host",
            "name or service not known",
            "nodename nor servname",
            "temporary failure in name resolution",
            "dns",
        ];
        if dns_words.iter().any(|word| text.contains(word)) {
            ConnectKind::Dns
        } else if text.contains("tls") || text.contains("certificate") || text.contains("handshake")
        {
            ConnectKind::Tls
        } else {
            ConnectKind::Other
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// Offline mode is on and the request's class is blocked.
    Offline,
    /// The request needs a signed-in session.
    NotLinked,
    /// The call took longer than its timeout.
    Timeout,
    /// The connection or TLS handshake failed: the kind and the message.
    Connect(ConnectKind, String),
    /// The answer was larger than the call's body limit.
    TooLarge,
    /// A non-2xx answer a caller chose to treat as an error: the status and
    /// at most `STATUS_BODY_CHARS` characters of the body.
    Status(u16, String),
    /// The answer was not the JSON the caller expected.
    Malformed(String),
    /// The request could not be built (a bad URL, a missing device key).
    Config(String),
    /// A 429 paused the request's path until this time (UTC ms): it was not
    /// sent (#499).
    RateLimited(i64),
}

impl ApiError {
    /// The variant's name, for messages like `Connect: connection failed`
    /// (0.x showed the exception class before its message).
    pub fn kind(&self) -> &'static str {
        match self {
            ApiError::Offline => "Offline",
            ApiError::NotLinked => "NotLinked",
            ApiError::Timeout => "Timeout",
            ApiError::Connect(_, _) => "Connect",
            ApiError::TooLarge => "TooLarge",
            ApiError::Status(_, _) => "Status",
            ApiError::Malformed(_) => "Malformed",
            ApiError::Config(_) => "Config",
            ApiError::RateLimited(_) => "RateLimited",
        }
    }

    /// The request never got an answer for a reason that passes on its
    /// own: no connection (DNS or the network not up yet, as at launch or
    /// wake) or a timeout. Any answer from the server, even an error, is
    /// not transient.
    pub fn is_transient(&self) -> bool {
        matches!(self, ApiError::Connect(_, _) | ApiError::Timeout)
    }

    /// `Status` for a response, the body cut to `STATUS_BODY_CHARS`
    /// characters (invalid UTF-8 replaced).
    pub fn status(status: u16, body: &[u8]) -> ApiError {
        let text: String = String::from_utf8_lossy(body)
            .chars()
            .take(STATUS_BODY_CHARS)
            .collect();
        ApiError::Status(status, text)
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Offline => write!(f, "offline mode is on"),
            ApiError::NotLinked => write!(f, "not signed in"),
            ApiError::Timeout => write!(f, "the request timed out"),
            ApiError::Connect(kind, message) => {
                write!(f, "connection failed ({}): {message}", kind.name())
            }
            ApiError::TooLarge => write!(f, "the answer was too large"),
            ApiError::Status(status, body) => write!(f, "HTTP {status}: {body}"),
            ApiError::Malformed(message) => write!(f, "unexpected answer: {message}"),
            ApiError::Config(message) => write!(f, "configuration: {message}"),
            ApiError::RateLimited(_) => write!(f, "rate limited by MyMTGO; trying again later"),
        }
    }
}

impl std::error::Error for ApiError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_no_connection_and_a_timeout_are_transient() {
        let dns = "io: No such host is known. (os error 11001)".to_string();
        assert!(ApiError::Connect(ConnectKind::Dns, dns).is_transient());
        assert!(ApiError::Timeout.is_transient());
        for answered in [
            ApiError::Status(500, String::new()),
            ApiError::Status(401, String::new()),
            ApiError::Malformed(String::new()),
            ApiError::Config(String::new()),
            ApiError::Offline,
            ApiError::NotLinked,
            ApiError::TooLarge,
            ApiError::RateLimited(0),
        ] {
            assert!(!answered.is_transient(), "{answered:?}");
        }
    }

    #[test]
    fn io_failures_are_sorted_by_kind_then_message() {
        use std::io::ErrorKind;
        let cases = [
            (ErrorKind::ConnectionRefused, "x", ConnectKind::Refused),
            (ErrorKind::ConnectionReset, "x", ConnectKind::Reset),
            (ErrorKind::ConnectionAborted, "x", ConnectKind::Reset),
            (ErrorKind::TimedOut, "x", ConnectKind::Timeout),
            (
                ErrorKind::Other,
                "failed to lookup address information: Name or service not known",
                ConnectKind::Dns,
            ),
            (
                ErrorKind::Other,
                "No such host is known. (os error 11001)",
                ConnectKind::Dns,
            ),
            (
                ErrorKind::InvalidData,
                "invalid peer certificate: UnknownIssuer",
                ConnectKind::Tls,
            ),
            (ErrorKind::Other, "something else", ConnectKind::Other),
        ];
        for (kind, message, expected) in cases {
            assert_eq!(ConnectKind::from_io(kind, message), expected, "{message}");
        }
    }

    #[test]
    fn a_connect_error_shows_its_kind() {
        let error = ApiError::Connect(ConnectKind::Refused, "refused".to_string());
        assert_eq!(error.to_string(), "connection failed (refused): refused");
    }

    #[test]
    fn a_status_error_keeps_500_characters_of_the_body() {
        let body = "é".repeat(600);
        let ApiError::Status(status, text) = ApiError::status(503, body.as_bytes()) else {
            panic!("a status error");
        };
        assert_eq!(status, 503);
        assert_eq!(text.chars().count(), 500);
    }
}
