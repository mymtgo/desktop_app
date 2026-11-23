//! Identity of the local player: the Login line and the per-game deck-used line.

use std::sync::LazyLock;

use regex::Regex;

/// MTGO usernames may contain spaces, so the name runs to the end of the line,
/// minus an optional trailing `(login id)`. 0.x used `\S+` and cut such names short.
static LOGIN_USERNAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Username:\s*(.+?)(?:\s+\((\d+)\))?\s*$").expect("valid regex"));

static DECK_USED_USERNAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Username:\s*(.+?) Deck Used in Game ID:").expect("valid regex"));

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Login {
    pub username: String,
    pub login_id: Option<u64>,
}

/// Parse a `Login` event (already classified as `EventKind::Login`).
pub fn parse_login(text: &str) -> Option<Login> {
    let first_line = text.lines().next()?;
    let caps = LOGIN_USERNAME.captures(first_line)?;
    Some(Login {
        username: caps[1].to_string(),
        login_id: caps.get(2).and_then(|id| id.as_str().parse().ok()),
    })
}

/// The local player named on a `Username: X Deck Used in Game ID: N` line.
pub fn deck_used_username(text: &str) -> Option<String> {
    DECK_USED_USERNAME
        .captures(text)
        .map(|caps| caps[1].trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_username_and_login_id() {
        let login =
            parse_login("12:23:25 [INF] (Login|MtGO Login Success) Username: Some_Name (6031972)")
                .expect("login");
        assert_eq!(login.username, "Some_Name");
        assert_eq!(login.login_id, Some(6_031_972));
    }

    #[test]
    fn login_id_is_optional() {
        let login = parse_login("12:23:25 [INF] (Login|MtGO Login Success) Username: Some_Name")
            .expect("login");
        assert_eq!(login.login_id, None);
    }

    #[test]
    fn login_username_keeps_spaces_and_dots() {
        let with_id =
            parse_login("12:23:25 [INF] (Login|MtGO Login Success) Username: Steve O.k-x (42)")
                .expect("login");
        assert_eq!(with_id.username, "Steve O.k-x");
        assert_eq!(with_id.login_id, Some(42));
        let without_id =
            parse_login("12:23:25 [INF] (Login|MtGO Login Success) Username: Steve O.k-x\r\n")
                .expect("login");
        assert_eq!(without_id.username, "Steve O.k-x");
        assert_eq!(without_id.login_id, None);
    }

    #[test]
    fn deck_used_username_keeps_spaces_and_dots() {
        let text = "12:25:49 [INF] (Twitch Info|Username: Steve O.k-x Deck Used in Game ID: 1) []";
        assert_eq!(deck_used_username(text).as_deref(), Some("Steve O.k-x"));
    }
}
