//! `Match State Changed for <token> from <State> to <State>` lines.

use std::sync::LazyLock;

use regex::Regex;

static STATE_CHANGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)Match State Changed for ([a-f0-9\-]+) from ([A-Za-z]+) to ([A-Za-z]+)")
        .expect("valid regex")
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateChange {
    pub token: String,
    pub from: String,
    pub to: String,
}

pub fn parse_state_change(text: &str) -> Option<StateChange> {
    let caps = STATE_CHANGE.captures(text)?;
    Some(StateChange {
        token: caps[1].to_string(),
        from: caps[2].to_string(),
        to: caps[3].to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_token_from_and_to_without_the_closing_paren() {
        let change = parse_state_change(
            "12:23:54 [INF] (Game Management|Match State Changed for b2723ac5-787a-42a0-8692-ca205448f6f2 from TournamentMatchUninitializedState to TournamentMatchClosedState) ",
        )
        .expect("change");
        assert_eq!(change.token, "b2723ac5-787a-42a0-8692-ca205448f6f2");
        assert_eq!(change.from, "TournamentMatchUninitializedState");
        assert_eq!(change.to, "TournamentMatchClosedState");
    }
}
