//! The opponent's archetype on the game overlay (overlay spec 7.3, 0.x
//! `ResolveOverlayOpponent`): which source answers first, the source's label
//! and the head-to-head text.
//!
//! The worker gathers every source (the manual row on the live match, the
//! live local read of the reveals, the archetype last faced in this format,
//! the three scouting answers and the API estimate) and this module picks
//! one in 0.x's order. Caches, the offline check and uuid lookups are the
//! caller's.

use serde::Serialize;

/// The lowest local-read confidence that lets the live read win (0.x
/// `LIVE_CONFIDENCE_THRESHOLD`, the same value as the archetypes crate's
/// local threshold).
pub const LIVE_CONFIDENCE_THRESHOLD: f64 = 0.8;

/// An archetype as a source names it: one v1 holds (its `archetypes.id`), or
/// one the API named that v1 does not hold yet (shown by the API's name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchetypeRef {
    Held(i64),
    Unheld { uuid: String, name: String },
}

/// The live local read of the revealed cards and its confidence (0 to 1).
#[derive(Debug, Clone, PartialEq)]
pub struct LiveEstimate {
    pub archetype: ArchetypeRef,
    pub confidence: f64,
}

/// Every source's answer, `None` when the source has none. `api` is `None`
/// when offline or nothing has been revealed (the caller never asks then).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OpponentSources {
    /// The manual opponent row on the live match.
    pub manual: Option<ArchetypeRef>,
    /// The live local read of this match's reveals.
    pub live: Option<LiveEstimate>,
    /// The archetype of the newest earlier match against them in this format.
    pub last_faced: Option<ArchetypeRef>,
    /// Scouting: their latest 5-0 list.
    pub league: Option<ArchetypeRef>,
    /// Scouting: the deck they last played through the tracker.
    pub tracked: Option<ArchetypeRef>,
    /// The API's estimate of the revealed cards (no confidence floor).
    pub api: Option<ArchetypeRef>,
    /// Scouting: another tracker user's read of them.
    pub observed: Option<ArchetypeRef>,
}

/// Which source answered (0.x `source` values).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Manual,
    Live,
    /// What we last faced them on (0.x `local`).
    Local,
    League,
    Tracked,
    Api,
    Observed,
    None,
}

impl Source {
    /// The 0.x `source` text.
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Manual => "manual",
            Source::Live => "live",
            Source::Local => "local",
            Source::League => "league",
            Source::Tracked => "tracked",
            Source::Api => "api",
            Source::Observed => "observed",
            Source::None => "none",
        }
    }

    /// The label the picker shows beside the archetype; none for a manual
    /// pick, a live read or no archetype.
    pub fn label(self) -> Option<&'static str> {
        match self {
            Source::Local => Some("you faced"),
            Source::League => Some("5-0 list"),
            Source::Tracked => Some("last played"),
            Source::Api => Some("guess"),
            Source::Observed => Some("seen by others"),
            Source::Manual | Source::Live | Source::None => None,
        }
    }
}

/// The resolved archetype, the source that named it and whether it is a
/// manual pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub archetype: Option<ArchetypeRef>,
    pub source: Source,
    pub manual: bool,
}

/// The first source that answers, in 0.x's order: manual, live (at or above
/// `LIVE_CONFIDENCE_THRESHOLD`), last faced, league, tracked, API, observed,
/// none.
pub fn resolve_opponent(sources: OpponentSources) -> Resolved {
    if let Some(archetype) = sources.manual {
        return Resolved {
            archetype: Some(archetype),
            source: Source::Manual,
            manual: true,
        };
    }

    if let Some(live) = sources.live
        && live.confidence >= LIVE_CONFIDENCE_THRESHOLD
    {
        return found(live.archetype, Source::Live);
    }

    // First-hand history above every API source; scouting by full lists
    // above the API's guess from a partial reveal, which sits above another
    // user's read from some other day.
    let ordered = [
        (sources.last_faced, Source::Local),
        (sources.league, Source::League),
        (sources.tracked, Source::Tracked),
        (sources.api, Source::Api),
        (sources.observed, Source::Observed),
    ];
    for (archetype, source) in ordered {
        if let Some(archetype) = archetype {
            return found(archetype, source);
        }
    }

    Resolved {
        archetype: None,
        source: Source::None,
        manual: false,
    }
}

/// An automatic answer from one source.
fn found(archetype: ArchetypeRef, source: Source) -> Resolved {
    Resolved {
        archetype: Some(archetype),
        source,
        manual: false,
    }
}

/// `Met {n}× · won {w}`, hidden (`None`) when there are no earlier matches.
pub fn head_to_head_text(matches: u32, wins: u32) -> Option<String> {
    if matches == 0 {
        return None;
    }
    Some(format!("Met {matches}× · won {wins}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(id: i64) -> Option<ArchetypeRef> {
        Some(ArchetypeRef::Held(id))
    }

    fn unheld(uuid: &str, name: &str) -> Option<ArchetypeRef> {
        Some(ArchetypeRef::Unheld {
            uuid: uuid.to_string(),
            name: name.to_string(),
        })
    }

    fn live(id: i64, confidence: f64) -> Option<LiveEstimate> {
        Some(LiveEstimate {
            archetype: ArchetypeRef::Held(id),
            confidence,
        })
    }

    /// Every scouting source answering at once (0.x `overlayFakeFullScouting`).
    fn full_scouting() -> OpponentSources {
        OpponentSources {
            league: held(21),
            tracked: held(22),
            observed: held(23),
            ..OpponentSources::default()
        }
    }

    #[test]
    fn prefers_a_manual_archetype_over_everything_else() {
        let sources = OpponentSources {
            manual: held(1),
            live: live(2, 1.0),
            last_faced: held(3),
            api: held(4),
            ..full_scouting()
        };

        let resolved = resolve_opponent(sources);

        assert_eq!(resolved.archetype, held(1));
        assert_eq!(resolved.source, Source::Manual);
        assert!(resolved.manual);
    }

    #[test]
    fn falls_back_to_the_last_archetype_this_opponent_was_seen_on() {
        let resolved = resolve_opponent(OpponentSources {
            last_faced: held(5),
            ..OpponentSources::default()
        });

        assert_eq!(resolved.archetype, held(5));
        assert_eq!(resolved.source, Source::Local);
        assert!(!resolved.manual);
    }

    #[test]
    fn reports_the_head_to_head_record_even_when_the_league_lookup_wins() {
        let resolved = resolve_opponent(OpponentSources {
            league: unheld("arch-league", "Mono Red Prowess"),
            ..OpponentSources::default()
        });

        assert_eq!(resolved.source, Source::League);
        assert_eq!(
            resolved.archetype,
            unheld("arch-league", "Mono Red Prowess")
        );
        // One win and one loss: two earlier matches, one won.
        assert_eq!(head_to_head_text(2, 1).as_deref(), Some("Met 2× · won 1"));
    }

    #[test]
    fn reports_no_archetype_when_nothing_resolves() {
        let resolved = resolve_opponent(OpponentSources::default());

        assert_eq!(resolved.archetype, None);
        assert_eq!(resolved.source, Source::None);
        assert!(!resolved.manual);
    }

    #[test]
    fn prefers_a_confident_live_estimate_over_a_faked_league_hit() {
        let resolved = resolve_opponent(OpponentSources {
            live: live(7, 0.95),
            league: held(8),
            ..OpponentSources::default()
        });

        assert_eq!(resolved.source, Source::Live);
        assert_eq!(resolved.archetype, held(7));
    }

    #[test]
    fn falls_through_past_a_live_estimate_below_the_confidence_floor() {
        let resolved = resolve_opponent(OpponentSources {
            live: live(7, 0.4),
            league: held(8),
            ..OpponentSources::default()
        });

        assert_eq!(resolved.source, Source::League);
        assert_eq!(resolved.archetype, held(8));
    }

    #[test]
    fn a_live_estimate_exactly_at_the_floor_wins() {
        let resolved = resolve_opponent(OpponentSources {
            live: live(7, LIVE_CONFIDENCE_THRESHOLD),
            league: held(8),
            ..OpponentSources::default()
        });

        assert_eq!(resolved.source, Source::Live);
    }

    #[test]
    fn falls_back_to_an_api_estimate_when_the_local_guess_is_thin_and_no_league_list_exists() {
        let resolved = resolve_opponent(OpponentSources {
            live: live(7, 0.4),
            api: unheld("api-guess-uuid", "API Guess"),
            ..OpponentSources::default()
        });

        assert_eq!(resolved.source, Source::Api);
        assert_eq!(resolved.archetype, unheld("api-guess-uuid", "API Guess"));
        assert!(!resolved.manual);
    }

    #[test]
    fn prefers_a_league_list_over_the_api_estimate() {
        let resolved = resolve_opponent(OpponentSources {
            live: live(7, 0.4),
            league: held(8),
            api: held(9),
            ..OpponentSources::default()
        });

        assert_eq!(resolved.source, Source::League);
        assert_eq!(resolved.archetype, held(8));
    }

    #[test]
    fn never_uses_an_api_estimate_the_caller_did_not_ask_for() {
        // Offline the caller asks no API, so `api` stays `None`: a thin live
        // read alone resolves nothing.
        let resolved = resolve_opponent(OpponentSources {
            live: live(7, 0.4),
            ..OpponentSources::default()
        });

        assert_eq!(resolved.source, Source::None);
        assert_eq!(resolved.archetype, None);
    }

    #[test]
    fn falls_back_to_the_deck_the_opponent_last_tracked_when_there_is_no_5_0() {
        let resolved = resolve_opponent(OpponentSources {
            tracked: unheld("arch-tracked", "Boros Energy"),
            observed: unheld("arch-observed", "Amulet Titan"),
            ..OpponentSources::default()
        });

        assert_eq!(resolved.source, Source::Tracked);
        assert_eq!(resolved.archetype, unheld("arch-tracked", "Boros Energy"));
    }

    #[test]
    fn falls_back_to_what_other_tracker_users_saw_when_the_opponent_has_no_full_list() {
        // Our own sighting was in another format, so the caller passes no
        // last-faced archetype.
        let resolved = resolve_opponent(OpponentSources {
            observed: held(23),
            ..OpponentSources::default()
        });

        assert_eq!(resolved.source, Source::Observed);
        assert_eq!(resolved.archetype, held(23));
    }

    #[test]
    fn prefers_what_we_last_faced_them_on_in_this_format_over_every_api_source() {
        let resolved = resolve_opponent(OpponentSources {
            last_faced: held(30),
            api: held(31),
            ..full_scouting()
        });

        assert_eq!(resolved.source, Source::Local);
        assert_eq!(resolved.archetype, held(30));
    }

    #[test]
    fn ignores_what_we_faced_them_on_in_another_format() {
        // The caller's last-faced read is per format, so it gives nothing.
        let resolved = resolve_opponent(OpponentSources::default());

        assert_eq!(resolved.source, Source::None);
        assert_eq!(resolved.archetype, None);
    }

    #[test]
    fn does_not_count_an_automatic_guess_on_the_live_match_as_history() {
        // The live match's own automatic row is neither manual nor last
        // faced, so the scouting answers decide.
        let resolved = resolve_opponent(full_scouting());

        assert_eq!(resolved.source, Source::League);
    }

    #[test]
    fn lets_a_confident_live_read_override_what_we_last_faced_them_on() {
        let resolved = resolve_opponent(OpponentSources {
            live: live(40, 0.9),
            last_faced: held(41),
            ..OpponentSources::default()
        });

        assert_eq!(resolved.source, Source::Live);
        assert_eq!(resolved.archetype, held(40));
    }

    #[test]
    fn counts_the_head_to_head_record_in_this_format_only() {
        // The caller counts one Modern win; the two Pauper losses are not
        // passed in.
        assert_eq!(head_to_head_text(1, 1).as_deref(), Some("Met 1× · won 1"));
    }

    #[test]
    fn hides_the_head_to_head_text_without_earlier_matches() {
        assert_eq!(head_to_head_text(0, 0), None);
    }

    #[test]
    fn labels_each_source_as_0x() {
        assert_eq!(Source::Manual.label(), None);
        assert_eq!(Source::Live.label(), None);
        assert_eq!(Source::None.label(), None);
        assert_eq!(Source::Local.label(), Some("you faced"));
        assert_eq!(Source::League.label(), Some("5-0 list"));
        assert_eq!(Source::Tracked.label(), Some("last played"));
        assert_eq!(Source::Api.label(), Some("guess"));
        assert_eq!(Source::Observed.label(), Some("seen by others"));
        assert_eq!(Source::Local.as_str(), "local");
    }
}
