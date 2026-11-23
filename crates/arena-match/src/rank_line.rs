//! The rank line: rank observations in log order, the gap between each pair,
//! and which matches fall in which gap. Pure, no clock: order is the log
//! position `(file_id, offset)`, never time.

use std::collections::HashSet;

pub const STEPS_PER_TIER: i32 = 6;
pub const MYTHIC_STEPS: i32 = 120;
/// Spark, the constructed class below Bronze, has four tiers of five steps.
pub const SPARK_STEPS_PER_TIER: i32 = 5;
/// Spark's whole class: Spark 4 step 0 is `-SPARK_STEPS`.
pub const SPARK_STEPS: i32 = 4 * SPARK_STEPS_PER_TIER;

#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub file_id: i64,
    pub offset: u64,
    pub at_ms: i64,
    pub class: String,
    pub level: u32,
    pub step: u32,
    pub won: u32,
    pub lost: u32,
    pub mythic_percentile: Option<f64>,
    pub mythic_placement: Option<u32>,
}

/// Where a match sits in the log. `Claim` names the observation ending the gap.
#[derive(Debug, Clone, PartialEq)]
pub enum MatchPlace {
    Log { file_id: i64, offset: u64 },
    Claim { file_id: i64, offset: u64 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct RankedMatch {
    pub token: String,
    pub place: MatchPlace,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GapKind {
    Attributed,
    PlayedElsewhere,
    Shared,
    Quiet,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Gap {
    pub from: usize,
    pub to: usize,
    pub kind: GapKind,
    pub change_steps: i32,
    pub mythic_change: Option<f64>,
    pub tokens: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RankLine {
    pub points: Vec<Observation>,
    pub gaps: Vec<Gap>,
    pub pending: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeckClimb {
    pub steps: i32,
    /// The climb in whole tiers and the steps left over (`climb_text`),
    /// counted on the ladder so a Spark tier is five steps and any other six.
    /// Both have the sign of the climb.
    pub tiers: i32,
    pub tier_steps: i32,
    pub mythic_change: Option<f64>,
    pub attributed: u32,
}

/// Steps from Bronze 4 step 0, or `None` for an unknown class or a reading
/// outside its tier (a level other than 1 to 4, or a step past the tier's
/// last), so a bad reading is dropped rather than spilling into the next
/// tier. Bronze to Diamond have tiers of six steps. Spark, below Bronze, has
/// tiers of five and is negative: Spark 4 step 0 is -20, Spark 1 step 4 is
/// -1. Keeping Bronze at 0 leaves every other step value as it was. Any
/// Mythic reading is `MYTHIC_STEPS`.
pub fn rank_steps(class: &str, level: u32, step: u32) -> Option<i32> {
    let (class_start, tier_steps) = match class {
        "Spark" => (-SPARK_STEPS, SPARK_STEPS_PER_TIER),
        "Bronze" => (0, STEPS_PER_TIER),
        "Silver" => (24, STEPS_PER_TIER),
        "Gold" => (48, STEPS_PER_TIER),
        "Platinum" => (72, STEPS_PER_TIER),
        "Diamond" => (96, STEPS_PER_TIER),
        "Mythic" => return Some(MYTHIC_STEPS),
        _ => return None,
    };
    if !(1..=4).contains(&level) || step >= tier_steps as u32 {
        return None;
    }
    let tier_in_class = 4 - level as i32;
    Some(class_start + tier_in_class * tier_steps + step as i32)
}

pub fn rank_label(observation: &Observation) -> String {
    if observation.class == "Mythic" {
        if let Some(percentile) = observation.mythic_percentile {
            return format!("Mythic {}%", percent_text(percentile));
        }
        if let Some(placement) = observation.mythic_placement {
            return format!("Mythic #{placement}");
        }
        return "Mythic".to_string();
    }
    format!("{} {}", observation.class, observation.level)
}

/// A Mythic percentile cut to one decimal, never rounded up: 99.5 stays
/// 99.5 and 99.96 reads 99.9, so the top of the ladder never shows as 100.
/// A whole number has no decimal. The tiny nudge keeps a value stored just
/// below its decimal (as floats can) from losing a tenth.
fn percent_text(percentile: f64) -> String {
    let tenths = (percentile * 10.0 + 1e-9).floor() as i64;
    if tenths % 10 == 0 {
        format!("{}", tenths / 10)
    } else {
        format!("{}.{}", tenths / 10, tenths % 10)
    }
}

pub(crate) fn position(file_id: i64, offset: u64) -> (i64, u64) {
    (file_id, offset)
}

fn counter_change(from: &Observation, to: &Observation) -> i64 {
    let won = i64::from(to.won) - i64::from(from.won);
    let lost = i64::from(to.lost) - i64::from(from.lost);
    won.max(0) + lost.max(0)
}

fn gap_between(
    from: usize,
    to: usize,
    a: &Observation,
    b: &Observation,
    tokens: Vec<String>,
) -> Gap {
    let steps_a = rank_steps(&a.class, a.level, a.step).unwrap_or(0);
    let steps_b = rank_steps(&b.class, b.level, b.step).unwrap_or(0);
    let mythic_change = match (a.mythic_percentile, b.mythic_percentile) {
        (Some(x), Some(y)) if a.class == "Mythic" && b.class == "Mythic" => Some(y - x),
        _ => None,
    };
    let change_steps = steps_b - steps_a;
    let counters = counter_change(a, b);
    let placement_moved = a.class == "Mythic"
        && b.class == "Mythic"
        && a.mythic_placement.is_some()
        && b.mythic_placement.is_some()
        && a.mythic_placement != b.mythic_placement;
    let moved = change_steps != 0
        || counters > 0
        || mythic_change.is_some_and(|c| c != 0.0)
        || placement_moved;
    let kind = if tokens.len() == 1 && counters <= 1 {
        GapKind::Attributed
    } else if tokens.is_empty() && moved {
        GapKind::PlayedElsewhere
    } else if tokens.is_empty() {
        GapKind::Quiet
    } else {
        GapKind::Shared
    };
    Gap {
        from,
        to,
        kind,
        change_steps,
        mythic_change,
        tokens,
    }
}

pub fn rank_line(observations: Vec<Observation>, matches: &[RankedMatch]) -> RankLine {
    let mut points: Vec<Observation> = observations
        .into_iter()
        .filter(|o| rank_steps(&o.class, o.level, o.step).is_some())
        .collect();
    points.sort_by_key(|o| position(o.file_id, o.offset));

    let mut tokens: Vec<Vec<String>> = vec![Vec::new(); points.len().saturating_sub(1)];
    let mut pending = Vec::new();
    for m in matches {
        match &m.place {
            MatchPlace::Log { file_id, offset } => {
                let at = position(*file_id, *offset);
                // First observation after the match ends its gap.
                let after = points
                    .iter()
                    .position(|o| position(o.file_id, o.offset) > at);
                match after {
                    Some(0) => {}
                    Some(i) => tokens[i - 1].push(m.token.clone()),
                    None => pending.push(m.token.clone()),
                }
            }
            MatchPlace::Claim { file_id, offset } => {
                let at = position(*file_id, *offset);
                let end = points
                    .iter()
                    .position(|o| position(o.file_id, o.offset) == at);
                if let Some(i) = end.filter(|i| *i > 0) {
                    tokens[i - 1].push(m.token.clone());
                }
            }
        }
    }

    let gaps = (0..tokens.len())
        .map(|i| {
            gap_between(
                i,
                i + 1,
                &points[i],
                &points[i + 1],
                std::mem::take(&mut tokens[i]),
            )
        })
        .collect();
    RankLine {
        points,
        gaps,
        pending,
    }
}

/// A gap a manual match can be claimed into: one played elsewhere (no
/// tracked match, the rank moved), or one that holds only claimed manual
/// matches (`claims` of them). `gap` indexes `RankLine::gaps`.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimableGap {
    pub gap: usize,
    pub claims: u32,
}

/// The gaps of `line` a manual match can be claimed into, oldest first.
/// `matches` are the matches the line was built from: a token placed by a
/// claim counts as a claim. A gap with a tracked match, or a quiet one, is
/// never claimable.
pub fn claimable(line: &RankLine, matches: &[RankedMatch]) -> Vec<ClaimableGap> {
    let claimed: HashSet<&String> = matches
        .iter()
        .filter(|m| matches!(m.place, MatchPlace::Claim { .. }))
        .map(|m| &m.token)
        .collect();
    let mut out = Vec::new();
    for (index, gap) in line.gaps.iter().enumerate() {
        let mut claims = 0;
        for token in &gap.tokens {
            if claimed.contains(token) {
                claims += 1;
            }
        }
        let only_claims = !gap.tokens.is_empty() && claims == gap.tokens.len() as u32;
        if gap.kind == GapKind::PlayedElsewhere || only_claims {
            out.push(ClaimableGap { gap: index, claims });
        }
    }
    out
}

/// Whether the gap ending at the reading at `(file_id, offset)` can be
/// claimed (`claimable`).
pub fn is_claimable(line: &RankLine, matches: &[RankedMatch], file_id: i64, offset: u64) -> bool {
    let at = position(file_id, offset);
    claimable(line, matches).iter().any(|option| {
        let end = &line.points[line.gaps[option.gap].to];
        position(end.file_id, end.offset) == at
    })
}

pub fn deck_climb(line: &RankLine, deck_tokens: &[String]) -> DeckClimb {
    let deck: HashSet<&String> = deck_tokens.iter().collect();
    let mut steps = 0;
    let mut spark = Leftover::default();
    let mut six = Leftover::default();
    let mut mythic: Option<f64> = None;
    let mut attributed = 0;
    for gap in &line.gaps {
        if gap.kind != GapKind::Attributed || !gap.tokens.iter().any(|t| deck.contains(t)) {
            continue;
        }
        attributed += 1;
        steps += gap.change_steps;
        let change = tier_move(&line.points[gap.from], &line.points[gap.to]);
        spark.tiers += change.spark_tiers;
        six.tiers += change.six_tiers;
        if change.rest_size == SPARK_STEPS_PER_TIER {
            spark.steps += change.rest;
        } else {
            six.steps += change.rest;
        }
        if let Some(change) = gap.mythic_change {
            mythic = Some(mythic.unwrap_or(0.0) + change);
        }
    }
    let (tiers, tier_steps) = combine(spark, six);
    DeckClimb {
        steps,
        tiers,
        tier_steps,
        mythic_change: mythic,
        attributed,
    }
}

/// A place on the ladder: the tier counted from Spark 4 (0, Bronze 4 is 4)
/// up to Mythic (24), and the step within it.
fn ladder_place(steps: i32) -> (i32, i32) {
    if steps >= MYTHIC_STEPS {
        return (4 + MYTHIC_STEPS / STEPS_PER_TIER, 0);
    }
    if steps < 0 {
        let from_spark = steps + SPARK_STEPS;
        return (
            from_spark / SPARK_STEPS_PER_TIER,
            from_spark % SPARK_STEPS_PER_TIER,
        );
    }
    (4 + steps / STEPS_PER_TIER, steps % STEPS_PER_TIER)
}

/// Steps in the tier at `ladder_place` index `tier`.
fn tier_size(tier: i32) -> i32 {
    if tier < 4 {
        SPARK_STEPS_PER_TIER
    } else {
        STEPS_PER_TIER
    }
}

/// One move on the ladder: whole tiers (a tier climbed from a Spark tier is
/// five steps, any other six), then `rest` steps counted in tiers of
/// `rest_size`.
struct TierMove {
    spark_tiers: i32,
    six_tiers: i32,
    rest: i32,
    rest_size: i32,
}

/// The move from reading `a` to reading `b`: whole tiers climbed (the same
/// step one tier up, each time), then the steps left. A drop is the climb
/// back, negated.
fn tier_move(a: &Observation, b: &Observation) -> TierMove {
    let from = rank_steps(&a.class, a.level, a.step).unwrap_or(0);
    let to = rank_steps(&b.class, b.level, b.step).unwrap_or(0);
    if to < from {
        let up = climb_between(to, from);
        return TierMove {
            spark_tiers: -up.spark_tiers,
            six_tiers: -up.six_tiers,
            rest: -up.rest,
            rest_size: up.rest_size,
        };
    }
    climb_between(from, to)
}

/// The climb from `low` up to `high` (both step values, `low <= high`).
fn climb_between(low: i32, high: i32) -> TierMove {
    let (low_tier, low_step) = ladder_place(low);
    let (high_tier, high_step) = ladder_place(high);
    let mut tiers = high_tier - low_tier;
    let mut rest = high_step - low_step;
    let mut rest_size = tier_size(high_tier);
    if rest < 0 {
        // One tier fewer: the steps run from the tier below `high`'s into it.
        tiers -= 1;
        rest_size = tier_size(high_tier - 1);
        rest += rest_size;
    }
    // Each tier climbed is as long as the tier it starts in.
    let mut spark_tiers = 0;
    for tier in low_tier..low_tier + tiers {
        if tier_size(tier) == SPARK_STEPS_PER_TIER {
            spark_tiers += 1;
        }
    }
    TierMove {
        spark_tiers,
        six_tiers: tiers - spark_tiers,
        rest,
        rest_size,
    }
}

/// Whole tiers and leftover steps of one tier size.
#[derive(Debug, Clone, Copy, Default)]
struct Leftover {
    tiers: i32,
    steps: i32,
}

impl Leftover {
    /// Leftover steps carried into whole tiers of `size`, and one tier
    /// borrowed when the steps go against the tiers, so both share a sign.
    fn settle(self, size: i32) -> Leftover {
        let mut tiers = self.tiers + self.steps / size;
        let mut steps = self.steps % size;
        if tiers != 0 && steps != 0 && tiers.signum() != steps.signum() {
            steps += tiers.signum() * size;
            tiers -= tiers.signum();
        }
        Leftover { tiers, steps }
    }
}

/// The climb's tiers and steps from the Spark (five-step) and other
/// (six-step) parts. Each part carries and borrows in its own tier size, so
/// a tier is always a whole tier of the size climbed and tiers times their
/// sizes plus the steps is exactly the net step change. Then:
///
/// - Tiers going both ways: the part with fewer steps in tiers gives its
///   tiers up as plain steps.
/// - Steps against the tiers: the remaining part lends a tier, of its own
///   size, until the steps agree (a Bronze tier up and a Spark step down is
///   5 steps).
///
/// Leftover steps of both sizes that make no whole tier of either are summed
/// as plain steps: 4 Spark steps and 5 Bronze steps read "9 steps". A sum of
/// mixed climbs has no truer tier count; the steps still add up to the net.
fn combine(spark: Leftover, six: Leftover) -> (i32, i32) {
    let mut spark = spark.settle(SPARK_STEPS_PER_TIER);
    let mut six = six.settle(STEPS_PER_TIER);
    let mut steps = spark.steps + six.steps;
    if spark.tiers.signum() * six.tiers.signum() < 0 {
        let spark_size = (spark.tiers * SPARK_STEPS_PER_TIER).abs();
        let six_size = (six.tiers * STEPS_PER_TIER).abs();
        if spark_size <= six_size {
            steps += spark.tiers * SPARK_STEPS_PER_TIER;
            spark.tiers = 0;
        } else {
            steps += six.tiers * STEPS_PER_TIER;
            six.tiers = 0;
        }
    }
    // The tiers now share a sign; each loan comes from a part that has one.
    loop {
        let tiers = spark.tiers + six.tiers;
        if tiers == 0 || steps == 0 || tiers.signum() == steps.signum() {
            return (tiers, steps);
        }
        let sign = tiers.signum();
        if six.tiers != 0 {
            six.tiers -= sign;
            steps += sign * STEPS_PER_TIER;
        } else {
            spark.tiers -= sign;
            steps += sign * SPARK_STEPS_PER_TIER;
        }
    }
}

fn plural(count: i32, word: &str) -> String {
    if count == 1 {
        format!("1 {word}")
    } else {
        format!("{count} {word}s")
    }
}

/// A climb as words: `1 tier, 3 steps`, `2 tiers`, `1 step` or
/// `no change` (`DeckClimb::tiers` and `tier_steps`; the sign is dropped).
pub fn climb_text(tiers: i32, steps: i32) -> String {
    let tiers = tiers.abs();
    let steps = steps.abs();
    match (tiers, steps) {
        (0, 0) => "no change".to_string(),
        (0, r) => plural(r, "step"),
        (t, 0) => plural(t, "tier"),
        (t, r) => format!("{}, {}", plural(t, "tier"), plural(r, "step")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(offset: u64, class: &str, level: u32, step: u32, won: u32, lost: u32) -> Observation {
        Observation {
            file_id: 1,
            offset,
            at_ms: 0,
            class: class.to_string(),
            level,
            step,
            won,
            lost,
            mythic_percentile: None,
            mythic_placement: None,
        }
    }

    fn mythic(offset: u64, percentile: f64, won: u32, lost: u32) -> Observation {
        Observation {
            mythic_percentile: Some(percentile),
            ..obs(offset, "Mythic", 1, 0, won, lost)
        }
    }

    fn log(token: &str, offset: u64) -> RankedMatch {
        RankedMatch {
            token: token.to_string(),
            place: MatchPlace::Log { file_id: 1, offset },
        }
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn rank_steps_orders_the_classes() {
        assert_eq!(rank_steps("Bronze", 4, 0), Some(0));
        assert_eq!(rank_steps("Bronze", 1, 5), Some(23));
        assert_eq!(rank_steps("Silver", 4, 0), Some(24));
        assert_eq!(rank_steps("Diamond", 1, 5), Some(119));
        assert_eq!(rank_steps("Mythic", 1, 0), Some(120));
        assert_eq!(rank_steps("Copper", 1, 0), None);
    }

    #[test]
    fn rank_steps_rejects_a_reading_outside_its_tier() {
        assert_eq!(rank_steps("Gold", 0, 2), None, "level below 1");
        assert_eq!(rank_steps("Gold", 5, 2), None, "level above 4");
        assert_eq!(
            rank_steps("Gold", 4, 6),
            None,
            "a step of 6 would spill into the next tier"
        );
        assert_eq!(rank_steps("Gold", 4, 5), Some(53));
    }

    #[test]
    fn spark_sits_below_bronze_in_tiers_of_five_steps() {
        assert_eq!(rank_steps("Spark", 4, 0), Some(-20));
        assert_eq!(rank_steps("Spark", 3, 0), Some(-15));
        assert_eq!(rank_steps("Spark", 1, 4), Some(-1));
        assert_eq!(rank_steps("Bronze", 4, 0), Some(0), "Bronze is unchanged");
        assert_eq!(
            rank_steps("Spark", 4, 5),
            None,
            "a step of 5 would spill into the next Spark tier"
        );
        assert_eq!(rank_steps("Spark", 0, 0), None, "level below 1");
    }

    #[test]
    fn a_spark_reading_is_on_the_line() {
        let line = rank_line(
            vec![obs(10, "Spark", 1, 4, 5, 5), obs(30, "Bronze", 4, 0, 6, 5)],
            &[log("m1", 20)],
        );
        assert_eq!(line.points.len(), 2);
        assert_eq!(line.gaps[0].kind, GapKind::Attributed);
        assert_eq!(line.gaps[0].change_steps, 1);
    }

    #[test]
    fn one_match_between_observations_is_attributed() {
        let line = rank_line(
            vec![obs(10, "Gold", 4, 2, 5, 5), obs(30, "Gold", 4, 4, 6, 5)],
            &[log("m1", 20)],
        );
        assert_eq!(line.gaps.len(), 1);
        assert_eq!(line.gaps[0].kind, GapKind::Attributed);
        assert_eq!(line.gaps[0].change_steps, 2);
        assert_eq!(line.gaps[0].tokens, strings(&["m1"]));
    }

    #[test]
    fn no_match_with_a_change_is_played_elsewhere() {
        let line = rank_line(
            vec![obs(10, "Gold", 4, 2, 5, 5), obs(30, "Gold", 4, 4, 6, 5)],
            &[],
        );
        assert_eq!(line.gaps[0].kind, GapKind::PlayedElsewhere);
    }

    #[test]
    fn an_unchanged_refetch_is_quiet() {
        let line = rank_line(
            vec![obs(10, "Gold", 4, 2, 5, 5), obs(30, "Gold", 4, 2, 5, 5)],
            &[],
        );
        assert_eq!(line.gaps[0].kind, GapKind::Quiet);
    }

    #[test]
    fn a_mythic_percentile_is_never_rounded_up() {
        assert_eq!(rank_label(&mythic(1, 99.5, 0, 0)), "Mythic 99.5%");
        assert_eq!(rank_label(&mythic(1, 99.96, 0, 0)), "Mythic 99.9%");
        assert_eq!(rank_label(&mythic(1, 0.04, 0, 0)), "Mythic 0%");
    }

    #[test]
    fn a_mythic_placement_change_alone_is_played_elsewhere() {
        let placed = |offset: u64, placement: u32| Observation {
            mythic_placement: Some(placement),
            ..obs(offset, "Mythic", 1, 0, 5, 5)
        };
        let line = rank_line(vec![placed(10, 900), placed(30, 850)], &[]);
        assert_eq!(line.gaps[0].kind, GapKind::PlayedElsewhere);
    }

    #[test]
    fn two_matches_share_a_gap() {
        let line = rank_line(
            vec![obs(10, "Gold", 4, 2, 5, 5), obs(50, "Gold", 4, 4, 7, 5)],
            &[log("m1", 20), log("m2", 30)],
        );
        assert_eq!(line.gaps[0].kind, GapKind::Shared);
        assert_eq!(line.gaps[0].tokens, strings(&["m1", "m2"]));
    }

    #[test]
    fn counters_showing_an_extra_match_make_it_shared() {
        let line = rank_line(
            vec![obs(10, "Gold", 4, 2, 5, 5), obs(30, "Gold", 4, 4, 7, 5)],
            &[log("m1", 20)],
        );
        assert_eq!(line.gaps[0].kind, GapKind::Shared);
    }

    #[test]
    fn a_claimed_manual_match_counts_in_its_gap() {
        let claim = RankedMatch {
            token: "manual".to_string(),
            place: MatchPlace::Claim {
                file_id: 1,
                offset: 30,
            },
        };
        let line = rank_line(
            vec![obs(10, "Gold", 4, 2, 5, 5), obs(30, "Gold", 4, 4, 6, 5)],
            &[claim],
        );
        assert_eq!(line.gaps[0].kind, GapKind::Attributed);
        assert_eq!(line.gaps[0].tokens, strings(&["manual"]));
    }

    #[test]
    fn matches_order_by_log_position_not_time() {
        let mut late = obs(10, "Gold", 4, 2, 5, 5);
        late.at_ms = 9_000;
        let mut early = obs(30, "Gold", 4, 3, 6, 5);
        early.at_ms = 1_000;
        let line = rank_line(
            vec![early, late, obs(60, "Gold", 4, 4, 7, 5)],
            &[log("m1", 40)],
        );
        assert_eq!(line.gaps[0].tokens, Vec::<String>::new());
        assert_eq!(line.gaps[1].tokens, strings(&["m1"]));
        assert_eq!(line.points[0].offset, 10);
    }

    #[test]
    fn a_match_after_the_last_observation_is_pending() {
        let line = rank_line(
            vec![obs(10, "Gold", 4, 2, 5, 5), obs(30, "Gold", 4, 4, 6, 5)],
            &[log("m1", 20), log("m2", 99), log("m0", 5)],
        );
        assert_eq!(line.pending, strings(&["m2"]));
        assert_eq!(line.gaps[0].tokens, strings(&["m1"]));
    }

    fn claim(token: &str, offset: u64) -> RankedMatch {
        RankedMatch {
            token: token.to_string(),
            place: MatchPlace::Claim { file_id: 1, offset },
        }
    }

    #[test]
    fn no_observations_make_an_empty_line() {
        let line = rank_line(Vec::new(), &[log("m1", 20), claim("c1", 30)]);
        assert!(line.points.is_empty());
        assert!(line.gaps.is_empty());
        assert_eq!(line.pending, strings(&["m1"]));
        assert!(claimable(&line, &[]).is_empty());
        assert_eq!(deck_climb(&line, &strings(&["m1"])).attributed, 0);
    }

    #[test]
    fn a_claim_off_a_gap_end_is_dropped() {
        let line = rank_line(
            vec![obs(10, "Gold", 4, 2, 5, 5), obs(30, "Gold", 4, 4, 6, 5)],
            // At the first reading (no gap ends there) and between readings.
            &[claim("first", 10), claim("between", 20)],
        );
        assert_eq!(line.gaps[0].tokens, Vec::<String>::new());
        assert_eq!(line.gaps[0].kind, GapKind::PlayedElsewhere);
        assert!(line.pending.is_empty());
    }

    #[test]
    fn claimable_gaps_are_played_elsewhere_or_hold_only_claims() {
        let matches = [log("tracked", 20), claim("manual", 50)];
        let line = rank_line(
            vec![
                obs(10, "Gold", 4, 0, 0, 0),
                obs(30, "Gold", 4, 2, 1, 0),
                obs(50, "Gold", 4, 4, 2, 0),
                obs(70, "Gold", 3, 0, 3, 0),
                obs(90, "Gold", 3, 0, 3, 0),
            ],
            &matches,
        );
        // Gap 0 holds a tracked match, gap 1 a claim, gap 2 moved with no
        // match, gap 3 is quiet.
        assert_eq!(
            claimable(&line, &matches),
            vec![
                ClaimableGap { gap: 1, claims: 1 },
                ClaimableGap { gap: 2, claims: 0 },
            ]
        );
        assert!(!is_claimable(&line, &matches, 1, 10), "the first reading");
        assert!(!is_claimable(&line, &matches, 1, 30), "a tracked match");
        assert!(is_claimable(&line, &matches, 1, 50));
        assert!(is_claimable(&line, &matches, 1, 70));
        assert!(!is_claimable(&line, &matches, 1, 90), "quiet");
        assert!(!is_claimable(&line, &matches, 2, 70), "another file");
    }

    #[test]
    fn deck_climb_sums_only_the_decks_attributed_gaps() {
        let line = rank_line(
            vec![
                obs(10, "Gold", 4, 0, 0, 0),
                obs(30, "Gold", 4, 2, 1, 0),
                obs(50, "Gold", 4, 0, 1, 1),
                obs(80, "Gold", 4, 3, 3, 1),
            ],
            &[log("a", 20), log("b", 40), log("a2", 60), log("c", 70)],
        );
        // Gap 3 holds a deck match and another match: shared, not counted.
        let climb = deck_climb(&line, &strings(&["a", "a2"]));
        assert_eq!(climb.steps, 2);
        assert_eq!(climb.attributed, 1);
    }

    #[test]
    fn mythic_changes_are_apart_from_steps() {
        let mut m90 = mythic(30, 90.0, 6, 5);
        m90.level = 1;
        let line = rank_line(vec![obs(10, "Diamond", 1, 5, 5, 5), m90], &[log("a", 20)]);
        assert_eq!(line.gaps[0].change_steps, 1);
        assert_eq!(line.gaps[0].mythic_change, None);

        let line = rank_line(
            vec![mythic(10, 90.0, 5, 5), mythic(30, 95.0, 6, 5)],
            &[log("a", 20)],
        );
        assert_eq!(line.gaps[0].change_steps, 0);
        assert_eq!(line.gaps[0].mythic_change, Some(5.0));
        let climb = deck_climb(&line, &strings(&["a"]));
        assert_eq!(climb.steps, 0);
        assert_eq!(climb.mythic_change, Some(5.0));
    }

    #[test]
    fn climb_text_names_tiers_and_steps() {
        assert_eq!(climb_text(1, 3), "1 tier, 3 steps");
        assert_eq!(climb_text(1, 0), "1 tier");
        assert_eq!(climb_text(2, 1), "2 tiers, 1 step");
        assert_eq!(climb_text(0, 2), "2 steps");
        assert_eq!(climb_text(0, 1), "1 step");
        assert_eq!(climb_text(-1, -3), "1 tier, 3 steps");
        assert_eq!(climb_text(0, 0), "no change");
    }

    /// The deck's climb over `points`, with one deck match in every gap.
    fn climb_through(points: &[(&str, u32, u32)]) -> DeckClimb {
        climb_of(points, &vec![true; points.len() - 1])
    }

    /// The deck's climb over `points`, with a deck match in each gap that
    /// `deck_gaps` marks; the other gaps were played elsewhere.
    fn climb_of(points: &[(&str, u32, u32)], deck_gaps: &[bool]) -> DeckClimb {
        let mut observations = Vec::new();
        let mut matches = Vec::new();
        let mut tokens = Vec::new();
        for (index, (class, level, step)) in points.iter().enumerate() {
            let offset = 10 + 20 * index as u64;
            observations.push(obs(offset, class, *level, *step, index as u32, 0));
            if index > 0 && deck_gaps[index - 1] {
                let token = format!("m{index}");
                matches.push(log(&token, offset - 10));
                tokens.push(token);
            }
        }
        deck_climb(&rank_line(observations, &matches), &tokens)
    }

    fn climb_words(points: &[(&str, u32, u32)]) -> String {
        let climb = climb_through(points);
        climb_text(climb.tiers, climb.tier_steps)
    }

    #[test]
    fn six_step_climbs_roll_into_tiers() {
        assert_eq!(
            climb_words(&[("Gold", 4, 0), ("Gold", 3, 3)]),
            "1 tier, 3 steps"
        );
        assert_eq!(
            climb_words(&[("Gold", 4, 4), ("Gold", 2, 1)]),
            "1 tier, 3 steps"
        );
        assert_eq!(
            climb_words(&[("Gold", 4, 0), ("Gold", 4, 4), ("Gold", 3, 2)]),
            "1 tier, 2 steps"
        );
        assert_eq!(
            climb_words(&[("Diamond", 1, 3), ("Mythic", 1, 0)]),
            "3 steps"
        );
    }

    #[test]
    fn a_climb_within_spark_counts_five_step_tiers() {
        // Spark 4 step 1 to Spark 3 step 3: 7 steps, one five-step tier and 2.
        let climb = climb_through(&[("Spark", 4, 1), ("Spark", 3, 3)]);
        assert_eq!(climb.steps, 7);
        assert_eq!(climb_text(climb.tiers, climb.tier_steps), "1 tier, 2 steps");
        // Three climbs of 2 steps from Spark 4 step 0 end at Spark 3 step 1.
        assert_eq!(
            climb_words(&[
                ("Spark", 4, 0),
                ("Spark", 4, 2),
                ("Spark", 4, 4),
                ("Spark", 3, 1)
            ]),
            "1 tier, 1 step"
        );
    }

    #[test]
    fn a_climb_from_spark_into_bronze_is_exact() {
        assert_eq!(
            climb_words(&[("Spark", 1, 2), ("Bronze", 4, 4)]),
            "1 tier, 2 steps"
        );
        assert_eq!(
            climb_words(&[("Spark", 2, 2), ("Bronze", 4, 4)]),
            "2 tiers, 2 steps"
        );
        assert_eq!(climb_words(&[("Spark", 1, 4), ("Bronze", 4, 0)]), "1 step");
        assert_eq!(climb_words(&[("Spark", 1, 3), ("Bronze", 4, 3)]), "1 tier");
    }

    #[test]
    fn a_drop_from_bronze_into_spark_is_exact() {
        let climb = climb_through(&[("Bronze", 4, 1), ("Spark", 1, 0)]);
        assert_eq!(climb.steps, -6);
        assert!(climb.tiers < 0 && climb.tier_steps < 0);
        assert_eq!(climb_text(climb.tiers, climb.tier_steps), "1 tier, 1 step");
        assert_eq!(climb_words(&[("Bronze", 4, 0), ("Spark", 1, 4)]), "1 step");
    }

    /// The words for a climb, checked against its net step change: the
    /// text's direction never disagrees with it.
    fn checked_words(climb: &DeckClimb) -> String {
        if climb.steps != 0 {
            let sign = if climb.tiers != 0 {
                climb.tiers.signum()
            } else {
                climb.tier_steps.signum()
            };
            assert_eq!(sign, climb.steps.signum(), "{climb:?}");
        }
        climb_text(climb.tiers, climb.tier_steps)
    }

    #[test]
    fn a_bronze_tier_up_and_a_spark_step_down_is_five_steps() {
        let climb = climb_of(
            &[
                ("Bronze", 4, 0),
                ("Bronze", 3, 0),
                ("Spark", 1, 1),
                ("Spark", 1, 0),
            ],
            &[true, false, true],
        );
        assert_eq!(climb.steps, 5);
        assert_eq!(checked_words(&climb), "5 steps");
    }

    #[test]
    fn leftover_steps_of_both_sizes_are_plain_steps() {
        // 4 Spark steps and 5 Bronze steps make no whole tier of either size.
        let climb = climb_of(
            &[
                ("Spark", 4, 0),
                ("Spark", 4, 4),
                ("Bronze", 4, 0),
                ("Bronze", 4, 5),
            ],
            &[true, false, true],
        );
        assert_eq!(climb.steps, 9);
        assert_eq!(checked_words(&climb), "9 steps");
    }

    #[test]
    fn a_spark_tier_up_and_a_bronze_tier_down_is_a_step_down() {
        let climb = climb_of(
            &[
                ("Spark", 4, 0),
                ("Spark", 3, 0),
                ("Bronze", 3, 0),
                ("Bronze", 4, 0),
            ],
            &[true, false, true],
        );
        assert_eq!(climb.steps, -1);
        assert_eq!(checked_words(&climb), "1 step");
        let climb = climb_of(
            &[
                ("Spark", 3, 0),
                ("Spark", 4, 0),
                ("Bronze", 4, 0),
                ("Bronze", 3, 0),
            ],
            &[true, false, true],
        );
        assert_eq!(climb.steps, 1);
        assert_eq!(checked_words(&climb), "1 step");
    }

    #[test]
    fn mixed_climbs_borrow_the_tier_they_climbed() {
        // Three Bronze tiers up (18) and one Spark tier down (5): 13 steps.
        let climb = climb_of(
            &[
                ("Bronze", 4, 0),
                ("Bronze", 1, 0),
                ("Spark", 3, 0),
                ("Spark", 4, 0),
            ],
            &[true, false, true],
        );
        assert_eq!(climb.steps, 13);
        assert_eq!(checked_words(&climb), "2 tiers, 1 step");
    }

    #[test]
    fn rank_label_names_the_rank() {
        assert_eq!(rank_label(&obs(1, "Gold", 2, 0, 0, 0)), "Gold 2");
        assert_eq!(rank_label(&mythic(1, 95.4, 0, 0)), "Mythic 95.4%");
        assert_eq!(rank_label(&mythic(1, 85.0, 0, 0)), "Mythic 85%");
        let placed = Observation {
            mythic_placement: Some(1234),
            ..obs(1, "Mythic", 1, 0, 0, 0)
        };
        assert_eq!(rank_label(&placed), "Mythic #1234");
    }
}
