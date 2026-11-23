//! The detection decision (spec section 6.1, as amended by the catalog
//! picker addendum, #489), given the local answers of one seat and whether
//! the API may be asked.

use crate::{Candidate, Estimate, ObservedCard, Source, fallback_match, rules_match, score};

/// 0.x `DetermineDeckArchetype::LOCAL_CONFIDENCE_THRESHOLD`.
pub const LOCAL_CONFIDENCE_THRESHOLD: f64 = 0.8;
/// The confidence of an offline fallback definition answer (spec 6.1 step 5).
pub const FALLBACK_CONFIDENCE: f64 = 0.5;
/// The two fallback seeds (0.x `Archetype::HOMEBREW_UUID`, `ROGUE_UUID`).
pub const HOMEBREW_UUID: &str = "00000000-0000-0000-0000-000000000001";
pub const ROGUE_UUID: &str = "00000000-0000-0000-0000-000000000002";
/// The smallest constructed maindeck. MTGO saves a deck file while it is
/// being built, and a guess from a part of the list is often wrong (the
/// server's rules pass on a handful of cards too).
pub const MIN_MAINDECK_CARDS: u32 = 60;

/// Whether a deck's maindeck is complete enough to prefill its archetype.
pub fn complete_maindeck(maindeck: &[ObservedCard]) -> bool {
    let total: u32 = maindeck.iter().map(|card| card.quantity).sum();
    total >= MIN_MAINDECK_CARDS
}

/// What the held archetypes say about one seat.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalAnswers {
    /// The opponent seat gets Homebrew when nothing answers; the player none.
    pub opponent: bool,
    /// The exact rules answer (confidence 1.0).
    pub rules: Option<(i64, f64)>,
    /// 0.x's local estimate.
    pub score: Option<Estimate>,
    /// The fallback definition answer (confidence 0.5), used offline only.
    pub fallback: Option<(i64, f64)>,
    /// The score's archetype is a manual one (the player's own, which the
    /// API cannot know).
    pub score_is_manual: bool,
}

/// The decision for one seat.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// A local answer good enough to write: offline, rules or a score at
    /// 0.8; online, a manual archetype's score at 0.8 for the player.
    Accept { archetype_id: i64, confidence: f64 },
    /// Ask the API's estimate. When it names no archetype the caller
    /// decides with `decide_after_empty_estimate`; when it fails, the
    /// caller writes nothing and asks again later.
    AskApi,
    /// Offline: the best local answer at any confidence.
    AcceptOffline { archetype_id: i64, confidence: f64 },
    /// No answer for the opponent: Homebrew at confidence 0.
    Homebrew,
    /// No answer for the player: no row.
    Nothing,
}

/// Runs the rules, the scorer and the fallback over the held candidates.
pub fn local_answers(
    observed: &[ObservedCard],
    candidates: &[Candidate],
    opponent: bool,
) -> LocalAnswers {
    let score = score(observed, candidates);
    let score_is_manual = score.as_ref().is_some_and(|estimate| {
        candidates.iter().any(|candidate| {
            candidate.archetype_id == estimate.archetype_id && candidate.source == Source::Manual
        })
    });
    LocalAnswers {
        opponent,
        rules: rules_match(observed, candidates),
        score,
        fallback: fallback_match(observed, candidates),
        score_is_manual,
    }
}

/// Spec section 6.1 steps 3 to 6 (catalog picker addendum, #489). Online
/// the API is asked first, as v1 holds only the archetypes it has met: a
/// confident local answer may only be the one held archetype of its
/// colours. Only a confident manual archetype for the player's own deck is
/// accepted without it: for an opponent a broad custom list would
/// override the server on every match.
pub fn decide(local: LocalAnswers, api_allowed: bool) -> Decision {
    if api_allowed {
        if !local.opponent
            && local.score_is_manual
            && let Some(estimate) = &local.score
            && estimate.confidence >= LOCAL_CONFIDENCE_THRESHOLD
        {
            return Decision::Accept {
                archetype_id: estimate.archetype_id,
                confidence: estimate.confidence,
            };
        }
        return Decision::AskApi;
    }
    if let Some(decision) = confident(&local) {
        return decision;
    }
    if let Some(estimate) = local.score {
        return Decision::AcceptOffline {
            archetype_id: estimate.archetype_id,
            confidence: estimate.confidence,
        };
    }
    if let Some((archetype_id, confidence)) = local.fallback {
        return Decision::AcceptOffline {
            archetype_id,
            confidence,
        };
    }
    if local.opponent {
        Decision::Homebrew
    } else {
        Decision::Nothing
    }
}

/// The API answered and named no archetype (a 200 `[]`): a rules match or
/// a score at 0.8 still stands, anything weaker is Homebrew for the
/// opponent and nothing for the player. The any-confidence offline answer
/// is not taken here: v1 holds only the archetypes it has met, so a weak
/// score there is often a wrong label.
pub fn decide_after_empty_estimate(local: LocalAnswers) -> Decision {
    if let Some(decision) = confident(&local) {
        return decision;
    }
    if local.opponent {
        Decision::Homebrew
    } else {
        Decision::Nothing
    }
}

/// The rules answer, else a score at 0.8: the local answers good enough
/// to accept on their own.
fn confident(local: &LocalAnswers) -> Option<Decision> {
    if let Some((archetype_id, confidence)) = local.rules {
        return Some(Decision::Accept {
            archetype_id,
            confidence,
        });
    }
    if let Some(estimate) = &local.score
        && estimate.confidence >= LOCAL_CONFIDENCE_THRESHOLD
    {
        return Some(Decision::Accept {
            archetype_id: estimate.archetype_id,
            confidence: estimate.confidence,
        });
    }
    None
}

/// Follows a merge one hop (0.x `ResolveMergedArchetype`): a merged
/// archetype becomes its parent; chains are not followed.
pub fn merge_hop(archetype_id: i64, merged_into: impl Fn(i64) -> Option<i64>) -> i64 {
    merged_into(archetype_id).unwrap_or(archetype_id)
}
