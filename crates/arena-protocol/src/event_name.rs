//! Arena's `InternalEventName` (for example `Traditional_Explorer_Ladder` or
//! `PremierDraft_FRA_20260929`): constructed or limited, ranked or not, and
//! the format code.

/// What an event name says about the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventName {
    /// `_Ladder` queues are ranked, `_Play` queues are not. Always false for
    /// a limited event (the limited ladder is not read).
    pub ranked: bool,
    /// The format code: `CPIONEER` for Explorer, `CSTANDARD` for bare
    /// `Ladder` and `Play`, otherwise `ARENA_` and the format part
    /// uppercased. A limited event takes MTGO's limited codes: `D` and the
    /// set three times for a draft (`DFRAFRAFRA`), `S6` and the set for
    /// sealed (`S6EOE`, six boosters as on Arena).
    pub format_code: String,
    /// A draft or sealed event (any `_Draft` or `_Sealed` event naming a set,
    /// Arena Directs included), not a constructed queue.
    pub limited: bool,
    /// A Brawl queue: a commander event, one commander and a 99 card deck.
    /// Its `format_code` is the event name's fallback (`ARENA_HISTORIC_BRAWL`
    /// or `ARENA_BRAWL`); the deck's own `Format` attribute is the better
    /// source (see `commander_format_code`).
    pub commander: bool,
}

/// The format code for a deck's `Format` attribute when it is a commander
/// format: `HistoricBrawl` is `ARENA_HISTORIC_BRAWL` and `Brawl` is
/// `ARENA_BRAWL`. Anything else is `None`.
pub fn commander_format_code(deck_format: &str) -> Option<String> {
    match deck_format {
        "HistoricBrawl" => Some("ARENA_HISTORIC_BRAWL".to_string()),
        "Brawl" => Some("ARENA_BRAWL".to_string()),
        _ => None,
    }
}

/// Reads a Brawl queue name, a constructed queue name or a limited event
/// name. Anything else (direct challenges, cubes and events without a set)
/// is `None`.
pub fn parse_event_name(name: &str) -> Option<EventName> {
    parse_brawl(name)
        .or_else(|| parse_constructed(name))
        .or_else(|| parse_limited(name))
}

/// A Brawl queue: not a `DirectGame` challenge, with a part that is `Brawl`
/// (`Play_Brawl_Historic`, `Brawl_Ladder`, `Play_Brawl`). Ranked when the last
/// part is `Ladder`. Historic Brawl when a part is `Historic` or the name is
/// the Brawl ladder, otherwise Brawl.
fn parse_brawl(name: &str) -> Option<EventName> {
    if name.starts_with("DirectGame") {
        return None;
    }
    let parts: Vec<&str> = name.split('_').collect();
    if !parts.contains(&"Brawl") {
        return None;
    }
    let ranked = parts.last() == Some(&"Ladder");
    let historic = parts.contains(&"Historic") || name == "Brawl_Ladder";
    let format_code = if historic {
        "ARENA_HISTORIC_BRAWL"
    } else {
        "ARENA_BRAWL"
    };
    Some(EventName {
        ranked,
        format_code: format_code.to_string(),
        limited: false,
        commander: true,
    })
}

/// A constructed queue: an optional `Traditional_` prefix (Bo3, which the
/// tracker reads from `matchWinCondition` instead), an optional format part,
/// then `Ladder` or `Play`.
fn parse_constructed(name: &str) -> Option<EventName> {
    let mut parts: Vec<&str> = name.split('_').collect();
    if parts.first() == Some(&"Traditional") {
        parts.remove(0);
    }
    let ranked = match parts.pop()? {
        "Ladder" => true,
        "Play" => false,
        _ => return None,
    };
    let format_code = match parts.as_slice() {
        [] => "CSTANDARD".to_string(),
        ["Explorer"] => "CPIONEER".to_string(),
        [format] if !format.is_empty() && format.chars().all(|c| c.is_ascii_alphabetic()) => {
            format!("ARENA_{}", format.to_ascii_uppercase())
        }
        _ => return None,
    };
    Some(EventName {
        ranked,
        format_code,
        limited: false,
        commander: false,
    })
}

/// A limited event: a part that is `Sealed` or ends in it, or else ends in
/// `Draft` (`PremierDraft`, `QuickDraft`, `TradDraft`, `TradSealed`), and a
/// set code part: 2 to 5 capitals and digits with a capital
/// (`ArenaDirect_EOE_Play_Sealed_20261106` is EOE sealed).
fn parse_limited(name: &str) -> Option<EventName> {
    let parts: Vec<&str> = name.split('_').collect();
    let sealed = parts.iter().any(|part| part.ends_with("Sealed"));
    let draft = parts.iter().any(|part| part.ends_with("Draft"));
    if !sealed && !draft {
        return None;
    }
    let set = parts.iter().skip(1).find(|part| is_set_code(part))?;
    let format_code = if sealed {
        format!("S6{set}")
    } else {
        format!("D{set}{set}{set}")
    };
    Some(EventName {
        ranked: false,
        format_code,
        limited: true,
        commander: false,
    })
}

fn is_set_code(part: &str) -> bool {
    (2..=5).contains(&part.len())
        && part
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        && part.chars().any(|c| c.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(name: &str) -> Option<(bool, String)> {
        parse_event_name(name)
            .filter(|event| !event.limited)
            .map(|event| (event.ranked, event.format_code))
    }

    fn limited(name: &str) -> Option<String> {
        parse_event_name(name)
            .filter(|event| event.limited && !event.ranked)
            .map(|event| event.format_code)
    }

    #[test]
    fn limited_events_take_mtgo_limited_codes() {
        assert_eq!(
            limited("ArenaDirect_EOE_Play_Sealed_20261106"),
            Some("S6EOE".to_string())
        );
        assert_eq!(
            limited("PremierDraft_FRA_20260929"),
            Some("DFRAFRAFRA".to_string())
        );
        assert_eq!(
            limited("QuickDraft_FDN_20261101"),
            Some("DFDNFDNFDN".to_string())
        );
        assert_eq!(
            limited("TradSealed_FRA_20260929"),
            Some("S6FRA".to_string())
        );
        assert_eq!(limited("Sealed_FDN_Play"), Some("S6FDN".to_string()));
    }

    #[test]
    fn explorer_is_pioneer() {
        assert_eq!(
            parsed("Traditional_Explorer_Ladder"),
            Some((true, "CPIONEER".to_string()))
        );
        assert_eq!(
            parsed("Explorer_Play"),
            Some((false, "CPIONEER".to_string()))
        );
    }

    #[test]
    fn bare_ladder_is_standard() {
        assert_eq!(parsed("Ladder"), Some((true, "CSTANDARD".to_string())));
        assert_eq!(
            parsed("Traditional_Ladder"),
            Some((true, "CSTANDARD".to_string()))
        );
        assert_eq!(parsed("Play"), Some((false, "CSTANDARD".to_string())));
        assert_eq!(
            parsed("Traditional_Play"),
            Some((false, "CSTANDARD".to_string()))
        );
    }

    #[test]
    fn other_formats_are_arena_codes() {
        assert_eq!(
            parsed("Historic_Ladder"),
            Some((true, "ARENA_HISTORIC".to_string()))
        );
        assert_eq!(
            parsed("Traditional_Timeless_Ladder"),
            Some((true, "ARENA_TIMELESS".to_string()))
        );
        assert_eq!(
            parsed("Alchemy_Play"),
            Some((false, "ARENA_ALCHEMY".to_string()))
        );
    }

    fn brawl(name: &str) -> Option<EventName> {
        parse_event_name(name)
    }

    fn brawl_queue(ranked: bool, format_code: &str) -> Option<EventName> {
        Some(EventName {
            ranked,
            format_code: format_code.to_string(),
            limited: false,
            commander: true,
        })
    }

    #[test]
    fn brawl_queues_are_commander_events() {
        assert_eq!(
            brawl("Play_Brawl_Historic"),
            brawl_queue(false, "ARENA_HISTORIC_BRAWL")
        );
        assert_eq!(
            brawl("Brawl_Ladder"),
            brawl_queue(true, "ARENA_HISTORIC_BRAWL")
        );
        assert_eq!(brawl("Play_Brawl"), brawl_queue(false, "ARENA_BRAWL"));
        assert_eq!(brawl("DirectGameBrawl"), None);
        assert_eq!(brawl("DirectGameBrawlRebalanced"), None);
        assert_eq!(brawl("DirectGame"), None);
    }

    #[test]
    fn other_events_are_not_commander_events() {
        assert_eq!(
            parse_event_name("Historic_Ladder").map(|e| e.commander),
            Some(false)
        );
        assert_eq!(
            parse_event_name("PremierDraft_FRA_20260929").map(|e| e.commander),
            Some(false)
        );
    }

    #[test]
    fn deck_format_gives_the_commander_format_code() {
        assert_eq!(
            commander_format_code("HistoricBrawl").as_deref(),
            Some("ARENA_HISTORIC_BRAWL")
        );
        assert_eq!(
            commander_format_code("Brawl").as_deref(),
            Some("ARENA_BRAWL")
        );
        assert_eq!(commander_format_code("Historic"), None);
        assert_eq!(commander_format_code(""), None);
    }

    #[test]
    fn unknown_event_names_are_none() {
        assert_eq!(parsed(""), None);
        assert_eq!(parsed("DirectGame"), None);
        assert_eq!(parse_event_name("Traditional_Draft"), None, "no set");
        assert_eq!(parse_event_name("CubeDraft_Arena_20260101"), None, "no set");
        assert_eq!(parsed("Traditional_"), None);
        assert_eq!(parsed("_Ladder"), None);
    }
}
