//! The recorder's status file (`status.txt`): `state=`, `match=` and `at=` lines.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecorderState {
    Waiting,
    Recording { match_token: Option<String> },
    Idle,
}

/// Parses the status text. An unknown or missing state gives `None`.
/// The `at=` line is ignored.
pub fn parse_status(text: &str) -> Option<RecorderState> {
    let mut state = None;
    let mut match_token = String::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(value) = line.strip_prefix("state=") {
            state = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("match=") {
            match_token = value.trim().to_string();
        }
    }
    match state?.as_str() {
        "waiting" => Some(RecorderState::Waiting),
        "idle" => Some(RecorderState::Idle),
        "recording" => Some(RecorderState::Recording {
            match_token: if match_token.is_empty() {
                None
            } else {
                Some(match_token)
            },
        }),
        _ => None,
    }
}
